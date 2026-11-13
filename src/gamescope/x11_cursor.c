#include "x11_cursor.h"

#include <xcb/xcb.h>
#include <xcb/xproto.h>
#include <xcb/xcbext.h>
#include <X11/extensions/xfixeswire.h>
#include <pthread.h>
#include <poll.h>
#include <sys/eventfd.h>
#include <sys/uio.h>
#include <unistd.h>
#include <stdbool.h>
#include <stdlib.h>
#include <stdint.h>
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

typedef struct {
    uint8_t  req_type;
    uint8_t  xfixes_req_type;
    uint16_t length;
    uint32_t major_version;
    uint32_t minor_version;
} xfixes_query_version_req_t;

typedef struct {
    uint8_t      req_type;
    uint8_t      xfixes_req_type;
    uint16_t     length;
    xcb_window_t window;
    uint32_t     event_mask;
} xfixes_select_cursor_input_req_t;

typedef struct {
    uint8_t  req_type;
    uint8_t  xfixes_req_type;
    uint16_t length;
} xfixes_get_cursor_image_req_t;

typedef struct {
    uint8_t  response_type;
    uint8_t  pad0;
    uint16_t sequence;
    uint32_t length;
    int16_t  x;
    int16_t  y;
    uint16_t width;
    uint16_t height;
    uint16_t xhot;
    uint16_t yhot;
    uint32_t cursor_serial;
    uint32_t pad1;
    uint32_t pad2;
} xfixes_get_cursor_image_reply_t;

typedef struct {
    uint8_t         response_type;
    uint8_t         subtype;
    uint16_t        sequence;
    xcb_window_t    window;
    uint32_t        cursor_serial;
    xcb_timestamp_t timestamp;
    xcb_atom_t      name;
    uint32_t        pad0;
    uint32_t        pad1;
    uint32_t        pad2;
} xfixes_cursor_notify_event_t;

static int xfixes_query_version(xcb_connection_t *conn, uint8_t major_opcode) {
    xfixes_query_version_req_t req = {
        .req_type = major_opcode,
        .xfixes_req_type = X_XFixesQueryVersion,
        .length = 3,
        .major_version = 4,
        .minor_version = 0,
    };
    struct iovec iov[3];
    memset(iov, 0, sizeof(iov));
    iov[2].iov_base = &req;
    iov[2].iov_len = sizeof(req);
    static const xcb_protocol_request_t proto_req = {
        .count = 1,
        .ext = NULL,
        .opcode = X_XFixesQueryVersion,
        .isvoid = 0,
    };
    unsigned int seq = xcb_send_request(conn, XCB_REQUEST_RAW, &iov[2], &proto_req);
    if (!seq) {
        return 0;
    }
    void *reply = xcb_wait_for_reply(conn, seq, NULL);
    if (!reply) {
        return 0;
    }
    free(reply);
    return !xcb_connection_has_error(conn);
}

static int xfixes_select_cursor_input(
    xcb_connection_t *conn,
    uint8_t major_opcode,
    xcb_window_t window,
    uint32_t event_mask
) {
    xfixes_select_cursor_input_req_t req = {
        .req_type = major_opcode,
        .xfixes_req_type = X_XFixesSelectCursorInput,
        .length = 3,
        .window = window,
        .event_mask = event_mask,
    };
    struct iovec iov[3];
    memset(iov, 0, sizeof(iov));
    iov[2].iov_base = &req;
    iov[2].iov_len = sizeof(req);
    static const xcb_protocol_request_t proto_req = {
        .count = 1,
        .ext = NULL,
        .opcode = X_XFixesSelectCursorInput,
        .isvoid = 1,
    };
    unsigned int seq = xcb_send_request(conn, XCB_REQUEST_RAW, &iov[2], &proto_req);
    if (!seq) {
        return 0;
    }
    return xcb_flush(conn) > 0 && !xcb_connection_has_error(conn);
}

static xfixes_get_cursor_image_reply_t *xfixes_get_cursor_image(
    xcb_connection_t *conn,
    uint8_t major_opcode
) {
    xfixes_get_cursor_image_req_t req = {
        .req_type = major_opcode,
        .xfixes_req_type = X_XFixesGetCursorImage,
        .length = 1,
    };
    struct iovec iov[3];
    memset(iov, 0, sizeof(iov));
    iov[2].iov_base = &req;
    iov[2].iov_len = sizeof(req);
    static const xcb_protocol_request_t proto_req = {
        .count = 1,
        .ext = NULL,
        .opcode = X_XFixesGetCursorImage,
        .isvoid = 0,
    };
    unsigned int seq = xcb_send_request(conn, XCB_REQUEST_RAW, &iov[2], &proto_req);
    if (!seq) {
        return NULL;
    }
    return (xfixes_get_cursor_image_reply_t *)xcb_wait_for_reply(conn, seq, NULL);
}

