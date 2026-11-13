#ifndef GAMESCOPE_PIPEWIRE_DMA_H
#define GAMESCOPE_PIPEWIRE_DMA_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define GAMESCOPE_MAX_DMA_PLANES 4

typedef void (*gamescope_dma_frame_cb)(
    void *user_data,
    uint32_t buffer_id,
    uint64_t pool_generation,
    uint32_t n_planes,
    const int *fds,
    const uint32_t *offsets,
    const uint32_t *strides,
    uint32_t width,
    uint32_t height,
    uint32_t drm_fourcc,
    uint64_t modifier
);

typedef void (*gamescope_dma_state_cb)(
    void *user_data,
    int state_code,
    const char *error_msg
);

typedef void (*gamescope_dma_remove_buffer_cb)(
    void *user_data,
    uint32_t buffer_id,
    uint64_t pool_generation
);

struct gamescope_dma_stream;

struct gamescope_dma_stream *gamescope_dma_stream_start(
    const char *target_object,
    const uint64_t *modifiers,
    size_t n_modifiers,
    gamescope_dma_frame_cb on_frame,
    gamescope_dma_state_cb on_state,
    gamescope_dma_remove_buffer_cb on_remove_buffer,
    void *user_data
);

void gamescope_dma_stream_stop(struct gamescope_dma_stream *stream);

#ifdef __cplusplus
}
#endif

#endif // GAMESCOPE_PIPEWIRE_DMA_H
