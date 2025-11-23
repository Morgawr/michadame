// src/main.rs

use anyhow::{anyhow, Context, Result};
use eframe::egui;
use libpulse_binding::callbacks::ListResult;
use libpulse_binding::def::Retval;
use libpulse_binding::context::{Context as PulseContext, FlagSet as PulseContextFlagSet, State as PulseContextState};
use libpulse_binding::mainloop::standard::{IterateResult, Mainloop};
use libpulse_binding::operation::State as OperationState;
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::rc::Rc;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::sync::mpsc;
use std::time::Instant;
use std::thread::{self, JoinHandle};

/// Holds the results of our background device scan.
enum DeviceScanResult {
    Success(Vec<String>, Vec<(String, String)>, Vec<(String, String)>, Vec<(String, String)>),
    Failure(anyhow::Error),
}

/// Defines the structure of our configuration file.
#[derive(Default, Serialize, Deserialize, Clone)]
struct MichadameConfig {
    video_device: Option<String>,
    usb_device: Option<String>,
    pulse_source: Option<String>,
    pulse_sink: Option<String>,
    video_format_fourcc: Option<String>,
    video_resolution: Option<(u32, u32)>,
    video_framerate: Option<u32>,
    reset_usb_on_startup: Option<bool>,
}

/// Holds resolution and its available framerates.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
    pub framerates: Vec<u32>,
}


/// Holds information about a supported video format.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoFormat {
    pub fourcc: String,
    pub description: String,
    pub resolutions: Vec<Resolution>,
}

impl Default for VideoFormat {
    fn default() -> Self {
        Self { fourcc: "0000".to_string(), description: "None".to_string(), resolutions: vec![] }
    }
}

/// Represents the state of our application
struct AppState {
    video_devices: Vec<String>,
    // (ID, Name)
    usb_devices: Vec<(String, String)>,
    selected_usb_device: Option<String>,
    selected_video_device: String,
    // (Description, Name)
    pulse_sources: Vec<(String, String)>,
    pulse_sinks: Vec<(String, String)>,
    selected_pulse_source_name: Option<String>,
    selected_pulse_sink_name: Option<String>,
    pulse_loopback_module_index: Option<u32>,
    status_message: String,
    // Video format state
    supported_formats: Vec<VideoFormat>,
    selected_format_index: usize,
    selected_resolution: (u32, u32),
    selected_framerate: u32,
    // Embedded video player state
    video_thread: Option<JoinHandle<()>>,
    stop_video_thread: Option<Arc<AtomicBool>>,
    video_texture: Option<egui::TextureHandle>,
    // A lock-free, single-element channel for the latest video frame.
    frame_receiver: Option<crossbeam_channel::Receiver<Arc<egui::ColorImage>>>,
    // Receiver for the background device scan
    device_scan_receiver: Option<mpsc::Receiver<DeviceScanResult>>,
    logo_texture: Option<egui::TextureHandle>,
    // FPS counter state
    last_fps_check: Instant,
    frames_since_last_check: u32,
    // Video FPS counter state
    last_video_fps_check: Instant,
    video_frames_since_last_check: u32,
    // Fullscreen state
    is_fullscreen: bool,
    reset_usb_on_startup: bool,
    // State machine for the fullscreen toggle workaround. 0: idle, 1: enter, 2: exit.
    fullscreen_toggle_state: u8,
}
impl Default for AppState {
    fn default() -> Self {
        Self {
            video_devices: Vec::new(),
            usb_devices: Vec::new(),
            selected_usb_device: None,
            selected_video_device: String::new(),
            pulse_sources: Vec::new(),
            pulse_sinks: Vec::new(),
            selected_pulse_source_name: None,
            selected_pulse_sink_name: None,
            pulse_loopback_module_index: None,
            status_message: "Loading devices...".to_string(),
            supported_formats: Vec::new(),
            selected_format_index: 0,
            selected_resolution: (0, 0),
            selected_framerate: 0,
            video_thread: None,
            stop_video_thread: None,
            video_texture: None,
            frame_receiver: None,
            device_scan_receiver: None,
            logo_texture: None,
            last_fps_check: Instant::now(),
            frames_since_last_check: 0,
            last_video_fps_check: Instant::now(),
            video_frames_since_last_check: 0,
            is_fullscreen: false,
            reset_usb_on_startup: false,
            fullscreen_toggle_state: 0,
        }
    }
}

/// Implement the eframe::App trait for our state.
/// This gives us more control than run_simple_native.
impl eframe::App for AppState {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // --- Handle the fullscreen toggle state machine ---
        if self.fullscreen_toggle_state == 2 {
            self.fullscreen_toggle_state = 0;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        } else if self.fullscreen_toggle_state == 1 {
            self.fullscreen_toggle_state = 2;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }

        // --- Fullscreen Toggle on 'F' key press ---
        if ctx.input(|i| i.key_pressed(egui::Key::F)) {
            self.is_fullscreen = !self.is_fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
        }

        // Conditionally remove the panel's frame/margin when in fullscreen.
        let panel_frame = if self.is_fullscreen {
            egui::Frame::none()
        } else {
            egui::Frame::central_panel(&ctx.style())
        };

