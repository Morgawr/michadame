mod app;
mod bank;
mod config;
mod devices;
pub mod dict;
pub mod gamescope;
mod ocr;
pub mod pc88;
pub mod pc98;
mod portable;
mod replay;
mod steam_deck;
mod twitch;
mod ui;
mod video;

use anyhow::Result;
use eframe::egui;

fn main() -> Result<(), eframe::Error> {
    crate::portable::initialize();
    // Setup logging
    let steam_deck = std::env::args_os().any(|arg| arg == "--steam-deck");
    if steam_deck {
        crate::steam_deck::logging::initialize();
    } else {
        tracing_subscriber::fmt::init();
    }
    // Game Mode identifies the viewer by its X11 STEAM_GAME property. Choose
    // Xwayland when available, before the UI or any worker threads are started.
    if steam_deck && std::env::var_os("DISPLAY").is_some() {
        std::env::remove_var("WAYLAND_DISPLAY");
    }

    // --- Load Icon ---
    let icon = image::load_from_memory(include_bytes!("../assets/logo.png"))
        .expect("Failed to load application icon")
        .to_rgba8();
    let (icon_width, icon_height) = icon.dimensions();

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([640.0, 480.0]) // Default starting size for the video window
        .with_min_inner_size([320.0, 240.0])
        .with_icon(egui::IconData {
            rgba: icon.into_raw(),
            width: icon_width,
            height: icon_height,
        });
    if steam_deck {
        viewport = viewport
            .with_inner_size([1280.0, 800.0])
            .with_fullscreen(true);
    }
    let options = eframe::NativeOptions {
        viewport,
        persist_window: !steam_deck,
        vsync: steam_deck,
        ..Default::default()
    };

    // Create a closure that will be called once to create the App state.
    let creator = move |cc: &eframe::CreationContext| {
        // --- Embed a local font for 100% robust character support ---
        let mut fonts = egui::FontDefinitions::default();

        fonts.font_data.insert(
            "roboto_slab".to_owned(),
            egui::FontData::from_static(include_bytes!("../assets/RobotoSlab-Regular.ttf")).tweak(
                egui::FontTweak {
                    scale: 1.05,
                    ..Default::default()
                },
            ),
        );
        fonts.font_data.insert(
            "noto_sans_jp".to_owned(),
            egui::FontData::from_static(include_bytes!("../assets/NotoSansJP-Regular.ttf")),
        );
        fonts.font_data.insert(
            "noto_emoji".to_owned(),
            egui::FontData::from_static(include_bytes!("../assets/NotoColorEmoji-Regular.ttf")),
        );

        fonts.families.insert(
            egui::FontFamily::Name("GothicCJK".into()),
            vec![
                "noto_sans_jp".to_owned(),
                "roboto_slab".to_owned(),
                "noto_emoji".to_owned(),
            ],
        );

        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .extend(vec![
                "roboto_slab".to_owned(),
                "noto_sans_jp".to_owned(),
                "noto_emoji".to_owned(),
            ]);

        fonts
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .unwrap()
            .extend(vec![
                "roboto_slab".to_owned(),
                "noto_sans_jp".to_owned(),
                "noto_emoji".to_owned(),
            ]);

        cc.egui_ctx.set_fonts(fonts);
        Box::new(app::init_app_state(cc, steam_deck)) as Box<dyn eframe::App>
    };

    let res = eframe::run_native("Michadame Viewer", options, Box::new(creator));
    if let Err(ref e) = res {
        tracing::error!("Application error: {e}");
    }
    res
}
