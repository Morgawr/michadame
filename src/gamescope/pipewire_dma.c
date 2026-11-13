#include "pipewire_dma.h"

#include <pipewire/pipewire.h>
#include <spa/param/video/format-utils.h>
#include <spa/param/video/raw.h>
#include <spa/param/buffers.h>
#include <spa/pod/iter.h>
#include <spa/debug/types.h>
#include <drm/drm_fourcc.h>

#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <stdatomic.h>

#define MAX_STREAM_MODIFIERS 64

struct gamescope_dma_stream {
    struct pw_thread_loop *loop;
    struct pw_context *context;
    struct pw_stream *stream;

    gamescope_dma_frame_cb on_frame;
    gamescope_dma_state_cb on_state;
    gamescope_dma_remove_buffer_cb on_remove_buffer;
    void *user_data;

    uint64_t supported_modifiers[MAX_STREAM_MODIFIERS];
    size_t n_supported_modifiers;

    uint32_t width;
    uint32_t height;
    uint32_t drm_fourcc;
    uint64_t modifier;
    bool has_format;
    uint32_t next_buffer_id;
    uint64_t pool_generation;
    atomic_bool stopping;
};

static uint32_t spa_format_to_drm_fourcc(enum spa_video_format format) {
    switch (format) {
        case SPA_VIDEO_FORMAT_BGRx:
            return DRM_FORMAT_XRGB8888;
        case SPA_VIDEO_FORMAT_BGRA:
            return DRM_FORMAT_ARGB8888;
        case SPA_VIDEO_FORMAT_RGBA:
            return DRM_FORMAT_ABGR8888;
        case SPA_VIDEO_FORMAT_RGBx:
            return DRM_FORMAT_XBGR8888;
        case SPA_VIDEO_FORMAT_NV12:
            return DRM_FORMAT_NV12;
        default:
            return DRM_FORMAT_XRGB8888;
    }
}

static const struct spa_pod *build_format_pod_with_modifiers(
    struct spa_pod_builder *b,
    const uint64_t *mods,
    size_t n_mods
) {
    struct spa_pod_frame obj_frame;
    struct spa_pod_frame choice_frame;
    struct spa_rectangle def_rect = SPA_RECTANGLE(3840, 2160);
    struct spa_rectangle min_rect = SPA_RECTANGLE(320, 240);
    struct spa_rectangle max_rect = SPA_RECTANGLE(4096, 4096);
    struct spa_fraction def_fps = SPA_FRACTION(120, 1);
    struct spa_fraction min_fps = SPA_FRACTION(0, 1);
    struct spa_fraction max_fps = SPA_FRACTION(1000, 1);

    spa_pod_builder_push_object(b, &obj_frame, SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat);
    spa_pod_builder_add(b,
        SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_video),
        SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
        SPA_FORMAT_VIDEO_format, SPA_POD_CHOICE_ENUM_Id(5,
            SPA_VIDEO_FORMAT_BGRx,
            SPA_VIDEO_FORMAT_BGRx,
            SPA_VIDEO_FORMAT_BGRA,
            SPA_VIDEO_FORMAT_RGBA,
            SPA_VIDEO_FORMAT_RGBx),
        SPA_FORMAT_VIDEO_size, SPA_POD_CHOICE_RANGE_Rectangle(
            &def_rect, &min_rect, &max_rect),
        SPA_FORMAT_VIDEO_framerate, SPA_POD_CHOICE_RANGE_Fraction(
            &def_fps, &min_fps, &max_fps),
        0);

    spa_pod_builder_prop(b, SPA_FORMAT_VIDEO_modifier,
        SPA_POD_PROP_FLAG_MANDATORY | SPA_POD_PROP_FLAG_DONT_FIXATE);
    spa_pod_builder_push_choice(b, &choice_frame, SPA_CHOICE_Enum, 0);
    spa_pod_builder_long(b, (int64_t)mods[0]);
    for (size_t i = 0; i < n_mods; i++) {
        spa_pod_builder_long(b, (int64_t)mods[i]);
    }
    spa_pod_builder_pop(b, &choice_frame);

    return (const struct spa_pod *)spa_pod_builder_pop(b, &obj_frame);
}

