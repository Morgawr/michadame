mod app;
mod config;
mod devices;
mod ui;
mod video;

use eframe::egui;

fn main() -> Result<(), eframe::Error> {
    // Setup logging
    tracing_subscriber::fmt::init();

    // --- Load Icon ---
    let icon = image::load_from_memory(include_bytes!("../assets/logo.png"))
        .expect("Failed to load application icon")
        .to_rgba8();
    let (icon_width, icon_height) = icon.dimensions();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([640.0, 480.0]) // Default starting size for the video window
            .with_min_inner_size([320.0, 240.0])
            .with_icon(egui::IconData {
                rgba: icon.into_raw(),
                width: icon_width,
                height: icon_height,
            }),
        persist_window: true,
        ..Default::default()
    };

    // Create a closure that will be called once to create the App state.
    let creator = |cc: &eframe::CreationContext| {
        // --- Embed a local font for 100% robust character support ---
        let mut fonts = egui::FontDefinitions::default();

        fonts.font_data.insert("roboto_slab".to_owned(), egui::FontData::from_static(include_bytes!("../assets/RobotoSlab-Regular.ttf")).tweak(
            egui::FontTweak { scale: 1.05, ..Default::default() },
        ),);
        fonts.font_data.insert("noto_sans_jp".to_owned(), egui::FontData::from_static(include_bytes!("../assets/NotoSansJP-Regular.ttf")));
        fonts.font_data.insert("noto_emoji".to_owned(), egui::FontData::from_static(include_bytes!("../assets/NotoColorEmoji-Regular.ttf")));

        fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap()
            .extend(vec!["roboto_slab".to_owned(), "noto_sans_jp".to_owned(), "noto_emoji".to_owned()]);

        fonts.families.get_mut(&egui::FontFamily::Monospace).unwrap()
            .extend(vec!["roboto_slab".to_owned(), "noto_sans_jp".to_owned(), "noto_emoji".to_owned()]);

        cc.egui_ctx.set_fonts(fonts);
        Box::new(app::AppState::new(cc)) as Box<dyn eframe::App>
    };

    eframe::run_native("Michadame Viewer", options, Box::new(creator))
}
