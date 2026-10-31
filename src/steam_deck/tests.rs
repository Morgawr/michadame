use super::*;
use egui::{Event, Key, Modifiers, RawInput};

fn deck_state() -> AppState {
    AppState {
        steam_deck: Some(SteamDeckState::default()),
        ..Default::default()
    }
}

fn key(key: Key, modifiers: Modifiers, pressed: bool) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers,
    }
}

fn frame(
    ctx: &egui::Context,
    events: Vec<Event>,
    run: impl FnMut(&egui::Context),
) -> egui::FullOutput {
    ctx.run(
        RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            events,
            ..Default::default()
        },
        run,
    )
}

#[test]
fn rear_shortcuts_work_during_gameplay_and_consume_releases() {
    let ctx = egui::Context::default();
    let mut state = deck_state();
    state.gamescope_hook = Some(crate::gamescope::GamescopeHookState {
        is_active: true,
        ..Default::default()
    });
    let chord = Modifiers::CTRL | Modifiers::SHIFT;
    for (events, tab) in [
        (vec![key(Key::ArrowLeft, chord, true)], Tab::Bank),
        (vec![key(Key::ArrowLeft, Modifiers::NONE, false)], Tab::Bank),
        (vec![key(Key::ArrowRight, chord, true)], Tab::Game),
        (vec![key(Key::ArrowRight, chord, false)], Tab::Game),
        (vec![key(Key::ArrowRight, chord, true)], Tab::Settings),
    ] {
        let _ = frame(&ctx, events, |ctx| {
            shortcuts(&mut state, ctx);
            assert!(ctx.input(|i| i.events.is_empty()));
        });
        assert_eq!(state.steam_deck.as_ref().unwrap().tab, tab);
    }
    let _ = frame(
        &ctx,
        vec![key(Key::Space, Modifiers::CTRL | Modifiers::ALT, true)],
        |ctx| shortcuts(&mut state, ctx),
    );
    assert!(!state.gamescope_hook.as_ref().unwrap().passthrough_enabled);
    let _ = frame(&ctx, vec![key(Key::H, Modifiers::NONE, true)], |ctx| {
        shortcuts(&mut state, ctx)
    });
    assert!(state.whiteboard.open);
    assert_eq!(state.steam_deck.as_ref().unwrap().tab, Tab::Game);
    let _ = frame(&ctx, vec![key(Key::H, Modifiers::NONE, false)], |ctx| {
        shortcuts(&mut state, ctx)
    });
    let _ = frame(&ctx, vec![key(Key::H, Modifiers::NONE, true)], |ctx| {
        shortcuts(&mut state, ctx)
    });
    assert!(!state.whiteboard.open);
}

#[test]
fn rear_shortcuts_walk_every_settings_tab_in_both_directions() {
    let ctx = egui::Context::default();
    let mut state = deck_state();
    state.steam_deck.as_mut().unwrap().tab = Tab::Bank;
    let views = [
        (Tab::Bank, Settings::Primary),
        (Tab::Game, Settings::Primary),
        (Tab::Settings, Settings::Primary),
        (Tab::Settings, Settings::Setup),
        (Tab::Settings, Settings::Shaders),
        (Tab::Settings, Settings::Geometry),
        (Tab::Settings, Settings::Effects),
        (Tab::Settings, Settings::Replay),
        (Tab::Settings, Settings::Audio),
        (Tab::Settings, Settings::Ocr),
        (Tab::Settings, Settings::RetroPc),
        (Tab::Settings, Settings::Controls),
    ];
    // Check the endpoints too: neither direction wraps into unrelated views.
    let forwards = views
        .iter()
        .skip(1)
        .chain(views.last())
        .map(|view| (Key::ArrowRight, *view));
    let backwards = views
        .iter()
        .rev()
        .skip(1)
        .chain(views.first())
        .map(|view| (Key::ArrowLeft, *view));
    for (arrow, (tab, settings)) in forwards.chain(backwards) {
        for pressed in [true, false] {
            let _ = frame(
                &ctx,
                vec![key(
                    arrow,
                    if pressed {
                        Modifiers::CTRL | Modifiers::SHIFT
                    } else {
                        Modifiers::NONE
                    },
                    pressed,
                )],
                |ctx| {
                    shortcuts(&mut state, ctx);
                    assert!(ctx.input(|i| i.events.is_empty()));
                },
            );
            let deck = state.steam_deck.as_ref().unwrap();
            assert_eq!((deck.tab, deck.settings), (tab, settings));
            // A held rear button advances once, including within settings.
            if pressed {
                let mut repeated = key(arrow, Modifiers::CTRL | Modifiers::SHIFT, true);
                if let Event::Key { repeat, .. } = &mut repeated {
                    *repeat = true;
                }
                let _ = frame(&ctx, vec![repeated], |ctx| {
                    shortcuts(&mut state, ctx);
                    assert!(ctx.input(|i| i.events.is_empty()));
                });
                let deck = state.steam_deck.as_ref().unwrap();
                assert_eq!((deck.tab, deck.settings), (tab, settings));
            }
        }
    }
}