        egui::CentralPanel::default().frame(panel_frame).show(ctx, |ui| {

            // --- Check for new video frames ---
            // Receive the latest frame, dropping any older ones in the queue.
            if let Some(rx) = &self.frame_receiver {
                if let Ok(image) = rx.try_recv() {
                    self.video_texture.as_mut().unwrap().set(image, egui::TextureOptions::LINEAR);
                    self.video_frames_since_last_check += 1;
                }
            }

            layout_top_ui(ui, self);

            // --- Check for device scan results ---
            if let Some(rx) = &self.device_scan_receiver {
                if let Ok(scan_result) = rx.try_recv() {
                    match scan_result {
                        DeviceScanResult::Success(video_devices, pulse_sources, pulse_sinks, usb_devices) => {
                            self.video_devices = video_devices;
                            self.selected_video_device = self.video_devices.first().cloned().unwrap_or_default();
                            self.pulse_sources = pulse_sources;
                            self.pulse_sinks = pulse_sinks;
                            self.usb_devices = usb_devices;
                            self.status_message = "Devices loaded successfully.".to_string();

                            // --- Load and apply saved configuration ---
                            if let Ok(cfg) = confy::load::<MichadameConfig>("michadame", None) {
                                // Apply video device if it still exists
                                if let Some(saved_device) = cfg.video_device {
                                    if self.video_devices.contains(&saved_device) {
                                        self.selected_video_device = saved_device;
                                    }
                                }
                                // Apply USB device if it still exists
                                if let Some(saved_usb) = cfg.usb_device {
                                    if self.usb_devices.iter().any(|(id, _)| id == &saved_usb) {
                                        self.selected_usb_device = Some(saved_usb);
                                    }
                                }
                                // Apply pulse source if it still exists
                                if let Some(saved_source) = cfg.pulse_source {
                                    if self.pulse_sources.iter().any(|(_, name)| name == &saved_source) {
                                        self.selected_pulse_source_name = Some(saved_source);
                                    }
                                }
                                // Apply pulse sink if it still exists
                                if let Some(saved_sink) = cfg.pulse_sink {
                                    if self.pulse_sinks.iter().any(|(_, name)| name == &saved_sink) {
                                        self.selected_pulse_sink_name = Some(saved_sink);
                                    }
                                }

                                // After loading the saved video device, automatically scan its formats.
                                if !self.selected_video_device.is_empty() {
                                    if let Ok(formats) = find_video_formats(&self.selected_video_device) {
                                        self.supported_formats = formats;
                                        // Now, try to apply the saved format and resolution
                                        if let Some(saved_fourcc) = cfg.video_format_fourcc {
                                            if let Some(idx) = self.supported_formats.iter().position(|f| f.fourcc == saved_fourcc) {
                                                self.selected_format_index = idx;
                                                if let Some(saved_res) = cfg.video_resolution {
                                                    if self.supported_formats[idx].resolutions.iter().any(|r| r.width == saved_res.0 && r.height == saved_res.1) {
                                                        self.selected_resolution = saved_res;
                                                        // Also apply saved framerate if it's valid for the restored resolution
                                                        if let Some(saved_fps) = cfg.video_framerate {
                                                            if let Some(res_info) = self.supported_formats[idx].resolutions.iter().find(|r| r.width == saved_res.0 && r.height == saved_res.1) {
                                                                if res_info.framerates.contains(&saved_fps) {
                                                                    self.selected_framerate = saved_fps;
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    } else {
                                        self.status_message = "Failed to auto-scan formats for saved device.".to_string();
                                    }
                                }

                                // Apply "Reset on Startup" setting
                                self.reset_usb_on_startup = cfg.reset_usb_on_startup.unwrap_or(false);
                                // Only auto-reset if the option is checked AND a valid device is selected.
                                if self.reset_usb_on_startup {
                                    if let Some(device_to_reset) = &self.selected_usb_device {
                                        self.status_message = match reset_usb_device(device_to_reset) {
                                            Ok(_) => "Auto-reset USB device successfully.".to_string(),
                                            Err(e) => format!("Failed to auto-reset USB: {}", e),
                                        };
                                        tracing::info!("USB device reset on startup as requested.");
                                    }
                                }
                            }
                        }
                        DeviceScanResult::Failure(e) => {
                            self.status_message = format!("Error: {}", e);
                        }
                    }
                    self.device_scan_receiver = None; // Stop checking
                }
                // Keep repainting the UI until the scan is complete.
                ctx.request_repaint();
            }

            // --- Embedded Video Player (only shown when stream is active) ---
            if self.video_thread.is_some() {
                if !self.is_fullscreen {
                    ui.separator();
                }
                let image_widget = if self.is_fullscreen {
                    // In fullscreen, stretch to fill the entire available space.
                    // Use fit_to_exact_size to force the image to fill the panel.
                    egui::Image::new(self.video_texture.as_ref().unwrap()).fit_to_exact_size(ui.available_size())
                } else if self.selected_resolution.0 > 0 {
                    // In normal mode, explicitly set the image size to the video's native resolution.
                    // This prevents it from incorrectly stretching to fill the window width.
                    let video_size = egui::vec2(self.selected_resolution.0 as f32, self.selected_resolution.1 as f32);
                    egui::Image::new(self.video_texture.as_ref().unwrap()).fit_to_exact_size(video_size)
                } else {
                    // Fallback for when no resolution is selected yet.
                    egui::Image::new(self.video_texture.as_ref().unwrap()).max_width(ui.available_width())
                };

                let response = ui.add(image_widget.sense(egui::Sense::click()));
                // --- Fullscreen toggle on double-click ---
                if response.double_clicked() {
                    self.is_fullscreen = !self.is_fullscreen;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
                }
            } else {
                // Placeholder
                if !self.is_fullscreen {
                    ui.allocate_space(egui::vec2(640.0, 360.0));
                }
            }

            // --- FPS Counter ---
            self.frames_since_last_check += 1;
            let now = Instant::now();
            let elapsed_secs = (now - self.last_fps_check).as_secs_f32();

            if elapsed_secs >= 1.0 {
                let _fps = self.frames_since_last_check as f32 / elapsed_secs;
                self.last_fps_check = now;
                self.frames_since_last_check = 0;
            }
            
            // --- Video FPS Counter ---
            let video_elapsed_secs = (now - self.last_video_fps_check).as_secs_f32();
            if video_elapsed_secs >= 1.0 {
                let _video_fps = self.video_frames_since_last_check as f32 / video_elapsed_secs;
                self.last_video_fps_check = now;
                self.video_frames_since_last_check = 0;
            }

            // --- Update Window Title with FPS ---
            let gui_fps = self.frames_since_last_check as f32 / (now - self.last_fps_check).as_secs_f32().max(0.001);
            let video_fps = self.video_frames_since_last_check as f32 / video_elapsed_secs.max(0.001);
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("Michadame Viewer | UI: {:.0} FPS | Video: {:.0} FPS", gui_fps, video_fps)));

            // Force the UI to repaint continuously. This is necessary for smooth video playback.
            ctx.request_repaint();
        });
    }
}

/// Lays out all the UI controls that appear above and below the video.
fn layout_top_ui(ui: &mut egui::Ui, state: &mut AppState) {
    if state.is_fullscreen {
        return;
    }
    layout_top_ui_content(ui, state);
}
fn layout_top_ui_content(ui: &mut egui::Ui, state: &mut AppState) {
    // --- App Header with Logo ---
    ui.horizontal(|ui| {
        if let Some(logo) = &state.logo_texture {
            ui.add(egui::Image::new(logo).max_height(160.0));
        }
        ui.heading("Michadame Viewer");
    });
    ui.separator();

    ui.horizontal(|ui| {
        ui.label("USB Device to Reset:");
        let selected_text = state.selected_usb_device.as_ref()
            .and_then(|selected_id| {
                state.usb_devices.iter().find(|(id, _)| id == selected_id)
                    .map(|(id, name)| format!("{} {}", id, name))
            })
            .unwrap_or_else(|| "None".to_string());
        egui::ComboBox::from_id_source("usb_device_selector")
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                // Add an option for "None"
                if ui.selectable_value(&mut state.selected_usb_device, None, "None").changed() {
                    save_config(state);
                }
                for (id, name) in &state.usb_devices {
                    if ui.selectable_value(&mut state.selected_usb_device, Some(id.clone()), format!("{} {}", id, name)).changed() {
                        save_config(state);
                    }
                }
            });

        if let Some(selected_device) = &state.selected_usb_device {
            if ui.button("Reset USB Device").clicked() {
                state.status_message = match reset_usb_device(selected_device) {
                    Ok(_) => "USB device reset successfully.".to_string(),
                    Err(e) => format!("Failed to reset USB: {}", e),
                };
            }
            if ui.checkbox(&mut state.reset_usb_on_startup, "Reset on startup").changed() {
                save_config(state);
            }
        }
    });

    ui.separator();

    // --- 2. Video Device Selection ---
    ui.horizontal(|ui| {
        ui.label("Video Device:");
        let _combo_box = egui::ComboBox::from_id_source("video_device_selector")
            .selected_text(state.selected_video_device.as_str())
            .show_ui(ui, |ui| {
                let mut changed = false;
                for device in &state.video_devices {
                    changed |= ui.selectable_value(&mut state.selected_video_device, device.clone(), device.as_str()).changed();
                }
                if changed && !state.selected_video_device.is_empty() {
                    save_config(state);
                    // Clear old format info when device changes
                    state.supported_formats.clear();
                    state.selected_format_index = 0;
                    state.selected_resolution = (0, 0);

                    // Automatically scan formats for the new device
                    match find_video_formats(&state.selected_video_device) {
                        Ok(formats) => {
                            state.status_message = format!("Found {} formats for {}.", formats.len(), state.selected_video_device);
                            state.supported_formats = formats;
                            if let Some(res) = state.supported_formats.first().and_then(|f| f.resolutions.first()) {
                                state.selected_resolution = (res.width, res.height);
                                state.selected_framerate = res.framerates.first().cloned().unwrap_or(0);
                            }
                        }
                        Err(e) => {
                            state.status_message = format!("Failed to scan formats: {}", e);
                        }
                    }
                }
            });
    });

    // --- Format and Resolution Selection ---
    if !state.supported_formats.is_empty() {
        ui.horizontal(|ui| {
            // To avoid borrow checker issues, we get the data we need from `self` first,
            // before building the UI that might mutate `self`.
            let selected_format_description = state.supported_formats[state.selected_format_index].description.clone();
            let resolutions = state.supported_formats[state.selected_format_index].resolutions.clone();

            // --- Format (Codec) Dropdown ---
            ui.label("Format:");
            egui::ComboBox::from_id_source("format_selector")
                .selected_text(selected_format_description)
                .show_ui(ui, |ui| {
                    for (i, format) in state.supported_formats.iter().enumerate() {
                        if ui.selectable_value(&mut state.selected_format_index, i, &format.description).changed() {
                            // When format changes, update selected resolution and framerate
                            if let Some(res) = state.supported_formats[i].resolutions.first() {
                                state.selected_resolution = (res.width, res.height);
                                state.selected_framerate = res.framerates.first().cloned().unwrap_or(0);
                            }
                            save_config(state);
                        }
                    }
                });

            // --- Resolution Dropdown ---
            ui.label("Resolution:");
            egui::ComboBox::from_id_source("resolution_selector")
                .selected_text(format!("{}x{}", state.selected_resolution.0, state.selected_resolution.1))
                .show_ui(ui, |ui| {
                    for res in &resolutions {
                        if ui.selectable_value(&mut state.selected_resolution, (res.width, res.height), format!("{}x{}", res.width, res.height)).changed() {
                            // When resolution changes, update framerate
                            state.selected_framerate = res.framerates.first().cloned().unwrap_or(0);
                            save_config(state);
                        }
                    }
                });

            // --- Framerate Dropdown ---
            if let Some(res_info) = resolutions.iter().find(|r| r.width == state.selected_resolution.0 && r.height == state.selected_resolution.1) {
                if !res_info.framerates.is_empty() {
                    ui.label("Framerate:");
                    egui::ComboBox::from_id_source("framerate_selector")
                        .selected_text(format!("{} fps", state.selected_framerate))
                        .show_ui(ui, |ui| {
                            for &fps in &res_info.framerates {
                                if ui.selectable_value(&mut state.selected_framerate, fps, format!("{} fps", fps)).changed() {
                                    save_config(state);
                                }
                            }
                        });
                    }
            }
        });
    }
    ui.separator();

    // --- 3. PulseAudio Info ---
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label("PulseAudio Configuration:");
            if ui.button("🔄 Refresh").clicked() {
                state.status_message = "Refresh clicked. Please restart the app to re-scan devices.".to_string();
            }
        });

