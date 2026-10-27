#ifndef GAMESCOPE_PIPEWIRE_DMA_H
#define GAMESCOPE_PIPEWIRE_DMA_H

#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*gamescope_dma_frame_cb)(
    void *user_data,
    uint32_t buffer_id,
    uint64_t pool_generation,
    int fd,
    uint32_t width,
    uint32_t height,
    uint32_t stride,
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