#[test]
fn rear_navigation_follows_selected_settings_and_reenters_at_primary() {
    let ctx = egui::Context::default();
    let mut state = deck_state();
    let deck = state.steam_deck.as_mut().unwrap();
    deck.tab = Tab::Settings;
    // A section selected by touch is the starting point for rear navigation.
    deck.settings = Settings::Shaders;
    for (arrow, expected) in [
        (Key::ArrowLeft, Settings::Setup),
        (Key::ArrowRight, Settings::Shaders),
    ] {
        let _ = frame(
            &ctx,
            vec![
                key(arrow, Modifiers::CTRL | Modifiers::SHIFT, true),
                key(arrow, Modifiers::NONE, false),
            ],
            |ctx| shortcuts(&mut state, ctx),
        );
        let deck = state.steam_deck.as_ref().unwrap();
        assert_eq!((deck.tab, deck.settings), (Tab::Settings, expected));
    }
    // Opening the whiteboard returns to the game from any settings section.
    let _ = frame(&ctx, vec![key(Key::H, Modifiers::NONE, true)], |ctx| {
        shortcuts(&mut state, ctx)
    });
    assert_eq!(state.steam_deck.as_ref().unwrap().tab, Tab::Game);
    let _ = frame(
        &ctx,
        vec![key(
            Key::ArrowRight,
            Modifiers::CTRL | Modifiers::SHIFT,
            true,
        )],
        |ctx| shortcuts(&mut state, ctx),
    );
    let deck = state.steam_deck.as_ref().unwrap();
    assert_eq!(
        (deck.tab, deck.settings),
        (Tab::Settings, Settings::Primary)
    );
}

#[test]
fn deck_cursor_shortcut_leaves_steam_overlay_chord_available() {
    let ctx = egui::Context::default();
    let mut state = deck_state();
    state.gamescope_hook = Some(crate::gamescope::GamescopeHookState {
        is_active: true,
        ..Default::default()
    });
    let _ = frame(&ctx, vec![key(Key::Tab, Modifiers::SHIFT, true)], |ctx| {
        shortcuts(&mut state, ctx);
        assert_eq!(ctx.input(|i| i.events.len()), 1);
    });
    assert!(state.gamescope_hook.as_ref().unwrap().passthrough_enabled);
    for (pressed, modifiers) in [
        (true, Modifiers::CTRL | Modifiers::ALT),
        (false, Modifiers::NONE),
    ] {
        let _ = frame(&ctx, vec![key(Key::Space, modifiers, pressed)], |ctx| {
            shortcuts(&mut state, ctx);
            assert!(ctx.input(|i| i.events.is_empty()));
        });
        assert!(!state.gamescope_hook.as_ref().unwrap().passthrough_enabled);
    }
    assert!(state.steam_deck.as_ref().unwrap().reserved_keys.is_empty());
}

#[test]
fn deck_relative_mouse_uses_supported_xwayland_confinement() {
    let ctx = egui::Context::default();
    let mut state = deck_state();
    state.pc98.is_active = true;
    state.pc98.passthrough_enabled = true;
    state.steam_deck.as_mut().unwrap().gamepad_error = Some("No hardware in this test".into());
    for (hooked, expected) in [
        (true, egui::CursorGrab::Confined),
        (false, egui::CursorGrab::None),
    ] {
        state.pc98.passthrough_enabled = hooked;
        let output = frame(&ctx, vec![], |ctx| {
            forward_inputs(&mut state, ctx, ctx.screen_rect());
        });
        assert!(output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(
                |cmd| matches!(cmd, egui::ViewportCommand::CursorGrab(grab) if *grab == expected)
            ));
    }
}

#[test]
fn deck_frame_limit_applies_to_idle_launcher_and_active_game() {
    let mut state = deck_state();
    for running in [false, true] {
        state.ui.video_window_open = running;
        state.next_gui_frame_time = None;
        state.pace_gui_frame();
        let started = std::time::Instant::now();
        for _ in 0..4 {
            state.pace_gui_frame();
        }
        let elapsed = started.elapsed();
        let minimum = frame_interval(running) * 4 - std::time::Duration::from_millis(1);
        assert!(
            elapsed >= minimum,
            "running={running}, elapsed={elapsed:?}, minimum={minimum:?}"
        );
    }
}