        let selected_source_desc = state.pulse_sources.iter()
            .find(|(_, name)| Some(name) == state.selected_pulse_source_name.as_ref())
            .map(|(desc, _)| desc.as_str())
            .unwrap_or("Select an Input");

        egui::ComboBox::from_label("Input (Source)")
            .selected_text(selected_source_desc)
            .show_ui(ui, |ui| {
                let mut changed = false;
                for (desc, name) in &state.pulse_sources {
                    changed |= ui.selectable_value(&mut state.selected_pulse_source_name, Some(name.clone()), desc).changed();
                }
                if changed { save_config(state); }
            });

        let selected_sink_desc = state.pulse_sinks.iter()
            .find(|(_, name)| Some(name) == state.selected_pulse_sink_name.as_ref())
            .map(|(desc, _)| desc.as_str())
            .unwrap_or("Select an Output");

        egui::ComboBox::from_label("Output (Sink)")
            .selected_text(selected_sink_desc)
            .show_ui(ui, |ui| {
                let mut changed = false;
                for (desc, name) in &state.pulse_sinks {
                    changed |= ui.selectable_value(&mut state.selected_pulse_sink_name, Some(name.clone()), desc).changed();
                }
                if changed { save_config(state); }
            });
    });
    ui.separator();

    // --- 4. Start/Stop Controls ---
    ui.horizontal(|ui| {
        let is_running = state.video_thread.is_some();
        if ui.add_enabled(!is_running && state.selected_resolution.0 > 0, egui::Button::new("▶ Start Stream")).clicked() {
            start_stream(state, ui.ctx()); // No change here, just for context
        }
        if ui.add_enabled(is_running, egui::Button::new("⏹ Stop Stream")).clicked() {
            stop_stream(state, ui.ctx());
        }
    });

    // --- Status Bar ---
    ui.separator();
    ui.label(&state.status_message);
}

