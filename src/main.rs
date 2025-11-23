// src/main.rs

use anyhow::{anyhow, Context, Result};
use eframe::egui;
use libpulse_binding::callbacks::ListResult;
use libpulse_binding::def::Retval;
use libpulse_binding::context::{Context as PulseContext, FlagSet as PulseContextFlagSet, State as PulseContextState};
use libpulse_binding::mainloop::standard::{IterateResult, Mainloop};
use libpulse_binding::operation::State as OperationState;
use std::process::{Child, Command};
use std::rc::Rc;
use serde::{Serialize, Deserialize};
use std::sync::mpsc;

/// Holds the results of our background device scan.
enum DeviceScanResult {
    Success(Vec<String>, Vec<(String, String)>, Vec<(String, String)>),
    Failure(anyhow::Error),
}
const USB_VENDOR: &str = "345f";
const USB_ID: &str = "2131";

/// Defines the structure of our configuration file.
#[derive(Default, Serialize, Deserialize, Clone)]
struct MichadameConfig {
    video_device: Option<String>,
    pulse_source: Option<String>,
    pulse_sink: Option<String>,
    video_format_fourcc: Option<String>,
    video_resolution: Option<(u32, u32)>,
    video_framerate: Option<u32>,
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
    selected_video_device: String,
    // (Description, Name)
    pulse_sources: Vec<(String, String)>,
    pulse_sinks: Vec<(String, String)>,
    selected_pulse_source_name: Option<String>,
    selected_pulse_sink_name: Option<String>,
    pulse_loopback_module_index: Option<u32>,
    gamescope_process: Option<Child>,
    status_message: String,
    // Video format state
    supported_formats: Vec<VideoFormat>,
    selected_format_index: usize,
    selected_resolution: (u32, u32),
    selected_framerate: u32,
    // Receiver for the background device scan
    device_scan_receiver: Option<mpsc::Receiver<DeviceScanResult>>,
}
impl Default for AppState {
    fn default() -> Self {
        Self {
            video_devices: Vec::new(),
            selected_video_device: String::new(),
            pulse_sources: Vec::new(),
            pulse_sinks: Vec::new(),
            selected_pulse_source_name: None,
            selected_pulse_sink_name: None,
            pulse_loopback_module_index: None,
            gamescope_process: None,
            status_message: "Loading devices...".to_string(),
            supported_formats: Vec::new(),
            selected_format_index: 0,
            selected_resolution: (0, 0),
            selected_framerate: 0,
            device_scan_receiver: None,
        }
    }
}

/// Implement the eframe::App trait for our state.
/// This gives us more control than run_simple_native.
impl eframe::App for AppState {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Capture Control");
            ui.separator();