#[test]
fn ordinary_game_keys_reach_embedded_game_even_with_cursor_unhooked() {
    let ctx = egui::Context::default();
    let mut state = deck_state();
    state.pc98.is_active = true;
    state.pc98.passthrough_enabled = false;
    state.steam_deck.as_mut().unwrap().tab = Tab::Settings;
    // This test exercises keyboard routing without opening a real input device.
    state.steam_deck.as_mut().unwrap().gamepad_error = Some("No hardware in this test".into());
    for pressed in [true, false] {
        let events = vec![
            key(Key::B, Modifiers::NONE, pressed),
            key(Key::ArrowLeft, Modifiers::NONE, pressed),
            key(Key::F5, Modifiers::NONE, pressed),
        ];
        let _ = frame(&ctx, events, |ctx| {
            shortcuts(&mut state, ctx);
            assert_eq!(ctx.input(|i| i.events.len()), 3);
            forward_inputs(&mut state, ctx, ctx.screen_rect());
            assert!(ctx.input(|i| i.events.is_empty()));
        });
        let input = state.pc98.input_state.lock().unwrap();
        for code in [
            crate::pc98::input::RETROK_B,
            crate::pc98::input::RETROK_KP4,
            crate::pc98::input::RETROK_F5,
        ] {
            assert_eq!(input.keys[code as usize], pressed);
        }
    }
    assert!(!state.bank.window_open);
    assert!(!state.ui.studio_window_open);
}

#[test]
fn deck_configuration_and_three_source_profiles_are_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let desktop = dir.path().join("desktop.toml");
    let deck = dir.path().join("deck.toml");
    let original = config::MichadameConfig::default();
    confy::store_path(&desktop, &original).unwrap();
    let before = std::fs::read(&desktop).unwrap();
    let mut state = deck_state();
    assert_ne!(
        config::configuration_path(&state).unwrap(),
        config::configuration_path(&AppState::default()).unwrap()
    );
    for (source, scan) in [
        (Source::Steam, -3.0),
        (Source::Pc88, -5.0),
        (Source::Pc98, -7.0),
    ] {
        change_source(&mut state, source);
        state.crt.hard_scan = scan;
        config::save_config_at(&deck, &mut state).unwrap();
    }
    let saved: config::MichadameConfig = confy::load_path(&deck).unwrap();
    assert_eq!(saved.gamescope_profile.unwrap().crt_hard_scan, Some(-3.0));
    assert_eq!(saved.pc88_profile.unwrap().crt_hard_scan, Some(-5.0));
    assert_eq!(saved.pc98_profile.unwrap().crt_hard_scan, Some(-7.0));
    assert_eq!(saved.video_device, None);
    assert_eq!(saved.video_resolution, None);
    assert_eq!(std::fs::read(desktop).unwrap(), before);
}

#[test]
fn persisted_desktop_features_cannot_activate_in_deck_mode() {
    let mut state = deck_state();
    let mut cfg = config::MichadameConfig::default();
    cfg.mic.enabled = true;
    cfg.webcam.enabled = true;
    cfg.twitch.channel = "example".into();
    cfg.twitch.chat_overlay_enabled = true;
    cfg.twitch.niconico_enabled = true;
    cfg.reset_usb_on_startup = Some(true);
    config::apply_config(&mut state, &cfg);
    assert!(!state.mic.config.enabled);
    assert!(!state.webcam.config.enabled);
    assert!(!state.twitch.config.chat_overlay_enabled);
    assert!(!state.twitch.config.niconico_enabled);
    assert!(!state.ui.reset_usb_on_startup);
    assert!(!state.ui.show_first_run_dialog);
    assert!(state.mic.active_stream.is_none());
    assert!(state.webcam.active_stream.is_none());
}

#[test]
fn stopping_and_switching_sources_preserves_gameplay_settings() {
    let mut state = deck_state();
    state.pc88.is_active = true;
    state.steam_deck.as_mut().unwrap().source = Source::Pc88;
    state.crt.hard_scan = -13.0;
    state.stop_pc88_mode();
    change_source(&mut state, Source::Pc98);
    state.pc98.is_active = true;
    state.crt.hard_scan = -15.0;
    state.stop_pc98_mode();
    change_source(&mut state, Source::Steam);
    change_source(&mut state, Source::Pc88);
    assert_eq!(state.crt.hard_scan, -13.0);
    change_source(&mut state, Source::Pc98);
    assert_eq!(state.crt.hard_scan, -15.0);
}

#[test]
fn capture_selection_never_falls_back_to_outer_gamescope() {
    let snapshot = br#"[
        {"id":1,"info":{"props":{"application.process.id":100}}},
        {"id":2,"info":{"props":{"application.process.id":"200"}}},
        {"id":10,"info":{"props":{"node.name":"gamescope","media.class":"Video/Source","client.id":1,"object.serial":1010}}},
        {"id":20,"info":{"props":{"node.name":"gamescope","media.class":"Video/Source","client.id":"2","object.serial":2020}}}
    ]"#;
    assert_eq!(launch::capture_target(snapshot, 200), Some("2020".into()));
    assert_eq!(launch::capture_target(snapshot, 300), None);
    assert_eq!(launch::capture_target(b"[]", 200), None);
}