impl Drop for AppState {
    fn drop(&mut self) {
        // Ensure the stream is stopped and resources are cleaned up when the app closes.
        // We don't have access to the egui::Context here, so we can't send viewport commands.
        // We'll create a helper function for the non-UI cleanup.
        stop_stream_resources(self);
    }
}

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
            .with_inner_size([640.0, 500.0]) // Start with a window tall enough for the logo and UI.
            .with_min_inner_size([500.0, 200.0])
            .with_icon(egui::IconData {
                rgba: icon.into_raw(),
                width: icon_width,
                height: icon_height,
            }),
        ..Default::default()
    };

    // Create a closure that will be called once to create the App state.
    let creator = |cc: &eframe::CreationContext| {
        // --- Embed a local font for 100% robust character support ---
        let mut fonts = egui::FontDefinitions::default();

        // Load the font data from the file we added to the assets folder.
        fonts.font_data.insert("roboto_slab".to_owned(), egui::FontData::from_static(include_bytes!("../assets/RobotoSlab-Regular.ttf")).tweak(
            egui::FontTweak { scale: 1.05, ..Default::default() },
        ),);
        fonts.font_data.insert("noto_sans_jp".to_owned(), egui::FontData::from_static(include_bytes!("../assets/NotoSansJP-Regular.ttf")));
        fonts.font_data.insert("noto_emoji".to_owned(), egui::FontData::from_static(include_bytes!("../assets/NotoColorEmoji-Regular.ttf")));

        // Create a fallback chain of fonts.
        // 1. Roboto Slab for general text.
        // 2. Noto Sans JP for Japanese characters.
        // 3. Noto Color Emoji for symbols and emoji.
        fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap()
            .extend(vec!["roboto_slab".to_owned(), "noto_sans_jp".to_owned(), "noto_emoji".to_owned()]);

        fonts.families.get_mut(&egui::FontFamily::Monospace).unwrap()
            .extend(vec!["roboto_slab".to_owned(), "noto_sans_jp".to_owned(), "noto_emoji".to_owned()]);

        cc.egui_ctx.set_fonts(fonts);
        let mut app_state = AppState::default();

        // --- Load UI Logo Texture ---
        let logo_image = image::load_from_memory(include_bytes!("../assets/logo.png")).expect("Failed to load logo");
        let logo_size = [logo_image.width() as _, logo_image.height() as _];
        let logo_rgba = logo_image.to_rgba8();
        let logo_pixels = logo_rgba.as_flat_samples();
        let logo_color_image = egui::ColorImage::from_rgba_unmultiplied(logo_size, logo_pixels.as_slice());
        let logo_texture = cc.egui_ctx.load_texture("logo", logo_color_image, Default::default());
        app_state.logo_texture = Some(logo_texture);

        // --- Asynchronous Device Scanning ---
        let (tx, rx) = mpsc::channel();
        app_state.device_scan_receiver = Some(rx);

        let egui_ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || {
            let video_result = find_video_devices();
            let pulse_result = find_pulse_devices();
            let usb_result = find_usb_devices();

            let result = match (video_result, pulse_result, usb_result) {
                (Ok(video_devices), Ok((pulse_sources, pulse_sinks)), Ok(usb_devices)) => {
                    DeviceScanResult::Success(video_devices, pulse_sources, pulse_sinks, usb_devices)
                }
                (Err(e), _, _) => DeviceScanResult::Failure(e.context("Failed to find video devices")),
                (_, Err(e), _) => DeviceScanResult::Failure(e.context("Failed to find PulseAudio devices")),
                (_, _, Err(e)) => DeviceScanResult::Failure(e.context("Failed to find USB devices")),
            };
            let _ = tx.send(result);
            egui_ctx.request_repaint(); // Wake up the GUI thread
        });        
        Box::new(app_state) as Box<dyn eframe::App>
    };

    // eframe::run_native takes control of the main thread and never returns on success.
    // If it does return, it's an error.
    eframe::run_native("Michadame Viewer", options, Box::new(creator))

}
/// Saves the current selections to the configuration file.
fn save_config(state: &AppState) {
    let cfg = MichadameConfig {
        video_device: Some(state.selected_video_device.clone()),
        usb_device: state.selected_usb_device.clone(),
        pulse_source: state.selected_pulse_source_name.clone(),
        pulse_sink: state.selected_pulse_sink_name.clone(),
        video_format_fourcc: state.supported_formats
            .get(state.selected_format_index)
            .map(|f| f.fourcc.clone()),
        video_resolution: if state.selected_resolution.0 > 0 {
            Some(state.selected_resolution)
        } else {
            None
        },
        video_framerate: if state.selected_framerate > 0 { Some(state.selected_framerate) } else { None },
        reset_usb_on_startup: Some(state.reset_usb_on_startup),
    };

    if let Err(e) = confy::store("michadame", None, cfg) {
        tracing::error!("Failed to save configuration: {}", e);
        // We can update the status message, but it might be annoying to the user.
    }
}

