//! Dedicated DualSense / DualSense Edge 1:1 controller monitor and feedback bridge.
//!
//! Strictly accepts ONLY Sony PS5 DualSense (`054c:0ce6`) and DualSense Edge (`054c:0df2`)
//! controllers over USB or Bluetooth. All SDL2 controller & USB-haptics calls are isolated
//! on a single worker thread for thread safety.

use crossbeam_channel::{Receiver, Sender};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[cfg(feature = "ps5-remote-play")]
use super::ffi;

pub const PS5_CONTROLLER_REQUIRED_ERROR: &str = "connect a PS5 controller";

pub const SONY_VENDOR_ID: u16 = 0x054c;
pub const DUALSENSE_PRODUCT_ID: u16 = 0x0ce6;
pub const DUALSENSE_EDGE_PRODUCT_ID: u16 = 0x0df2;

pub fn is_supported_ps5_controller_vid_pid(vendor_id: u16, product_id: u16) -> bool {
    vendor_id == SONY_VENDOR_ID
        && matches!(product_id, DUALSENSE_PRODUCT_ID | DUALSENSE_EDGE_PRODUCT_ID)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ps5DualSenseStatus {
    pub connected: bool,
    pub is_edge: bool,
    pub is_wireless: bool,
    pub haptics_audio_connected: bool,
    pub vendor_id: u16,
    pub product_id: u16,
    pub name: String,
}

impl Ps5DualSenseStatus {
    pub fn short_label(&self) -> String {
        if !self.connected {
            return "No PS5 Controller".to_string();
        }
        let model = if self.is_edge {
            "DualSense Edge"
        } else {
            "DualSense"
        };
        let transport = if self.is_wireless { "Bluetooth" } else { "USB" };
        if self.haptics_audio_connected {
            format!("{model} ({transport} + Haptics)")
        } else {
            format!("{model} ({transport})")
        }
    }
}

pub enum ControllerCommand {
    AttachSession {
        session_ptr: usize,
        enable_haptics: bool,
        ack: Sender<()>,
    },
    DetachSession {
        ack: Sender<()>,
    },
    SetRumble {
        left: u8,
        right: u8,
    },
    SetTriggerEffects {
        type_left: u8,
        left: [u8; 10],
        type_right: u8,
        right: [u8; 10],
    },
    SetLed {
        r: u8,
        g: u8,
        b: u8,
    },
    SetPlayerIndex(u8),
    SetHapticIntensity(i32),
    SetTriggerIntensity(i32),
    ResetMotion,
    PushHaptics(Vec<u8>),
    Shutdown,
}

#[derive(Clone)]
pub struct DualSenseBridgeHandle {
    tx: Sender<ControllerCommand>,
    connected: Arc<AtomicBool>,
    status: Arc<Mutex<Ps5DualSenseStatus>>,
}

impl DualSenseBridgeHandle {
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub fn status(&self) -> Ps5DualSenseStatus {
        self.status.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn attach_session(&self, session_ptr: usize, enable_haptics: bool) {
        let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
        if self
            .tx
            .send(ControllerCommand::AttachSession {
                session_ptr,
                enable_haptics,
                ack: ack_tx,
            })
            .is_ok()
        {
            let _ = ack_rx.recv_timeout(Duration::from_secs(2));
        }
    }

    pub fn detach_session(&self) {
        let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
        if self
            .tx
            .send(ControllerCommand::DetachSession { ack: ack_tx })
            .is_ok()
        {
            let _ = ack_rx.recv_timeout(Duration::from_secs(2));
        }
    }

    pub fn send_effect(&self, cmd: ControllerCommand) {
        let _ = self.tx.try_send(cmd);
    }
}

pub struct DualSenseMonitor {
    handle: DualSenseBridgeHandle,
    stop_flag: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl DualSenseMonitor {
    pub fn start(request_repaint: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        // Bounded queue prevents unbounded memory growth if haptics audio arrives faster than consumed.
        let (tx, rx) = crossbeam_channel::bounded::<ControllerCommand>(128);
        let connected = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Ps5DualSenseStatus::default()));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let handle = DualSenseBridgeHandle {
            tx,
            connected: Arc::clone(&connected),
            status: Arc::clone(&status),
        };

        let stop_clone = Arc::clone(&stop_flag);
        let conn_clone = Arc::clone(&connected);
        let status_clone = Arc::clone(&status);

        let worker = thread::Builder::new()
            .name("ps5-dualsense-monitor".to_string())
            .spawn(move || {
                run_dualsense_thread(rx, stop_clone, conn_clone, status_clone, request_repaint);
            })
            .ok();

        Self {
            handle,
            stop_flag,
            worker,
        }
    }

    pub fn handle(&self) -> DualSenseBridgeHandle {
        self.handle.clone()
    }

    pub fn is_connected(&self) -> bool {
        self.handle.is_connected()
    }

    pub fn status(&self) -> Ps5DualSenseStatus {
        self.handle.status()
    }
}

impl Drop for DualSenseMonitor {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        let _ = self.handle.tx.try_send(ControllerCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(feature = "ps5-remote-play")]
fn run_dualsense_thread(
    rx: Receiver<ControllerCommand>,
    stop_flag: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    status: Arc<Mutex<Ps5DualSenseStatus>>,
    request_repaint: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    let ds = unsafe { ffi::michadame_ds_create() };
    if ds.is_null() {
        tracing::warn!("Failed to initialize DualSense SDL monitor");
        return;
    }

    let mut active_session_ptr: usize = 0;
    let mut prev_status = Ps5DualSenseStatus::default();

    while !stop_flag.load(Ordering::Acquire) {
        let wait = if active_session_ptr != 0 {
            Duration::from_millis(4)
        } else {
            Duration::from_millis(200)
        };

        // Block briefly or drain immediately on incoming commands
        match rx.recv_timeout(wait) {
            Ok(first) => {
                if handle_cmd(ds, first, &mut active_session_ptr) {
                    break;
                }
                while let Ok(cmd) = rx.try_recv() {
                    if handle_cmd(ds, cmd, &mut active_session_ptr) {
                        stop_flag.store(true, Ordering::Release);
                        break;
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }

        if stop_flag.load(Ordering::Acquire) {
            break;
        }

        let mut raw_info = ffi::RawDualSenseInfo::default();
        unsafe {
            ffi::michadame_ds_poll(
                ds,
                active_session_ptr as *mut ffi::MichadamePs5Session,
                &mut raw_info,
            );
        }

        let new_status = Ps5DualSenseStatus {
            connected: raw_info.connected,
            is_edge: raw_info.is_edge,
            is_wireless: raw_info.is_wireless,
            haptics_audio_connected: raw_info.haptics_audio_connected,
            vendor_id: raw_info.vendor_id,
            product_id: raw_info.product_id,
            name: ffi::c_char_buf_to_string(&raw_info.name),
        };

        connected.store(new_status.connected, Ordering::Release);
        if new_status != prev_status {
            prev_status = new_status.clone();
            if let Ok(mut guard) = status.lock() {
                *guard = new_status;
            }
            if let Some(ref repaint) = request_repaint {
                repaint();
            }
        }
    }

    unsafe {
        ffi::michadame_ds_stop_haptics(ds);
        ffi::michadame_ds_destroy(ds);
    }
    connected.store(false, Ordering::Release);
}

#[cfg(feature = "ps5-remote-play")]
fn handle_cmd(
    ds: *mut ffi::MichadameDualSense,
    cmd: ControllerCommand,
    active_session_ptr: &mut usize,
) -> bool {
    unsafe {
        match cmd {
            ControllerCommand::AttachSession {
                session_ptr,
                enable_haptics,
                ack,
            } => {
                *active_session_ptr = session_ptr;
                if enable_haptics {
                    ffi::michadame_ds_start_haptics(ds);
                } else {
                    ffi::michadame_ds_stop_haptics(ds);
                }
                let _ = ack.send(());
                false
            }
            ControllerCommand::DetachSession { ack } => {
                *active_session_ptr = 0;
                ffi::michadame_ds_stop_haptics(ds);
                let _ = ack.send(());
                false
            }
            ControllerCommand::SetRumble { left, right } => {
                ffi::michadame_ds_set_rumble(ds, left, right);
                false
            }
            ControllerCommand::SetTriggerEffects {
                type_left,
                left,
                type_right,
                right,
            } => {
                ffi::michadame_ds_set_trigger_effects(
                    ds,
                    type_left,
                    left.as_ptr(),
                    type_right,
                    right.as_ptr(),
                );
                false
            }
            ControllerCommand::SetLed { r, g, b } => {
                ffi::michadame_ds_set_led(ds, r, g, b);
                false
            }
            ControllerCommand::SetPlayerIndex(idx) => {
                ffi::michadame_ds_set_player_index(ds, idx);
                false
            }
            ControllerCommand::SetHapticIntensity(intensity) => {
                ffi::michadame_ds_set_haptic_intensity(ds, intensity);
                false
            }
            ControllerCommand::SetTriggerIntensity(intensity) => {
                ffi::michadame_ds_set_trigger_intensity(ds, intensity);
                false
            }
            ControllerCommand::ResetMotion => {
                ffi::michadame_ds_reset_motion(ds);
                false
            }
            ControllerCommand::PushHaptics(bytes) => {
                ffi::michadame_ds_push_haptics(ds, bytes.as_ptr(), bytes.len());
                false
            }
            ControllerCommand::Shutdown => true,
        }
    }
}

#[cfg(not(feature = "ps5-remote-play"))]
fn run_dualsense_thread(
    rx: Receiver<ControllerCommand>,
    stop_flag: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    _status: Arc<Mutex<Ps5DualSenseStatus>>,
    _request_repaint: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    connected.store(false, Ordering::Release);
    while !stop_flag.load(Ordering::Acquire) {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(ControllerCommand::AttachSession { ack, .. })
            | Ok(ControllerCommand::DetachSession { ack }) => {
                let _ = ack.send(());
            }
            Ok(ControllerCommand::Shutdown) => break,
            Ok(_) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
}