#[test]
fn deck_launcher_publishes_only_inner_endpoints_and_preserves_game_arguments() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    for (name, contents) in [
        ("launcher.sh", include_str!("../../scripts/michadame-steam-deck.sh")),
        ("game.sh", include_str!("../../scripts/michadame-steam-deck-game.sh")),
        ("pactl", "#!/bin/sh\nprintf '1 michadame_gamescope module-null-sink.c s16le 2ch 48000Hz RUNNING\\n'\n"),
        ("gamescopectl", r#"#!/bin/sh
set -eu
test "$1" = debug_set_fps_limit
test "$2" = 60
test "$GAMESCOPE_WAYLAND_DISPLAY" = "$(dirname "$0")/runtime/gamescope-inner"
test "$DISPLAY" = :inner
printf '%s\n' "$GAMESCOPE_WAYLAND_DISPLAY" > "$(dirname "$0")/frame-limit-applied"
"#),
        ("gamescope", r#"#!/bin/bash
set -eu
test -z "${WAYLAND_DISPLAY:-}"
test -z "${GAMESCOPE_WAYLAND_DISPLAY:-}"
test "$PULSE_SINK" = michadame_gamescope
test "$SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS" = 1
test "$DISABLE_VK_LAYER_VALVE_steam_overlay_1" = 1
test "$GAMESCOPE_MANGOAPP_SOCKET_DISABLE" = 1
test "$LD_PRELOAD" = libm.so.6
test "$(cat "$(dirname "$0")/gamescope.pid")" = "$$"
test "$GAMESCOPE_LIMITER_FILE" = "$(dirname "$0")/frame-limiter"
printf '%s\n' "$@" > "$(dirname "$0")/gamescope.args"
while [ "$1" != -- ]; do shift; done
shift
export GAMESCOPE_WAYLAND_DISPLAY=gamescope-inner
export DISPLAY=:inner
exec "$@"
"#),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, contents).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap()
    );
    let output = std::process::Command::new("bash")
        .arg(dir.path().join("launcher.sh"))
        .args([
            "/bin/sh",
            "-c",
            "test -z \"${WAYLAND_DISPLAY:-}\" && printf '%s' \"$1\"",
            "game",
            "a game argument with spaces",
        ])
        .env("PATH", path)
        .env("WAYLAND_DISPLAY", "wayland-outer")
        .env("GAMESCOPE_WAYLAND_DISPLAY", "gamescope-outer")
        .env("LD_PRELOAD", "/unused/gameoverlayrenderer.so:libm.so.6")
        .env("DISABLE_VK_LAYER_VALVE_steam_overlay_1", "0")
        .env("GAMESCOPE_MANGOAPP_SOCKET_DISABLE", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        std::fs::read_to_string(dir.path().join("launch.log")).unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("endpoints")).unwrap(),
        format!("{}/runtime/gamescope-inner\n:inner\n", dir.path().display())
    );
    assert!(std::fs::read_to_string(dir.path().join("launch.log"))
        .unwrap()
        .ends_with("a game argument with spaces"));
    let arguments = std::fs::read_to_string(dir.path().join("gamescope.args")).unwrap();
    assert!(arguments
        .lines()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|pair| pair == ["--framerate-limit", "60"]));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("frame-limit-applied")).unwrap(),
        format!("{}/runtime/gamescope-inner\n", dir.path().display())
    );
}

#[test]
fn compact_settings_halve_physical_text_at_the_deck_native_scale() {
    fn render(zoom: f32) -> (f32, egui::FullOutput) {
        let ctx = deck_context();
        let mut state = deck_state();
        configure(&mut state, &ctx);
        state.steam_deck.as_mut().unwrap().tab = Tab::Settings;
        let native = 13.0 / 6.0;
        let mut output = frame(&ctx, vec![], |_| {});
        ctx.set_zoom_factor(zoom);
        for _ in 0..3 {
            let mut input = RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0) / (ctx.zoom_factor() * native),
                )),
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(native);
            output = ctx.run(input, |ctx| {
                crate::ui::steam_deck::draw(&mut state, ctx);
            });
        }
        assert!((ctx.pixels_per_point() - native * zoom).abs() < 0.001);
        assert!(
            (dictionary_scale(&ctx) * ctx.pixels_per_point() - 0.2 * native).abs() < 0.001,
            "Dictionary was shrunk along with settings"
        );
        (ctx.pixels_per_point(), output)
    }
    fn primary_text_size(output: &egui::FullOutput, ppp: f32) -> egui::Vec2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Primary" => {
                    Some(text.galley.rect.size() * ppp)
                }
                _ => None,
            })
            .unwrap()
    }
    let (original_ppp, original) = render(1.0);
    let (compact_ppp, compact) = render(UI_ZOOM);
    let a = primary_text_size(&original, original_ppp);
    let b = primary_text_size(&compact, compact_ppp);
    assert!(
        (b.x / a.x - 0.5).abs() < 0.04 && (b.y / a.y - 0.5).abs() < 0.04,
        "{a:?} => {b:?}"
    );
    for label in [
        "Primary",
        "CRT:",
        "OCR boxes:",
        "Lights:",
        "Replay buffer:",
        "Save last 30 seconds",
        "Capture OCR now",
        "Hook / unhook",
    ] {
        assert!(
            visible_text_position(&compact, label).is_some(),
            "Clipped compact control: {label}"
        );
    }
}

