/* Exercise the real focus bridge against two isolated X servers. */
#include <X11/Xlib.h>
#include <X11/Xatom.h>
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <unistd.h>

int michadame_deck_sync_focus(const char *, const char *, uint32_t, uint32_t, uint32_t, int, int, uint32_t *);
int michadame_deck_tag_window(const char *, uint32_t, uint32_t, uint32_t *);

static void set(Display *d, Window w, const char *name, unsigned long *v, int n) {
    XChangeProperty(d, w, XInternAtom(d, name, False), XA_CARDINAL, 32,
                    PropModeReplace, (unsigned char *)v, n);
    XSync(d, False);
}

static unsigned long get_slot(Display *d, Window w, const char *name, long slot) {
    Atom type;
    int format;
    unsigned long count, after;
    unsigned char *data = NULL;
    assert(XGetWindowProperty(d, w, XInternAtom(d, name, False), slot, 1,
        False, XA_CARDINAL, &type, &format, &count, &after, &data) == Success);
    unsigned long value = count ? (uint32_t)*(unsigned long *)data : 0;
    if (data) XFree(data);
    return value;
}

static unsigned long get(Display *d, Window w, const char *name) {
    return get_slot(d, w, name, 0);
}

int main(int argc, char **argv) {
    assert(argc == 3);
    Display *viewer = XOpenDisplay(argv[1]), *control = XOpenDisplay(argv[2]);
    assert(viewer && control);
    Window window = XCreateSimpleWindow(viewer, DefaultRootWindow(viewer), 0, 0, 1280, 800, 0, 0, 0);
    unsigned long pid = getpid(), shortcut = 2474271361u, game = 3259780;
    set(viewer, window, "_NET_WM_PID", &pid, 1);
    XMapWindow(viewer, window);
    XSync(viewer, False);
    uint32_t previous;
    assert(michadame_deck_tag_window(argv[1], pid, shortcut, &previous));
    assert(previous == 0);
    Window root = DefaultRootWindow(control);
    unsigned long zero = 0;
    uint32_t menu_override = 0;
    set(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW", &zero, 1);
    unsigned long cases[][3] = {
        {413091, game, 769}, {413091, 769, game},
        {413091, game, 769}, {413091, shortcut, 769},
        {413091, game, 769}, {413091, 999, 769},
    };
    for (unsigned i = 0; i < sizeof(cases)/sizeof(cases[0]); ++i) {
        set(control, root, "GAMESCOPECTRL_BASELAYER_APPID", cases[i], 3);
        assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 0, 0, &menu_override));
        assert(get(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW") == (i % 2 == 0 ? window : 0));
        assert(get(viewer, window, "STEAM_GAME") == shortcut);
    }
    set(control, root, "GAMESCOPECTRL_BASELAYER_APPID", cases[0], 3);
    assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 0, 0, &menu_override));
    assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 1, 0, &menu_override));
    assert(get(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW") == 0);
    unsigned long foreign = window + 1;
    set(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW", &foreign, 1);
    for (int restore = 0; restore <= 1; ++restore) {
        assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, restore, 0, &menu_override));
        assert(get(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW") == foreign);
    }
    set(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW", &zero, 1);
    unsigned long loading[] = {413091, 769, game};
    set(control, root, "GAMESCOPECTRL_BASELAYER_APPID", loading, 3);
    assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 0, 1, &menu_override) == 2);
    assert(get_slot(control, root, "GAMESCOPECTRL_BASELAYER_APPID", 1) == shortcut);
    assert(get_slot(control, root, "GAMESCOPECTRL_BASELAYER_APPID", 2) == 769);
    assert(get(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW") == 0);
    assert(get(viewer, window, "STEAM_GAME") == shortcut);
    /* A later Steam menu selection must remain in front. */
    set(control, root, "GAMESCOPECTRL_BASELAYER_APPID", cases[1], 3);
    assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 0, 0, &menu_override));
    assert(get(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW") == 0);
    /* Regression: Steam draws the menu but leaves the shortcut first and its
     * ordinary Big Picture window untagged as an overlay. Honor its actual
     * input request instead of leaving our viewer in front of it. */
    Window menu = XCreateSimpleWindow(control, root, 0, 0, 1280, 800, 0, 0, 0);
    Window base_view = XCreateSimpleWindow(control, menu, 0, 0, 1280, 800, 0, 0, 0);
    Window side_view = XCreateSimpleWindow(control, menu, 0, 0, 360, 800, 0, 0, 0);
    unsigned long steam = 769, one = 1, two = 2;
    set(control, menu, "STEAM_GAME", &steam, 1);
    set(control, menu, "STEAM_OVERLAY", &zero, 1);
    XMapWindow(control, menu);
    XMapWindow(control, base_view);
    XSync(control, False);
    set(control, root, "GAMESCOPECTRL_BASELAYER_APPID", cases[3], 3);
    set(control, menu, "STEAM_INPUT_FOCUS", &one, 1);
    assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 0, 0, &menu_override));
    assert(get(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW") == 0);
    for (unsigned i = 0; i < 2; ++i) {
        unsigned long order[] = {413091, i == 0 ? shortcut : game, 769};
        set(control, root, "GAMESCOPECTRL_BASELAYER_APPID", order, 3);
        set(control, menu, "STEAM_INPUT_FOCUS", &one, 1);
        XMapWindow(control, side_view);
        XSync(control, False);
        for (int poll = 0; poll < 3; ++poll) {
            assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 0, 0, &menu_override));
            assert(get(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW") == menu);
            assert(menu_override == menu);
            assert(get_slot(control, root, "GAMESCOPECTRL_BASELAYER_APPID", 1) == order[1]);
            assert(get(viewer, window, "STEAM_GAME") == shortcut);
        }
        /* Menu close must return to the game even if Steam retains focus=1. */
        XUnmapWindow(control, side_view);
        XSync(control, False);
        assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 0, 0, &menu_override));
        assert(menu_override == 0);
        assert(get(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW") == (i == 0 ? 0 : window));
    }
    set(control, root, "GAMESCOPECTRL_BASELAYER_APPID", cases[3], 3);
    set(control, menu, "STEAM_INPUT_FOCUS", &one, 1);
    XMapWindow(control, side_view);
    XSync(control, False);
    assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 0, 0, &menu_override));
    assert(menu_override == menu);
    assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 1, 0, &menu_override));
    assert(menu_override == 0);
    assert(get(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW") == 0);
    /* Native overlays and virtual keyboards retain Gamescope's own routing. */
    set(control, menu, "STEAM_OVERLAY", &one, 1);
    assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 0, 0, &menu_override));
    assert(get(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW") == 0);
    set(control, menu, "STEAM_OVERLAY", &zero, 1);
    set(control, menu, "STEAM_INPUT_FOCUS", &two, 1);
    assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 0, 0, &menu_override));
    assert(get(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW") == 0);
    /* Never clear or replace an override owned by Steam or another app. */
    set(control, menu, "STEAM_INPUT_FOCUS", &one, 1);
    assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 0, 0, &menu_override));
    set(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW", &foreign, 1);
    assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 0, 0, &menu_override));
    assert(menu_override == 0);
    assert(get(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW") == foreign);
    assert(michadame_deck_sync_focus(argv[1], argv[2], pid, shortcut, game, 1, 0, &menu_override));
    assert(get(control, root, "GAMESCOPECTRL_BASELAYER_WINDOW") == foreign);
    XCloseDisplay(viewer);
    XCloseDisplay(control);
    puts("Focus handoff, ordinary Steam menus, overlay/keyboard routing and owned restoration passed");
    return 0;
}
