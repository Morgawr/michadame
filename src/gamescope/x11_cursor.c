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

static int is_cursor_empty(Display *dpy) {
    XFixesCursorImage *cur = XFixesGetCursorImage(dpy);
    if (!cur) {
        return 1;
    }
    int empty = 1;
    if (cur->width > 0 && cur->height > 0 && cur->pixels != NULL) {
        size_t count = (size_t)cur->width * (size_t)cur->height;
        for (size_t i = 0; i < count; i++) {
            unsigned long p = cur->pixels[i];
            if ((p >> 24) & 0xFF) {
                empty = 0;
                break;
            }
        }
    }
    XFree(cur);
    return empty;
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

    int last_hidden = is_cursor_empty(dpy);
    if (m->cb) {
        m->cb(last_hidden, m->user_data);
    }

    int x11_fd = ConnectionNumber(dpy);

    while (!m->stop_requested) {
        while (XPending(dpy)) {
            XEvent ev;
            XNextEvent(dpy, &ev);
            if (ev.type == event_base + XFixesCursorNotify) {
                int hidden = is_cursor_empty(dpy);
                if (hidden != last_hidden) {
                    last_hidden = hidden;
                    if (m->cb) {
                        m->cb(hidden, m->user_data);
                    }
                }
            }
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