static const struct spa_pod *build_linear_fallback_format_pod(struct spa_pod_builder *b) {
    struct spa_rectangle def_rect = SPA_RECTANGLE(3840, 2160);
    struct spa_rectangle min_rect = SPA_RECTANGLE(320, 240);
    struct spa_rectangle max_rect = SPA_RECTANGLE(4096, 4096);
    struct spa_fraction def_fps = SPA_FRACTION(120, 1);
    struct spa_fraction min_fps = SPA_FRACTION(0, 1);
    struct spa_fraction max_fps = SPA_FRACTION(1000, 1);

    return (const struct spa_pod *)spa_pod_builder_add_object(b,
        SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat,
        SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_video),
        SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
        SPA_FORMAT_VIDEO_format, SPA_POD_CHOICE_ENUM_Id(5,
            SPA_VIDEO_FORMAT_BGRx,
            SPA_VIDEO_FORMAT_BGRx,
            SPA_VIDEO_FORMAT_BGRA,
            SPA_VIDEO_FORMAT_RGBA,
            SPA_VIDEO_FORMAT_RGBx),
        SPA_FORMAT_VIDEO_size, SPA_POD_CHOICE_RANGE_Rectangle(
            &def_rect, &min_rect, &max_rect),
        SPA_FORMAT_VIDEO_framerate, SPA_POD_CHOICE_RANGE_Fraction(
            &def_fps, &min_fps, &max_fps),
        SPA_FORMAT_VIDEO_modifier, SPA_POD_Long((int64_t)DRM_FORMAT_MOD_LINEAR));
}

static uint64_t select_fixated_modifier(
    const struct gamescope_dma_stream *s,
    const struct spa_pod_prop *mod_prop
) {
    if (!mod_prop) {
        return DRM_FORMAT_MOD_LINEAR;
    }

    uint32_t n_vals = 0;
    uint32_t choice_type = SPA_CHOICE_None;
    const struct spa_pod *vals_pod =
        spa_pod_get_values(&mod_prop->value, &n_vals, &choice_type);
    if (!vals_pod || vals_pod->type != SPA_TYPE_Long ||
        vals_pod->size < sizeof(int64_t) || n_vals == 0) {
        return DRM_FORMAT_MOD_LINEAR;
    }

    const int64_t *candidates = (const int64_t *)SPA_POD_BODY_CONST(vals_pod);
    for (uint32_t i = 0; i < n_vals; i++) {
        uint64_t cand = (uint64_t)candidates[i];
        if (cand == DRM_FORMAT_MOD_INVALID) {
            continue;
        }
        for (size_t j = 0; j < s->n_supported_modifiers; j++) {
            if (s->supported_modifiers[j] == cand) {
                return cand;
            }
        }
    }

    if ((uint64_t)candidates[0] != DRM_FORMAT_MOD_INVALID) {
        return (uint64_t)candidates[0];
    }
    return DRM_FORMAT_MOD_LINEAR;
}

static void on_stream_state_changed(void *data, enum pw_stream_state old,
                                    enum pw_stream_state state, const char *error) {
    (void)old;
    struct gamescope_dma_stream *s = (struct gamescope_dma_stream *)data;
    if (!s) return;

    int code = 0;
    switch (state) {
        case PW_STREAM_STATE_ERROR:
            code = -1;
            break;
        case PW_STREAM_STATE_UNCONNECTED:
            code = 0;
            break;
        case PW_STREAM_STATE_CONNECTING:
            code = 1;
            break;
        case PW_STREAM_STATE_PAUSED:
            code = 2;
            break;
        case PW_STREAM_STATE_STREAMING:
            code = 3;
            break;
    }
    if (s->on_state && !atomic_load(&s->stopping)) {
        s->on_state(s->user_data, code, error);
    }
}

