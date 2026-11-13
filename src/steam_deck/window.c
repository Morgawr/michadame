#include <X11/Xlib.h>
#include <X11/Xatom.h>
#include <errno.h>
#include <poll.h>
#include <pthread.h>
#include <setjmp.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

typedef struct michadame_deck_focus_ctx {
    Display *viewer_dpy;
    char *viewer_display_name;
    Atom viewer_pid_atom;
    Window cached_viewer_win;
    uint32_t cached_viewer_pid;

    Display *ctrl_dpy;
    char *ctrl_display_name;
    Atom baselayer_window_atom;
    Atom baselayer_appid_atom;
    Atom steam_game_atom;
    Atom steam_input_focus_atom;
    Atom steam_overlay_atom;
} michadame_deck_focus_ctx;

static pthread_once_t g_xerror_once = PTHREAD_ONCE_INIT;
static XErrorHandler g_prev_xerror_handler = NULL;

static __thread Display *tls_owned_dpy_a = NULL;
static __thread Display *tls_owned_dpy_b = NULL;
static __thread volatile int tls_x_error_code = 0;
static __thread jmp_buf *tls_io_jmp = NULL;
static __thread Display *tls_io_failed_dpy = NULL;

static int michadame_x_error_handler(Display *dpy, XErrorEvent *ev) {
    if (dpy && (dpy == tls_owned_dpy_a || dpy == tls_owned_dpy_b)) {
        tls_x_error_code = ev ? (int)ev->error_code : 1;
        return 0;
    }
    if (g_prev_xerror_handler) {
        return g_prev_xerror_handler(dpy, ev);
    }
    return 0;
}

static void install_x_error_handler_once(void) {
    XInitThreads();
    g_prev_xerror_handler = XSetErrorHandler(michadame_x_error_handler);
}

static void michadame_x_io_error_exit(Display *dpy, void *user_data) {
    (void)user_data;
    tls_io_failed_dpy = dpy;
    if (tls_io_jmp) {
        longjmp(*tls_io_jmp, 1);
    }
}

static int is_display_socket_healthy(Display *dpy) {
    if (!dpy) return 0;
    int fd = ConnectionNumber(dpy);
    if (fd < 0) return 0;
    struct pollfd pfd;
    pfd.fd = fd;
    pfd.events = POLLIN;
    pfd.revents = 0;
    int pr = poll(&pfd, 1, 0);
    if (pr < 0) {
        return errno == EINTR;
    }
    if (pfd.revents & (POLLERR | POLLHUP | POLLNVAL)) {
        return 0;
    }
    if (pfd.revents & POLLIN) {
        char probe = 0;
        ssize_t n = recv(fd, &probe, 1, MSG_PEEK | MSG_DONTWAIT);
        if (n == 0) {
            return 0;
        }
        if (n < 0 && errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR) {
            return 0;
        }
    }
    return 1;
}

static void safe_close_display(Display **slot) {
    if (!slot || !*slot) return;
    Display *dpy = *slot;
    *slot = NULL;
    int fd = ConnectionNumber(dpy);
    Display *prev_a = tls_owned_dpy_a;
    jmp_buf *prev_jmp = tls_io_jmp;
    jmp_buf close_jmp;

    tls_owned_dpy_a = dpy;
    tls_io_failed_dpy = NULL;
    tls_io_jmp = &close_jmp;
    if (setjmp(close_jmp) == 0) {
        XCloseDisplay(dpy);
    } else if (fd >= 0) {
        close(fd);
    }
    tls_io_jmp = prev_jmp;
    tls_owned_dpy_a = prev_a;
}

static void reset_viewer_connection(michadame_deck_focus_ctx *ctx) {
    safe_close_display(&ctx->viewer_dpy);
    free(ctx->viewer_display_name);
    ctx->viewer_display_name = NULL;
    ctx->viewer_pid_atom = None;
    ctx->cached_viewer_win = None;
    ctx->cached_viewer_pid = 0;
}