/// Action to start the stream
fn start_stream(state: &mut AppState, ctx: &egui::Context) {
    // Load PulseAudio loopback module
    match (&state.selected_pulse_source_name, &state.selected_pulse_sink_name) {
        (Some(mic), Some(sink)) => {
            match load_pulse_loopback(mic, sink) {
                Ok(index) => {
                    state.pulse_loopback_module_index = Some(index);
                    state.status_message = "PulseAudio loopback loaded.".to_string();
                }
                Err(e) => {
                    state.status_message = format!("Failed to load loopback: {}", e);
                    return;
                }
            }
        },
        _ => {
            state.status_message = "Cannot start: Missing PulseAudio devices.".to_string();
            return;
        }
    }

    let format = if let Some(f) = state.supported_formats.get(state.selected_format_index) {
        f
    } else {
        state.status_message = "Cannot start: No video format selected.".to_string();
        return;
    };

    // --- Create the texture handle right before starting the stream ---
    if state.video_texture.is_none() {
        let tex_manager = ctx.tex_manager();
        let tex_id = tex_manager.write().alloc(
            "ffmpeg_video".to_string(), egui::ImageData::Color(egui::ColorImage::new([1, 1], egui::Color32::BLACK).into()), egui::TextureOptions::LINEAR);
        state.video_texture = Some(egui::TextureHandle::new(tex_manager, tex_id));
    }

    let stop_flag = Arc::new(AtomicBool::new(false));
    state.stop_video_thread = Some(stop_flag.clone());

    let device = state.selected_video_device.clone();
    let format = format.clone();
    let resolution = state.selected_resolution;
    let framerate = state.selected_framerate;
    let (tx, rx) = crossbeam_channel::bounded(1); // Bounded channel with capacity of 1.
    state.frame_receiver = Some(rx);

    let handle = thread::spawn(move || {
        if let Err(e) = video_thread_main(tx, stop_flag, device, format, resolution, framerate) {
            tracing::error!("Video thread error: {}", e);
        }
    });

    state.video_thread = Some(handle);
    state.status_message = "Stream started.".to_string();

    // --- Set Minimum Size AND Toggle Fullscreen ---
    // This combination ensures the window is the correct size, cannot be shrunk,
    // and the layout is forcibly recalculated to prevent visual glitches.
    if !state.is_fullscreen {
        // 1. Use a fixed UI height and add it to the video resolution.
        let top_ui_height = 500.0; // A generous fixed height for the UI controls.
        let required_size = egui::vec2(resolution.0 as f32, resolution.1 as f32 + top_ui_height);

        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(required_size));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(required_size));

        // 2. Trigger the fullscreen toggle state machine to force a layout reset.
        state.fullscreen_toggle_state = 1;
    }
}