static void on_stream_param_changed(void *data, uint32_t id, const struct spa_pod *param) {
    struct gamescope_dma_stream *s = (struct gamescope_dma_stream *)data;
    if (!s || param == NULL || id != SPA_PARAM_Format)
        return;

    struct spa_video_info_raw info;
    spa_zero(info);
    if (spa_format_video_raw_parse(param, &info) < 0) {
        pw_log_warn("Failed to parse video format param");
        return;
    }

    const struct spa_pod_prop *mod_prop =
        spa_pod_find_prop(param, NULL, SPA_FORMAT_VIDEO_modifier);
    bool needs_fixation = mod_prop != NULL &&
        (mod_prop->flags & SPA_POD_PROP_FLAG_DONT_FIXATE);

    if (needs_fixation) {
        uint64_t chosen_mod = select_fixated_modifier(s, mod_prop);
        struct spa_video_info_raw fixated_info = info;
        fixated_info.modifier = chosen_mod;
        fixated_info.flags |= SPA_VIDEO_FLAG_MODIFIER;
        fixated_info.flags &= ~(uint32_t)SPA_VIDEO_FLAG_MODIFIER_FIXATION_REQUIRED;

        s->width = fixated_info.size.width;
        s->height = fixated_info.size.height;
        s->drm_fourcc = spa_format_to_drm_fourcc(fixated_info.format);
        s->modifier = chosen_mod;
        s->has_format = true;

        pw_log_info("Fixating Gamescope modifier: %ux%u (DRM FourCC: 0x%08x, modifier: 0x%lx)",
                    s->width, s->height, s->drm_fourcc, (unsigned long)s->modifier);

        uint8_t fix_buf[4096];
        struct spa_pod_builder b_fix = SPA_POD_BUILDER_INIT(fix_buf, sizeof(fix_buf));
        const struct spa_pod *fix_params[3];
        uint32_t n_fix_params = 0;

        fix_params[n_fix_params++] =
            (const struct spa_pod *)spa_format_video_raw_build(&b_fix, SPA_PARAM_EnumFormat, &fixated_info);
        if (s->n_supported_modifiers > 1) {
            fix_params[n_fix_params++] = build_format_pod_with_modifiers(
                &b_fix, s->supported_modifiers, s->n_supported_modifiers
            );
        } else {
            fix_params[n_fix_params++] = build_linear_fallback_format_pod(&b_fix);
        }
        fix_params[n_fix_params++] = (const struct spa_pod *)spa_pod_builder_add_object(&b_fix,
            SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
            SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(4, 2, 8),
            SPA_PARAM_BUFFERS_blocks, SPA_POD_CHOICE_RANGE_Int(1, 1, GAMESCOPE_MAX_DMA_PLANES),
            SPA_PARAM_BUFFERS_dataType, SPA_POD_CHOICE_FLAGS_Int(1 << SPA_DATA_DmaBuf));

        pw_stream_update_params(s->stream, fix_params, n_fix_params);
        return;
    }

    s->width = info.size.width;
    s->height = info.size.height;
    s->drm_fourcc = spa_format_to_drm_fourcc(info.format);
    s->modifier = (info.flags & SPA_VIDEO_FLAG_MODIFIER) ? info.modifier : DRM_FORMAT_MOD_LINEAR;
    s->has_format = true;
    s->pool_generation++;
    if (s->pool_generation == 0) {
        s->pool_generation = 1;
    }

    pw_log_info("Gamescope format negotiated: %ux%u (DRM FourCC: 0x%08x, modifier: 0x%lx, gen: %lu)",
                s->width, s->height, s->drm_fourcc, (unsigned long)s->modifier,
                (unsigned long)s->pool_generation);

    // After format is negotiated, update stream parameters to request DMA-BUF buffers (1..=4 planes)
    uint8_t buffer_bufs[512];
    struct spa_pod_builder b_buf = SPA_POD_BUILDER_INIT(buffer_bufs, sizeof(buffer_bufs));
    const struct spa_pod *b_params[1];
    b_params[0] = (const struct spa_pod *)spa_pod_builder_add_object(&b_buf,
        SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
        SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(4, 2, 8),
        SPA_PARAM_BUFFERS_blocks, SPA_POD_CHOICE_RANGE_Int(1, 1, GAMESCOPE_MAX_DMA_PLANES),
        SPA_PARAM_BUFFERS_dataType, SPA_POD_CHOICE_FLAGS_Int(1 << SPA_DATA_DmaBuf));

    pw_stream_update_params(s->stream, b_params, 1);
}

static void on_stream_add_buffer(void *data, struct pw_buffer *b) {
    struct gamescope_dma_stream *s = (struct gamescope_dma_stream *)data;
    if (!s || !b) return;
    s->next_buffer_id++;
    if (s->next_buffer_id == 0) {
        s->next_buffer_id = 1;
    }
    b->user_data = (void *)(uintptr_t)s->next_buffer_id;
}