static void reset_ctrl_connection(michadame_deck_focus_ctx *ctx) {
    safe_close_display(&ctx->ctrl_dpy);
    free(ctx->ctrl_display_name);
    ctx->ctrl_display_name = NULL;
    ctx->baselayer_window_atom = None;
    ctx->baselayer_appid_atom = None;
    ctx->steam_game_atom = None;
    ctx->steam_input_focus_atom = None;
    ctx->steam_overlay_atom = None;
}

static int display_name_matches(const char *cached, const char *requested) {
    const char *a = cached ? cached : "";
    const char *b = requested ? requested : "";
    return strcmp(a, b) == 0;
}

static char *dup_cstr_or_empty(const char *s) {
    const char *src = s ? s : "";
    size_t len = strlen(src);
    char *copy = (char *)malloc(len + 1);
    if (copy) {
        memcpy(copy, src, len + 1);
    }
    return copy;
}

static unsigned long cardinal(Display *display, Window window, Atom atom) {
    if (atom == None || window == None) return 0;
    Atom type;
    int format;
    unsigned long count, after, value = 0;
    unsigned char *data = NULL;
    tls_x_error_code = 0;
    if (XGetWindowProperty(display, window, atom, 0, 1, False, XA_CARDINAL,
            &type, &format, &count, &after, &data) == Success &&
            tls_x_error_code == 0 &&
            type == XA_CARDINAL && format == 32 && count == 1 && data) {
        value = (uint32_t)*(unsigned long *)data;
    }
    if (data) XFree(data);
    return value;
}

static Window find_window_by_pid(Display *display, Window root, Atom pid_atom,
                                 unsigned long pid, int depth) {
    if (root == None || pid_atom == None) return None;
    if (cardinal(display, root, pid_atom) == pid) return root;
    if (depth == 0) return None;
    Window returned_root, parent, *children = NULL, found = None;
    unsigned int count = 0;
    tls_x_error_code = 0;
    if (XQueryTree(display, root, &returned_root, &parent, &children, &count) &&
            tls_x_error_code == 0) {
        for (unsigned int i = 0; i < count && found == None; ++i)
            found = find_window_by_pid(display, children[i], pid_atom, pid, depth - 1);
    }
    if (children) XFree(children);
    return found;
}

static void refresh_ctrl_atoms(michadame_deck_focus_ctx *ctx) {
    Display *dpy = ctx->ctrl_dpy;
    if (!dpy) return;
    if (ctx->baselayer_window_atom == None)
        ctx->baselayer_window_atom = XInternAtom(dpy, "GAMESCOPECTRL_BASELAYER_WINDOW", True);
    if (ctx->baselayer_appid_atom == None)
        ctx->baselayer_appid_atom = XInternAtom(dpy, "GAMESCOPECTRL_BASELAYER_APPID", True);
    if (ctx->steam_game_atom == None)
        ctx->steam_game_atom = XInternAtom(dpy, "STEAM_GAME", True);
    if (ctx->steam_input_focus_atom == None)
        ctx->steam_input_focus_atom = XInternAtom(dpy, "STEAM_INPUT_FOCUS", True);
    if (ctx->steam_overlay_atom == None)
        ctx->steam_overlay_atom = XInternAtom(dpy, "STEAM_OVERLAY", True);
}

/* Steam's overlay precedes the game in this list while its menu is open. */
static uint32_t selected_app_atom(Display *display, Atom atom,
                                  uint32_t shortcut, uint32_t game) {
    if (atom == None) return 0;
    Atom type;
    int format;
    unsigned long count, after;
    unsigned char *data = NULL;
    uint32_t selected = 0;
    if (XGetWindowProperty(display, DefaultRootWindow(display),
            atom, 0, 1024, False, XA_CARDINAL, &type, &format,
            &count, &after, &data) == Success &&
            type == XA_CARDINAL && format == 32 && data) {
        unsigned long *apps = (unsigned long *)data;
        for (unsigned long i = 0; i < count; ++i) {
            uint32_t app = (uint32_t)apps[i];
            if (app == 769) break;
            if (app == shortcut || app == game) {
                selected = app;
                break;
            }
        }
    }
    if (data) XFree(data);
    return selected;
}

/* Inspect GAMESCOPECTRL_BASELAYER_APPID without server grab:
 * returns 2 if `game` precedes `shortcut` (requires property write),
 * returns 1 if `shortcut` is already active (`done`),
 * returns 0 otherwise. */