            // --- Check for device scan results ---
            if let Some(rx) = &self.device_scan_receiver {
                if let Ok(scan_result) = rx.try_recv() {
                    match scan_result {
                        DeviceScanResult::Success(video_devices, pulse_sources, pulse_sinks) => {
                            self.video_devices = video_devices;
                            self.selected_video_device = self.video_devices.first().cloned().unwrap_or_default();
                            self.pulse_sources = pulse_sources;
                            self.pulse_sinks = pulse_sinks;
                            self.status_message = "Devices loaded successfully.".to_string();

                            // --- Load and apply saved configuration ---
                            if let Ok(cfg) = confy::load::<MichadameConfig>("michadame", None) {
                                // Apply video device if it still exists
                                if let Some(saved_device) = cfg.video_device {
                                    if self.video_devices.contains(&saved_device) {
                                        self.selected_video_device = saved_device;
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

            // --- 1. USB Reset ---
            ui.horizontal(|ui| {
                ui.label("USB Device:");
                ui.monospace(format!("{}:{}", USB_VENDOR, USB_ID));
                if ui.button("Reset USB Device").clicked() {
                    self.status_message = match reset_usb_device() {
                        Ok(_) => "USB device reset successfully.".to_string(),
                        Err(e) => format!("Failed to reset USB: {}", e),
                    };
                }
            });
            ui.separator();

            // --- 2. Video Device Selection ---
            ui.horizontal(|ui| {
                ui.label("Video Device:");
                let _combo_box = egui::ComboBox::from_id_source("video_device_selector")
                    .selected_text(self.selected_video_device.as_str())
                    .show_ui(ui, |ui| {
                        let mut changed = false;
                        for device in &self.video_devices {
                            changed |= ui.selectable_value(&mut self.selected_video_device, device.clone(), device.as_str()).changed();
                        }
                        if changed && !self.selected_video_device.is_empty() {
                            save_config(self);
                            // Clear old format info when device changes
                            self.supported_formats.clear();
                            self.selected_format_index = 0;
                            self.selected_resolution = (0, 0);

                            // Automatically scan formats for the new device
                            match find_video_formats(&self.selected_video_device) {
                                Ok(formats) => {
                                    self.status_message = format!("Found {} formats for {}.", formats.len(), self.selected_video_device);
                                    self.supported_formats = formats;
                                    if let Some(res) = self.supported_formats.first().and_then(|f| f.resolutions.first()) {
                                        self.selected_resolution = (res.width, res.height);
                                        self.selected_framerate = res.framerates.first().cloned().unwrap_or(0);
                                    }
                                }
                                Err(e) => {
                                    self.status_message = format!("Failed to scan formats: {}", e);
                                }
                            }
                        }
                    });
            });

            // --- Format and Resolution Selection ---
            if !self.supported_formats.is_empty() {
                ui.horizontal(|ui| {
                    // To avoid borrow checker issues, we get the data we need from `self` first,
                    // before building the UI that might mutate `self`.
                    let selected_format_description = self.supported_formats[self.selected_format_index].description.clone();
                    let resolutions = self.supported_formats[self.selected_format_index].resolutions.clone();

                    // --- Format (Codec) Dropdown ---
                    ui.label("Format:");
                    egui::ComboBox::from_id_source("format_selector")
                        .selected_text(selected_format_description)
                        .show_ui(ui, |ui| {
                            for (i, format) in self.supported_formats.iter().enumerate() {
                                if ui.selectable_value(&mut self.selected_format_index, i, &format.description).changed() {
                                    // When format changes, update selected resolution and framerate
                                    if let Some(res) = self.supported_formats[i].resolutions.first() {
                                        self.selected_resolution = (res.width, res.height);
                                        self.selected_framerate = res.framerates.first().cloned().unwrap_or(0);
                                    }
                                    save_config(self);
                                }
                            }
                        });

                    // --- Resolution Dropdown ---
                    ui.label("Resolution:");
                    egui::ComboBox::from_id_source("resolution_selector")
                        .selected_text(format!("{}x{}", self.selected_resolution.0, self.selected_resolution.1))
                        .show_ui(ui, |ui| {
                            for res in &resolutions {
                                if ui.selectable_value(&mut self.selected_resolution, (res.width, res.height), format!("{}x{}", res.width, res.height)).changed() {
                                    // When resolution changes, update framerate
                                    self.selected_framerate = res.framerates.first().cloned().unwrap_or(0);
                                    save_config(self);
                                }
                            }
                        });

                    // --- Framerate Dropdown ---
                    if let Some(res_info) = resolutions.iter().find(|r| r.width == self.selected_resolution.0 && r.height == self.selected_resolution.1) {
                        if !res_info.framerates.is_empty() {
                            ui.label("Framerate:");
                            egui::ComboBox::from_id_source("framerate_selector")
                                .selected_text(format!("{} fps", self.selected_framerate))
                                .show_ui(ui, |ui| {
                                    for &fps in &res_info.framerates {
                                        if ui.selectable_value(&mut self.selected_framerate, fps, format!("{} fps", fps)).changed() {
                                            save_config(self);
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
                        self.status_message = "Refresh clicked. Please restart the app to re-scan devices.".to_string();
                    }
                });

                let selected_source_desc = self.pulse_sources.iter()
                    .find(|(_, name)| Some(name) == self.selected_pulse_source_name.as_ref())
                    .map(|(desc, _)| desc.as_str())
                    .unwrap_or("Select an Input");

                egui::ComboBox::from_label("Input (Source)")
                    .selected_text(selected_source_desc)
                    .show_ui(ui, |ui| {
                        let mut changed = false;
                        for (desc, name) in &self.pulse_sources {
                            changed |= ui.selectable_value(&mut self.selected_pulse_source_name, Some(name.clone()), desc).changed();
                        }
                        if changed {
                            save_config(self);
                        }
                    });

                let selected_sink_desc = self.pulse_sinks.iter()
                    .find(|(_, name)| Some(name) == self.selected_pulse_sink_name.as_ref())
                    .map(|(desc, _)| desc.as_str())
                    .unwrap_or("Select an Output");

                egui::ComboBox::from_label("Output (Sink)")
                    .selected_text(selected_sink_desc)
                    .show_ui(ui, |ui| {
                        let mut changed = false;
                        for (desc, name) in &self.pulse_sinks {
                            changed |= ui.selectable_value(&mut self.selected_pulse_sink_name, Some(name.clone()), desc).changed();
                        }
                        if changed {
                            save_config(self);
                        }
                    });
            });
            ui.separator();

            // --- 4. Start/Stop Controls ---
            ui.horizontal(|ui| {
                let is_running = self.gamescope_process.is_some();
                if ui.add_enabled(!is_running && self.selected_resolution.0 > 0, egui::Button::new("▶ Start Stream")).clicked() {
                    start_stream(self);
                }
                if ui.add_enabled(is_running, egui::Button::new("⏹ Stop Stream")).clicked() {
                    stop_stream(self);
                }
            });
            
            // --- Status Bar ---
            ui.separator();
            ui.label(&self.status_message);

            // Poll the process to see if it has exited
            if let Some(process) = &mut self.gamescope_process {
                if let Ok(Some(_)) = process.try_wait() {
                    stop_stream(self);
                    self.status_message = "Stream process finished.".to_string();
                }
            }
        });
    }
}

fn main() -> Result<(), eframe::Error> {
    // Setup logging
    tracing_subscriber::fmt::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([640.0, 480.0]),
        ..Default::default()
    };

    // Create a closure that will be called once to create the App state.
    let creator = |cc: &eframe::CreationContext| {
        // --- Embed a local font for 100% robust character support ---
        let mut fonts = egui::FontDefinitions::default();

        // Load the font data from the file we added to the assets folder.
        fonts.font_data.insert("roboto_slab".to_owned(), egui::FontData::from_static(include_bytes!("../assets/RobotoSlab-Regular.ttf")).tweak(
            egui::FontTweak { scale: 1.05, ..Default::default() }
        ));
        fonts.font_data.insert("noto_sans_jp".to_owned(), egui::FontData::from_static(include_bytes!("../assets/NotoSansJP-Regular.ttf")));
        fonts.font_data.insert("noto_emoji".to_owned(), egui::FontData::from_static(include_bytes!("../assets/NotoColorEmoji-Regular.ttf")));

        // Create a fallback chain of fonts.
        // 1. Roboto Slab for general text.
        // 2. Noto Sans JP for Japanese characters.
        // 3. Noto Color Emoji for symbols and emoji.
        fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "roboto_slab".to_owned());
        fonts.families.entry(egui::FontFamily::Proportional).or_default().push("noto_sans_jp".to_owned());
        fonts.families.entry(egui::FontFamily::Proportional).or_default().push("noto_emoji".to_owned());

        fonts.families.entry(egui::FontFamily::Monospace).or_default().insert(0, "roboto_slab".to_owned());
        fonts.families.entry(egui::FontFamily::Monospace).or_default().push("noto_sans_jp".to_owned());
        fonts.families.entry(egui::FontFamily::Monospace).or_default().push("noto_emoji".to_owned());

        cc.egui_ctx.set_fonts(fonts);
        let mut app_state = AppState::default();

        // --- Asynchronous Device Scanning ---
        let (tx, rx) = mpsc::channel();
        app_state.device_scan_receiver = Some(rx);

        let ctx_clone = cc.egui_ctx.clone();
        std::thread::spawn(move || {
        let video_result = find_video_devices();
        let pulse_result = find_pulse_devices();

        let result = match (video_result, pulse_result) {
            (Ok(video_devices), Ok((pulse_sources, pulse_sinks))) => {
                DeviceScanResult::Success(video_devices, pulse_sources, pulse_sinks)
            }
            (Err(e), _) => DeviceScanResult::Failure(e.context("Failed to find video devices")),
            (_, Err(e)) => DeviceScanResult::Failure(e.context("Failed to find PulseAudio devices")),
        };
        let _ = tx.send(result);
            ctx_clone.request_repaint(); // Wake up the GUI thread
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
    };

    if let Err(e) = confy::store("michadame", None, cfg) {
        tracing::error!("Failed to save configuration: {}", e);
        // We can update the status message, but it might be annoying to the user.
    }
}

/// Action to start the stream
fn start_stream(state: &mut AppState) {
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

    // Launch gamescope + mpv
    let format = if let Some(f) = state.supported_formats.get(state.selected_format_index) {
        f
    } else {
        state.status_message = "Cannot start: No video format selected.".to_string();
        return;
    };

    match launch_gamescope(&state.selected_video_device, format, state.selected_resolution, state.selected_framerate) {
        Ok(child) => {
            state.gamescope_process = Some(child);
            state.status_message = "Gamescope process started.".to_string();
        }
        Err(e) => {
            state.status_message = format!("Failed to start gamescope: {}", e);
            if let Some(index) = state.pulse_loopback_module_index.take() {
                if let Err(e) = unload_pulse_loopback(index) {
                     tracing::error!("Failed to unload loopback module after launch failure: {}", e);
                }
            }
        }
    }
}

/// Action to stop the stream
fn stop_stream(state: &mut AppState) {
    if let Some(mut child) = state.gamescope_process.take() {
        if let Err(e) = child.kill() {
            tracing::error!("Failed to kill gamescope process: {}", e);
        }
        let _ = child.wait();
        state.status_message = "Stream stopped.".to_string();
    }

    if let Some(index) = state.pulse_loopback_module_index.take() {
        if let Err(e) = unload_pulse_loopback(index) {
            state.status_message = format!("Stream stopped, but failed to unload PulseAudio module: {}", e);
        } else {
            state.status_message = "Stream stopped and PulseAudio module unloaded.".to_string();
        }
    }
}

// --- Helper Functions ---

fn reset_usb_device() -> Result<()> {
    let device_id = format!("{}:{}", USB_VENDOR, USB_ID);
    let status = Command::new("pkexec")
        .arg("usbreset")
        .arg(&device_id)
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

fn launch_gamescope(device: &str, format: &VideoFormat, resolution: (u32, u32), framerate: u32) -> Result<Child> {
    let av_url = format!("av://v4l2:{}", device);

    // The FourCC code needs to be converted to a lowercase string for mpv.
    // It's a 4-byte code, often with non-printable characters, so we handle it carefully.
    let mut pixel_format_str = format.fourcc.trim_end_matches('\0').to_lowercase();

    // Map common v4l2 FourCC codes to their ffmpeg equivalents.
    match pixel_format_str.as_str() {
        "yuyv" => pixel_format_str = "yuyv422".to_string(),
        "mjpg" => pixel_format_str = "mjpeg".to_string(),
        _ => {} // Use the lowercase FourCC directly for other formats
    }

    let demuxer_opts = format!(
        "video_size={}x{},framerate={},input_format=rawvideo,pixel_format={},fflags=+nobuffer,analyzeduration=100000,probesize=32",
        resolution.0,
        resolution.1,
        framerate,
        pixel_format_str
    );


    Command::new("gamescope")
        .args([
            "-h", "2160", "-w", "3840", "-H", "2160", "-W", "3840", "-r", "144", "-f", "--rt", "--",
        ])
        .arg("mpv")
        .args([
            "--hwdec=auto", "--framedrop=decoder+vo",
            "--no-hidpi-window-scale", "--video-sync=desync",
            "--vd-lavc-threads=1", "--vd-lavc-dr=no", "--speed=1", "--hr-seek=no",
            "--demuxer-readahead-secs=0", "--demuxer-max-bytes=512K", "--audio-buffer=0",
            "--cache-pause=no", "--no-correct-pts", "--no-cache", "--untimed",
            "--no-demuxer-thread", "--container-fps-override=60", "--opengl-glfinish=yes",
            "--opengl-swapinterval=0", "--gpu-api=opengl", "--vo=gpu-next", "--speed=1.01"
        ])
        .arg(format!("--demuxer-lavf-o={}", demuxer_opts))
        .args([
            "--demuxer-lavf-probe-info=nostreams", "--fullscreen",
        ])
        .arg(&av_url)
        .spawn()
        .context("Failed to spawn gamescope process")
}
