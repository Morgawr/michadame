/* Used by scripts/test-steam-deck-capture.py with a private PipeWire daemon. */
#include <stdint.h>
#include <stdatomic.h>
#include <stdio.h>
#include <unistd.h>

struct deck_capture;
typedef void (*frame_cb)(void *, const uint8_t *, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t);
typedef void (*state_cb)(void *, int, const char *);
struct deck_capture *michadame_deck_capture_start(const char *, frame_cb, state_cb, void *);
void michadame_deck_capture_stop(struct deck_capture *);
static atomic_int frames = 0;

static void frame(void *context, const uint8_t *pixels, uint32_t size,
                  uint32_t width, uint32_t height, uint32_t stride, uint32_t bgr) {
    (void)context;
    if (width == 1280 && height == 800 && size >= (height - 1) * stride + width * 4 &&
            bgr == 1 && pixels[0] == 0 && pixels[1] == 0 && pixels[2] == 255) {
        if (atomic_fetch_add(&frames, 1) == 0)
            printf("Verified red frame: %ux%u stride=%u bgr=%u\n", width, height, stride, bgr);
    } else {
        fprintf(stderr, "Unexpected frame: %ux%u stride=%u bgr=%u\n", width, height, stride, bgr);
    }
}

static void state(void *context, int state, const char *error) {
    (void)context;
    fprintf(stderr, "state=%d %s\n", state, error ? error : "");
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    struct deck_capture *capture = michadame_deck_capture_start(argv[1], frame, state, NULL);
    if (!capture) return 3;
    for (int i = 0; i < 600 && atomic_load(&frames) < 30; i++) usleep(10000);
    michadame_deck_capture_stop(capture);
    printf("Received %d valid frames\n", atomic_load(&frames));
    return atomic_load(&frames) >= 30 ? 0 : 1;
}