static int inspect_handoff_state(Display *display, Atom atom,
                                 uint32_t shortcut, uint32_t game) {
    if (atom == None) return 0;
    Atom type;
    int format;
    unsigned long count, after;
    unsigned char *data = NULL;
    int state = 0;
    if (XGetWindowProperty(display, DefaultRootWindow(display),
            atom, 0, 1024, False, XA_CARDINAL, &type, &format,
            &count, &after, &data) == Success &&
            type == XA_CARDINAL && format == 32 && data) {
        unsigned long *apps = (unsigned long *)data;
        for (unsigned long i = 0; i < count; ++i) {
            uint32_t app = (uint32_t)apps[i];
            if (app == shortcut) { state = 1; break; }
            if (app == game) { state = 2; break; }
        }
    }
    if (data) XFree(data);
    return state;
}

/* Complete Steam's launch handoff once the viewer has received a game frame.
 * Steam can leave its loading screen ahead of a game with no outer window.
 * Request our shortcut once, preserving the rest of the focus order. Future
 * menu and application selections remain owned by Steam. */
static int handoff_app_atom(Display *display, Atom atom,
                            uint32_t shortcut, uint32_t game) {
    if (atom == None) return 0;
    Atom type;
    int format;
    unsigned long count, after;
    unsigned char *data = NULL;
    int done = 0;
    if (XGetWindowProperty(display, DefaultRootWindow(display),
            atom, 0, 1024, False, XA_CARDINAL, &type, &format,
            &count, &after, &data) == Success &&
            type == XA_CARDINAL && format == 32 && data) {
        unsigned long *apps = (unsigned long *)data;
        unsigned long menu = count;
        for (unsigned long i = 0; i < count; ++i) {
            uint32_t app = (uint32_t)apps[i];
            if (app == 769 && menu == count) menu = i;
            if (app == shortcut) { done = 1; break; }
            if (app == game) {
                apps[i] = shortcut;
                if (menu < i) {
                    apps[i] = 769;
                    apps[menu] = shortcut;
                }
                XChangeProperty(display, DefaultRootWindow(display), atom,
                    XA_CARDINAL, 32, PropModeReplace, data, count);
                done = 1;
                break;
            }
        }
    }
    if (data) XFree(data);
    return done;
}

/* Big Picture keeps its main content view mapped while closed menus are
 * unmapped child views. The input-focus flag alone can remain set on the
 * background client, so only yield while an auxiliary content view is shown. */
static int steam_has_auxiliary_view(Display *display, Window steam) {
    Window root, parent, *children = NULL;
    unsigned int count = 0, visible = 0;
    tls_x_error_code = 0;
    if (XQueryTree(display, steam, &root, &parent, &children, &count) &&
            tls_x_error_code == 0) {
        for (unsigned int i = 0; i < count && visible < 2; ++i) {
            XWindowAttributes attributes;
            tls_x_error_code = 0;
            if (XGetWindowAttributes(display, children[i], &attributes) &&
                    tls_x_error_code == 0 &&
                    attributes.map_state == IsViewable &&
                    attributes.class == InputOutput &&
                    attributes.width > 1 && attributes.height > 1)
                ++visible;
        }
    }
    if (children) XFree(children);
    return visible >= 2;
}

/* Steam can draw its side menus as an ordinary Big Picture window, leaving
 * STEAM_OVERLAY=0 and the game's AppID first. Its input-focus request is still
 * authoritative: otherwise our viewer stays in front of a fully rendered menu. */
static Window requested_steam_menu_atoms(Display *display, Window root,
                                         Atom game, Atom input, Atom overlay) {
    if (game == None || input == None) return None;
    Window returned_root, parent, *children = NULL, found = None;
    unsigned int count = 0;
    tls_x_error_code = 0;
    if (XQueryTree(display, root, &returned_root, &parent, &children, &count) &&
            tls_x_error_code == 0) {
        for (unsigned int i = 0; i < count && found == None; ++i) {
            if (cardinal(display, children[i], game) != 769 ||
                    cardinal(display, children[i], input) != 1 ||
                    (overlay != None && cardinal(display, children[i], overlay) != 0)) continue;
            XWindowAttributes attributes;
            tls_x_error_code = 0;
            if (XGetWindowAttributes(display, children[i], &attributes) &&
                    tls_x_error_code == 0 &&
                    attributes.map_state == IsViewable &&
                    attributes.width > 1 && attributes.height > 1 &&
                    steam_has_auxiliary_view(display, children[i]))
                found = children[i];
        }
    }
    if (children) XFree(children);
    return found;
}