/// Action to stop the stream's background resources.
fn stop_stream_resources(state: &mut AppState) {
    if let Some(stop_flag) = state.stop_video_thread.take() {
        stop_flag.store(true, Ordering::Relaxed);
    }
    if let Some(handle) = state.video_thread.take() {
        let _ = handle.join();
    }

    if let Some(index) = state.pulse_loopback_module_index.take() {
        if let Err(e) = unload_pulse_loopback(index) {
            state.status_message = format!("Stream stopped, but failed to unload PulseAudio module: {}", e);
        } else {
            state.status_message = "Stream stopped and PulseAudio module unloaded.".to_string();
        }
    } else {
        state.status_message = "Stream stopped.".to_string();
    }

    // Drop the texture and frame receiver to release resources and hide the video area.
    state.video_texture = None;
    state.frame_receiver = None;
}

/// Action to stop the stream, including UI updates. Called from the UI.
fn stop_stream(state: &mut AppState, ctx: &egui::Context) {
    stop_stream_resources(state);

    // Reset the minimum window size to allow it to shrink again.
    ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize([500.0, 200.0].into()));
}

// --- Helper Functions ---

fn reset_usb_device(device_id: &str) -> Result<()> {
    let status = std::process::Command::new("pkexec")
        .arg("usbreset")
        .arg(device_id)
        .status()
        .context("Failed to execute 'pkexec usbreset'. Is pkexec installed?")?;

    if status.success() {
        Ok(())
    } else {
        let msg = format!("'pkexec usbreset' failed with status: {}. Check if 'usbreset' is in your PATH.", status);
        tracing::error!("{}", msg);
        Err(anyhow!(msg))
    }
}

fn find_video_devices() -> Result<Vec<String>> {
    let mut devices = Vec::new();
    for entry in glob::glob("/dev/video*").context("Failed to read glob pattern /dev/video*")? {
        match entry {
            Ok(path) => {
                if let Some(path_str) = path.to_str() {
                    devices.push(path_str.to_string());
                }
            }
            Err(e) => tracing::error!("Glob error: {:?}", e),
        }
    }
    Ok(devices)
}

/// Finds USB devices by parsing the output of `lsusb`.
fn find_usb_devices() -> Result<Vec<(String, String)>> {
    let output = Command::new("lsusb")
        .output()
        .context("Failed to execute 'lsusb'. Is it installed and in your PATH?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("lsusb failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let devices = stdout.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() > 6 && parts[4] == "ID" {
                let id = parts[5].to_string();
                let name = parts[6..].join(" ");
                Some((id, name))
            } else {
                None
            }
        })
        .collect();

    Ok(devices)
}

