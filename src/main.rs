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
use std::sync::mpsc;

/// Holds the results of our background device scan.
enum DeviceScanResult {
    Success(Vec<String>, Vec<(String, String)>, Vec<(String, String)>),
    Failure(anyhow::Error),
}
const USB_VENDOR: &str = "345f";
const USB_ID: &str = "2131";

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
                egui::ComboBox::from_id_source("video_device_selector")
                    .selected_text(self.selected_video_device.as_str())
                    .show_ui(ui, |ui| {
                        for device in &self.video_devices {
                            ui.selectable_value(&mut self.selected_video_device, device.clone(), device.as_str());
                        }
                    });
            });
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
                        for (desc, name) in &self.pulse_sources {
                            ui.selectable_value(&mut self.selected_pulse_source_name, Some(name.clone()), desc);
                        }
                    });

                let selected_sink_desc = self.pulse_sinks.iter()
                    .find(|(_, name)| Some(name) == self.selected_pulse_sink_name.as_ref())
                    .map(|(desc, _)| desc.as_str())
                    .unwrap_or("Select an Output");

                egui::ComboBox::from_label("Output (Sink)")
                    .selected_text(selected_sink_desc)
                    .show_ui(ui, |ui| {
                        for (desc, name) in &self.pulse_sinks {
                            ui.selectable_value(&mut self.selected_pulse_sink_name, Some(name.clone()), desc);
                        }
                    });
            });
            ui.separator();

            // --- 4. Start/Stop Controls ---
            ui.horizontal(|ui| {
                let is_running = self.gamescope_process.is_some();
                if ui.add_enabled(!is_running, egui::Button::new("▶ Start Stream")).clicked() {
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
    match launch_gamescope(&state.selected_video_device) {
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

fn launch_gamescope(device: &str) -> Result<Child> {
    let av_url = format!("av://v4l2:{}", device);

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
            "--opengl-swapinterval=0", "--gpu-api=opengl", "--vo=gpu-next", "--speed=1.01",
            "--demuxer-lavf-o=video_size=1920x1080,framerate=60,input_format=rawvideo,pixel_format=yuyv422,fflags=+nobuffer,analyzeduration=100000,probesize=32",
            "--demuxer-lavf-probe-info=nostreams", "--fullscreen",
        ])
        .arg(&av_url)
        .spawn()
        .context("Failed to spawn gamescope process")
}