#[test]
fn capture_session_tag_wins_over_conflicting_container_pids() {
    let snapshot = br#"[
      {"id":1,"info":{"props":{"node.name":"gamescope","media.class":"Video/Source","application.process.id":2,"object.serial":111}}},
      {"id":2,"info":{"props":{"node.name":"gamescope","media.class":"Video/Source","application.process.id":99,"object.serial":222,"michadame.deck.session":"my-session"}}}
    ]"#;
    assert_eq!(
        launch::capture_target_for_session(snapshot, Some(2), Some("my-session")),
        Some("222".into())
    );
    assert_eq!(
        launch::capture_target_for_session(snapshot, Some(2), Some("missing")),
        None
    );
}

fn visible_text_position(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
    fn visit(shape: &egui::Shape, clip: egui::Rect, needle: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.text().contains(needle) => {
                let center = text.galley.rect.translate(text.pos.to_vec2()).center();
                clip.contains(center).then_some(center)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| visit(shape, clip, needle)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| visit(&shape.shape, shape.clip_rect, needle))
}

fn small_screen_frame(
    ctx: &egui::Context,
    state: &mut AppState,
    size: egui::Vec2,
    events: Vec<Event>,
) -> egui::FullOutput {
    ctx.run(
        RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ctx| {
            crate::ui::steam_deck::draw(state, ctx);
        },
    )
}

fn deck_context() -> egui::Context {
    let ctx = egui::Context::default();
    let mut fonts = egui::FontDefinitions::default();
    fonts.families.insert(
        egui::FontFamily::Name("GothicCJK".into()),
        fonts.families[&egui::FontFamily::Proportional].clone(),
    );
    ctx.set_fonts(fonts);
    ctx
}

#[test]
fn deck_mining_tag_is_editable_and_commits_when_leaving_setup() {
    let ctx = deck_context();
    let mut state = deck_state();
    configure(&mut state, &ctx);
    let deck = state.steam_deck.as_mut().unwrap();
    deck.tab = Tab::Settings;
    deck.settings = Settings::Setup;
    state.bank.known_tags = vec!["Final Fantasy 7".into()];
    let size = egui::vec2(1280.0, 800.0);
    let mut output = small_screen_frame(&ctx, &mut state, size, vec![]);
    for _ in 0..2 {
        output = small_screen_frame(&ctx, &mut state, size, vec![]);
    }
    assert!(visible_text_position(&output, "Mining Tag:").is_some());
    assert!(visible_text_position(&output, "Save mining tag").is_some());
    let position = visible_text_position(&output, "e.g. Final Fantasy 7").unwrap();
    for pressed in [true, false] {
        small_screen_frame(
            &ctx,
            &mut state,
            size,
            vec![
                Event::PointerMoved(position),
                Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
    }
    assert!(ctx.wants_keyboard_input());
    small_screen_frame(
        &ctx,
        &mut state,
        size,
        vec![Event::Text("  final fantasy 7  ".into())],
    );
    assert_eq!(state.bank.current_tag, "  final fantasy 7  ");
    assert!(state.bank.saved_current_tag.is_empty());
    // Navigation hides the field before TextEdit can report a lost-focus frame.
    small_screen_frame(
        &ctx,
        &mut state,
        size,
        vec![key(
            Key::ArrowRight,
            Modifiers::CTRL | Modifiers::SHIFT,
            true,
        )],
    );
    assert_eq!(
        state.steam_deck.as_ref().unwrap().settings,
        Settings::Shaders
    );
    assert_eq!(state.bank.current_tag, "Final Fantasy 7");
    assert_eq!(state.bank.saved_current_tag, "Final Fantasy 7");
    // The explicit save button also commits and normalizes an edited tag.
    small_screen_frame(
        &ctx,
        &mut state,
        size,
        vec![key(
            Key::ArrowLeft,
            Modifiers::CTRL | Modifiers::SHIFT,
            true,
        )],
    );
    output = small_screen_frame(&ctx, &mut state, size, vec![]);
    let position = visible_text_position(&output, "Save mining tag").unwrap();
    state.bank.current_tag = "  Persona 5  ".into();
    for pressed in [true, false] {
        small_screen_frame(
            &ctx,
            &mut state,
            size,
            vec![
                Event::PointerMoved(position),
                Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
    }
    assert_eq!(state.bank.current_tag, "Persona 5");
    assert_eq!(state.bank.saved_current_tag, "Persona 5");
}

#[test]
fn deck_setup_and_emulator_file_buttons_remain_reachable_at_small_sizes() {
    for size in [
        egui::vec2(1280.0, 800.0),
        egui::vec2(960.0, 600.0),
        egui::vec2(800.0, 500.0),
    ] {
        for (page, system, button) in [
            (
                Settings::Setup,
                crate::pc88::RetroPcSystem::Pc98,
                "Import presets from",
            ),
            (
                Settings::RetroPc,
                crate::pc88::RetroPcSystem::Pc98,
                "Browse...",
            ),
            (
                Settings::RetroPc,
                crate::pc88::RetroPcSystem::Pc88,
                "Browse...",
            ),
        ] {
            let ctx = deck_context();
            let mut state = deck_state();
            configure(&mut state, &ctx);
            state.steam_deck.as_mut().unwrap().tab = Tab::Settings;
            state.steam_deck.as_mut().unwrap().settings = page;
            state.retro_pc_system = system;
            let mut position = None;
            for scroll in 0..15 {
                let events = if scroll < 2 {
                    vec![]
                } else {
                    vec![
                        Event::PointerMoved(egui::pos2(size.x * 0.5, size.y * 0.8)),
                        Event::Scroll(egui::vec2(0.0, -160.0)),
                    ]
                };
                let output = small_screen_frame(&ctx, &mut state, size, events);
                assert!(
                    visible_text_position(&output, "Primary").is_some(),
                    "Navigation scrolled away at {size:?}"
                );
                position = visible_text_position(&output, button)
                    .filter(|pos| pos.x < size.x && pos.y < size.y);
                if scroll > 0 && position.is_some() {
                    break;
                }
            }
            let position = position.unwrap_or_else(|| {
                panic!("Unreachable {button} on {page:?}/{system:?} at {size:?}")
            });
            let _ = small_screen_frame(
                &ctx,
                &mut state,
                size,
                vec![
                    Event::PointerMoved(position),
                    Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::NONE,
                    },
                ],
            );
            let output = small_screen_frame(
                &ctx,
                &mut state,
                size,
                vec![Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::NONE,
                }],
            );
            assert!(
                state.steam_deck.as_ref().unwrap().file_picker.is_some(),
                "File picker did not open for {button} at {size:?}"
            );
            assert_eq!(
                output.viewport_output.len(),
                1,
                "Opened a native window in Deck mode"
            );
        }
    }
}

#[test]
fn deck_notifications_and_warning_dialogs_are_not_drawn() {
    let ctx = deck_context();
    let mut state = deck_state();
    configure(&mut state, &ctx);
    state.config_load_error = Some("hidden config warning".into());
    state.ui.show_quit_dialog = true;
    state.steam_deck.as_mut().unwrap().capture_error = Some("hidden capture warning".into());
    state.info("hidden info toast");
    state.error("hidden error toast");
    for tab in [Tab::Game, Tab::Settings, Tab::Bank] {
        state.steam_deck.as_mut().unwrap().tab = tab;
        let output = ctx.run(
            RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            },
            |ctx| {
                crate::ui::steam_deck::draw(&mut state, ctx);
                state.toasts.show(ctx);
            },
        );
        for hidden in [
            "hidden config",
            "hidden capture",
            "hidden info",
            "hidden error",
            "Configuration Warning",
            "Quit?",
        ] {
            assert!(
                visible_text_position(&output, hidden).is_none(),
                "Unexpected {hidden} on {tab:?}"
            );
        }
    }
}

#[test]
fn deck_preset_import_preserves_existing_presets_and_ignores_hdmi_hardware() {
    let mut state = deck_state();
    state
        .profiles
        .insert("My preset".into(), config::Profile::default());
    let mut configuration = config::MichadameConfig::default();
    let mut profile = config::Profile::default();
    profile.video_format_fourcc = Some("MJPG".into());
    profile.video_resolution = Some((3840, 2160));
    profile.scaler_filter = Some(3);
    configuration.profiles.insert("My preset".into(), profile);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("desktop-config.toml");
    confy::store_path(&path, configuration).unwrap();
    let before = std::fs::read(&path).unwrap();
    crate::ui::steam_deck::import_presets(&mut state, &path).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), before);
    let imported = state.profiles.get("My preset (2)").unwrap();
    assert_eq!(imported.video_format_fourcc, None);
    assert_eq!(imported.video_resolution, None);
    assert_eq!(imported.scaler_filter, Some(3));
}

