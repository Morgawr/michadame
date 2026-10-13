#!/usr/bin/env bash
# michadame-gamescope-steam.sh
# Steam launch wrapper for Gamescope headless mode with michadame hook.
# Usage in Steam Launch Options:
#   /path/to/michadame/scripts/michadame-gamescope-steam.sh %command%

set -euo pipefail

# 1. Target resolution and refresh rate (Defaults to 4K 60Hz)
GAMESCOPE_WIDTH="${MICHADAME_WIDTH:-3840}"
GAMESCOPE_HEIGHT="${MICHADAME_HEIGHT:-2160}"
GAMESCOPE_REFRESH="${MICHADAME_REFRESH:-60}"

# 2. Audio isolation setup
# Direct PulseAudio/ALSA audio output to the dedicated null-sink captured by michadame
SINK_NAME="michadame_gamescope"
if command -v pactl >/dev/null 2>&1; then
    if ! pactl list short sinks | grep -q "${SINK_NAME}"; then
        pactl load-module module-null-sink sink_name="${SINK_NAME}" sink_properties="device.description=${SINK_NAME}" >/dev/null 2>&1 || true
    fi
fi
export PULSE_SINK="${SINK_NAME}"

# 3. Gamepad / Controller background input
# Ensure SDL2 games process controller events even when running in a headless Wayland compositor
export SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS=1

# 4. Check that a game command was provided
if [ $# -eq 0 ]; then
    echo "Error: No command provided to michadame-gamescope-steam.sh" >&2
    echo "Usage: $0 <game-executable> [args...]" >&2
    exit 1
fi

# 5. Rewrite arguments to ensure WAYLAND_DISPLAY is unset inside container
# SteamLinuxRuntime (pressure-vessel) inspects the host and passes WAYLAND_DISPLAY=wayland-0
# into the game container. Inside the container, this conflicts with GAMESCOPE_WAYLAND_DISPLAY
# and triggers a Zenity WSI modal error dialog, blocking game swapchain presentation.
# By injecting `env -u WAYLAND_DISPLAY SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS=1` after the last `--`
# (which precedes the game binary/Proton), we ensure the game runs cleanly under Gamescope
# without any WSI layer conflict and receives background controller inputs inside the container.
cmd=()
last_dash_dash=-1
idx=0
for arg in "$@"; do
    if [ "$arg" = "--" ]; then
        last_dash_dash=$idx
    fi
    cmd+=("$arg")
    idx=$((idx + 1))
done

if [ $last_dash_dash -ge 0 ]; then
    new_cmd=("${cmd[@]:0:$((last_dash_dash + 1))}" "env" "-u" "WAYLAND_DISPLAY" "SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS=1" "${cmd[@]:$((last_dash_dash + 1))}")
else
    new_cmd=("env" "-u" "WAYLAND_DISPLAY" "SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS=1" "$@")
fi

# 6. Exec gamescope in headless mode
# Unset host WAYLAND_DISPLAY so gamescope does not inherit the host Wayland socket
unset WAYLAND_DISPLAY

# -W / -H: internal game render resolution (4K)
# -w / -h: output display resolution (4K)
# -r: target refresh rate (60Hz)
# --generate-drm-mode fixed: ensures fixed 4K@60Hz headless mode
# --virtual-connector-strategy SingleApplication: ensures gamescope automatically focuses
# the game window and composites frames to PipeWire without requiring SteamOS BPM focus atoms
# --force-windows-fullscreen: scales the game window to fill the 4K headless canvas
exec gamescope \
    --backend headless \
    -W "${GAMESCOPE_WIDTH}" \
    -H "${GAMESCOPE_HEIGHT}" \
    -w "${GAMESCOPE_WIDTH}" \
    -h "${GAMESCOPE_HEIGHT}" \
    -r "${GAMESCOPE_REFRESH}" \
    --generate-drm-mode fixed \
    --virtual-connector-strategy SingleApplication \
    --force-windows-fullscreen \
    -- "${new_cmd[@]}"

