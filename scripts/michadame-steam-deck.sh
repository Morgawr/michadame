#!/usr/bin/env bash
# Embedded by --steam-deck into a private per-launch directory. Desktop launcher
# defaults remain in michadame-gamescope-steam.sh.
set -euo pipefail
session_dir="$(cd -- "$(dirname -- "$0")" && pwd)"
exec >> "$session_dir/launch.log" 2>&1
flock_cmd=flock
if [ -f "$session_dir/flock.path" ]; then flock_cmd="$(cat "$session_dir/flock.path")"; fi
if [ "${MICHADAME_COMPOSITOR_LOCKED:-0}" != 1 ]; then
    exec "$flock_cmd" --close "$session_dir/compositor.lock" env MICHADAME_COMPOSITOR_LOCKED=1 bash "$0" "$@"
fi
unset MICHADAME_COMPOSITOR_LOCKED
if [ "$#" -eq 0 ]; then
    echo "No game command supplied" >&2
    exit 1
fi
command -v gamescope >/dev/null
# Keep Game Mode's system menus on the outer compositor. An injected overlay
# in the headless game can register windows from a different Xwayland server.
if [ -n "${LD_PRELOAD:-}" ]; then
    read -r -a preloads <<< "${LD_PRELOAD//:/ }"
    kept_preloads=()
    for library in "${preloads[@]}"; do
        case "$library" in
            */gameoverlayrenderer.so|gameoverlayrenderer.so) ;;
            *) kept_preloads+=("$library") ;;
        esac
    done
    if [ "${#kept_preloads[@]}" -gt 0 ]; then
        export LD_PRELOAD="${kept_preloads[*]}"
    else
        unset LD_PRELOAD
    fi
fi
export DISABLE_VK_LAYER_VALVE_steam_overlay_1=1
# Steam's Mangoapp counts messages from a shared queue. Only the outer
# compositor should report frames; otherwise game + viewer are counted twice.
export GAMESCOPE_MANGOAPP_SOCKET_DISABLE=1
printf 'Deck launcher pid=%s SteamAppId=%s DISPLAY=%s\n' "$$" "${SteamAppId:-}" "${DISPLAY:-}"
pactl_cmd=pactl
if [ -f "$session_dir/pactl.path" ]; then
    pactl_cmd="$(cat "$session_dir/pactl.path")"
fi
command -v "$pactl_cmd" >/dev/null
if ! "$pactl_cmd" list short sinks | awk '{print $2}' | grep -Fxq michadame_gamescope; then
    "$pactl_cmd" load-module module-null-sink sink_name=michadame_gamescope \
        sink_properties=device.description=michadame_gamescope >/dev/null
fi
export PULSE_SINK=michadame_gamescope
export SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS=1
# Keep the nested compositor separate from SteamOS's display server.
unset WAYLAND_DISPLAY GAMESCOPE_WAYLAND_DISPLAY
export MICHADAME_ORIGINAL_RUNTIME="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
export PIPEWIRE_RUNTIME_DIR="${PIPEWIRE_RUNTIME_DIR:-$MICHADAME_ORIGINAL_RUNTIME}"
export PULSE_SERVER="${PULSE_SERVER:-unix:$MICHADAME_ORIGINAL_RUNTIME/pulse/native}"
mkdir -p "$session_dir/runtime"
chmod 700 "$session_dir/runtime"
export XDG_RUNTIME_DIR="$session_dir/runtime"
# The nested compositor must not overwrite the outer session's WSI limiter flag.
export GAMESCOPE_LIMITER_FILE="$session_dir/frame-limiter"
export MICHADAME_PW_PROPS_WAS_SET="${PIPEWIRE_PROPS+x}"
export MICHADAME_OLD_PW_PROPS="${PIPEWIRE_PROPS:-}"
capture_tag="$(cat "$session_dir/capture-tag" 2>/dev/null || basename "$session_dir")"
export PIPEWIRE_PROPS="${PIPEWIRE_PROPS:-} michadame.deck.session=$capture_tag"
printf '%s\n' "$$" > "$session_dir/gamescope.pid"
exec gamescope --backend headless \
    -W 1280 -H 800 -w 1280 -h 800 -r 60 --framerate-limit 60 \
    --generate-drm-mode fixed --virtual-connector-strategy SingleApplication \
    --force-windows-fullscreen -- \
    bash "$session_dir/game.sh" "$@"