#[test]
fn controller_buttons_triggers_and_both_sticks_reach_libretro() {
    let (buttons, axes) =
        gamepad::map_snapshot([true; 15], [-32768, 32767, 12345, -23456, 32767, 32767]);
    let mut input = crate::pc98::Pc98InputState {
        joypad_buttons: buttons,
        analog_axes: axes,
        ..Default::default()
    };
    assert_eq!(input.controller_value(1, 0, 256) as u16, u16::MAX);
    for id in 0..16 {
        assert_eq!(input.controller_value(1, 0, id), 1);
    }
    assert_eq!(input.controller_value(5, 0, 0), -32768);
    assert_eq!(input.controller_value(5, 1, 1), -23456);
    input.release_all();
    assert_eq!(input.controller_value(1, 0, 256), 0);
    assert_eq!(input.controller_value(5, 1, 1), 0);
}

#[test]
fn deck_tabs_use_one_native_viewport_and_primary_controls_are_visible() {
    let ctx = egui::Context::default();
    let mut state = deck_state();
    let mut fonts = egui::FontDefinitions::default();
    fonts.families.insert(
        egui::FontFamily::Name("GothicCJK".into()),
        fonts.families[&egui::FontFamily::Proportional].clone(),
    );
    ctx.set_fonts(fonts);
    configure(&mut state, &ctx);
    for tab in [Tab::Game, Tab::Bank, Tab::Settings] {
        state.steam_deck.as_mut().unwrap().tab = tab;
        let output = frame(&ctx, vec![], |ctx| {
            crate::ui::steam_deck::draw(&mut state, ctx);
        });
        assert_eq!(output.viewport_output.len(), 1);
        // Navigation stays on L4/R4 and never consumes space above the game.
        assert!(output.shapes.iter().all(|shape| match &shape.shape {
            egui::Shape::Text(t) =>
                !["◀ Word bank", "Main game", "Settings ▶"].contains(&t.galley.text()),
            _ => true,
        }));
        if tab == Tab::Settings {
            let mut text = String::new();
            fn visit(shape: &egui::Shape, text: &mut String) {
                match shape {
                    egui::Shape::Text(t) => {
                        text.push_str(t.galley.text());
                        text.push('\n');
                    }
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            visit(shape, text);
                        }
                    }
                    _ => {}
                }
            }
            for shape in &output.shapes {
                visit(&shape.shape, &mut text);
            }
            for label in [
                "CRT:",
                "OCR boxes:",
                "Lights:",
                "Replay buffer:",
                "Save last 30 seconds",
            ] {
                assert!(text.contains(label), "Missing {label}: {text}");
            }
            for hidden in ["HDMI", "Webcam", "Twitch", "Audio & Mic", "Video File"] {
                assert!(!text.contains(hidden));
            }
        }
    }
}

