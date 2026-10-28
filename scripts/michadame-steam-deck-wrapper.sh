#!/bin/sh
# MICHADAME_WRAPPER_V1
# MICHADAME_SESSION=@SESSION_HEADER@
# MICHADAME_REAL_DIGEST=@DIGEST_HEADER@
{
    SELF="$0"
    REAL="${SELF}.real"
    STEAM_ROOT=@STEAM_ROOT@
    TARGET_APPID=@APPID@
    MICHADAME_PID=@PID@
    MICHADAME_SESSION=@SESSION@
    LAUNCHER_SCRIPT=@LAUNCHER@
    FLOCK=@FLOCK@
    SESSION_DIR="${LAUNCHER_SCRIPT%/*}"
    restore_all() {
        for dir in "$STEAM_ROOT/steamrt64" "$STEAM_ROOT/ubuntu12_32"; do
            if [ -f "$dir/steam-launch-wrapper.real" ] &&
               grep -Fxq '# MICHADAME_WRAPPER_V1' "$dir/steam-launch-wrapper" 2>/dev/null &&
               grep -Fxq "# MICHADAME_SESSION=$MICHADAME_SESSION" "$dir/steam-launch-wrapper" 2>/dev/null; then
                mv -f "$dir/steam-launch-wrapper.real" "$dir/steam-launch-wrapper"
                rm -f "$dir/.steam-launch-wrapper.michadame"
            fi
        done
    }
    owner_alive=0
    if [ -f "$SESSION_DIR/viewer.lock" ]; then
        # A file lock is shared across Steam's PID namespaces; kill -0 is not.
        "$FLOCK" -n -E 75 "$SESSION_DIR/viewer.lock" true
        if [ "$?" = 75 ]; then owner_alive=1; fi
    elif kill -0 "$MICHADAME_PID" 2>/dev/null; then
        owner_alive=1
    fi
    if [ "$owner_alive" = 0 ]; then
        restore_all
        exec "$SELF" "$@"
    fi
    is_target=0
    explicit_appid=0
    for arg in "$@"; do
        case "$arg" in
            AppId=*) explicit_appid=1; [ "$arg" != "AppId=$TARGET_APPID" ] || is_target=1 ;;
            Install=1) exec "$REAL" "$@" ;;
        esac
    done
    if [ "$explicit_appid" = 0 ] && { [ "${SteamAppId:-}" = "$TARGET_APPID" ] || [ "${SteamGameId:-}" = "$TARGET_APPID" ]; }; then
        is_target=1
    fi
    if [ "$is_target" = 0 ]; then
        # Background tools and unrelated games must not consume the armed hook.
        exec "$REAL" "$@"
    fi
    printf 'Deck wrapper matched AppId=%s session=%s pid=%s\n' "$TARGET_APPID" "$MICHADAME_SESSION" "$$" >> "$SESSION_DIR/launch.log"
    restore_all
    if [ "${MICHADAME_DECK_SESSION:-}" = "$MICHADAME_SESSION" ]; then exec "$SELF" "$@"; fi
    export MICHADAME_DECK_SESSION="$MICHADAME_SESSION"
    exec "$LAUNCHER_SCRIPT" "$SELF" "$@"
}
