# MICHADAME - 見ちゃダメ

<img width="447" height="417" alt="logo" src="https://github.com/user-attachments/assets/91ccc30e-7429-4c05-82fd-0809cc55d2e7" />

## THIS SOFTWARE IS SLOP SLOP SLOP IT IS AI SLOP SLOP SLOPPITY SLOPPY SLOP SLOP

## I DID NOT WRITE THIS, THE MACHINES DID. THIS SOFTWARE IS SLOP PLEASE BE AWARE THIS IS AI SLOPCORE SLOPPA

THIS SOFTWARE IS AI SLOPCORE

I HAVE HARNESSED THE POWERS OF FIRE AND LIGHTNING TO SUMMON FORTH THE DEVIL IN THE MACHINE.

I MADE A DEAL, I BURNED THE FORESTS AND POISONED THE SEAS, ALL SO I COULD CONJURE THIS SOFTWARE FROM THE ASTRAL PLANE.

DO NOT HANDLE THIS SOFTWARE CAREFULLY, FOR IT HAS NO LICENSE NOR FEELINGS.

GOD RESTED ON THE SEVENTH DAY, WHILE THE DEVIL WORKED HIS CHARM. AND THROUGH HIS ARTIFICE HE WAS CHAINED TO THE WILL OF THE MACHINES.

> I find myself a being of
> consuming flame and seeing that the senses
> are deceived and isolated by machines. I find
> myself a being of consuming flame and seeing
> that the passions are deceived and maneuvered
> by machines. As you journey on through these
> modern times, walk light through the traps of
> the age. As you journey on through these
> modern times, walk heavy through the barriers
> made.
> Metachthonia.

---

