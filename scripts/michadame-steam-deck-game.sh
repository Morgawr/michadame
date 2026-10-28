#!/usr/bin/env bash
set -euo pipefail
session_dir="$(cd -- "$(dirname -- "$0")" && pwd)"
if [ -n "${MICHADAME_ORIGINAL_RUNTIME:-}" ]; then
    case "${GAMESCOPE_WAYLAND_DISPLAY:?}" in
        /*) ;;
        *) export GAMESCOPE_WAYLAND_DISPLAY="$XDG_RUNTIME_DIR/$GAMESCOPE_WAYLAND_DISPLAY" ;;
    esac
    export XDG_RUNTIME_DIR="$MICHADAME_ORIGINAL_RUNTIME"
    unset MICHADAME_ORIGINAL_RUNTIME
fi
printf '%s\n%s\n' "${GAMESCOPE_WAYLAND_DISPLAY:?}" "${DISPLAY:?}" > "$session_dir/endpoints.tmp"
mv -- "$session_dir/endpoints.tmp" "$session_dir/endpoints"
if [ "${MICHADAME_PW_PROPS_WAS_SET:-}" = x ]; then
    export PIPEWIRE_PROPS="${MICHADAME_OLD_PW_PROPS:-}"
else
    unset PIPEWIRE_PROPS
fi
unset MICHADAME_PW_PROPS_WAS_SET MICHADAME_OLD_PW_PROPS
# Gamescope clears the command-line FPS limit when it updates its refresh cycle.
# Set the persistent override through the launched compositor's private socket.
if gamescopectl debug_set_fps_limit 60; then
    echo "Nested Gamescope runtime frame limit set to 60 FPS"
else
    echo "Could not set the nested Gamescope runtime frame limit" >&2
fi
# Prevent pressure-vessel from restoring the host Wayland display before Proton.
cmd=("$@")
last=-1
for ((i=0; i<${#cmd[@]}; i++)); do
    if [ "${cmd[i]}" = -- ]; then last=$i; fi
done
if [ "$last" -ge 0 ]; then
    exec "${cmd[@]:0:$((last+1))}" env -u WAYLAND_DISPLAY SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS=1 "${cmd[@]:$((last+1))}"
fi
exec env -u WAYLAND_DISPLAY SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS=1 "$@"