#[test]
fn mapped_video_rows_are_converted_without_padding_or_channel_swaps() {
    let bytes = [
        10, 20, 30, 0, 40, 50, 60, 0, 99, 99, 99, 99, 70, 80, 90, 0, 100, 110, 120, 0,
    ];
    assert_eq!(
        capture::rgba_pixels(&bytes, 2, 2, 12, true).unwrap(),
        [30, 20, 10, 255, 60, 50, 40, 255, 90, 80, 70, 255, 120, 110, 100, 255]
    );
    assert_eq!(
        capture::rgba_pixels(&bytes, 2, 2, 12, false).unwrap()[..4],
        [10, 20, 30, 255]
    );
    assert!(capture::rgba_pixels(&bytes[..19], 2, 2, 12, true).is_none());
    assert!(capture::rgba_pixels(&bytes, 2, 2, 4, true).is_none());
    assert!(capture::rgba_pixels(&bytes, 0, 2, 12, true).is_none());
}

#[test]
fn restart_reconnects_only_to_a_live_session_for_the_selected_game() {
    let runtime = tempfile::tempdir().unwrap();
    for (name, appid, pid) in [
        ("michadame-deck-alive", "1234", 100),
        ("michadame-deck-dead", "1234", 200),
        ("michadame-deck-other", "5678", 300),
    ] {
        let path = runtime.path().join(name);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("appid"), appid).unwrap();
        std::fs::write(path.join("gamescope.pid"), pid.to_string()).unwrap();
        std::fs::write(path.join("endpoints"), "gamescope-1\n:3\n").unwrap();
    }
    assert_eq!(
        launch::reusable_session(runtime.path(), "1234", |pid, _| pid != 200),
        Some(runtime.path().join("michadame-deck-alive"))
    );
    assert!(launch::reusable_session(runtime.path(), "9999", |_, _| true).is_none());
    assert!(launch::reusable_session(runtime.path(), "1234", |_, _| false).is_none());
    let legacy = runtime.path().join("michadame-deck-alive");
    std::fs::remove_file(legacy.join("appid")).unwrap();
    std::fs::write(
        legacy.join("launch.log"),
        "  steam app id:                  1234\n",
    )
    .unwrap();
    assert_eq!(
        launch::reusable_session(runtime.path(), "1234", |pid, _| pid == 100),
        Some(legacy)
    );
}