static void on_stream_remove_buffer(void *data, struct pw_buffer *b) {
    struct gamescope_dma_stream *s = (struct gamescope_dma_stream *)data;
    if (!b) return;

    uint32_t buffer_id = (uint32_t)(uintptr_t)b->user_data;
    b->user_data = NULL;

    if (s && buffer_id != 0 && s->on_remove_buffer && !atomic_load(&s->stopping)) {
        s->on_remove_buffer(s->user_data, buffer_id, s->pool_generation);
    }
}

static void on_stream_process(void *data) {
    struct gamescope_dma_stream *s = (struct gamescope_dma_stream *)data;
    if (!s || !s->stream || atomic_load(&s->stopping)) return;

    struct pw_buffer *b = pw_stream_dequeue_buffer(s->stream);
    if (!b) return;

    if (!b->user_data) {
        s->next_buffer_id++;
        if (s->next_buffer_id == 0) {
            s->next_buffer_id = 1;
        }
        b->user_data = (void *)(uintptr_t)s->next_buffer_id;
    }
    uint32_t buffer_id = (uint32_t)(uintptr_t)b->user_data;

    struct spa_buffer *buf = b->buffer;
    if (buf && buf->n_datas > 0 && s->has_format) {
        int fds[GAMESCOPE_MAX_DMA_PLANES];
        uint32_t offsets[GAMESCOPE_MAX_DMA_PLANES];
        uint32_t strides[GAMESCOPE_MAX_DMA_PLANES];
        uint32_t n_planes = 0;
        bool corrupted = false;

        uint32_t max_datas = buf->n_datas < GAMESCOPE_MAX_DMA_PLANES
            ? buf->n_datas
            : GAMESCOPE_MAX_DMA_PLANES;

        for (uint32_t i = 0; i < max_datas; i++) {
            struct spa_data *d = &buf->datas[i];
            if (d->chunk && (d->chunk->flags & SPA_CHUNK_FLAG_CORRUPTED)) {
                corrupted = true;
                break;
            }
            if (d->type != SPA_DATA_DmaBuf || d->fd < 0) {
                break;
            }
            if (i > 0 && d->chunk &&
                d->chunk->size == 0 && d->chunk->stride == 0 && d->chunk->offset == 0) {
                break;
            }

            fds[n_planes] = (int)d->fd;
            offsets[n_planes] = d->chunk ? d->chunk->offset : 0;
            strides[n_planes] = (d->chunk && d->chunk->stride > 0)
                ? (uint32_t)d->chunk->stride
                : (n_planes == 0 ? s->width * 4 : 0);
            n_planes++;
        }

        if (!corrupted && n_planes > 0 && s->on_frame && !atomic_load(&s->stopping)) {
            s->on_frame(
                s->user_data,
                buffer_id,
                s->pool_generation,
                n_planes,
                fds,
                offsets,
                strides,
                s->width,
                s->height,
                s->drm_fourcc,
                s->modifier
            );
        }
    }

    pw_stream_queue_buffer(s->stream, b);
}

static const struct pw_stream_events stream_events = {
    PW_VERSION_STREAM_EVENTS,
    .state_changed = on_stream_state_changed,
    .param_changed = on_stream_param_changed,
    .add_buffer = on_stream_add_buffer,
    .remove_buffer = on_stream_remove_buffer,
    .process = on_stream_process,
};

