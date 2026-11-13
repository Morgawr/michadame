use anyhow::{Context, Result};
use std::ffi::{c_char, c_int, c_void, CString};
use std::os::raw::c_uint;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[repr(C)]
pub struct Ei {
    _unused: [u8; 0],
}
#[repr(C)]
pub struct EiSeat {
    _unused: [u8; 0],
}
#[repr(C)]
pub struct EiDevice {
    _unused: [u8; 0],
}
#[repr(C)]
pub struct EiEvent {
    _unused: [u8; 0],
}

pub const EI_EVENT_CONNECT: c_uint = 1;
pub const EI_EVENT_DISCONNECT: c_uint = 2;
pub const EI_EVENT_SEAT_ADDED: c_uint = 3;
pub const EI_EVENT_SEAT_REMOVED: c_uint = 4;
pub const EI_EVENT_DEVICE_ADDED: c_uint = 5;
pub const EI_EVENT_DEVICE_REMOVED: c_uint = 6;
pub const EI_EVENT_DEVICE_PAUSED: c_uint = 7;
pub const EI_EVENT_DEVICE_RESUMED: c_uint = 8;

pub const EI_DEVICE_CAP_POINTER: c_uint = 1 << 0; // 1
pub const EI_DEVICE_CAP_POINTER_ABSOLUTE: c_uint = 1 << 1; // 2
pub const EI_DEVICE_CAP_KEYBOARD: c_uint = 1 << 2; // 4
pub const EI_DEVICE_CAP_TOUCH: c_uint = 1 << 3; // 8
pub const EI_DEVICE_CAP_SCROLL: c_uint = 1 << 4; // 16
pub const EI_DEVICE_CAP_BUTTON: c_uint = 1 << 5; // 32
pub const EI_DEVICE_CAP_TEXT: c_uint = 1 << 6; // 64

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

