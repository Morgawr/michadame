/* Deck/Xwayland capture uses mapped PipeWire memory, avoiding EGLImage imports
 * into a GLX context. The desktop DMA-BUF capture implementation is unchanged. */
#include <pipewire/pipewire.h>
#include <spa/param/video/format-utils.h>
#include <spa/param/buffers.h>
#include <stdatomic.h>
#include <stdlib.h>

typedef void (*deck_frame_cb)(void *, const uint8_t *, uint32_t, uint32_t,
                              uint32_t, uint32_t, uint32_t);
typedef void (*deck_state_cb)(void *, int, const char *);

struct deck_capture {
    struct pw_thread_loop *loop;
    struct pw_stream *stream;
    struct spa_video_info_raw info;
    deck_frame_cb on_frame;
    deck_state_cb on_state;
    void *data;
    atomic_bool stopping;
};

static void state_changed(void *data, enum pw_stream_state old,
                          enum pw_stream_state state, const char *error) {
    (void)old;
    struct deck_capture *capture = data;
    if (!atomic_load(&capture->stopping) && capture->on_state)
        capture->on_state(capture->data, state, error);
}

static void format_changed(void *data, uint32_t id, const struct spa_pod *param) {
    struct deck_capture *capture = data;
    if (id != SPA_PARAM_Format || !param) return;
    if (spa_format_video_raw_parse(param, &capture->info) < 0) return;
    const uint32_t width = capture->info.size.width, height = capture->info.size.height;
    if (!width || !height || width > 8192 || height > 8192) return;
    uint8_t buffer[512];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(buffer, sizeof(buffer));
    const struct spa_pod *params[] = {
        spa_pod_builder_add_object(&builder, SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
            SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(4, 2, 8),
            SPA_PARAM_BUFFERS_blocks, SPA_POD_Int(1),
            SPA_PARAM_BUFFERS_size, SPA_POD_Int(width * height * 4),
            SPA_PARAM_BUFFERS_stride, SPA_POD_Int(width * 4),
            SPA_PARAM_BUFFERS_dataType, SPA_POD_CHOICE_FLAGS_Int(1 << SPA_DATA_MemFd))
    };
    pw_stream_update_params(capture->stream, params, 1);
}

static void process(void *data) {
    struct deck_capture *capture = data;
    struct pw_buffer *buffer = pw_stream_dequeue_buffer(capture->stream);
    if (!buffer) return;
    const uint32_t width = capture->info.size.width, height = capture->info.size.height;
    struct spa_buffer *spa = buffer->buffer;
    if (!atomic_load(&capture->stopping) && spa && spa->n_datas == 1 &&
            width && height && width <= 8192 && height <= 8192) {
        struct spa_data *pixels = &spa->datas[0];
        struct spa_chunk *chunk = pixels->chunk;
        if (pixels->data && chunk && chunk->stride >= (int32_t)(width * 4) &&
                !(chunk->flags & SPA_CHUNK_FLAG_CORRUPTED) &&
                chunk->offset <= pixels->maxsize && chunk->size <= pixels->maxsize - chunk->offset) {
            uint64_t needed = (uint64_t)(height - 1) * chunk->stride + width * 4;
            if (needed <= chunk->size && capture->on_frame) {
                uint32_t bgr = capture->info.format == SPA_VIDEO_FORMAT_BGRx ||
                    capture->info.format == SPA_VIDEO_FORMAT_BGRA;
                capture->on_frame(capture->data, (const uint8_t *)pixels->data + chunk->offset,
                    chunk->size, width, height, chunk->stride, bgr);
            }
        }
    }
    pw_stream_queue_buffer(capture->stream, buffer);
}

static const struct pw_stream_events events = {
    PW_VERSION_STREAM_EVENTS,
    .state_changed = state_changed,
    .param_changed = format_changed,
    .process = process,
};

void michadame_deck_capture_stop(struct deck_capture *capture) {
    if (!capture) return;
    atomic_store(&capture->stopping, true);
    pw_thread_loop_stop(capture->loop);
    if (capture->stream) pw_stream_destroy(capture->stream);
    pw_thread_loop_destroy(capture->loop);
    free(capture);
}

struct deck_capture *michadame_deck_capture_start(const char *target,
        deck_frame_cb frame, deck_state_cb state, void *data) {
    if (!target || !target[0]) return NULL;
    pw_init(NULL, NULL);
    struct deck_capture *capture = calloc(1, sizeof(*capture));
    if (!capture) return NULL;
    capture->on_frame = frame;
    capture->on_state = state;
    capture->data = data;
    atomic_init(&capture->stopping, false);
    capture->loop = pw_thread_loop_new("deck-video", NULL);
    if (!capture->loop) { free(capture); return NULL; }
    capture->stream = pw_stream_new_simple(pw_thread_loop_get_loop(capture->loop),
        "michadame-deck-video", pw_properties_new(
            PW_KEY_MEDIA_TYPE, "Video", PW_KEY_MEDIA_CATEGORY, "Capture",
            PW_KEY_TARGET_OBJECT, target, NULL), &events, capture);
    if (!capture->stream) { michadame_deck_capture_stop(capture); return NULL; }
    uint8_t buffer[1024];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(buffer, sizeof(buffer));
    /* Omitting SPA_FORMAT_VIDEO_modifier selects Gamescope's MemFd offer. */
    const struct spa_pod *params[] = {
        spa_pod_builder_add_object(&builder, SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat,
            SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_video),
            SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
            SPA_FORMAT_VIDEO_format, SPA_POD_CHOICE_ENUM_Id(5,
                SPA_VIDEO_FORMAT_BGRx, SPA_VIDEO_FORMAT_BGRx, SPA_VIDEO_FORMAT_RGBA, SPA_VIDEO_FORMAT_BGRA, SPA_VIDEO_FORMAT_RGBx),
            SPA_FORMAT_VIDEO_size, SPA_POD_CHOICE_RANGE_Rectangle(
                &SPA_RECTANGLE(1280, 800), &SPA_RECTANGLE(1, 1), &SPA_RECTANGLE(8192, 8192)),
            SPA_FORMAT_VIDEO_framerate, SPA_POD_CHOICE_RANGE_Fraction(
                &SPA_FRACTION(60, 1), &SPA_FRACTION(0, 1), &SPA_FRACTION(60, 1)))
    };
    if (pw_stream_connect(capture->stream, PW_DIRECTION_INPUT, PW_ID_ANY,
            PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS, params, 1) < 0 ||
            pw_thread_loop_start(capture->loop) < 0) {
        michadame_deck_capture_stop(capture);
        return NULL;
    }
    return capture;
}