static int ensure_viewer_connection(michadame_deck_focus_ctx *ctx, const char *viewer_display) {
    if (ctx->viewer_dpy) {
        if (!display_name_matches(ctx->viewer_display_name, viewer_display) ||
                !is_display_socket_healthy(ctx->viewer_dpy)) {
            reset_viewer_connection(ctx);
        }
    }
    if (!ctx->viewer_dpy) {
        Display *dpy = XOpenDisplay(viewer_display);
        if (!dpy) return 0;
        XSetIOErrorExitHandler(dpy, michadame_x_io_error_exit, NULL);
        ctx->viewer_dpy = dpy;
        ctx->viewer_display_name = dup_cstr_or_empty(viewer_display);
        tls_owned_dpy_a = dpy;
        ctx->viewer_pid_atom = XInternAtom(dpy, "_NET_WM_PID", False);
        ctx->cached_viewer_win = None;
        ctx->cached_viewer_pid = 0;
    }
    return 1;
}

static int ensure_ctrl_connection(michadame_deck_focus_ctx *ctx, const char *control_display) {
    if (ctx->ctrl_dpy) {
        if (!display_name_matches(ctx->ctrl_display_name, control_display) ||
                !is_display_socket_healthy(ctx->ctrl_dpy)) {
            reset_ctrl_connection(ctx);
        }
    }
    if (!ctx->ctrl_dpy) {
        Display *dpy = XOpenDisplay(control_display);
        if (!dpy) return 0;
        XSetIOErrorExitHandler(dpy, michadame_x_io_error_exit, NULL);
        ctx->ctrl_dpy = dpy;
        ctx->ctrl_display_name = dup_cstr_or_empty(control_display);
        ctx->baselayer_window_atom = None;
        ctx->baselayer_appid_atom = None;
        ctx->steam_game_atom = None;
        ctx->steam_input_focus_atom = None;
        ctx->steam_overlay_atom = None;
        tls_owned_dpy_b = dpy;
        refresh_ctrl_atoms(ctx);
    }
    return 1;
}

michadame_deck_focus_ctx *michadame_deck_focus_ctx_create(void) {
    pthread_once(&g_xerror_once, install_x_error_handler_once);
    michadame_deck_focus_ctx *ctx =
        (michadame_deck_focus_ctx *)calloc(1, sizeof(michadame_deck_focus_ctx));
    return ctx;
}

void michadame_deck_focus_ctx_destroy(michadame_deck_focus_ctx *ctx) {
    if (!ctx) return;
    reset_viewer_connection(ctx);
    reset_ctrl_connection(ctx);
    free(ctx);
}

/* Persistent X11 focus synchronization context:
 * - Reuses Display* connections and interned Atoms across polls.
 * - Caches the viewer Window ID, re-validating via XGetWindowAttributes.
 * - Avoids XGrabServer / XUngrabServer on read-only polls when focus matches. */
