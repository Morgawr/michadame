#include "x11_cursor.h"

#include <X11/Xlib.h>
#include <X11/extensions/Xfixes.h>
#include <pthread.h>
#include <poll.h>
#include <sys/eventfd.h>
#include <unistd.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <errno.h>

struct gamescope_x11_cursor_monitor {
    pthread_t thread;
    int stop_fd;
    char *display_name;
    gamescope_x11_cursor_cb cb;
    void *user_data;
    volatile int stop_requested;
};

static void query_and_notify_cursor(
    Display *dpy,
    gamescope_x11_cursor_cb cb,
    void *user_data,
    int *last_hidden,
    unsigned long *last_serial
) {
    if (!cb) return;

    XFixesCursorImage *cur = XFixesGetCursorImage(dpy);
    if (!cur) {
        if (*last_hidden != 1) {
            *last_hidden = 1;
            cb(1, NULL, user_data);
        }
        return;
    }

    int empty = 1;
    if (cur->width > 0 && cur->height > 0 && cur->pixels != NULL) {
        size_t count = (size_t)cur->width * (size_t)cur->height;
        for (size_t i = 0; i < count; i++) {
            if ((cur->pixels[i] >> 24) & 0xFF) {
                empty = 0;
                break;
            }
        }
    }

    if (empty) {
        if (*last_hidden != 1) {
            *last_hidden = 1;
            cb(1, NULL, user_data);
        }
    } else {
        int visibility_changed = (*last_hidden != 0);
        int serial_changed = (cur->cursor_serial != *last_serial);

        if (visibility_changed || serial_changed) {
            *last_hidden = 0;
            *last_serial = cur->cursor_serial;

            size_t count = (size_t)cur->width * (size_t)cur->height;
            unsigned char *rgba = (unsigned char *)malloc(count * 4);
            if (rgba) {
                for (size_t i = 0; i < count; i++) {
                    unsigned long p = cur->pixels[i];
                    rgba[i * 4 + 0] = (unsigned char)((p >> 16) & 0xFF); // R
                    rgba[i * 4 + 1] = (unsigned char)((p >> 8) & 0xFF);  // G
                    rgba[i * 4 + 2] = (unsigned char)(p & 0xFF);         // B
                    rgba[i * 4 + 3] = (unsigned char)((p >> 24) & 0xFF); // A
                }

                gamescope_x11_cursor_image img;
                img.width = cur->width;
                img.height = cur->height;
                img.xhot = cur->xhot;
                img.yhot = cur->yhot;
                img.cursor_serial = cur->cursor_serial;
                img.rgba_pixels = rgba;

                cb(0, &img, user_data);
                free(rgba);
            }
        }
    }

    XFree(cur);
}

static void *monitor_thread_func(void *arg) {
    gamescope_x11_cursor_monitor *m = (gamescope_x11_cursor_monitor *)arg;

    Display *dpy = NULL;
    // Retry connecting for up to 5 seconds (50 * 100ms) in case Xwayland is initializing
    for (int retry = 0; retry < 50 && !m->stop_requested; retry++) {
        dpy = XOpenDisplay(m->display_name);
        if (dpy) {
            break;
        }
        usleep(100000);
    }

    if (!dpy) {
        return NULL;
    }

    int event_base = 0, error_base = 0;
    if (!XFixesQueryExtension(dpy, &event_base, &error_base)) {
        XCloseDisplay(dpy);
        return NULL;
    }

    Window root = DefaultRootWindow(dpy);
    XFixesSelectCursorInput(dpy, root, XFixesDisplayCursorNotifyMask);

    int last_hidden = -1;
    unsigned long last_serial = 0;
    query_and_notify_cursor(dpy, m->cb, m->user_data, &last_hidden, &last_serial);

    int x11_fd = ConnectionNumber(dpy);

    while (!m->stop_requested) {
        int cursor_event_received = 0;
        while (XPending(dpy)) {
            XEvent ev;
            XNextEvent(dpy, &ev);
            if (ev.type == event_base + XFixesCursorNotify) {
                cursor_event_received = 1;
            }
        }
        if (cursor_event_received) {
            query_and_notify_cursor(dpy, m->cb, m->user_data, &last_hidden, &last_serial);
        }

        struct pollfd fds[2];
        fds[0].fd = x11_fd;
        fds[0].events = POLLIN;
        fds[1].fd = m->stop_fd;
        fds[1].events = POLLIN;

        int ret = poll(fds, 2, -1);
        if (ret < 0) {
            if (errno == EINTR) continue;
            break;
        }
        if (fds[1].revents & POLLIN) {
            break;
        }
    }

    XCloseDisplay(dpy);
    return NULL;
}

gamescope_x11_cursor_monitor *gamescope_x11_cursor_monitor_start(
    const char *display_name,
    gamescope_x11_cursor_cb cb,
    void *user_data
) {
    if (!display_name || !*display_name) {
        return NULL;
    }

    gamescope_x11_cursor_monitor *m = calloc(1, sizeof(*m));
    if (!m) return NULL;

    m->display_name = strdup(display_name);
    m->cb = cb;
    m->user_data = user_data;
    m->stop_fd = eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC);
    if (m->stop_fd < 0) {
        free(m->display_name);
        free(m);
        return NULL;
    }

    if (pthread_create(&m->thread, NULL, monitor_thread_func, m) != 0) {
        close(m->stop_fd);
        free(m->display_name);
        free(m);
        return NULL;
    }

    return m;
}

void gamescope_x11_cursor_monitor_stop(gamescope_x11_cursor_monitor *m) {
    if (!m) return;

    m->stop_requested = 1;
    uint64_t one = 1;
    ssize_t s = write(m->stop_fd, &one, sizeof(one));
    (void)s;

    pthread_join(m->thread, NULL);

    close(m->stop_fd);
    free(m->display_name);
    free(m);
}