struct gamescope_dma_stream *gamescope_dma_stream_start(
    const char *target_object,
    const uint64_t *modifiers,
    size_t n_modifiers,
    gamescope_dma_frame_cb on_frame,
    gamescope_dma_state_cb on_state,
    gamescope_dma_remove_buffer_cb on_remove_buffer,
    void *user_data
) {
    pw_init(NULL, NULL);

    struct gamescope_dma_stream *s = calloc(1, sizeof(struct gamescope_dma_stream));
    if (!s) return NULL;

    s->on_frame = on_frame;
    s->on_state = on_state;
    s->on_remove_buffer = on_remove_buffer;
    s->user_data = user_data;
    s->pool_generation = 1;
    atomic_init(&s->stopping, false);

    bool has_linear = false;
    if (modifiers != NULL && n_modifiers > 0) {
        for (size_t i = 0; i < n_modifiers && s->n_supported_modifiers < (MAX_STREAM_MODIFIERS - 1); i++) {
            uint64_t mod = modifiers[i];
            if (mod == DRM_FORMAT_MOD_INVALID) {
                continue;
            }
            bool duplicate = false;
            for (size_t j = 0; j < s->n_supported_modifiers; j++) {
                if (s->supported_modifiers[j] == mod) {
                    duplicate = true;
                    break;
                }
            }
            if (!duplicate) {
                if (mod == DRM_FORMAT_MOD_LINEAR) {
                    has_linear = true;
                }
                s->supported_modifiers[s->n_supported_modifiers++] = mod;
            }
        }
    }
    if (!has_linear && s->n_supported_modifiers < MAX_STREAM_MODIFIERS) {
        s->supported_modifiers[s->n_supported_modifiers++] = DRM_FORMAT_MOD_LINEAR;
    }

    s->loop = pw_thread_loop_new("gamescope-dma-loop", NULL);
    if (!s->loop) {
        free(s);
        return NULL;
    }

    s->context = pw_context_new(pw_thread_loop_get_loop(s->loop), NULL, 0);
    if (!s->context) {
        pw_thread_loop_destroy(s->loop);
        free(s);
        return NULL;
    }

    if (pw_thread_loop_start(s->loop) < 0) {
        pw_context_destroy(s->context);
        pw_thread_loop_destroy(s->loop);
        free(s);
        return NULL;
    }

    pw_thread_loop_lock(s->loop);

    struct pw_properties *props = pw_properties_new(
        PW_KEY_MEDIA_TYPE, "Video",
        PW_KEY_MEDIA_CATEGORY, "Capture",
        PW_KEY_MEDIA_ROLE, "Game",
        PW_KEY_TARGET_OBJECT, (target_object && target_object[0]) ? target_object : "gamescope",
        NULL
    );

    s->stream = pw_stream_new_simple(
        pw_context_get_main_loop(s->context),
        "michadame-gamescope-dma",
        props,
        &stream_events,
        s
    );

    if (!s->stream) {
        pw_thread_loop_unlock(s->loop);
        pw_thread_loop_stop(s->loop);
        pw_context_destroy(s->context);
        pw_thread_loop_destroy(s->loop);
        free(s);
        return NULL;
    }

    // Build format negotiation parameters
    uint8_t buffer_format[4096];
    struct spa_pod_builder b_fmt = SPA_POD_BUILDER_INIT(buffer_format, sizeof(buffer_format));

    const struct spa_pod *params[2];
    uint32_t n_params = 0;

    if (s->n_supported_modifiers > 1) {
        params[n_params++] = build_format_pod_with_modifiers(
            &b_fmt, s->supported_modifiers, s->n_supported_modifiers
        );
        params[n_params++] = build_linear_fallback_format_pod(&b_fmt);
    } else {
        params[n_params++] = build_linear_fallback_format_pod(&b_fmt);
    }

    int ret = pw_stream_connect(
        s->stream,
        PW_DIRECTION_INPUT,
        PW_ID_ANY,
        PW_STREAM_FLAG_AUTOCONNECT,
        params,
        n_params
    );

    pw_thread_loop_unlock(s->loop);

    if (ret < 0) {
        pw_log_error("Failed to connect gamescope DMA stream: %s", strerror(-ret));
        gamescope_dma_stream_stop(s);
        return NULL;
    }

    return s;
}

void gamescope_dma_stream_stop(struct gamescope_dma_stream *s) {
    if (!s) return;

    atomic_store(&s->stopping, true);

    if (s->loop) {
        pw_thread_loop_lock(s->loop);
        if (s->stream) {
            pw_stream_disconnect(s->stream);
            pw_stream_destroy(s->stream);
            s->stream = NULL;
        }
        pw_thread_loop_unlock(s->loop);

        pw_thread_loop_stop(s->loop);

        if (s->context) {
            pw_context_destroy(s->context);
            s->context = NULL;
        }

        pw_thread_loop_destroy(s->loop);
        s->loop = NULL;
    }

    free(s);
}

void michadame_alsa_silent_error_handler(
    const char *file, int line, const char *function, int err, const char *fmt, ...
) {
    (void)file;
    (void)line;
    (void)function;
    (void)err;
    (void)fmt;
}