int michadame_deck_focus_ctx_sync(michadame_deck_focus_ctx *ctx,
                                  const char *viewer_display,
                                  const char *control_display,
                                  uint32_t pid, uint32_t shortcut, uint32_t game,
                                  int restore, int handoff, uint32_t *menu_override) {
    if (!ctx || !menu_override) return 0;
    pthread_once(&g_xerror_once, install_x_error_handler_once);

    jmp_buf io_jmp;
    tls_io_failed_dpy = NULL;
    tls_io_jmp = &io_jmp;

    if (setjmp(io_jmp) != 0) {
        tls_io_jmp = NULL;
        tls_owned_dpy_a = NULL;
        tls_owned_dpy_b = NULL;
        if (tls_io_failed_dpy && tls_io_failed_dpy == ctx->viewer_dpy) {
            int fd = ConnectionNumber(ctx->viewer_dpy);
            if (fd >= 0) close(fd);
            ctx->viewer_dpy = NULL;
            free(ctx->viewer_display_name);
            ctx->viewer_display_name = NULL;
            ctx->viewer_pid_atom = None;
            ctx->cached_viewer_win = None;
            ctx->cached_viewer_pid = 0;
        }
        if (tls_io_failed_dpy && tls_io_failed_dpy == ctx->ctrl_dpy) {
            int fd = ConnectionNumber(ctx->ctrl_dpy);
            if (fd >= 0) close(fd);
            ctx->ctrl_dpy = NULL;
            free(ctx->ctrl_display_name);
            ctx->ctrl_display_name = NULL;
            ctx->baselayer_window_atom = None;
            ctx->baselayer_appid_atom = None;
            ctx->steam_game_atom = None;
            ctx->steam_input_focus_atom = None;
            ctx->steam_overlay_atom = None;
        }
        return 0;
    }

    if (!ensure_viewer_connection(ctx, viewer_display)) {
        tls_io_jmp = NULL;
        tls_owned_dpy_a = NULL;
        tls_owned_dpy_b = NULL;
        return 0;
    }
    tls_owned_dpy_a = ctx->viewer_dpy;

    if (ctx->cached_viewer_pid != pid) {
        ctx->cached_viewer_win = None;
        ctx->cached_viewer_pid = pid;
    }
    if (ctx->cached_viewer_win != None) {
        XWindowAttributes attrs;
        tls_x_error_code = 0;
        if (!XGetWindowAttributes(ctx->viewer_dpy, ctx->cached_viewer_win, &attrs) ||
                tls_x_error_code != 0) {
            ctx->cached_viewer_win = None;
        }
    }
    if (ctx->cached_viewer_win == None) {
        if (ctx->viewer_pid_atom == None) {
            ctx->viewer_pid_atom = XInternAtom(ctx->viewer_dpy, "_NET_WM_PID", False);
        }
        ctx->cached_viewer_win = find_window_by_pid(
            ctx->viewer_dpy,
            DefaultRootWindow(ctx->viewer_dpy),
            ctx->viewer_pid_atom,
            pid,
            4
        );
    }

    Window viewer = ctx->cached_viewer_win;
    if (viewer == None) {
        tls_io_jmp = NULL;
        tls_owned_dpy_a = NULL;
        tls_owned_dpy_b = NULL;
        return 0;
    }

    if (!ensure_ctrl_connection(ctx, control_display)) {
        tls_io_jmp = NULL;
        tls_owned_dpy_a = NULL;
        tls_owned_dpy_b = NULL;
        return 0;
    }
    Display *control = ctx->ctrl_dpy;
    tls_owned_dpy_b = control;
    refresh_ctrl_atoms(ctx);

    int handed_off = 0;
    Window root = DefaultRootWindow(control);
    Atom atom = ctx->baselayer_window_atom;
    if (atom != None) {
        unsigned long current = cardinal(control, root, atom);
        if (*menu_override != 0 && current != *menu_override)
            *menu_override = 0;
        int owned = current == viewer || current == 0 ||
            (*menu_override != 0 && current == *menu_override);

        if (owned) {
            int handoff_state = (!restore && handoff)
                ? inspect_handoff_state(control, ctx->baselayer_appid_atom, shortcut, game)
                : 0;

            if (handoff_state == 1) {
                handed_off = 1;
            } else if (handoff_state == 2) {
                XGrabServer(control);
                current = cardinal(control, root, atom);
                if (*menu_override != 0 && current != *menu_override)
                    *menu_override = 0;
                owned = current == viewer || current == 0 ||
                    (*menu_override != 0 && current == *menu_override);
                if (owned) {
                    handed_off = handoff_app_atom(control, ctx->baselayer_appid_atom, shortcut, game);
                    uint32_t selected = restore ? 0 : selected_app_atom(
                        control, ctx->baselayer_appid_atom, shortcut, game);
                    unsigned long desired = selected == game && !restore ? viewer : 0;
                    Window menu = !restore && !handed_off &&
                        (selected == shortcut || selected == game)
                        ? requested_steam_menu_atoms(
                            control,
                            root,
                            ctx->steam_game_atom,
                            ctx->steam_input_focus_atom,
                            ctx->steam_overlay_atom)
                        : None;
                    if (menu != None) desired = menu;
                    if (current != desired) {
                        XChangeProperty(control, root, atom, XA_CARDINAL, 32,
                            PropModeReplace, (unsigned char *)&desired, 1);
                    }
                    *menu_override = (uint32_t)menu;
                }
                XUngrabServer(control);
                XSync(control, False);
                tls_io_jmp = NULL;
                tls_owned_dpy_a = NULL;
                tls_owned_dpy_b = NULL;
                return handed_off ? 2 : 1;
            }

            uint32_t selected = restore ? 0 : selected_app_atom(
                control, ctx->baselayer_appid_atom, shortcut, game);
            unsigned long desired = selected == game && !restore ? viewer : 0;
            Window menu = !restore && !handed_off &&
                (selected == shortcut || selected == game)
                ? requested_steam_menu_atoms(
                    control,
                    root,
                    ctx->steam_game_atom,
                    ctx->steam_input_focus_atom,
                    ctx->steam_overlay_atom)
                : None;
            if (menu != None) desired = menu;

            if (current != desired) {
                XGrabServer(control);
                unsigned long latest = cardinal(control, root, atom);
                if (*menu_override != 0 && latest != *menu_override)
                    *menu_override = 0;
                int still_owned = latest == viewer || latest == 0 ||
                    (*menu_override != 0 && latest == *menu_override);
                if (still_owned) {
                    if (latest != desired) {
                        XChangeProperty(control, root, atom, XA_CARDINAL, 32,
                            PropModeReplace, (unsigned char *)&desired, 1);
                    }
                    *menu_override = (uint32_t)menu;
                }
                XUngrabServer(control);
                XSync(control, False);
            } else {
                *menu_override = (uint32_t)menu;
            }
        }
    }

    tls_io_jmp = NULL;
    tls_owned_dpy_a = NULL;
    tls_owned_dpy_b = NULL;
    return handed_off ? 2 : 1;
}