#[test]
fn tapped_dictionary_survives_finger_lift_and_closes_on_outside_tap() {
    let ctx = egui::Context::default();
    let mut fonts = egui::FontDefinitions::default();
    fonts.families.insert(
        egui::FontFamily::Name("GothicCJK".into()),
        fonts.families[&egui::FontFamily::Proportional].clone(),
    );
    ctx.set_fonts(fonts);
    let mut state = deck_state();
    state.bank.custom_names.push(crate::bank::CustomNameEntry {
        id: 1,
        created_at: 0,
        kanji: "テスト".into(),
        furigana: "てすと".into(),
        source_tag: None,
        notes: None,
    });
    state.ocr.boxes.push(crate::ocr::OcrBox {
        text: "テスト".into(),
        center_x: 0.5,
        center_y: 0.1,
        width: 0.2,
        height: 0.08,
        lines: vec![],
        writing_direction: crate::ocr::WritingDirection::LeftToRight,
    });
    let word = egui::pos2(540.0, 80.0);
    let render = |state: &mut AppState, ctx: &egui::Context| {
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                crate::ocr::overlay::draw_ocr_overlay(ui, state, ctx.screen_rect())
            });
    };
    let _ = frame(&ctx, vec![], |ctx| render(&mut state, ctx));
    for pressed in [true, false] {
        let _ = frame(
            &ctx,
            vec![
                Event::PointerMoved(word),
                Event::PointerButton {
                    pos: word,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
            |ctx| render(&mut state, ctx),
        );
    }
    assert!(state.steam_deck.as_ref().unwrap().pinned_dictionary);
    assert!(state.dict.popup.is_some(), "tap must open dictionary");
    let _ = frame(&ctx, vec![Event::PointerGone], |ctx| {
        render(&mut state, ctx)
    });
    assert!(
        state.dict.popup.is_some(),
        "finger lift must keep popup open"
    );
    let outside = egui::pos2(5.0, 5.0);
    for pressed in [true, false] {
        let _ = frame(
            &ctx,
            vec![
                Event::PointerMoved(outside),
                Event::PointerButton {
                    pos: outside,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
            |ctx| render(&mut state, ctx),
        );
    }
    assert!(state.dict.popup.is_none());
}

#[test]
fn hooked_cursor_hides_ocr_overlay_and_popup_and_passes_clicks_over_ocr_boxes() {
    let ctx = deck_context();
    let mut state = deck_state();
    state.pc98.is_active = true;
    state.pc98.passthrough_enabled = false;
    state.steam_deck.as_mut().unwrap().gamepad_error = Some("No hardware in this test".into());
    state.bank.custom_names.push(crate::bank::CustomNameEntry {
        id: 1,
        created_at: 0,
        kanji: "テスト".into(),
        furigana: "てすと".into(),
        source_tag: None,
        notes: None,
    });
    state.ocr.boxes.push(crate::ocr::OcrBox {
        text: "テスト".into(),
        center_x: 0.5,
        center_y: 0.1,
        width: 0.2,
        height: 0.08,
        lines: vec![],
        writing_direction: crate::ocr::WritingDirection::LeftToRight,
    });
    let word = egui::pos2(540.0, 80.0);
    let render = |state: &mut AppState, ctx: &egui::Context| {
        shortcuts(state, ctx);
        let screen = ctx.screen_rect();
        forward_inputs(state, ctx, screen);
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                crate::ocr::overlay::draw_ocr_overlay(ui, state, screen)
            });
    };

    // Unhooked: tapping an OCR word opens the dictionary popup and does not click in-game.
    let _ = frame(&ctx, vec![], |ctx| render(&mut state, ctx));
    for pressed in [true, false] {
        let _ = frame(
            &ctx,
            vec![
                Event::PointerMoved(word),
                Event::PointerButton {
                    pos: word,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
            |ctx| render(&mut state, ctx),
        );
        assert!(!state.pc98.input_state.lock().unwrap().mouse_left);
    }
    assert!(state.dict.popup.is_some());
    assert!(state.steam_deck.as_ref().unwrap().pinned_dictionary);

    // Hooking via L5 (Ctrl+Alt+Space) immediately dismisses the popup and hides OCR overlay.
    let _ = frame(
        &ctx,
        vec![key(Key::Space, Modifiers::CTRL | Modifiers::ALT, true)],
        |ctx| render(&mut state, ctx),
    );
    assert!(state.pc98.passthrough_enabled);
    assert!(state.dict.popup.is_none());
    assert!(!state.steam_deck.as_ref().unwrap().pinned_dictionary);
    assert!(!state.ocr.is_box_hovered);

    // While hooked, hovering and clicking directly on the OCR box passes the click to the game
    // and never raises the OCR dictionary popup.
    let _ = frame(
        &ctx,
        vec![
            Event::PointerMoved(word),
            Event::PointerButton {
                pos: word,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
        |ctx| render(&mut state, ctx),
    );
    assert!(
        state.pc98.input_state.lock().unwrap().mouse_left,
        "Mouse click over OCR box must reach the game when hooked"
    );
    assert!(
        state.dict.popup.is_none(),
        "OCR popup must not appear while input is hooked"
    );
    assert!(!state.ocr.is_box_hovered);

    let _ = frame(
        &ctx,
        vec![Event::PointerButton {
            pos: word,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
        |ctx| render(&mut state, ctx),
    );
    assert!(!state.pc98.input_state.lock().unwrap().mouse_left);
}
