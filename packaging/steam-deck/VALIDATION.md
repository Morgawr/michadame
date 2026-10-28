The portable package is built by `scripts/build-steam-deck.sh`. Its manifest
records source hashes, the source revision and local-change status, included
libraries, host libraries, and hashes of all packaged files.

To check an extracted package in a minimal Ubuntu 22.04 environment:

```sh
docker build -f packaging/steam-deck/Dockerfile.runtime-test \
    -t michadame-steam-deck-runtime-test:ubuntu22 packaging/steam-deck
docker run --rm --network none \
    -v "$PWD/target/steam-deck/michadame-steam-deck:/opt/a package with spaces:ro" \
    -v "$PWD/packaging/steam-deck/verify-bundle.py:/verify-bundle.py:ro" \
    michadame-steam-deck-runtime-test:ubuntu22 \
    python3 /verify-bundle.py '/opt/a package with spaces'
```

This verifies file hashes, resolution of every ELF object's libraries, required
glibc versions, input/audio plugin loading, FFmpeg codecs and generated frames,
desktop shortcut creation, and 15 seconds of application startup with software
OpenGL. The image supplies the graphics stack and has no system FFmpeg, SDL,
libei, PipeWire tools, or ALSA/PulseAudio plugins. It has no audio server, so
PulseAudio connection-refused messages are expected.

For a real shared-memory capture check, compile the probe in the build image,
then run it against a private PipeWire server on the development host:

```sh
docker run --rm --network none --user "$(id -u):$(id -g)" \
    -v "$PWD:/work" -w /work michadame-steam-deck-builder:ubuntu22 bash -c '
    cc -Wall -Wextra -Werror tests/steam_deck_capture_probe.c src/steam_deck/capture.c \
        $(pkg-config --cflags libpipewire-0.3) \
        -L/work/target/steam-deck/michadame-steam-deck/lib -lpipewire-0.3 \
        -Wl,-rpath,\$ORIGIN/../steam-deck/michadame-steam-deck/lib \
        -o target/steam-deck-cache/capture-probe'
python3 scripts/test-steam-deck-capture.py \
    --bundle target/steam-deck/michadame-steam-deck \
    --probe target/steam-deck-cache/capture-probe
```

The host needs matching PipeWire/GStreamer tools and the `videotestsrc` and
`pipewiresink` plugins. Avoid mixing the build image's old system GStreamer
plugin with its separately built modern PipeWire runtime. These are development
test dependencies; the application does not use GStreamer in Deck mode.

Validation of the built archive passed with 66 ELF objects requiring at most
glibc 2.35. The baseline-compiled capture probe received 30 valid red 1280×800
frames using packaged client libraries, configuration, and plugins, against an
isolated PipeWire 1.6.9 server. The extracted archive's file hashes and executable
permissions were checked separately. Unit/shader checks passed (575 passed,
10 ignored); Clippy completed with existing warnings.

Import checks also passed using the real NHK2016 archive: 73,100 rows and 84,242
pitch patterns, including reading-only entries and stale CRC fields. Screen AI
153.05 was downloaded through the application installer, initialized, and used
for OCR. These checks passed on the desktop and with the baseline-built test
runner in the minimal glibc 2.35 image, using the packaged libraries. Live
download/import tests are opt-in via `MICHADAME_TEST_SCREEN_AI_DIR` and
`MICHADAME_TEST_PITCH_ZIP`; neither test changes the user's installed models or
dictionary database.

Deck layout checks exercise clickable emulator media and preset-import buttons
at 1280×800, 960×600, and 800×500 logical sizes. They verify that navigation
remains visible, notifications/warning dialogs are suppressed, and file selection
stays in one native viewport. Software-OpenGL screenshots also verified the
rendered setup/emulator pages at 100% and 150% scaling. Normal desktop controls
measured 1208×656 physical pixels on the 1280×800 test display at 150% scaling.