#[allow(improper_ctypes)]
#[link(name = "ei")]
extern "C" {
    fn ei_new_sender(user_data: *mut c_void) -> *mut Ei;
    fn ei_unref(ei: *mut Ei) -> *mut Ei;
    fn ei_setup_backend_socket(ei: *mut Ei, socket_path: *const c_char) -> c_int;
    fn ei_dispatch(ei: *mut Ei);
    fn ei_get_event(ei: *mut Ei) -> *mut EiEvent;
    fn ei_event_unref(event: *mut EiEvent) -> *mut EiEvent;
    fn ei_event_get_type(event: *mut EiEvent) -> c_uint;
    fn ei_event_get_seat(event: *mut EiEvent) -> *mut EiSeat;
    fn ei_event_get_device(event: *mut EiEvent) -> *mut EiDevice;
    fn ei_seat_bind_capabilities(seat: *mut EiSeat, ...) -> c_int;
    fn ei_device_ref(device: *mut EiDevice) -> *mut EiDevice;
    fn ei_device_unref(device: *mut EiDevice) -> *mut EiDevice;
    fn ei_device_has_capability(device: *mut EiDevice, cap: c_uint) -> bool;
    fn ei_device_start_emulating(device: *mut EiDevice, sequence: u32);
    fn ei_device_stop_emulating(device: *mut EiDevice);
    fn ei_device_pointer_motion_absolute(device: *mut EiDevice, x: f64, y: f64);
    fn ei_device_pointer_motion(device: *mut EiDevice, x: f64, y: f64);
    fn ei_device_button_button(device: *mut EiDevice, button: u32, is_press: bool);
    fn ei_device_scroll_delta(device: *mut EiDevice, x: f64, y: f64);
    fn ei_device_keyboard_key(device: *mut EiDevice, keycode: u32, is_press: bool);
    fn ei_device_frame(device: *mut EiDevice, time: u64);
    fn ei_now(ei: *mut Ei) -> u64;
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputCmd {
    PointerMotionAbsolute(f64, f64),
    PointerMotion(f64, f64),
    Button(u32, bool),
    Scroll(f64, f64),
    Key(u32, bool),
}

struct EiWorkerSession {
    ei: *mut Ei,
    devices: Vec<*mut EiDevice>,
    is_emulating: bool,
    disconnected: Arc<AtomicBool>,
}

unsafe impl Send for EiWorkerSession {}

impl Drop for EiWorkerSession {
    fn drop(&mut self) {
        unsafe {
            for dev in self.devices.drain(..) {
                if !dev.is_null() {
                    if self.is_emulating {
                        ei_device_stop_emulating(dev);
                    }
                    ei_device_unref(dev);
                }
            }
            if !self.ei.is_null() {
                ei_unref(self.ei);
                self.ei = std::ptr::null_mut();
            }
        }
    }
}

pub struct GamescopeInputClient {
    cmd_tx: Option<crossbeam_channel::Sender<InputCmd>>,
    disconnected: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for GamescopeInputClient {
    fn drop(&mut self) {
        drop(self.cmd_tx.take());
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

pub fn find_gamescope_ei_socket() -> PathBuf {
    let runtime_dir =
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".to_string());

    // 1. If explicit GAMESCOPE_WAYLAND_DISPLAY is specified in environment
    if let Ok(display) = std::env::var("GAMESCOPE_WAYLAND_DISPLAY") {
        let p = PathBuf::from(&runtime_dir).join(format!("{display}-ei"));
        if p.exists() {
            return p;
        }
    }

    // 2. Search for active gamescope-*-ei sockets (e.g. gamescope-0-ei, gamescope-1-ei)
    if let Ok(entries) = std::fs::read_dir(&runtime_dir) {
        let mut candidates = Vec::new();
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            if name.starts_with("gamescope-") && name.ends_with("-ei") {
                candidates.push(entry.path());
            }
        }
        // Reverse sort so highest numbered / newest socket is probed first
        candidates.sort_by(|a, b| b.cmp(a));
        for path in candidates {
            if UnixStream::connect(&path).is_ok() {
                return path;
            }
        }
    }

    // 3. Fallback default
    PathBuf::from(runtime_dir).join("gamescope-0-ei")
}

/// Drains all currently queued `InputCmd`s following `first`, emitting the entire batch before
/// invoking `dispatch_once` a single time for the batch.
fn process_cmd_batch<E, D>(
    first: InputCmd,
    cmd_rx: &crossbeam_channel::Receiver<InputCmd>,
    mut emit: E,
    mut dispatch_once: D,
) where
    E: FnMut(InputCmd),
    D: FnMut(),
{
    emit(first);
    while let Ok(cmd) = cmd_rx.try_recv() {
        emit(cmd);
    }
    dispatch_once();
}

impl EiWorkerSession {
    unsafe fn device_for_cap(&self, cap: c_uint) -> *mut EiDevice {
        for &dev in &self.devices {
            if !dev.is_null() && ei_device_has_capability(dev, cap) {
                return dev;
            }
        }
        self.devices
            .first()
            .copied()
            .unwrap_or(std::ptr::null_mut())
    }

    fn dispatch_once(&mut self) {
        unsafe {
            if self.ei.is_null() {
                return;
            }
            ei_dispatch(self.ei);

            while let Some(event) = {
                let ev = ei_get_event(self.ei);
                if ev.is_null() {
                    None
                } else {
                    Some(ev)
                }
            } {
                let ev_type = ei_event_get_type(event);
                match ev_type {
                    EI_EVENT_CONNECT => {
                        tracing::debug!("EI client connected event received");
                    }
                    EI_EVENT_DISCONNECT => {
                        for d in self.devices.drain(..) {
                            if !d.is_null() {
                                ei_device_unref(d);
                            }
                        }
                        self.is_emulating = false;
                        self.disconnected.store(true, Ordering::Release);
                        tracing::info!("Gamescope EIS input client disconnected");
                    }
                    EI_EVENT_SEAT_ADDED => {
                        let seat = ei_event_get_seat(event);
                        if !seat.is_null() {
                            tracing::debug!("Binding capabilities to EI seat");
                            ei_seat_bind_capabilities(
                                seat,
                                EI_DEVICE_CAP_POINTER,
                                EI_DEVICE_CAP_POINTER_ABSOLUTE,
                                EI_DEVICE_CAP_KEYBOARD,
                                EI_DEVICE_CAP_BUTTON,
                                EI_DEVICE_CAP_SCROLL,
                                0 as c_uint,
                            );
                        }
                    }
                    EI_EVENT_DEVICE_ADDED => {
                        let dev = ei_event_get_device(event);
                        if !dev.is_null() && !self.devices.contains(&dev) {
                            self.devices.push(ei_device_ref(dev));
                            tracing::debug!("EI device acquired (total: {})", self.devices.len());
                        }
                    }
                    EI_EVENT_DEVICE_RESUMED => {
                        let dev = ei_event_get_device(event);
                        if !dev.is_null() {
                            ei_device_start_emulating(dev, 0);
                            self.is_emulating = true;
                            tracing::debug!("EI device resumed and started emulating");
                        } else {
                            for &d in &self.devices {
                                if !d.is_null() {
                                    ei_device_start_emulating(d, 0);
                                }
                            }
                            if !self.devices.is_empty() {
                                self.is_emulating = true;
                            }
                        }
                    }
                    EI_EVENT_DEVICE_PAUSED => {
                        self.is_emulating = false;
                        tracing::debug!("EI device paused");
                    }
                    EI_EVENT_DEVICE_REMOVED => {
                        let dev = ei_event_get_device(event);
                        if !dev.is_null() {
                            if let Some(pos) = self.devices.iter().position(|&d| d == dev) {
                                let removed = self.devices.remove(pos);
                                ei_device_unref(removed);
                            }
                        } else {
                            for d in self.devices.drain(..) {
                                if !d.is_null() {
                                    ei_device_unref(d);
                                }
                            }
                        }
                        if self.devices.is_empty() {
                            self.is_emulating = false;
                        }
                        tracing::debug!("EI device removed (remaining: {})", self.devices.len());
                    }
                    _ => {}
                }
                ei_event_unref(event);
            }
        }
    }

    fn emit_cmd(&mut self, cmd: InputCmd) {
        if !self.is_emulating || self.ei.is_null() {
            return;
        }
        unsafe {
            match cmd {
                InputCmd::PointerMotionAbsolute(x, y) => {
                    let dev = self.device_for_cap(EI_DEVICE_CAP_POINTER_ABSOLUTE);
                    if !dev.is_null() {
                        let now = ei_now(self.ei);
                        ei_device_pointer_motion_absolute(dev, x, y);
                        ei_device_frame(dev, now);
                    }
                }
                InputCmd::PointerMotion(dx, dy) => {
                    let dev = self.device_for_cap(EI_DEVICE_CAP_POINTER);
                    if !dev.is_null() {
                        let now = ei_now(self.ei);
                        ei_device_pointer_motion(dev, dx, dy);
                        ei_device_frame(dev, now);
                    }
                }
                InputCmd::Button(button, is_press) => {
                    let dev = self.device_for_cap(EI_DEVICE_CAP_BUTTON);
                    if !dev.is_null() {
                        let now = ei_now(self.ei);
                        ei_device_button_button(dev, button, is_press);
                        ei_device_frame(dev, now);
                    }
                }
                InputCmd::Scroll(dx, dy) => {
                    let dev = self.device_for_cap(EI_DEVICE_CAP_SCROLL);
                    if !dev.is_null() {
                        let now = ei_now(self.ei);
                        ei_device_scroll_delta(dev, dx, dy);
                        ei_device_frame(dev, now);
                    }
                }
                InputCmd::Key(keycode, is_press) => {
                    let dev = self.device_for_cap(EI_DEVICE_CAP_KEYBOARD);
                    if !dev.is_null() {
                        let now = ei_now(self.ei);
                        ei_device_keyboard_key(dev, keycode, is_press);
                        ei_device_frame(dev, now);
                    }
                }
            }
        }
    }
}

impl GamescopeInputClient {
    pub fn connect() -> Result<Self> {
        let socket_path_buf = find_gamescope_ei_socket();
        Self::connect_to(&socket_path_buf)
    }

    pub fn connect_to(path: &std::path::Path) -> Result<Self> {
        let socket_path = path.to_string_lossy().to_string();

        let c_socket = CString::new(socket_path.clone())
            .context("Failed to format socket path into CString")?;

        let ei = unsafe {
            let ei = ei_new_sender(std::ptr::null_mut());
            if ei.is_null() {
                anyhow::bail!("ei_new_sender returned NULL");
            }

            let ret = ei_setup_backend_socket(ei, c_socket.as_ptr());
            if ret != 0 {
                ei_unref(ei);
                anyhow::bail!(
                    "ei_setup_backend_socket failed connecting to {socket_path}: error code {ret}"
                );
            }
            ei
        };

        let disconnected = Arc::new(AtomicBool::new(false));
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<InputCmd>();
        let mut session = EiWorkerSession {
            ei,
            devices: Vec::new(),
            is_emulating: false,
            disconnected: Arc::clone(&disconnected),
        };

        let worker = std::thread::Builder::new()
            .name("gamescope-eis-input".to_string())
            .spawn(move || {
                // Negotiate initial seat capabilities and resume device before draining commands.
                for _ in 0..20 {
                    session.dispatch_once();
                    if session.is_emulating || session.disconnected.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                tracing::info!(
                    "Gamescope EIS input client connected to {socket_path} (emulating: {})",
                    session.is_emulating
                );

                loop {
                    if session.disconnected.load(Ordering::Relaxed) {
                        break;
                    }
                    match cmd_rx.recv_timeout(Duration::from_millis(16)) {
                        Ok(first_cmd) => {
                            if !session.is_emulating {
                                session.dispatch_once();
                            }
                            emit_and_dispatch_batch(first_cmd, &cmd_rx, &mut session);
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                            session.dispatch_once();
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                            while let Ok(cmd) = cmd_rx.try_recv() {
                                session.emit_cmd(cmd);
                            }
                            session.dispatch_once();
                            break;
                        }
                    }
                }
            })
            .context("Failed to spawn Gamescope EIS input worker thread")?;

        Ok(Self {
            cmd_tx: Some(cmd_tx),
            disconnected,
            worker: Some(worker),
        })
    }

    #[cfg(test)]
    pub fn mock_for_test() -> (Self, crossbeam_channel::Receiver<InputCmd>) {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let client = Self {
            cmd_tx: Some(cmd_tx),
            disconnected: Arc::new(AtomicBool::new(false)),
            worker: None,
        };
        (client, cmd_rx)
    }

    pub fn dispatch(&mut self) {
        // Handled per command batch on the dedicated EIS worker thread.
    }

    pub fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Acquire)
    }

    fn enqueue(&self, cmd: InputCmd) {
        if let Some(ref tx) = self.cmd_tx {
            let _ = tx.send(cmd);
        }
    }

    pub fn send_pointer_motion_absolute(&mut self, x: f64, y: f64) {
        self.enqueue(InputCmd::PointerMotionAbsolute(x, y));
    }

    pub fn send_pointer_motion(&mut self, dx: f64, dy: f64) {
        self.enqueue(InputCmd::PointerMotion(dx, dy));
    }

    pub fn send_button(&mut self, button: u32, is_press: bool) {
        self.enqueue(InputCmd::Button(button, is_press));
    }

    pub fn send_scroll(&mut self, dx: f64, dy: f64) {
        self.enqueue(InputCmd::Scroll(dx, dy));
    }

    pub fn send_key(&mut self, keycode: u32, is_press: bool) {
        self.enqueue(InputCmd::Key(keycode, is_press));
    }
}

fn emit_and_dispatch_batch(
    first_cmd: InputCmd,
    cmd_rx: &crossbeam_channel::Receiver<InputCmd>,
    session: &mut EiWorkerSession,
) {
    let mut pending = Vec::with_capacity(8);
    process_cmd_batch(first_cmd, cmd_rx, |cmd| pending.push(cmd), || {});
    for cmd in pending {
        session.emit_cmd(cmd);
    }
    session.dispatch_once();
}

pub fn egui_key_to_evdev(key: eframe::egui::Key) -> Option<u32> {
    use eframe::egui::Key;
    match key {
        Key::Escape => Some(1),
        Key::Num1 => Some(2),
        Key::Num2 => Some(3),
        Key::Num3 => Some(4),
        Key::Num4 => Some(5),
        Key::Num5 => Some(6),
        Key::Num6 => Some(7),
        Key::Num7 => Some(8),
        Key::Num8 => Some(9),
        Key::Num9 => Some(10),
        Key::Num0 => Some(11),
        Key::Minus => Some(12),
        Key::Equals => Some(13),
        Key::Backspace => Some(14),
        Key::Tab => Some(15),
        Key::Q => Some(16),
        Key::W => Some(17),
        Key::E => Some(18),
        Key::R => Some(19),
        Key::T => Some(20),
        Key::Y => Some(21),
        Key::U => Some(22),
        Key::I => Some(23),
        Key::O => Some(24),
        Key::P => Some(25),
        Key::OpenBracket => Some(26),
        Key::CloseBracket => Some(27),
        Key::Enter => Some(28),
        Key::A => Some(30),
        Key::S => Some(31),
        Key::D => Some(32),
        Key::F => Some(33),
        Key::G => Some(34),
        Key::H => Some(35),
        Key::J => Some(36),
        Key::K => Some(37),
        Key::L => Some(38),
        Key::Semicolon => Some(39),
        Key::Backslash => Some(43),
        Key::Z => Some(44),
        Key::X => Some(45),
        Key::C => Some(46),
        Key::V => Some(47),
        Key::B => Some(48),
        Key::N => Some(49),
        Key::M => Some(50),
        Key::Comma => Some(51),
        Key::Period => Some(52),
        Key::Slash => Some(53),
        Key::Space => Some(57),
        Key::F1 => Some(59),
        Key::F2 => Some(60),
        Key::F3 => Some(61),
        Key::F4 => Some(62),
        Key::F5 => Some(63),
        Key::F6 => Some(64),
        Key::F7 => Some(65),
        Key::F8 => Some(66),
        Key::F9 => Some(67),
        Key::F10 => Some(68),
        Key::F11 => Some(87),
        Key::F12 => Some(88),
        Key::Home => Some(102),
        Key::ArrowUp => Some(103),
        Key::PageUp => Some(104),
        Key::ArrowLeft => Some(105),
        Key::ArrowRight => Some(106),
        Key::End => Some(107),
        Key::ArrowDown => Some(108),
        Key::PageDown => Some(109),
        Key::Insert => Some(110),
        Key::Delete => Some(111),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Key;

    #[test]
    fn test_egui_key_to_evdev_mappings() {
        assert_eq!(egui_key_to_evdev(Key::Escape), Some(1));
        assert_eq!(egui_key_to_evdev(Key::Space), Some(57));
        assert_eq!(egui_key_to_evdev(Key::Enter), Some(28));
        assert_eq!(egui_key_to_evdev(Key::Tab), Some(15));
        assert_eq!(egui_key_to_evdev(Key::W), Some(17));
        assert_eq!(egui_key_to_evdev(Key::A), Some(30));
        assert_eq!(egui_key_to_evdev(Key::S), Some(31));
        assert_eq!(egui_key_to_evdev(Key::D), Some(32));
        assert_eq!(egui_key_to_evdev(Key::F1), Some(59));
        assert_eq!(egui_key_to_evdev(Key::F12), Some(88));
    }

    #[test]
    fn test_libei_event_and_capability_constants() {
        // Event codes must match libei enum ei_event_type exactly
        assert_eq!(EI_EVENT_CONNECT, 1);
        assert_eq!(EI_EVENT_DISCONNECT, 2);
        assert_eq!(EI_EVENT_SEAT_ADDED, 3);
        assert_eq!(EI_EVENT_SEAT_REMOVED, 4);
        assert_eq!(EI_EVENT_DEVICE_ADDED, 5);
        assert_eq!(EI_EVENT_DEVICE_REMOVED, 6);
        assert_eq!(EI_EVENT_DEVICE_PAUSED, 7);
        assert_eq!(EI_EVENT_DEVICE_RESUMED, 8);

        // Capabilities must be powers of 2 matching enum ei_device_capability
        assert_eq!(EI_DEVICE_CAP_POINTER, 1);
        assert_eq!(EI_DEVICE_CAP_POINTER_ABSOLUTE, 2);
        assert_eq!(EI_DEVICE_CAP_KEYBOARD, 4);
        assert_eq!(EI_DEVICE_CAP_TOUCH, 8);
        assert_eq!(EI_DEVICE_CAP_SCROLL, 16);
        assert_eq!(EI_DEVICE_CAP_BUTTON, 32);
        assert_eq!(EI_DEVICE_CAP_TEXT, 64);
    }

    #[test]
    fn test_find_gamescope_ei_socket_fallback() {
        let p = find_gamescope_ei_socket();
        assert!(p.to_string_lossy().contains("gamescope-"));
        assert!(p.to_string_lossy().ends_with("-ei"));
    }

    #[test]
    fn test_process_cmd_batch_drains_queued_commands_into_single_dispatch() {
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(InputCmd::Key(29, true)).unwrap();
        tx.send(InputCmd::Key(44, true)).unwrap();
        tx.send(InputCmd::PointerMotionAbsolute(640.0, 400.0))
            .unwrap();
        tx.send(InputCmd::Button(BTN_LEFT, true)).unwrap();

        let first = rx.recv().unwrap();
        let mut emitted = Vec::new();
        let mut dispatches = 0usize;

        process_cmd_batch(first, &rx, |cmd| emitted.push(cmd), || dispatches += 1);

        assert_eq!(
            emitted,
            vec![
                InputCmd::Key(29, true),
                InputCmd::Key(44, true),
                InputCmd::PointerMotionAbsolute(640.0, 400.0),
                InputCmd::Button(BTN_LEFT, true),
            ]
        );
        assert_eq!(dispatches, 1);
    }
}