/// Call `v4l2-ctl` to find all supported formats and their resolutions for a given device.
fn find_video_formats(device_path: &str) -> Result<Vec<VideoFormat>> {
    let output = Command::new("v4l2-ctl")
        .arg("--list-formats-ext")
        .arg("-d")
        .arg(device_path)
        .output()
        .context("Failed to execute 'v4l2-ctl'. Is it installed and in your PATH?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("v4l2-ctl failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut formats = Vec::new();
    let mut current_format: Option<VideoFormat> = None;
    let mut current_resolution: Option<Resolution> = None;

    for line in stdout.lines().filter(|l| !l.is_empty()) {
        let trimmed = line.trim();

        // Check for a new format line, e.g., `[0]: 'YUYV' (YUYV 4:2:2)`
        if trimmed.starts_with('[') && trimmed.contains(':') && trimmed.contains('\'') {
            // Save the previous format if it exists and has resolutions
            if let Some(mut format) = current_format.take() {
                // Also save the last resolution of the previous format
                if let Some(res) = current_resolution.take() {
                    if !res.framerates.is_empty() {
                        format.resolutions.push(res);
                    }
                }
                if !format.resolutions.is_empty() {
                    formats.push(format);
                }
            }

            // Parse the new format line
            let parts: Vec<&str> = trimmed.split('\'').collect();
            if parts.len() >= 2 {
                let fourcc = parts[1].to_string();
                let description = trimmed.split(|c| c == '(' || c == ')').nth(1).unwrap_or("").to_string();
                
                current_format = Some(VideoFormat {
                    fourcc,
                    description,
                    resolutions: Vec::new(),
                });
            }
        } else if trimmed.starts_with("Size: Discrete") { // e.g., `Size: Discrete 1920x1080`
            if let Some(format) = &mut current_format {
                // Save the previous resolution before starting a new one
                if let Some(res) = current_resolution.take() {
                    if !res.framerates.is_empty() {
                        format.resolutions.push(res);
                    }
                }

                let res_parts: Vec<&str> = trimmed.split_whitespace().collect();
                if res_parts.len() >= 3 {
                    let res_str = res_parts[2];
                    let dim_parts: Vec<&str> = res_str.split('x').collect();
                    if dim_parts.len() == 2 {
                        if let (Ok(w), Ok(h)) = (dim_parts[0].parse(), dim_parts[1].parse()) {
                            current_resolution = Some(Resolution { width: w, height: h, framerates: Vec::new() });
                        }
                    }
                }
            }
        } else if trimmed.starts_with("Interval: Discrete") { // e.g., `Interval: Discrete 0.017s (60.000 fps)`
            if let Some(res) = &mut current_resolution {
                if let Some(fps_part) = trimmed.split(|c| c == '(' || c == ')').nth(1) {
                    if let Some(fps_str) = fps_part.split_whitespace().next() {
                        if let Ok(fps_float) = fps_str.parse::<f64>() {
                            res.framerates.push(fps_float.round() as u32);
                        }
                    }
                }
            }
        }
    }

    // Add the last processed format
    if let Some(mut format) = current_format.take() {
        if let Some(res) = current_resolution.take() {
            if !res.framerates.is_empty() {
                format.resolutions.push(res);
            }
        }
        if !format.resolutions.is_empty() {
            formats.push(format);
        }
    }
    Ok(formats)
}

/// A generic function to run a one-shot PulseAudio operation.
/// This uses the simple `standard::Mainloop` and avoids all the deadlock-prone complexity.
fn run_pulse_op<F, T>(op_logic: F) -> Result<T>
where
    F: FnOnce(&mut PulseContext, &mut Mainloop) -> Result<T>,
{
    let mut mainloop = Mainloop::new().context("Failed to create mainloop")?;
    let mut context = PulseContext::new(&mainloop, "pa-client").context("Failed to create context")?;

    context.connect(None, PulseContextFlagSet::empty(), None).context("Failed to connect context")?;

    // Wait for context to be ready
    loop {
        match mainloop.iterate(false) {
            IterateResult::Err(e) => return Err(anyhow!("Mainloop iterate error: {}", e)),
            IterateResult::Quit(_) => return Err(anyhow!("Mainloop quit unexpectedly")),
            _ => {}
        }
        match context.get_state() {
            PulseContextState::Ready => break,
            PulseContextState::Failed | PulseContextState::Terminated => {
                return Err(anyhow!("Context state failed or terminated"));
            }
            _ => {}
        }
    }

    let result = op_logic(&mut context, &mut mainloop);
    context.disconnect();
    result
}

fn find_pulse_devices() -> Result<(Vec<(String, String)>, Vec<(String, String)>)> {
    run_pulse_op(|context, mainloop| {
        use std::cell::RefCell;
        let sources = Rc::new(RefCell::new(Vec::new()));
        let sinks = Rc::new(RefCell::new(Vec::new()));
        let lists_completed = Rc::new(RefCell::new(0));

        { // Scoping block for operations
            let op_source = context.introspect().get_source_info_list({
                let sources = Rc::clone(&sources);
                let lists_completed = Rc::clone(&lists_completed);
                move |res| {
                    if let ListResult::Item(item) = res {
                        // FORCE a lossy UTF-8 conversion to handle any potential encoding issues from the C library.
                        // This is the key fix for the encoding problem.
                        if let (Some(name_cstr), Some(desc_cstr)) = (item.name.as_ref(), item.description.as_ref()) {
                            let name = String::from_utf8_lossy(name_cstr.as_bytes()).to_string();
                            let desc = String::from_utf8_lossy(desc_cstr.as_bytes()).to_string();
                            tracing::info!(source_name = %name, source_desc = %desc, "Found PulseAudio Source");
                            sources.borrow_mut().push((desc, name));
                        }
                    } else {
                        *lists_completed.borrow_mut() += 1;
                    }
                }
            });

            let op_sink = context.introspect().get_sink_info_list({
                let sinks = Rc::clone(&sinks);
                let lists_completed = Rc::clone(&lists_completed);
                move |res| {
                    if let ListResult::Item(item) = res {
                        // FORCE a lossy UTF-8 conversion here as well.
                        if let (Some(name_cstr), Some(desc_cstr)) = (item.name.as_ref(), item.description.as_ref()) {
                            let name = String::from_utf8_lossy(name_cstr.as_bytes()).to_string();
                            let desc = String::from_utf8_lossy(desc_cstr.as_bytes()).to_string();
                            tracing::info!(sink_name = %name, sink_desc = %desc, "Found PulseAudio Sink");
                            sinks.borrow_mut().push((desc, name));
                        }
                    } else {
                        *lists_completed.borrow_mut() += 1;
                    }
                }
            });

            // Wait for callbacks to complete
            while *lists_completed.borrow() < 2 {
                if mainloop.iterate(false) == IterateResult::Quit(Retval(0)) {
                     return Err(anyhow!("Mainloop quit while getting devices"));
                }
            }
            drop(op_source);
            drop(op_sink);
        }

        let final_sources = sources.borrow().clone();
        let final_sinks = sinks.borrow().clone();
        Ok((final_sources, final_sinks))
    })
}

fn load_pulse_loopback(source: &String, sink: &String) -> Result<u32> {
    let args = format!(r#"source="{}" sink="{}""#, source, sink);
    use std::cell::RefCell;
    run_pulse_op(|context, mainloop| {
        let index = Rc::new(RefCell::new(None));
        {
            let op = context.introspect().load_module("module-loopback", &args, {
                let index_clone = Rc::clone(&index);
                move |idx| {
                    *index_clone.borrow_mut() = Some(idx);
                }
            });

            while op.get_state() == OperationState::Running {
                if mainloop.iterate(false) == IterateResult::Quit(Retval(0)) {
                    return Err(anyhow!("Mainloop quit while loading module"));
                }
            }
        }
        // Explicitly scope the borrow to avoid the temporary living too long.
        let result = index.borrow_mut().take().context("Failed to get module index");
        result
    })
}

fn unload_pulse_loopback(module_index: u32) -> Result<()> {
    run_pulse_op(|context, mainloop| {
        let op = context.introspect().unload_module(module_index, |_| {});
        while op.get_state() == OperationState::Running {
            if mainloop.iterate(false) == IterateResult::Quit(Retval(0)) {
                return Err(anyhow!("Mainloop quit while unloading module"));
            }
        }
        Ok(())
    })
}

/// The main logic for the background video processing thread.
fn video_thread_main(
    frame_sender: crossbeam_channel::Sender<Arc<egui::ColorImage>>,
    stop_flag: Arc<AtomicBool>,
    device: String,
    format: VideoFormat,
    resolution: (u32, u32),
    framerate: u32,
) -> Result<()> {
    use ffmpeg_next::format::Pixel;

    ffmpeg_next::init().context("Failed to initialize FFmpeg")?;
    
    let mut pixel_format_str = format.fourcc.trim_end_matches('\0').to_lowercase();
    match pixel_format_str.as_str() {
        "yuyv" => pixel_format_str = "yuyv422".to_string(),
        "mjpg" => pixel_format_str = "mjpeg".to_string(),
        _ => {}
    }
    
    let mut ffmpeg_options = ffmpeg_next::Dictionary::new();
    ffmpeg_options.set("video_size", &format!("{}x{}", resolution.0, resolution.1));
    ffmpeg_options.set("framerate", &framerate.to_string());
    ffmpeg_options.set("f", "v4l2");
    // Add options to handle low-latency live stream, replicating the working mpv command.
    ffmpeg_options.set("fflags", "nobuffer+discardcorrupt");
    ffmpeg_options.set("probesize", "32");
    ffmpeg_options.set("analyzeduration", "100000"); // Use a safe, small value.

    // Set the input format based on the codec.
    // For MJPEG, we let ffmpeg decode it. For others, we treat it as raw video and specify the pixel format.
    if pixel_format_str == "mjpeg" {
        ffmpeg_options.set("input_format", "mjpeg");
    } else {
        ffmpeg_options.set("input_format", "rawvideo");
        ffmpeg_options.set("pixel_format", &pixel_format_str);
    }

    tracing::info!(device = %device, options = ?ffmpeg_options, "Starting FFmpeg with options");

    let ictx = ffmpeg_next::format::input_with_dictionary(&device, ffmpeg_options)
        .context("Failed to open input device with ffmpeg")?;

    let input = ictx.streams().best(ffmpeg_next::media::Type::Video).context("Could not find best video stream")?;
    let video_stream_index = input.index();

    // --- Hardware Acceleration Setup ---
    let mut decoder = ffmpeg_next::codec::context::Context::from_parameters(input.parameters())
        .and_then(|c| c.decoder().video())
        .context("Failed to create software video decoder")?;

    decoder.set_threading(ffmpeg_next::codec::threading::Config::default());
    let (packet_tx, packet_rx) = crossbeam_channel::bounded(1);
    let reader_stop_flag = stop_flag.clone();
    let _reader_thread = thread::spawn(move || {
        let mut ictx = ictx; // Take ownership of the context
        for (stream, packet) in ictx.packets() {
            if reader_stop_flag.load(Ordering::Relaxed) { break; }
            if stream.index() == video_stream_index {
                // This is a non-blocking send. If the decoder is busy, the old packet
                // in the channel is dropped and replaced with this new one.
                let _ = packet_tx.try_send(packet);
            }
        }
        tracing::info!("Packet reader thread finished.");
    });

    // --- Active Decoding Loop ---
    let mut scaler = None;
    while !stop_flag.load(Ordering::Relaxed) {
        // Use a non-blocking receive to get the latest packet from the reader thread.
        if let Ok(packet) = packet_rx.try_recv() {
            decoder.send_packet(&packet).context("Failed to send packet to decoder")?;
            let mut decoded = ffmpeg_next::frame::Video::empty();
            while decoder.receive_frame(&mut decoded).is_ok() {
                let frame_to_process = &decoded;

                let scaler = scaler.get_or_insert_with(|| {
                    ffmpeg_next::software::scaling::context::Context::get(
                        frame_to_process.format(), 
                        frame_to_process.width(), 
                        frame_to_process.height(),
                        Pixel::RGB24, decoded.width(), decoded.height(),
                        ffmpeg_next::software::scaling::flag::Flags::FAST_BILINEAR,
                    ).unwrap()
                });
                let mut rgb_frame = ffmpeg_next::frame::Video::empty();
                scaler.run(frame_to_process, &mut rgb_frame).context("Scaler failed")?;
                
                let image_data = rgb_frame.data(0);
                let image = Arc::new(egui::ColorImage::from_rgb([rgb_frame.width() as usize, rgb_frame.height() as usize], image_data));

                if let Err(e) = frame_sender.try_send(image) {
                    tracing::warn!("Failed to send frame, receiver disconnected: {}", e);
                    break;
                }
            }
        } else {
            thread::yield_now();
        }
    }
    tracing::info!("Video thread finished.");
    Ok(())
}