static int query_and_notify_cursor(
    xcb_connection_t *conn,
    uint8_t major_opcode,
    gamescope_x11_cursor_cb cb,
    void *user_data,
    int *last_hidden,
    uint32_t *last_cursor_serial,
    bool *has_last_serial
) {
    if (!cb) return 1;

    xfixes_get_cursor_image_reply_t *cur = xfixes_get_cursor_image(conn, major_opcode);
    if (!cur) {
        if (xcb_connection_has_error(conn)) {
            return 0;
        }
        if (*last_hidden != 1) {
            *last_hidden = 1;
            cb(1, NULL, user_data);
        }
        return 1;
    }

    size_t count = (size_t)cur->width * (size_t)cur->height;
    const uint32_t *pixels = (const uint32_t *)(cur + 1);
    int has_pixels = (cur->width > 0 && cur->height > 0 && (size_t)cur->length >= count);

    int empty = 1;
    if (has_pixels) {
        for (size_t i = 0; i < count; i++) {
            if ((pixels[i] >> 24) & 0xFF) {
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
        uint32_t serial = cur->cursor_serial;
        int serial_changed = (!*has_last_serial || serial != *last_cursor_serial);

        if (visibility_changed || serial_changed) {
            *last_hidden = 0;
            *last_cursor_serial = serial;
            *has_last_serial = true;

            unsigned char *rgba = (unsigned char *)malloc(count * 4);
            if (rgba) {
                for (size_t i = 0; i < count; i++) {
                    uint32_t p = pixels[i];
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
                img.cursor_serial = (unsigned long)serial;
                img.rgba_pixels = rgba;

                cb(0, &img, user_data);
                free(rgba);
            }
        }
    }

    free(cur);
    return 1;
}

static xcb_window_t root_window_for_screen(xcb_connection_t *conn, int screen_num) {
    const xcb_setup_t *setup = xcb_get_setup(conn);
    if (!setup) {
        return XCB_NONE;
    }
    xcb_screen_iterator_t iter = xcb_setup_roots_iterator(setup);
    for (int i = 0; iter.rem > 0; i++, xcb_screen_next(&iter)) {
        if (i == screen_num || screen_num < 0) {
            return iter.data->root;
        }
    }
    iter = xcb_setup_roots_iterator(setup);
    return iter.rem > 0 ? iter.data->root : XCB_NONE;
}

static void *monitor_thread_func(void *arg) {
    gamescope_x11_cursor_monitor *m = (gamescope_x11_cursor_monitor *)arg;

    xcb_connection_t *conn = NULL;
    int screen_num = 0;
    // Retry connecting for up to 5 seconds (50 * 100ms) in case Xwayland is initializing
    for (int retry = 0; retry < 50 && !m->stop_requested; retry++) {
        conn = xcb_connect(m->display_name, &screen_num);
        if (conn && !xcb_connection_has_error(conn)) {
            break;
        }
        if (conn) {
            xcb_disconnect(conn);
            conn = NULL;
        }
        struct pollfd stop_pfd;
        stop_pfd.fd = m->stop_fd;
        stop_pfd.events = POLLIN;
        stop_pfd.revents = 0;
        int ret = poll(&stop_pfd, 1, 100);
        if (ret > 0 && (stop_pfd.revents & POLLIN)) {
            break;
        }
    }

    if (!conn) {
        if (!m->stop_requested && m->cb) {
            m->cb(-1, NULL, m->user_data);
        }
        return NULL;
    }

    xcb_query_extension_cookie_t ext_cookie =
        xcb_query_extension(conn, (uint16_t)strlen(XFIXES_NAME), XFIXES_NAME);
    xcb_query_extension_reply_t *ext_reply =
        xcb_query_extension_reply(conn, ext_cookie, NULL);
    if (!ext_reply || !ext_reply->present) {
        free(ext_reply);
        xcb_disconnect(conn);
        if (!m->stop_requested && m->cb) {
            m->cb(-1, NULL, m->user_data);
        }
        return NULL;
    }

    uint8_t major_opcode = ext_reply->major_opcode;
    uint8_t first_event = ext_reply->first_event;
    free(ext_reply);

    xcb_window_t root = root_window_for_screen(conn, screen_num);
    if (root == XCB_NONE ||
        !xfixes_query_version(conn, major_opcode) ||
        !xfixes_select_cursor_input(conn, major_opcode, root, XFixesDisplayCursorNotifyMask)) {
        xcb_disconnect(conn);
        if (!m->stop_requested && m->cb) {
            m->cb(-1, NULL, m->user_data);
        }
        return NULL;
    }

    int last_hidden = -1;
    uint32_t last_cursor_serial = 0;
    bool has_last_serial = false;
    if (!query_and_notify_cursor(
            conn,
            major_opcode,
            m->cb,
            m->user_data,
            &last_hidden,
            &last_cursor_serial,
            &has_last_serial)) {
        xcb_disconnect(conn);
        if (!m->stop_requested && m->cb) {
            m->cb(-1, NULL, m->user_data);
        }
        return NULL;
    }

    int x11_fd = xcb_get_file_descriptor(conn);

    while (!m->stop_requested) {
        if (xcb_connection_has_error(conn)) {
            break;
        }

        // Drain all events already buffered in libxcb's user-space queue before calling poll()
        // so burst events read during xcb_wait_for_reply() are never stranded behind poll().
        int connection_lost = 0;
        for (;;) {
            int need_cursor_query = 0;
            xcb_generic_event_t *ev = NULL;
            while ((ev = xcb_poll_for_queued_event(conn)) != NULL) {
                uint8_t response_type = ev->response_type & ~0x80;
                if (response_type == first_event + XFixesCursorNotify) {
                    const xfixes_cursor_notify_event_t *notify =
                        (const xfixes_cursor_notify_event_t *)ev;
                    bool is_hidden = (last_hidden != 0);
                    if (!(!is_hidden && has_last_serial &&
                          notify->cursor_serial == last_cursor_serial)) {
                        need_cursor_query = 1;
                    }
                }
                free(ev);
            }

            if (xcb_connection_has_error(conn)) {
                connection_lost = 1;
                break;
            }
            if (!need_cursor_query) {
                break;
            }
            if (!query_and_notify_cursor(
                    conn,
                    major_opcode,
                    m->cb,
                    m->user_data,
                    &last_hidden,
                    &last_cursor_serial,
                    &has_last_serial)) {
                connection_lost = 1;
                break;
            }
        }

        if (connection_lost || xcb_connection_has_error(conn)) {
            break;
        }

        struct pollfd fds[2];
        fds[0].fd = x11_fd;
        fds[0].events = POLLIN;
        fds[0].revents = 0;
        fds[1].fd = m->stop_fd;
        fds[1].events = POLLIN;
        fds[1].revents = 0;

        int ret = poll(fds, 2, -1);
        if (ret < 0) {
            if (errno == EINTR) continue;
            break;
        }
        if (fds[1].revents & POLLIN) {
            break;
        }
        if (fds[0].revents & (POLLERR | POLLNVAL)) {
            break;
        }

        if (fds[0].revents & (POLLIN | POLLHUP)) {
            int need_cursor_query = 0;
            xcb_generic_event_t *ev = NULL;
            while ((ev = xcb_poll_for_event(conn)) != NULL) {
                uint8_t response_type = ev->response_type & ~0x80;
                if (response_type == first_event + XFixesCursorNotify) {
                    const xfixes_cursor_notify_event_t *notify =
                        (const xfixes_cursor_notify_event_t *)ev;
                    bool is_hidden = (last_hidden != 0);
                    if (!(!is_hidden && has_last_serial &&
                          notify->cursor_serial == last_cursor_serial)) {
                        need_cursor_query = 1;
                    }
                }
                free(ev);
            }

            if (xcb_connection_has_error(conn)) {
                break;
            }

            if (need_cursor_query) {
                if (!query_and_notify_cursor(
                        conn,
                        major_opcode,
                        m->cb,
                        m->user_data,
                        &last_hidden,
                        &last_cursor_serial,
                        &has_last_serial)) {
                    break;
                }
            }
        }
    }

    xcb_disconnect(conn);
    if (!m->stop_requested && m->cb) {
        m->cb(-1, NULL, m->user_data);
    }
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