Deck wrapper tests cover unrelated launches, installation helpers, an unusable
viewer PID with a held shared lease, environment-only AppIDs, and an existing
outer Gamescope/audio sink. Capture discovery selects by the unique session
property even when process IDs conflict between namespaces. The real PipeWire
fixture verifies that its source publishes this property and delivers frames
using the packaged client.

Additional physical-Deck diagnostics are saved under `target/violet-diagnostics/`.
On SteamOS 3.8.28 with Gamescope 3.16.23.6, the launched Proton game was observed
on the nested headless Xwayland server, and its title screen was captured from
the Michadame viewer. Steam's logs confirmed that permanently assigning the
game's AppID to the viewer switched its controller layout and left the original
shortcut without a window. A crash report also identified Steam's injected GLX
overlay inside the viewer's `glXSwapBuffers` call; the packaged launcher now
filters that injection before starting the viewer.

The installed update retained the viewer's shortcut AppID and Steam Input
profile, had no injected Steam overlay in its process mappings, and measured
approximately 59-60 UI FPS and 60 captured video FPS. No cursor-grab warnings
were emitted in the recorded session. The user confirmed working game input
after the original profile was restored during diagnosis.

The focus bridge is separately exercised against two isolated X servers by
`tests/steam_deck_window_probe.c`: launching the headless game, returning to the
shortcut, Steam menus, unrelated games, foreign window overrides, cleanup, and
the initial loading-screen handoff. Compile it with `src/steam_deck/window.c`
and `-lX11`, then pass the viewer and control X displays as its two arguments.
The final Rust/shader suite passed 579 checks with 10 ignored. Popup tests at
the Deck's actual logical screen size verify 20% scaling and aligned mining
button hit targets both above and beneath CRT rendering. Frame-pacing tests
cover the idle launcher and active game; desktop pacing is unchanged.

Physical rear-button remapping, touchscreen popup sizing, repeated fresh Steam
launches, and hardware VA-API replay encoding remain separate user/device
acceptance checks; unit tests and software OpenGL do not establish those.

The subsequent OCR alignment update was validated locally without connecting
to the Deck. OCR glyphs now fit their actual character hit cells; tests sample
visible pixels from the bundled Japanese font and run dictionary lookup at
those positions at six UI scales, horizontally and vertically. Letterbox
readback uses a correctly converted top-origin inset. Constant scans refresh
line/character geometry even when content-based backoff considers text unchanged.
Changing CRT projection invalidates older OCR results and reschedules pending
captures. Pixelation/Anime4K fitting uses shared integer texture dimensions.

The clean OCR pass uses the displayed CRT curvature and Halo zoom without
scanlines or phosphor masks. A software-EGL test compares actual capture and
display readbacks in 24 cases across four source resolutions, Lottes/Halo,
curvature/zoom, and returning to flat/pixelated presentation. This test forces
Mesa software rendering and does not require physical graphics devices:

```sh
XDG_DATA_HOME=/tmp/michadame-ocr-tests/data \
XDG_CONFIG_HOME=/tmp/michadame-ocr-tests/config \
LIBGL_ALWAYS_SOFTWARE=1 EGL_PLATFORM=surfaceless \
cargo test --locked --bin michadame \
  clean_ocr_capture_matches_crt_display_geometry -- \
  --ignored --nocapture --test-threads=1
```

The ordinary unit/shader suite passed 584 checks with 11 ignored; the explicitly
run software-EGL regression passed separately. Formatting and Clippy passed
(existing Clippy warnings remain). This OCR update was subsequently deployed;
physical OCR alignment was not verified by these local checks.

The compact UI/frame-limit update uses 50% UI zoom in Deck mode and compensates
the dictionary popup to retain its prior physical size. A rendered label test
at the Deck's reported 13/6 native pixels-per-point verifies the 50% reduction
and visible primary controls. A 1280×800 software-OpenGL screenshot at that
native scale confirms the settings fit; the idle viewer reported 29.9 UI FPS.
OCR glyph hit tests also cover the resulting 13/12 effective UI scale.

