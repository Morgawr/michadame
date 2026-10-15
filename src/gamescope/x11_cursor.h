#ifndef GAMESCOPE_X11_CURSOR_H
#define GAMESCOPE_X11_CURSOR_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct gamescope_x11_cursor_monitor gamescope_x11_cursor_monitor;

typedef struct gamescope_x11_cursor_image {
    unsigned short width;
    unsigned short height;
    unsigned short xhot;
    unsigned short yhot;
    unsigned long cursor_serial;
    const unsigned char *rgba_pixels;
} gamescope_x11_cursor_image;

typedef void (*gamescope_x11_cursor_cb)(
    int is_hidden,
    const gamescope_x11_cursor_image *image,
    void *user_data
);

gamescope_x11_cursor_monitor *gamescope_x11_cursor_monitor_start(
    const char *display_name,
    gamescope_x11_cursor_cb cb,
    void *user_data
);

void gamescope_x11_cursor_monitor_stop(gamescope_x11_cursor_monitor *monitor);

#ifdef __cplusplus
}
#endif

#endif // GAMESCOPE_X11_CURSOR_H