/* Keep the viewer's original identity and its Steam Input layout. If Steam
 * selects the headless game, direct Gamescope's base window to the viewer.
 * Release only our own override when Steam menus/another application take
 * over, when our shortcut is selected normally, or when the game stops. */
int michadame_deck_sync_focus(const char *viewer_display, const char *control_display,
                             uint32_t pid, uint32_t shortcut, uint32_t game,
                             int restore, int handoff, uint32_t *menu_override) {
    michadame_deck_focus_ctx *ctx = michadame_deck_focus_ctx_create();
    if (!ctx) return 0;
    int rc = michadame_deck_focus_ctx_sync(
        ctx, viewer_display, control_display, pid, shortcut, game,
        restore, handoff, menu_override
    );
    michadame_deck_focus_ctx_destroy(ctx);
    return rc;
}

/* A permanent game tag strands the original Michadame shortcut on a spinner
 * and switches Steam Input away from its configured rear-button mappings. */
int michadame_deck_tag_window(const char *display_name, uint32_t pid,
                             uint32_t appid, uint32_t *previous) {
    pthread_once(&g_xerror_once, install_x_error_handler_once);
    Display *display = XOpenDisplay(display_name);
    if (!display) return 0;
    tls_owned_dpy_a = display;
    /* Prevent window destruction between querying the tree and its properties. */
    XGrabServer(display);
    Window viewer = find_window_by_pid(display, DefaultRootWindow(display),
        XInternAtom(display, "_NET_WM_PID", False), pid, 4);
    if (viewer != None) {
        Atom game = XInternAtom(display, "STEAM_GAME", False);
        unsigned long current = cardinal(display, viewer, game);
        if (previous) *previous = current;
        unsigned long value = appid;
        if (current != value)
            XChangeProperty(display, viewer, game, XA_CARDINAL, 32,
                PropModeReplace, (unsigned char *)&value, 1);
    }
    XUngrabServer(display);
    XSync(display, False);
    tls_owned_dpy_a = NULL;
    XCloseDisplay(display);
    return viewer != None;
}