Frame-pacing tests verify 30 FPS while idle and 60 FPS with an active game.
Capture-limiter tests cover 30/60/90/120/144/240 Hz sources, callback jitter,
and recovery after a gap. The limiter skips excess frames before RGBA copying
and repaint requests. Launcher tests verify Gamescope's explicit 60 FPS limit
and a private limiter-status file, independent of Steam's outer compositor.
These local checks do not establish battery savings or reproduce the user's
120 FPS reading on the physical Deck.

The compact UI/frame-limit suite passed 587 checks with 11 ignored; formatting and
Clippy completed (existing warnings remain). The native shared-memory capture
fixture passed with 30 red 1280×800 frames using the packaged PipeWire client.

Rear-navigation regressions exercise the actual Ctrl+Shift+Left/Right events
through Word bank, Main game, and all ten settings sections, in both directions.
They verify consumed key releases, ignored held-key repeats, clamped endpoints,
navigation from a section selected by touch, and re-entry at Primary after the
whiteboard returns to the game. The displayed settings tabs and shortcuts use
one shared section order.
The subsequent unit/shader suite passed 589 checks with 11 ignored.

Read-only diagnostics from the user's later Deck session showed approximately
59-60 UI/capture FPS, but the nested compositor's private frame-limiter file
contained zero despite its `--framerate-limit 60` argument. Gamescope's refresh
update clears that command-line setting. The game script now calls
`gamescopectl debug_set_fps_limit 60` against its private Wayland socket before
launching the game, setting the persistent refresh-cycle override.

Steam received the Guide button presses and created its MainMenu/QuickAccess
views, but logged BadWindow errors during overlay state changes. The headless
launcher now removes the injected GLX overlay and disables the Vulkan overlay
using the environment key declared in Steam's installed layer manifests.
Other preloaded libraries, Steam Input and the existing focus bridge are kept.
The launcher regression checks overlay isolation and the scoped runtime-limit
command. Physical game FPS and both system menus still need user verification
after restarting the app and game; no input was sent to the Deck by the agent.

Runtime control reference:
https://github.com/ValveSoftware/gamescope/blob/master/src/Apps/gamescopectl.cpp

Subsequent read-only diagnostics confirmed that Steam's overlay displayed 119
FPS while the viewer/capture reported approximately 60 FPS and the private game
limiter was enabled. Sampling the shared SysV queue's last-sender PID (without
consuming messages) found both the outer Gamescope PID 39429 and the active
headless Gamescope PID 53606. The nested launcher now sets
GAMESCOPE_MANGOAPP_SOCKET_DISABLE=1, which suppresses its commit reports into
the shared Mangoapp counter. The desktop launcher's 120 Hz default is separate
from the Deck launcher and was left unchanged.

The overlay-injection change alone did not restore the side menus. A screenshot
of Steam's ordinary Big Picture window showed a fully rendered MainMenu behind
the viewer; STEAM_INPUT_FOCUS was 1, STEAM_OVERLAY was 0, and the shortcut still
preceded Steam in the base app order. The focus bridge now honors that menu input
request with a temporary owned window override. It releases the override when
the menu view is unmapped, even if Steam retains its input-focus flag. A mapped
auxiliary content view is required so the ordinary background client cannot
take focus during launch. It does not alter native overlay/virtual-keyboard routing or
foreign overrides. The real C bridge passed the expanded two-X-server probe,
including repeated polls, both game identities, menu close, stop and restoration.

Setup & presets now exposes the shared Mining Tag editor and a save button.
Its UI regression types a tag, checks normalization/canonical casing, switches
away with a rear shortcut, and checks that the edit was committed. It also
exercises the save button. The unit/shader suite passed 590 checks with 11 ignored.
These changes require an app/game restart and separate user acceptance on Deck.
