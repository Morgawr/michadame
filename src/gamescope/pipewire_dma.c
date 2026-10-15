#include "pipewire_dma.h"

#include <pipewire/pipewire.h>
#include <spa/param/video/format-utils.h>
#include <spa/param/video/raw.h>
#include <spa/param/buffers.h>
#include <spa/debug/types.h>
#include <drm/drm_fourcc.h>

#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>

struct gamescope_dma_stream {
    struct pw_thread_loop *loop;
    struct pw_context *context;
    struct pw_stream *stream;

    gamescope_dma_frame_cb on_frame;
    gamescope_dma_state_cb on_state;
    void *user_data;

    uint32_t width;
    uint32_t height;
    uint32_t drm_fourcc;
    uint64_t modifier;
    bool has_format;
    uint32_t next_buffer_id;
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

static void on_stream_state_changed(void *data, enum pw_stream_state old,
                                    enum pw_stream_state state, const char *error) {
    (void)old;
    struct gamescope_dma_stream *s = (struct gamescope_dma_stream *)data;
    if (!s || !s->on_state) return;

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
    s->on_state(s->user_data, code, error);
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

    s->width = info.size.width;
    s->height = info.size.height;
    s->drm_fourcc = spa_format_to_drm_fourcc(info.format);
    s->modifier = (info.flags & SPA_VIDEO_FLAG_MODIFIER) ? info.modifier : DRM_FORMAT_MOD_LINEAR;
    s->has_format = true;

    pw_log_info("Gamescope format negotiated: %ux%u (DRM FourCC: 0x%08x, modifier: 0x%lx)",
                s->width, s->height, s->drm_fourcc, (unsigned long)s->modifier);

    // After format is negotiated, update stream parameters to request DMA-BUF / MemFd buffers
    uint8_t buffer_bufs[512];
    struct spa_pod_builder b_buf = SPA_POD_BUILDER_INIT(buffer_bufs, sizeof(buffer_bufs));
    const struct spa_pod *b_params[1];
    b_params[0] = (const struct spa_pod *)spa_pod_builder_add_object(&b_buf,
        SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
        SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(4, 2, 8),
        SPA_PARAM_BUFFERS_blocks, SPA_POD_Int(1),
        SPA_PARAM_BUFFERS_dataType, SPA_POD_CHOICE_FLAGS_Int(
            (1 << SPA_DATA_DmaBuf) | (1 << SPA_DATA_MemFd)));

    pw_stream_update_params(s->stream, b_params, 1);
}

static void on_stream_add_buffer(void *data, struct pw_buffer *b) {
    struct gamescope_dma_stream *s = (struct gamescope_dma_stream *)data;
    if (!s || !b) return;
    s->next_buffer_id++;
    b->user_data = (void *)(uintptr_t)s->next_buffer_id;
}

static void on_stream_remove_buffer(void *data, struct pw_buffer *b) {
    (void)data;
    if (b) {
        b->user_data = NULL;
    }
}

static void on_stream_process(void *data) {
    struct gamescope_dma_stream *s = (struct gamescope_dma_stream *)data;
    if (!s) return;

    struct pw_buffer *b = pw_stream_dequeue_buffer(s->stream);
    if (!b) return;

    if (!b->user_data) {
        s->next_buffer_id++;
        b->user_data = (void *)(uintptr_t)s->next_buffer_id;
    }
    uint32_t buffer_id = (uint32_t)(uintptr_t)b->user_data;

    struct spa_buffer *buf = b->buffer;
    if (buf && buf->n_datas > 0 && s->has_format) {
        struct spa_data *d = &buf->datas[0];
        if (d->type == SPA_DATA_DmaBuf && d->fd >= 0) {
            uint32_t stride = (d->chunk && d->chunk->stride > 0) ? (uint32_t)d->chunk->stride : (s->width * 4);

            if (s->on_frame) {
                s->on_frame(
                    s->user_data,
                    buffer_id,
                    d->fd,
                    s->width,
                    s->height,
                    stride,
                    s->drm_fourcc,
                    s->modifier
                );
            }
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
    gamescope_dma_frame_cb on_frame,
    gamescope_dma_state_cb on_state,
    void *user_data
) {
    pw_init(NULL, NULL);

    struct gamescope_dma_stream *s = calloc(1, sizeof(struct gamescope_dma_stream));
    if (!s) return NULL;

    s->on_frame = on_frame;
    s->on_state = on_state;
    s->user_data = user_data;

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
    uint8_t buffer_format[1024];
    struct spa_pod_builder b_fmt = SPA_POD_BUILDER_INIT(buffer_format, sizeof(buffer_format));

    const struct spa_pod *params[2];
    params[0] = (const struct spa_pod *)spa_pod_builder_add_object(&b_fmt,
        SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat,
        SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_video),
        SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
        SPA_FORMAT_VIDEO_format, SPA_POD_CHOICE_ENUM_Id(5,
            SPA_VIDEO_FORMAT_BGRx,
            SPA_VIDEO_FORMAT_BGRx,
            SPA_VIDEO_FORMAT_BGRA,
            SPA_VIDEO_FORMAT_RGBA,
            SPA_VIDEO_FORMAT_NV12),
        SPA_FORMAT_VIDEO_size, SPA_POD_CHOICE_RANGE_Rectangle(
            &SPA_RECTANGLE(3840, 2160),
            &SPA_RECTANGLE(320, 240),
            &SPA_RECTANGLE(4096, 4096)),
        SPA_FORMAT_VIDEO_framerate, SPA_POD_CHOICE_RANGE_Fraction(
            &SPA_FRACTION(120, 1),
            &SPA_FRACTION(0, 1),
            &SPA_FRACTION(1000, 1)),
        SPA_FORMAT_VIDEO_modifier, SPA_POD_CHOICE_ENUM_Long(2,
            (long long)DRM_FORMAT_MOD_LINEAR,
            (long long)DRM_FORMAT_MOD_LINEAR));

    int ret = pw_stream_connect(
        s->stream,
        PW_DIRECTION_INPUT,
        PW_ID_ANY,
        PW_STREAM_FLAG_AUTOCONNECT,
        params,
        1
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
