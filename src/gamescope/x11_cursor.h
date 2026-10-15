#ifndef GAMESCOPE_X11_CURSOR_H
#define GAMESCOPE_X11_CURSOR_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct gamescope_x11_cursor_monitor gamescope_x11_cursor_monitor;

typedef void (*gamescope_x11_cursor_cb)(int is_hidden, void *user_data);

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
