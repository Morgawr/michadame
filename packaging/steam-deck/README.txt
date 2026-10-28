Michadame — portable Steam Deck build (x86_64)

1. Extract the entire folder somewhere permanent, for example:
   /home/deck/Applications/michadame-steam-deck
2. In Desktop Mode, run install-steam-shortcut.sh once.
3. Steam → Games → Add a Non-Steam Game → Michadame Steam Deck.
   Alternatively, browse directly to the executable michadame-steam-deck script.
4. Leave Steam's "Force the use of a specific Steam Play compatibility tool"
   unchecked. This is a native Linux application; do not select Proton.
5. Open it in Game Mode. No additional launch flag is needed.

Assign rear-button hotkeys in Michadame's Steam Input layout:
L4 Ctrl+Shift+Left, R4 Ctrl+Shift+Right, L5 Ctrl+Alt+Space, R5 H.
Steam's own overlay shortcut remains Shift+Tab.
The viewer retains this controller layout while a headless game is running.
L4/R4 step through Word bank, Main game, then every settings tab in its displayed
order. L4 from Primary returns to the game. Navigation stops at either end.
Set the mining tag in Setup & presets -> Mining bank. Save mining tag commits
the edit; leaving the field or changing tabs also saves it. An empty tag clears it.

Deck controls use 50% UI zoom. The dictionary popup keeps its existing physical
size. The viewer caps rendering at 60 FPS during gameplay and 30 FPS in the idle
launcher/setup screens; the nested game and video capture are limited to 60 FPS.
The main log periodically reports UI FPS and capture FPS separately.
Only the outer compositor reports frames to Steam's performance overlay, avoiding
double counting the nested game and viewer as approximately 120 FPS.
The headless launcher uses gamescopectl to set the persistent game FPS limit.
Injected Steam overlays are disabled in the viewer and headless game; Game Mode
provides the Steam and Quick Access system menus on the outer display.

Keep the folder in place while a game is running. Run ./michadame-steam-deck
--check in Desktop Mode for dependency/audio diagnostics. Session launch logs:
~/.local/state/michadame-steam-deck/sessions/michadame-deck-*/launch.log
Main Deck log: ~/.local/state/michadame-steam-deck/michadame.log
Both paths follow XDG_STATE_HOME when configured.

The package includes FFmpeg 9, libei, SDL2, PipeWire/PulseAudio client libraries,
ALSA plugins and pw-dump/pactl. It uses the SteamOS graphics drivers, Steam,
Gamescope (including gamescopectl), PipeWire/PulseAudio services and the system C/C++ runtime. It requires
glibc 2.35 or later. Do not install development packages or change SteamOS's
read-only system partition for this package.

PC-88/PC-98 cores and OCR data retain their existing first-use downloads.
BIOS files, game files and dictionaries are supplied/configured separately.

Source and build recipe: packaging/steam-deck in the Michadame source tree.
Third-party notices: share/licenses. Library/source manifest:
share/michadame/bundle-manifest.json.