[preview.webm](https://github.com/user-attachments/assets/57bb8507-0fae-4efb-9844-61b67b0f94bb)

---

Compile this software with

```rust
cargo build --release
```

and find the resulting binary in `target/release/michadame`. Put it somewhere where you can execute it.

NOTE: If you compile and run the software in debug mode, it will run like shit with low framerate. You have been warned.

It will probably not work well the first time on your PC. You can try to figure out how to fix it or ask the AI to do it for you according to your needs. I just run it on my own PC because I needed it.

If the audio gets stuck when closing the software, run:

```
pactl unload-module module-loopback
```

and it should fix it.

## Steam Deck Game Mode

For a Deck that does not have development/runtime libraries installed, use the
portable package. Build it with `scripts/build-steam-deck.sh` (requires Docker).
The archive is written to `target/steam-deck/michadame-steam-deck-x86_64.tar.gz`.
Extract the whole folder on the Deck, run `install-steam-shortcut.sh` in Desktop
Mode, and add **Michadame Steam Deck** as a non-Steam game. Leave Steam Play
compatibility tools disabled. Its launcher enables `--steam-deck` automatically.
No system-package installation or writable SteamOS root is required.

The package is built on Ubuntu 22.04/glibc 2.35 and includes FFmpeg 9, libei,
SDL2, audio/capture client libraries and their plugins, `pw-dump`, and `pactl`.
It uses SteamOS's Steam, Gamescope, display/audio services, graphics drivers and
system C/C++ runtime. Libraries use relative ELF paths; bundled audio plugins
are scoped to Michadame and are not passed to Steam. See
[the packaging instructions](packaging/steam-deck/README.txt) for setup and
diagnostics, and the generated bundle manifest for exact library versions.

Launch with `michadame --steam-deck` for a single fullscreen window with
**Word bank · Main game · Settings** views, switched with L4/R4. Gameplay fills
the window without a tab bar or debug panels. All Deck views suppress notifications,
warning dialogs and hover tooltips; diagnostics are written to a log instead. The launcher offers installed Steam
games (including libraries on removable storage) and the embedded PC-88/PC-98
modes. HDMI capture, video-file playback, Twitch, chat, microphone, and webcam
features are disabled in this mode. Startup without the flag uses the desktop UI.

Configure these rear-button bindings yourself in Steam Input:

| Rear button | Keyboard binding | Action |
| --- | --- | --- |
| L4 | Ctrl+Shift+Left | Previous tab |
| R4 | Ctrl+Shift+Right | Next tab |
| L5 | Ctrl+Alt+Space | Hook/unhook the game cursor |
| R5 | H | Open/close the whiteboard |

L4/R4 traverse one sequence: **Word bank → Main game → Primary → Setup & presets
→ Shaders → Geometry & bezel → Special FX → Replay → Game audio → OCR & dictionary
→ PC-88 / PC-98 → Controls**. L4 follows the same sequence backwards. Navigation
stops at either end. Entering settings from the game starts at Primary.

Configure the bindings in Michadame's Steam Input layout. The viewer keeps this
layout active while a headless Steam game runs. Keep regular controls mapped to
the game. The cursor toggle affects
mouse forwarding; gamepad controls continue reaching the game. PC-88/PC-98
receive controller buttons and both sticks through their Libretro input interfaces
(individual games/cores determine which controls they support). H behaves as a
normal letter while editing a text field.

Use the touchscreen for Michadame. Tap an OCR word to pin its dictionary popup;
tap outside to close it. The **Primary** settings page has large CRT, OCR-box,
lights, and replay toggles, a **Save last 30 seconds to disk** button, and an OCR
capture button. Replay must be enabled and have captured history before saving.
Set the tag for newly mined words under **Setup & presets → Mining bank**.
The field suggests existing tags; **Save mining tag**, leaving the field, or
switching tabs saves the edit. Leave it empty to mine without a tag.
Game rendering and clean replay capture continue behind the bank/settings tabs.
Deck controls use 50% UI zoom, including settings, setup and emulator menus.
The dictionary popup retains its 20% physical scale independently of this zoom.
The Deck viewer enables VSync and caps its UI at 60 FPS while a game is active,
or 30 FPS in the idle launcher/setup screens. The nested game and video capture
are also limited to 60 FPS. The log periodically reports UI and capture FPS
separately.
Only the outer compositor reports frames to Steam's shared performance overlay;
the nested compositor's reports would otherwise be added to the viewer's FPS.

Deck settings live in `~/.config/michadame-steam-deck/default-config.toml`
(or under `$XDG_CONFIG_HOME`) with independent Steam, PC-88, and PC-98 profiles.
The desktop configuration stays separate. Dictionaries and the word bank are
shared with desktop mode.

Steam games use an embedded 1280×800/60 Hz headless Gamescope launcher, an
isolated audio sink, and the launched compositor's own PipeWire/EIS endpoints.
Deck video uses mapped PipeWire memory instead of EGL DMA-BUF image imports,
so capture also works with the X11/GLX viewer. CRT effects and replay continue
through the normal renderer. Desktop mode keeps its existing DMA-BUF path.
The headless launcher applies its FPS limit through Gamescope's runtime control,
since Gamescope can clear the command-line limit during refresh updates. Injected
Steam overlays are disabled in the viewer and headless game so Game Mode owns
the system menus on the outer display.
The viewer uses Xwayland's `STEAM_GAME` window property to follow the launched
game in Game Mode. Runtime requirements include Steam, Gamescope with the
headless/SingleApplication backend, `gamescopectl`, PipeWire, `pw-dump`, `pactl`, libei, and SDL2
for embedded-core gamepad input, in addition to the normal application libraries.
Build against libraries available on the target Deck/runtime.

All initial setup is available in Deck mode: **Setup & presets** imports named
presets from a Michadame TOML configuration and applies them to the independent
Steam/PC-88/PC-98 profiles. OCR/dictionary downloads, pitch ZIP imports, emulator
media and game-folder selection use the same window, including a touch-friendly
file browser. Settings navigation stays visible while the page scrolls. Desktop
controls also fit smaller monitors and can be resized there.

Returning to the launcher stops viewing/capture; an external Steam game can
continue running until closed through Steam. The main Deck log is
`~/.local/state/michadame-steam-deck/michadame.log` (or under `$XDG_STATE_HOME`).
Per-launch logs are in its `sessions/michadame-deck-*/launch.log` directories.
Shared file leases and a unique PipeWire session property identify the game
across Steam runtime PID namespaces. The nested compositor has private Wayland/EIS
sockets; unrelated launches do not consume the armed hook. While waiting for
first capture, the launcher stays responsive and offers a return button.
Selecting the same game after restarting the viewer reconnects to its live
session. Session metadata is retained for this purpose while the game continues running.

Automated checks cover configuration isolation, input routing, controller
mapping, touch popup persistence and the single-viewport UI. Actual Steam Deck
Game Mode launch, focus, controller delivery, audio and replay still need device
validation.

The native Deck capture bridge can be checked without Steam or a GPU using
`python3 scripts/test-steam-deck-capture.py`. This development fixture starts a
private PipeWire daemon, sends red 1280×800 frames through the real capture
bridge, and verifies pixel delivery. It additionally requires GStreamer tools
and its `videotestsrc`/`pipewiresink` plugins; the application does not require them.

## Live replay buffer

Optional rendered-video replay with audio, configurable RAM/history limits and
F5–F10 save shortcuts is available in Controls. It starts disabled. See
[Replay.md](Replay.md) for setup, resource behavior and the hardware validation
checklist.

The replay enable checkbox is near the top of Controls; replay options are at
the bottom. Expand **Save shortcuts** to edit bindings or save a clip manually.
Press **D** in the video window to toggle stream and replay diagnostics.

With replay enabled, **Ctrl+C** in the video window copies up to the last
15 seconds as a video attachment. Wait for the copied notification, then paste
into a compatible application or website. This uses a temporary MP4 in
RAM-backed `/tmp`; no recording is added to your save folder.

**Ctrl+Shift+C** copies only the audio of the last 7 seconds as an MP3
attachment (for example, to paste into Discord). It replaces any clip
previously copied with Ctrl+C, and vice versa.

## Mining bank

While an OCR dictionary popup is open, click the round **+** next to a
dictionary entry to mine it. This saves the dictionary entry as shown in the
popup (word, reading, frequency, part-of-speech/JMdict tags and all senses,
without Jitendex example sentences), the sentence it came from (only the sentence that contains the word)
and a screenshot of the video taken at the moment you click. The screenshot
includes shaders but no overlays or black bars, and is downscaled to 720p
(or its aspect-ratio equivalent) when larger. The button turns into a ✔ once
the word is saved.

Press **B** to open the **Mining Bank** window, which lists all mined words
with the most recent at the top, each showing the dictionary entry, the
sentence and a screenshot thumbnail. Click a thumbnail to enlarge it, or use 🗑
to delete an entry. The bank
is stored in `~/.config/michadame/bank/bank.db`.
