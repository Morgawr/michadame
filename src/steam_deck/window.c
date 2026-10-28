#include <X11/Xlib.h>
#include <X11/Xatom.h>
#include <stdint.h>

static unsigned long cardinal(Display *display, Window window, Atom atom) {
    Atom type;
    int format;
    unsigned long count, after, value = 0;
    unsigned char *data = NULL;
    if (XGetWindowProperty(display, window, atom, 0, 1, False, XA_CARDINAL,
            &type, &format, &count, &after, &data) == Success &&
            type == XA_CARDINAL && format == 32 && count == 1 && data) {
        value = (uint32_t)*(unsigned long *)data;
    }
    if (data) XFree(data);
    return value;
}

static Window find_viewer(Display *display, Window root, Atom pid_atom,
                          unsigned long pid, int depth) {
    if (cardinal(display, root, pid_atom) == pid) return root;
    if (depth == 0) return None;
    Window returned_root, parent, *children = NULL, found = None;
    unsigned int count = 0;
    if (XQueryTree(display, root, &returned_root, &parent, &children, &count)) {
        for (unsigned int i = 0; i < count && found == None; ++i)
            found = find_viewer(display, children[i], pid_atom, pid, depth - 1);
    }
    if (children) XFree(children);
    return found;
}

/* Steam's overlay precedes the game in this list while its menu is open. */
static uint32_t selected_app(Display *display, uint32_t shortcut, uint32_t game) {
    Atom type;
    int format;
    unsigned long count, after;
    unsigned char *data = NULL;
    uint32_t selected = 0;
    Atom atom = XInternAtom(display, "GAMESCOPECTRL_BASELAYER_APPID", True);
    if (atom != None && XGetWindowProperty(display, DefaultRootWindow(display),
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

/* Complete Steam's launch handoff once the viewer has received a game frame.
 * Steam can leave its loading screen ahead of a game with no outer window.
 * Request our shortcut once, preserving the rest of the focus order. Future
 * menu and application selections remain owned by Steam. */
static int handoff_app(Display *display, uint32_t shortcut, uint32_t game) {
    Atom type;
    int format;
    unsigned long count, after;
    unsigned char *data = NULL;
    Atom atom = XInternAtom(display, "GAMESCOPECTRL_BASELAYER_APPID", True);
    int done = 0;
    if (atom != None && XGetWindowProperty(display, DefaultRootWindow(display),
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
    if (XQueryTree(display, steam, &root, &parent, &children, &count)) {
        for (unsigned int i = 0; i < count && visible < 2; ++i) {
            XWindowAttributes attributes;
            if (XGetWindowAttributes(display, children[i], &attributes) &&
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
static Window requested_steam_menu(Display *display, Window root) {
    Atom game = XInternAtom(display, "STEAM_GAME", True);
    Atom input = XInternAtom(display, "STEAM_INPUT_FOCUS", True);
    Atom overlay = XInternAtom(display, "STEAM_OVERLAY", True);
    if (game == None || input == None) return None;
    Window returned_root, parent, *children = NULL, found = None;
    unsigned int count = 0;
    if (XQueryTree(display, root, &returned_root, &parent, &children, &count)) {
        for (unsigned int i = 0; i < count && found == None; ++i) {
            if (cardinal(display, children[i], game) != 769 ||
                    cardinal(display, children[i], input) != 1 ||
                    (overlay != None && cardinal(display, children[i], overlay) != 0)) continue;
            XWindowAttributes attributes;
            if (XGetWindowAttributes(display, children[i], &attributes) &&
                    attributes.map_state == IsViewable &&
                    attributes.width > 1 && attributes.height > 1 &&
                    steam_has_auxiliary_view(display, children[i]))
                found = children[i];
        }
    }
    if (children) XFree(children);
    return found;
}

/* Keep the viewer's original identity and its Steam Input layout. If Steam
 * selects the headless game, direct Gamescope's base window to the viewer.
 * Release only our own override when Steam menus/another application take
 * over, when our shortcut is selected normally, or when the game stops. */
int michadame_deck_sync_focus(const char *viewer_display, const char *control_display,
                             uint32_t pid, uint32_t shortcut, uint32_t game,
                             int restore, int handoff, uint32_t *menu_override) {
    Display *viewer_connection = XOpenDisplay(viewer_display);
    if (!viewer_connection) return 0;
    XGrabServer(viewer_connection);
    Window viewer = find_viewer(viewer_connection,
        DefaultRootWindow(viewer_connection),
        XInternAtom(viewer_connection, "_NET_WM_PID", False), pid, 4);
    XUngrabServer(viewer_connection);
    XCloseDisplay(viewer_connection);
    if (viewer == None) return 0;
    Display *control = XOpenDisplay(control_display);
    if (!control) return 0;
    XGrabServer(control);
    int handed_off = 0;
    Window root = DefaultRootWindow(control);
    Atom atom = XInternAtom(control, "GAMESCOPECTRL_BASELAYER_WINDOW", True);
    if (atom != None) {
        unsigned long current = cardinal(control, root, atom);
        if (*menu_override != 0 && current != *menu_override)
            *menu_override = 0;
        int owned = current == viewer || current == 0 ||
            (*menu_override != 0 && current == *menu_override);
        if (owned && !restore && handoff)
            handed_off = handoff_app(control, shortcut, game);
        uint32_t selected = restore ? 0 : selected_app(control, shortcut, game);
        unsigned long desired = selected == game && !restore ? viewer : 0;
        Window menu = !restore && !handed_off &&
            (selected == shortcut || selected == game) ?
            requested_steam_menu(control, root) : None;
        if (menu != None) desired = menu;
        if (owned && current != desired)
            XChangeProperty(control, root, atom, XA_CARDINAL, 32,
                PropModeReplace, (unsigned char *)&desired, 1);
        if (owned) *menu_override = menu;
    }
    XUngrabServer(control);
    XSync(control, False);
    XCloseDisplay(control);
    return handed_off ? 2 : 1;
}

/* A permanent game tag strands the original Michadame shortcut on a spinner
 * and switches Steam Input away from its configured rear-button mappings. */
int michadame_deck_tag_window(const char *display_name, uint32_t pid,
                             uint32_t appid, uint32_t *previous) {
    Display *display = XOpenDisplay(display_name);
    if (!display) return 0;
    /* Prevent window destruction between querying the tree and its properties. */
    XGrabServer(display);
    Window viewer = find_viewer(display, DefaultRootWindow(display),
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
    XCloseDisplay(display);
    return viewer != None;
}
