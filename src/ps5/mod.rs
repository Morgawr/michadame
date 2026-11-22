//! PlayStation 5 (and PS5 Pro) Remote Play streaming client.
//!
//! Powered at build time by a pinned `streetpea/chiaki-ng` `libchiaki` static library
//! (`ps5-remote-play` Cargo feature, enabled by default), with strict 1:1 Sony DualSense /
//! DualSense Edge controller passthrough, PSN OAuth2 login + remote holepunch streaming,
//! LAN discovery + PIN registration, PS5 virtual keyboard prompts, and full integration at
//! the bottom of Michadame's shader/rendering/replay pipeline.

pub mod controller;
pub mod ffi;
pub mod psn;
pub mod session;

use std::sync::{atomic::AtomicBool, Arc};
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender};
use serde::{Deserialize, Serialize};

pub use controller::{
    is_supported_ps5_controller_vid_pid, DualSenseBridgeHandle, DualSenseMonitor,
    Ps5DualSenseStatus, PS5_CONTROLLER_REQUIRED_ERROR,
};
pub use ffi::{DiscoveredHostInfo, PsnRemoteDeviceInfo, RegisteredConsoleData};
pub use psn::StoredPsnToken;
pub use session::{Ps5AudioTelemetry, Ps5Command, Ps5SessionEvent, Ps5SessionParams};

pub const PS5_MIN_BITRATE_KBPS: u32 = 2_000;
pub const PS5_MAX_BITRATE_KBPS: u32 = 100_000;
pub const PS5_DEFAULT_BITRATE_KBPS: u32 = 15_000;
pub const PS5_MIN_AUDIO_BUFFER_MS: u32 = 50;
pub const PS5_MAX_AUDIO_BUFFER_MS: u32 = 200;
pub const PS5_DEFAULT_AUDIO_BUFFER_MS: u32 = 100;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ps5Resolution {
    P360,
    P540,
    P720,
    #[default]
    P1080,
}

impl Ps5Resolution {
    pub const ALL: [Self; 4] = [Self::P360, Self::P540, Self::P720, Self::P1080];

    pub fn dimensions(self) -> (u32, u32) {
        match self {
            Self::P360 => (640, 360),
            Self::P540 => (960, 540),
            Self::P720 => (1280, 720),
            Self::P1080 => (1920, 1080),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::P360 => "360p (640x360)",
            Self::P540 => "540p (960x540)",
            Self::P720 => "720p (1280x720)",
            Self::P1080 => "1080p (1920x1080)",
        }
    }

    pub fn recommended_bitrate_kbps(self) -> u32 {
        match self {
            Self::P360 => 3_000,
            Self::P540 => 6_000,
            Self::P720 => 10_000,
            Self::P1080 => 15_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ps5Fps {
    Fps30,
    #[default]
    Fps60,
}

impl Ps5Fps {
    pub const ALL: [Self; 2] = [Self::Fps30, Self::Fps60];

    pub fn as_u32(self) -> u32 {
        match self {
            Self::Fps30 => 30,
            Self::Fps60 => 60,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Fps30 => "30 FPS",
            Self::Fps60 => "60 FPS",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ps5Codec {
    H264,
    #[default]
    H265,
}

impl Ps5Codec {
    pub const ALL: [Self; 2] = [Self::H265, Self::H264];

    pub fn label(self) -> &'static str {
        match self {
            Self::H265 => "H.265 (HEVC)",
            Self::H264 => "H.264 (AVC)",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ps5ConnectionMode {
    #[default]
    Auto,
    LocalLan,
    PsnRemote,
}

impl Ps5ConnectionMode {
    pub const ALL: [Self; 3] = [Self::Auto, Self::LocalLan, Self::PsnRemote];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (LAN → PSN)",
            Self::LocalLan => "Local LAN Only",
            Self::PsnRemote => "Remote via PSN",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ps5HostPowerState {
    #[default]
    Unknown,
    Ready,
    Standby,
}

impl Ps5HostPowerState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "Online",
            Self::Standby => "Rest Mode",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ps5ConsoleEntry {
    pub id: String,
    pub nickname: String,
    pub host: String,
    pub mac: String,
    pub regist_key_hex: String,
    pub rp_key_hex: String,
    pub psn_duid_hex: String,
    pub console_pin: String,
    pub preferred_mode: Ps5ConnectionMode,
}

impl Ps5ConsoleEntry {
    pub fn display_name(&self) -> String {
        let name = if !self.nickname.trim().is_empty() {
            self.nickname.trim().to_string()
        } else if !self.host.trim().is_empty() {
            self.host.trim().to_string()
        } else if !self.mac.trim().is_empty() {
            format!("PS5 ({})", self.mac.trim())
        } else {
            "PlayStation 5".to_string()
        };

        let mut tags = Vec::new();
        if !self.host.trim().is_empty() {
            tags.push(self.host.trim().to_string());
        }
        if self.has_local_registration() {
            tags.push("LAN".to_string());
        }
        if self.has_psn_target() {
            tags.push("PSN".to_string());
        }
        if tags.is_empty() {
            name
        } else {
            format!("{name} [{}]", tags.join(" · "))
        }
    }

    pub fn regist_key_bytes(&self) -> Option<[u8; 16]> {
        psn::hex_decode_fixed::<16>(&self.regist_key_hex).ok()
    }

    pub fn rp_key_bytes(&self) -> Option<[u8; 16]> {
        psn::hex_decode_fixed::<16>(&self.rp_key_hex).ok()
    }

    pub fn has_local_registration(&self) -> bool {
        !self.host.trim().is_empty()
            && self.regist_key_bytes().is_some()
            && self.rp_key_bytes().is_some()
    }

    pub fn has_psn_target(&self) -> bool {
        !self.psn_duid_hex.trim().is_empty()
    }
}

/// Persisted PS5 Remote Play configuration (stored in `michadame.toml`).
///
/// NOTE: All scalar fields MUST precede `consoles` (`Vec<Ps5ConsoleEntry>`) so `toml`
/// serialization never emits a value after an array-of-tables (`ValueAfterTable`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ps5Config {
    pub selected_console_id: String,
    pub resolution: Ps5Resolution,
    pub fps: Ps5Fps,
    pub bitrate_kbps: u32,
    pub audio_buffer_ms: u32,
    pub codec: Ps5Codec,
    pub connection_mode: Ps5ConnectionMode,
    pub enable_keyboard: bool,
    pub enable_dualsense_haptics: bool,
    pub auto_go_home_on_cant_display: bool,
    pub psn_account_id_b64: String,
    pub auto_discover_on_tab_open: bool,
    pub consoles: Vec<Ps5ConsoleEntry>,
}

impl Default for Ps5Config {
    fn default() -> Self {
        Self {
            selected_console_id: String::new(),
            resolution: Ps5Resolution::P1080,
            fps: Ps5Fps::Fps60,
            bitrate_kbps: PS5_DEFAULT_BITRATE_KBPS,
            audio_buffer_ms: PS5_DEFAULT_AUDIO_BUFFER_MS,
            codec: Ps5Codec::H265,
            connection_mode: Ps5ConnectionMode::Auto,
            enable_keyboard: true,
            enable_dualsense_haptics: true,
            auto_go_home_on_cant_display: false,
            psn_account_id_b64: String::new(),
            auto_discover_on_tab_open: true,
            consoles: Vec::new(),
        }
    }
}

impl Ps5Config {
    pub fn clamped_bitrate_kbps(&self) -> u32 {
        self.bitrate_kbps
            .clamp(PS5_MIN_BITRATE_KBPS, PS5_MAX_BITRATE_KBPS)
    }

    pub fn clamped_audio_buffer_ms(&self) -> u32 {
        self.audio_buffer_ms
            .clamp(PS5_MIN_AUDIO_BUFFER_MS, PS5_MAX_AUDIO_BUFFER_MS)
    }
}

pub type DiscoveryTaskResult = Result<Vec<DiscoveredHostInfo>, String>;
pub type PsnAuthTaskResult = Result<StoredPsnToken, String>;
pub type PsnDevicesTaskResult = Result<(Vec<PsnRemoteDeviceInfo>, Option<StoredPsnToken>), String>;
pub type RegistTaskResult = Result<(String, String, String, RegisteredConsoleData), String>;
pub type WakeupTaskResult = Result<String, String>;

pub struct Ps5State {
    pub config: Ps5Config,

    // Active streaming session state
    pub is_active: bool,
    pub is_connecting: bool,
    pub connected_console_id: String,
    pub connected_console_name: String,
    pub status_message: String,
    pub resolution: (u32, u32),
    pub cant_display_warning: bool,
    pub auto_login_pin_attempted: bool,
    pub command_tx: Option<Sender<Ps5Command>>,
    pub event_rx: Option<Receiver<Ps5SessionEvent>>,
    pub stop_requested: Option<Arc<AtomicBool>>,
    pub worker_thread: Option<JoinHandle<()>>,
    pub audio_telemetry: Arc<Ps5AudioTelemetry>,

    // DualSense / DualSense Edge 1:1 controller monitor
    pub controller_monitor: Option<DualSenseMonitor>,
    pub controller_status: Ps5DualSenseStatus,
    pub controller_disconnect_warned: bool,
    #[cfg(test)]
    pub test_controller_override: Option<bool>,

    // PS5 virtual keyboard & Login PIN overlay state
    pub keyboard_prompt_open: bool,
    pub keyboard_text: String,
    pub keyboard_needs_focus: bool,
    pub login_pin_prompt_open: bool,
    pub login_pin_input: String,
    pub login_pin_incorrect: bool,
    pub login_pin_needs_focus: bool,

    // Local LAN discovery state
    pub discovered_consoles: Vec<DiscoveredHostInfo>,
    pub discovery_in_progress: bool,
    pub discovery_rx: Option<Receiver<DiscoveryTaskResult>>,
    pub discovery_auto_triggered: bool,

    // PSN OAuth2 login & remote console list state
    pub psn_token: Option<StoredPsnToken>,
    pub psn_login_client_duid: String,
    pub psn_login_url: String,
    pub psn_redirect_input: String,
    pub psn_auth_in_progress: bool,
    pub psn_auth_rx: Option<Receiver<PsnAuthTaskResult>>,
    pub psn_devices: Vec<PsnRemoteDeviceInfo>,
    pub psn_devices_in_progress: bool,
    pub psn_devices_rx: Option<Receiver<PsnDevicesTaskResult>>,

    // Local PIN registration / PSN auto-registration form & task
    pub regist_host_input: String,
    pub regist_broadcast: bool,
    pub regist_pin_input: String,
    pub regist_console_pin_input: String,
    pub regist_in_progress: bool,
    pub regist_rx: Option<Receiver<RegistTaskResult>>,

    // Manual console add form
    pub manual_nickname_input: String,
    pub manual_host_input: String,

    // Wakeup async task
    pub wakeup_in_progress: bool,
    pub wakeup_rx: Option<Receiver<WakeupTaskResult>>,
}

impl Default for Ps5State {
    fn default() -> Self {
        Self {
            config: Ps5Config::default(),
            is_active: false,
            is_connecting: false,
            connected_console_id: String::new(),
            connected_console_name: String::new(),
            status_message: String::new(),
            resolution: (1920, 1080),
            cant_display_warning: false,
            auto_login_pin_attempted: false,
            command_tx: None,
            event_rx: None,
            stop_requested: None,
            worker_thread: None,
            audio_telemetry: Arc::new(Ps5AudioTelemetry::default()),
            controller_monitor: None,
            controller_status: Ps5DualSenseStatus::default(),
            controller_disconnect_warned: false,
            #[cfg(test)]
            test_controller_override: None,
            keyboard_prompt_open: false,
            keyboard_text: String::new(),
            keyboard_needs_focus: false,
            login_pin_prompt_open: false,
            login_pin_input: String::new(),
            login_pin_incorrect: false,
            login_pin_needs_focus: false,
            discovered_consoles: Vec::new(),
            discovery_in_progress: false,
            discovery_rx: None,
            discovery_auto_triggered: false,
            psn_token: None,
            psn_login_client_duid: String::new(),
            psn_login_url: String::new(),
            psn_redirect_input: String::new(),
            psn_auth_in_progress: false,
            psn_auth_rx: None,
            psn_devices: Vec::new(),
            psn_devices_in_progress: false,
            psn_devices_rx: None,
            regist_host_input: String::new(),
            regist_broadcast: false,
            regist_pin_input: String::new(),
            regist_console_pin_input: String::new(),
            regist_in_progress: false,
            regist_rx: None,
            manual_nickname_input: String::new(),
            manual_host_input: String::new(),
            wakeup_in_progress: false,
            wakeup_rx: None,
        }
    }
}

impl Ps5State {
    pub fn ensure_controller_monitor(
        &mut self,
        request_repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    ) {
        if self.controller_monitor.is_none() {
            let monitor = DualSenseMonitor::start(request_repaint);
            self.controller_status = monitor.status();
            self.controller_monitor = Some(monitor);
        }
    }

    pub fn refresh_controller_status(&mut self) {
        if let Some(ref monitor) = self.controller_monitor {
            self.controller_status = monitor.status();
        }
    }

    pub fn is_controller_connected(&self) -> bool {
        #[cfg(test)]
        if let Some(override_val) = self.test_controller_override {
            return override_val;
        }
        self.controller_monitor
            .as_ref()
            .is_some_and(DualSenseMonitor::is_connected)
    }

    pub fn controller_handle(&self) -> Option<DualSenseBridgeHandle> {
        self.controller_monitor
            .as_ref()
            .map(DualSenseMonitor::handle)
    }

    pub fn is_modal_prompt_open(&self) -> bool {
        self.keyboard_prompt_open || self.login_pin_prompt_open
    }

    pub fn has_background_tasks(&self) -> bool {
        self.discovery_in_progress
            || self.psn_auth_in_progress
            || self.psn_devices_in_progress
            || self.regist_in_progress
            || self.wakeup_in_progress
            || self.is_connecting
    }

    pub fn load_saved_psn_token(&mut self) {
        if let Some(tok) = psn::load_stored_token() {
            if self.config.psn_account_id_b64.trim().is_empty() && !tok.account_id_b64.is_empty() {
                self.config.psn_account_id_b64 = tok.account_id_b64.clone();
            }
            self.psn_token = Some(tok);
        }
    }

    pub fn is_psn_signed_in(&self) -> bool {
        self.psn_token
            .as_ref()
            .is_some_and(StoredPsnToken::is_signed_in)
    }

    pub fn effective_psn_account_id(&self) -> Option<[u8; 8]> {
        if !self.config.psn_account_id_b64.trim().is_empty() {
            if let Ok(bytes) = psn::decode_account_id_b64(&self.config.psn_account_id_b64) {
                return Some(bytes);
            }
        }
        if let Some(ref tok) = self.psn_token {
            if let Ok(bytes) = psn::decode_account_id_b64(&tok.account_id_b64) {
                return Some(bytes);
            }
        }
        None
    }

    pub fn selected_console(&self) -> Option<&Ps5ConsoleEntry> {
        if !self.config.selected_console_id.is_empty() {
            if let Some(found) = self
                .config
                .consoles
                .iter()
                .find(|c| c.id == self.config.selected_console_id)
            {
                return Some(found);
            }
        }
        self.config.consoles.first()
    }

    pub fn send_command(&self, cmd: Ps5Command) {
        if let Some(ref tx) = self.command_tx {
            let _ = tx.send(cmd);
        }
    }

    pub fn start_psn_login_flow(&mut self) -> Result<String, String> {
        let duid = ffi::generate_client_duid().map_err(|e| e.to_string())?;
        let url = psn::build_psn_login_url(&duid);
        self.psn_login_client_duid = duid;
        self.psn_login_url = url.clone();
        self.psn_redirect_input.clear();
        Ok(url)
    }

    pub fn start_psn_code_exchange(
        &mut self,
        request_repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<(), String> {
        if self.psn_auth_in_progress {
            return Err("PSN sign-in is already in progress.".to_string());
        }
        let redirect_input = self.psn_redirect_input.trim().to_string();
        if redirect_input.is_empty() {
            return Err("Paste the Sony redirect URL (or authorization code) first.".to_string());
        }
        let duid = if self.psn_login_client_duid.is_empty() {
            ffi::generate_client_duid().map_err(|e| e.to_string())?
        } else {
            self.psn_login_client_duid.clone()
        };

        let (tx, rx) = crossbeam_channel::bounded(1);
        self.psn_auth_in_progress = true;
        self.psn_auth_rx = Some(rx);
        self.status_message = "Completing PSN sign-in...".to_string();

        let _ = thread::Builder::new()
            .name("ps5-psn-auth".to_string())
            .spawn(move || {
                let res =
                    psn::exchange_redirect_code(&redirect_input, &duid).map_err(|e| e.to_string());
                let _ = tx.send(res);
                if let Some(cb) = request_repaint {
                    cb();
                }
            });
        Ok(())
    }

    pub fn sign_out_psn(&mut self) {
        psn::delete_stored_token();
        self.psn_token = None;
        self.psn_devices.clear();
        self.psn_login_url.clear();
        self.psn_redirect_input.clear();
        self.status_message = "Signed out of PSN.".to_string();
    }

    pub fn start_psn_device_refresh(
        &mut self,
        request_repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<(), String> {
        if self.psn_devices_in_progress {
            return Ok(());
        }
        let Some(token) = self.psn_token.clone() else {
            return Err("Sign in to PSN first to fetch linked PS5 consoles.".to_string());
        };

        let (tx, rx) = crossbeam_channel::bounded(1);
        self.psn_devices_in_progress = true;
        self.psn_devices_rx = Some(rx);
        self.status_message = "Fetching PS5 consoles from PSN...".to_string();

        let _ = thread::Builder::new()
            .name("ps5-psn-devices".to_string())
            .spawn(move || {
                let res = (|| -> Result<_, String> {
                    let fresh = psn::ensure_fresh_psn_token(&token).map_err(|e| e.to_string())?;
                    let updated = if fresh.access_token != token.access_token {
                        Some(fresh.clone())
                    } else {
                        None
                    };
                    let devices = ffi::list_psn_ps5_devices(&fresh.access_token)
                        .map_err(|e| e.to_string())?;
                    Ok((devices, updated))
                })();
                let _ = tx.send(res);
                if let Some(cb) = request_repaint {
                    cb();
                }
            });
        Ok(())
    }

    pub fn start_discovery(&mut self, request_repaint: Option<Arc<dyn Fn() + Send + Sync>>) {
        if self.discovery_in_progress {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.discovery_in_progress = true;
        self.discovery_rx = Some(rx);
        self.status_message = "Scanning local network for PS5 consoles...".to_string();

        let _ = thread::Builder::new()
            .name("ps5-lan-discovery".to_string())
            .spawn(move || {
                let res = ffi::discover_local_ps5s(2500).map_err(|e| e.to_string());
                let _ = tx.send(res);
                if let Some(cb) = request_repaint {
                    cb();
                }
            });
    }

    pub fn start_local_registration(
        &mut self,
        request_repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<(), String> {
        if self.regist_in_progress {
            return Err("Console registration is already in progress.".to_string());
        }
        let host = self.regist_host_input.trim().to_string();
        if host.is_empty() && !self.regist_broadcast {
            return Err(
                "Enter the PS5 IP address (or select a discovered PS5 / enable broadcast)."
                    .to_string(),
            );
        }
        let host_for_call = if host.is_empty() {
            "255.255.255.255".to_string()
        } else {
            host.clone()
        };

        let account_id = self.effective_psn_account_id().ok_or_else(|| {
            "PSN Account-ID (8-byte Base64) is required. Sign in to PSN or paste your Account-ID."
                .to_string()
        })?;

        let pin_digits: String = self
            .regist_pin_input
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect();
        if pin_digits.len() != 8 {
            return Err(
                "Enter the 8-digit PIN from PS5 Settings → System → Remote Play → Link Device."
                    .to_string(),
            );
        }
        let pin: u32 = pin_digits
            .parse()
            .map_err(|_| "Invalid 8-digit registration PIN.".to_string())?;

        let console_pin_str: String = self
            .regist_console_pin_input
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect();
        let console_pin: u32 = if console_pin_str.is_empty() {
            0
        } else {
            console_pin_str
                .parse()
                .map_err(|_| "Invalid 4-digit console login PIN.".to_string())?
        };
        let broadcast = self.regist_broadcast;

        let (tx, rx) = crossbeam_channel::bounded(1);
        self.regist_in_progress = true;
        self.regist_rx = Some(rx);
        self.status_message = format!("Registering PS5 at {host_for_call}...");

        let _ = thread::Builder::new()
            .name("ps5-local-regist".to_string())
            .spawn(move || {
                let res = ffi::register_local_ps5(
                    &host_for_call,
                    broadcast,
                    &account_id,
                    pin,
                    console_pin,
                )
                .map(|data| (host, String::new(), console_pin_str, data))
                .map_err(|e| e.to_string());
                let _ = tx.send(res);
                if let Some(cb) = request_repaint {
                    cb();
                }
            });
        Ok(())
    }

    pub fn start_psn_auto_registration(
        &mut self,
        psn_duid_hex: &str,
        device_name: &str,
        request_repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<(), String> {
        if self.regist_in_progress {
            return Err("Console registration is already in progress.".to_string());
        }
        let token = self
            .psn_token
            .clone()
            .ok_or_else(|| "Sign in to PSN first to auto-register.".to_string())?;
        let account_id = self
            .effective_psn_account_id()
            .ok_or_else(|| "PSN Account-ID missing. Sign in to PSN again.".to_string())?;
        let duid = psn_duid_hex.trim().to_string();
        if duid.is_empty() {
            return Err("Selected PSN device is missing its device UID.".to_string());
        }
        let console_name = if device_name.trim().is_empty() {
            "PlayStation 5".to_string()
        } else {
            device_name.trim().to_string()
        };

        let (tx, rx) = crossbeam_channel::bounded(1);
        self.regist_in_progress = true;
        self.regist_rx = Some(rx);
        self.status_message = format!("Auto-registering {console_name} via PSN...");

        let _ = thread::Builder::new()
            .name("ps5-psn-autoregist".to_string())
            .spawn(move || {
                let (ev_tx, ev_rx) = crossbeam_channel::unbounded();
                let (_cmd_tx, cmd_rx) = crossbeam_channel::bounded(4);
                let (frame_tx, _frame_rx) = crossbeam_channel::bounded(1);
                let stop_flag = Arc::new(AtomicBool::new(false));
                let noop_repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});

                let params = Ps5SessionParams {
                    console_name: console_name.clone(),
                    host: String::new(),
                    regist_key: None,
                    morning: None,
                    psn_duid_hex: duid.clone(),
                    psn_token: Some(token),
                    psn_account_id: Some(account_id),
                    console_login_pin: None,
                    mode: Ps5ConnectionMode::PsnRemote,
                    auto_regist_only: true,
                    width: 1280,
                    height: 720,
                    max_fps: 30,
                    bitrate_kbps: 6000,
                    codec: Ps5Codec::H264,
                    enable_keyboard: false,
                    enable_dualsense_haptics: false,
                    allow_hwdec: false,
                    scaler_filter: Arc::new(std::sync::atomic::AtomicU8::new(0)),
                    color_range: Arc::new(std::sync::atomic::AtomicU8::new(0)),
                    master_volume: 0.0,
                    peak_amplitude: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                    audio_latency_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                    audio_telemetry: Arc::new(Ps5AudioTelemetry::default()),
                    queue_drops: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                    replay_audio: Arc::new(crate::replay::AudioTap::default()),
                    frame_tx,
                    command_rx: cmd_rx,
                    event_tx: ev_tx,
                    stop_requested: stop_flag,
                    request_repaint: noop_repaint,
                    controller_handle: None,
                };

                session::run_ps5_session_worker(params);

                let mut reg_data: Option<RegisteredConsoleData> = None;
                let mut err_msg: Option<String> = None;
                while let Ok(ev) = ev_rx.try_recv() {
                    match ev {
                        Ps5SessionEvent::AutoRegistSucceeded(d) => reg_data = Some(d),
                        Ps5SessionEvent::Failed(e) => err_msg = Some(e),
                        _ => {}
                    }
                }

                let res = if let Some(d) = reg_data {
                    Ok((String::new(), duid, String::new(), d))
                } else {
                    Err(err_msg.unwrap_or_else(|| {
                        "PSN auto-registration did not receive credentials from the PS5."
                            .to_string()
                    }))
                };
                let _ = tx.send(res);
                if let Some(cb) = request_repaint {
                    cb();
                }
            });
        Ok(())
    }

    pub fn start_wakeup(
        &mut self,
        console_id: &str,
        request_repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<(), String> {
        if self.wakeup_in_progress {
            return Err("Wakeup request is already in progress.".to_string());
        }
        let console = self
            .config
            .consoles
            .iter()
            .find(|c| c.id == console_id)
            .ok_or_else(|| "Select a saved PS5 console first.".to_string())?
            .clone();

        let host = console.host.trim().to_string();
        if host.is_empty() {
            return Err("This PS5 does not have a local LAN IP address saved.".to_string());
        }
        let regist_key = console.regist_key_bytes().ok_or_else(|| {
            "Register this PS5 first before sending a Wake-Up packet.".to_string()
        })?;
        let display = console.display_name();

        let (tx, rx) = crossbeam_channel::bounded(1);
        self.wakeup_in_progress = true;
        self.wakeup_rx = Some(rx);
        self.status_message = format!("Sending Wake-Up packet to {display}...");

        let _ = thread::Builder::new()
            .name("ps5-wakeup".to_string())
            .spawn(move || {
                let res = ffi::send_wakeup(&host, &regist_key)
                    .map(|()| format!("Wake-Up packet sent to {display}."))
                    .map_err(|e| e.to_string());
                let _ = tx.send(res);
                if let Some(cb) = request_repaint {
                    cb();
                }
            });
        Ok(())
    }

    pub fn upsert_registered_console(
        &mut self,
        host: &str,
        psn_duid_hex: &str,
        console_pin: &str,
        data: &RegisteredConsoleData,
    ) -> String {
        let mac = ffi::format_mac(&data.server_mac);
        let mac_nonzero = data.server_mac.iter().any(|&b| b != 0);
        let regist_key_hex = psn::hex_encode(&data.rp_regist_key);
        let rp_key_hex = psn::hex_encode(&data.rp_key);
        let nickname = if !data.server_nickname.trim().is_empty() {
            data.server_nickname.trim().to_string()
        } else {
            "PlayStation 5".to_string()
        };
        let clean_host = host.trim();
        let clean_duid = psn_duid_hex.trim();

        // Match existing entry by MAC, PSN DUID, host IP, or exact nickname
        let existing_idx = self.config.consoles.iter().position(|c| {
            (mac_nonzero && !c.mac.is_empty() && c.mac.eq_ignore_ascii_case(&mac))
                || (!clean_duid.is_empty()
                    && !c.psn_duid_hex.is_empty()
                    && c.psn_duid_hex.eq_ignore_ascii_case(clean_duid))
                || (!clean_host.is_empty()
                    && !c.host.is_empty()
                    && c.host.eq_ignore_ascii_case(clean_host))
                || (!nickname.is_empty()
                    && c.nickname.eq_ignore_ascii_case(&nickname)
                    && c.regist_key_hex.is_empty())
        });

        let id = if let Some(idx) = existing_idx {
            let entry = &mut self.config.consoles[idx];
            if !nickname.is_empty() {
                entry.nickname = nickname;
            }
            if !clean_host.is_empty() {
                entry.host = clean_host.to_string();
            }
            if mac_nonzero {
                entry.mac = mac;
            }
            entry.regist_key_hex = regist_key_hex;
            entry.rp_key_hex = rp_key_hex;
            if !clean_duid.is_empty() {
                entry.psn_duid_hex = clean_duid.to_string();
            }
            if !console_pin.trim().is_empty() {
                entry.console_pin = console_pin.trim().to_string();
            } else if data.console_pin != 0 {
                entry.console_pin = format!("{:04}", data.console_pin);
            }
            entry.id.clone()
        } else {
            let new_id = if mac_nonzero {
                format!("mac:{}", psn::hex_encode(&data.server_mac))
            } else if !clean_duid.is_empty() {
                format!("psn:{clean_duid}")
            } else if !clean_host.is_empty() {
                format!("host:{clean_host}")
            } else {
                format!("ps5:{}", self.config.consoles.len() + 1)
            };
            let pin_saved = if !console_pin.trim().is_empty() {
                console_pin.trim().to_string()
            } else if data.console_pin != 0 {
                format!("{:04}", data.console_pin)
            } else {
                String::new()
            };
            self.config.consoles.push(Ps5ConsoleEntry {
                id: new_id.clone(),
                nickname,
                host: clean_host.to_string(),
                mac: if mac_nonzero { mac } else { String::new() },
                regist_key_hex,
                rp_key_hex,
                psn_duid_hex: clean_duid.to_string(),
                console_pin: pin_saved,
                preferred_mode: Ps5ConnectionMode::Auto,
            });
            new_id
        };

        self.config.selected_console_id = id.clone();
        id
    }

    pub fn upsert_psn_devices_into_consoles(&mut self, devices: &[PsnRemoteDeviceInfo]) {
        for dev in devices {
            let duid = dev.device_uid_hex.trim();
            if duid.is_empty() {
                continue;
            }
            let existing = self.config.consoles.iter().position(|c| {
                c.psn_duid_hex.eq_ignore_ascii_case(duid)
                    || (!dev.device_name.trim().is_empty()
                        && c.nickname.eq_ignore_ascii_case(dev.device_name.trim())
                        && c.psn_duid_hex.is_empty())
            });
            if let Some(idx) = existing {
                let entry = &mut self.config.consoles[idx];
                entry.psn_duid_hex = duid.to_string();
                if entry.nickname.trim().is_empty() && !dev.device_name.trim().is_empty() {
                    entry.nickname = dev.device_name.trim().to_string();
                }
            } else {
                let id = format!("psn:{duid}");
                self.config.consoles.push(Ps5ConsoleEntry {
                    id: id.clone(),
                    nickname: if dev.device_name.trim().is_empty() {
                        "PlayStation 5 (PSN)".to_string()
                    } else {
                        dev.device_name.trim().to_string()
                    },
                    host: String::new(),
                    mac: String::new(),
                    regist_key_hex: String::new(),
                    rp_key_hex: String::new(),
                    psn_duid_hex: duid.to_string(),
                    console_pin: String::new(),
                    preferred_mode: Ps5ConnectionMode::Auto,
                });
                if self.config.selected_console_id.is_empty() {
                    self.config.selected_console_id = id;
                }
            }
        }
    }

    pub fn merge_discovered_hosts_into_consoles(&mut self, hosts: &[DiscoveredHostInfo]) {
        for h in hosts {
            let addr = h.host_addr.trim();
            if addr.is_empty() {
                continue;
            }
            let host_id_norm = h.host_id.trim().replace(':', "").to_lowercase();
            let existing = self.config.consoles.iter().position(|c| {
                let c_mac_norm = c.mac.trim().replace(':', "").to_lowercase();
                (!host_id_norm.is_empty() && !c_mac_norm.is_empty() && c_mac_norm == host_id_norm)
                    || (!h.host_name.trim().is_empty()
                        && c.nickname.eq_ignore_ascii_case(h.host_name.trim()))
                    || (!c.host.is_empty() && c.host.eq_ignore_ascii_case(addr))
            });
            if let Some(idx) = existing {
                let entry = &mut self.config.consoles[idx];
                entry.host = addr.to_string();
                if entry.nickname.trim().is_empty() && !h.host_name.trim().is_empty() {
                    entry.nickname = h.host_name.trim().to_string();
                }
            }
        }
    }

    pub fn add_manual_console(&mut self) -> Result<String, String> {
        let host = self.manual_host_input.trim().to_string();
        if host.is_empty() {
            return Err("Enter an IP address or hostname for the PS5.".to_string());
        }
        let nickname = if self.manual_nickname_input.trim().is_empty() {
            format!("PS5 ({host})")
        } else {
            self.manual_nickname_input.trim().to_string()
        };
        let id = format!("host:{host}");
        if let Some(existing) = self
            .config
            .consoles
            .iter_mut()
            .find(|c| c.host.eq_ignore_ascii_case(&host))
        {
            existing.nickname = nickname;
            self.config.selected_console_id = existing.id.clone();
            self.manual_host_input.clear();
            self.manual_nickname_input.clear();
            return Ok(existing.id.clone());
        }
        self.config.consoles.push(Ps5ConsoleEntry {
            id: id.clone(),
            nickname,
            host,
            mac: String::new(),
            regist_key_hex: String::new(),
            rp_key_hex: String::new(),
            psn_duid_hex: String::new(),
            console_pin: String::new(),
            preferred_mode: Ps5ConnectionMode::Auto,
        });
        self.config.selected_console_id = id.clone();
        self.manual_host_input.clear();
        self.manual_nickname_input.clear();
        Ok(id)
    }

    pub fn remove_console(&mut self, id: &str) {
        self.config.consoles.retain(|c| c.id != id);
        if self.config.selected_console_id == id {
            self.config.selected_console_id = self
                .config
                .consoles
                .first()
                .map(|c| c.id.clone())
                .unwrap_or_default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps5_config_toml_roundtrip_without_value_after_table() {
        let mut cfg = Ps5Config::default();
        cfg.selected_console_id = "mac:aabbccddeeff".to_string();
        cfg.resolution = Ps5Resolution::P1080;
        cfg.fps = Ps5Fps::Fps60;
        cfg.bitrate_kbps = 25_000;
        cfg.codec = Ps5Codec::H265;
        cfg.connection_mode = Ps5ConnectionMode::Auto;
        cfg.auto_go_home_on_cant_display = true;
        cfg.psn_account_id_b64 = "AAAAAAAAAAA=".to_string();
        cfg.consoles.push(Ps5ConsoleEntry {
            id: "mac:aabbccddeeff".to_string(),
            nickname: "Living Room PS5 Pro".to_string(),
            host: "192.168.1.55".to_string(),
            mac: "aa:bb:cc:dd:ee:ff".to_string(),
            regist_key_hex: "31323334353637380000000000000000".to_string(),
            rp_key_hex: "00112233445566778899aabbccddeeff".to_string(),
            psn_duid_hex: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
            console_pin: "1234".to_string(),
            preferred_mode: Ps5ConnectionMode::Auto,
        });

        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("ps5_cfg.toml");
        confy::store_path(&path, &cfg).expect("Ps5Config must serialize cleanly to TOML");
        let parsed: Ps5Config =
            confy::load_path(&path).expect("Ps5Config must deserialize cleanly from TOML");
        assert_eq!(cfg, parsed);

        // Also test full MichadameConfig round-trip with populated Ps5Config + ps5_profile
        let mut full = crate::config::MichadameConfig::default();
        full.ps5 = cfg.clone();
        let mut ps5_prof = crate::config::default_ps5_profile();
        ps5_prof.crt_filter = Some(2);
        ps5_prof.brightness = Some(1.15);
        full.ps5_profile = Some(ps5_prof.clone());
        let full_path = tmp.path().join("michadame_ps5.toml");
        confy::store_path(&full_path, &full)
            .expect("MichadameConfig with PS5 consoles must serialize without ValueAfterTable");
        let loaded_full: crate::config::MichadameConfig =
            confy::load_path(&full_path).expect("MichadameConfig must deserialize cleanly");
        assert_eq!(loaded_full.ps5, cfg);
        assert_eq!(loaded_full.ps5_profile.unwrap().brightness, Some(1.15));
    }

    #[test]
    fn ps5_requires_dualsense_controller_with_exact_error_message() {
        let mut app = crate::app::AppState::default();
        app.ps5.test_controller_override = Some(false);
        app.ps5.config.consoles.push(Ps5ConsoleEntry {
            id: "mac:aabbccddeeff".to_string(),
            nickname: "My PS5".to_string(),
            host: "192.168.1.50".to_string(),
            mac: "aa:bb:cc:dd:ee:ff".to_string(),
            regist_key_hex: "31323334353637380000000000000000".to_string(),
            rp_key_hex: "00112233445566778899aabbccddeeff".to_string(),
            ..Default::default()
        });
        app.ps5.config.selected_console_id = "mac:aabbccddeeff".to_string();

        let err = app
            .validate_ps5_start()
            .expect_err("Must refuse to start without PS5 controller");
        assert_eq!(err, "connect a PS5 controller");

        // When DualSense is detected, validation succeeds (if backend is compiled)
        app.ps5.test_controller_override = Some(true);
        if ffi::is_backend_compiled() {
            let entry = app
                .validate_ps5_start()
                .expect("Validation should succeed when DualSense is connected and PS5 is paired");
            assert_eq!(entry.nickname, "My PS5");
        }
    }

    #[test]
    fn ps5_dedicated_profile_does_not_overwrite_normal_profile() {
        let mut app = crate::app::AppState::default();
        app.hardware.selected_resolution = (1280, 720);
        app.hardware.selected_framerate = 60;
        app.video.brightness = 1.0;

        // Enter simulated active PS5 mode with dedicated ps5_profile
        let current_normal = crate::config::build_profile_from_state(&app);
        app.profiles
            .insert(app.active_profile.clone(), current_normal);
        app.previous_ps5_normal_profile = Some(app.active_profile.clone());
        app.previous_capture_resolution = app.hardware.selected_resolution;
        app.previous_capture_framerate = app.hardware.selected_framerate;
        let ps5_prof = app.ps5_profile.clone();
        crate::config::apply_profile_to_state(&mut app, &ps5_prof);
        app.ps5.is_active = true;
        app.hardware.selected_resolution = (1920, 1080);

        // Adjust shader & brightness while in PS5 mode
        app.video.brightness = 1.35;
        app.crt_filter
            .store(2, std::sync::atomic::Ordering::Relaxed);
        crate::config::sync_active_profile_in_state(&mut app);

        assert_eq!(app.ps5_profile.brightness, Some(1.35));
        assert_eq!(app.ps5_profile.crt_filter, Some(2));
        // Normal profile must remain untouched
        assert_eq!(
            app.profiles.get("Default").and_then(|p| p.brightness),
            Some(1.0)
        );

        // Stop PS5 mode and verify normal profile + capture resolution are restored
        app.stop_ps5_mode();
        assert!(!app.ps5.is_active);
        assert_eq!(app.hardware.selected_resolution, (1280, 720));
        assert!((app.video.brightness - 1.0).abs() < 1e-4);
        assert_eq!(app.ps5_profile.brightness, Some(1.35));
    }

    #[test]
    fn dualsense_vid_pid_filter_is_strictly_ps5_only() {
        // DualSense & DualSense Edge MUST pass
        assert!(is_supported_ps5_controller_vid_pid(0x054c, 0x0ce6));
        assert!(is_supported_ps5_controller_vid_pid(0x054c, 0x0df2));

        // DualShock 4, Xbox, Nintendo, Valve Steam Virtual, zero MUST be rejected
        assert!(!is_supported_ps5_controller_vid_pid(0x054c, 0x05c4)); // DS4 v1
        assert!(!is_supported_ps5_controller_vid_pid(0x054c, 0x09cc)); // DS4 v2
        assert!(!is_supported_ps5_controller_vid_pid(0x045e, 0x02ea)); // Xbox One
        assert!(!is_supported_ps5_controller_vid_pid(0x045e, 0x0b12)); // Xbox Series
        assert!(!is_supported_ps5_controller_vid_pid(0x057e, 0x2009)); // Switch Pro
        assert!(!is_supported_ps5_controller_vid_pid(0x28de, 0x11ff)); // Steam Virtual
        assert!(!is_supported_ps5_controller_vid_pid(0x0000, 0x0000));
    }

    #[test]
    fn psn_oauth_url_and_code_extraction() {
        let duid = "0000000700410080deadbeefcafebabe0123456789abcdef";
        let url = psn::build_psn_login_url(duid);
        assert!(url.contains("auth.api.sonyentertainmentnetwork.com/2.0/oauth/authorize"));
        assert!(url.contains(&format!("duid={duid}")));
        assert!(url.contains("smcid=remoteplay"));

        assert_eq!(
            psn::extract_redirect_code(
                "https://remoteplay.dl.playstation.net/remoteplay/redirect?code=v3.ABC_123&cid=xyz"
            )
            .as_deref(),
            Some("v3.ABC_123")
        );
        assert_eq!(
            psn::extract_redirect_code("code=v3.XYZ999&state=foo").as_deref(),
            Some("v3.XYZ999")
        );
        assert_eq!(
            psn::extract_redirect_code("  v3.RAWCODE  ").as_deref(),
            Some("v3.RAWCODE")
        );
        assert!(psn::extract_redirect_code("   ").is_none());
        assert!(psn::extract_redirect_code("https://example.com/no-code-here").is_none());
    }

    #[test]
    fn psn_user_id_to_account_id_b64_roundtrip() {
        let user_id = "1234567890123456789";
        let b64 = psn::user_id_to_account_id_b64(user_id).expect("valid numeric user_id");
        let bytes = psn::decode_account_id_b64(&b64).expect("decodes to 8 bytes");
        assert_eq!(u64::from_le_bytes(bytes), 1234567890123456789u64);

        assert!(psn::decode_account_id_b64("AAAA").is_err()); // only 3 bytes
        assert!(psn::user_id_to_account_id_b64("not_a_number").is_err());
    }

    #[test]
    fn psn_token_file_roundtrip_and_expiry_checks() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("psn_token.json");
        let tok = StoredPsnToken {
            access_token: "acc-123".to_string(),
            refresh_token: "ref-456".to_string(),
            expires_at_unix: psn::unix_now_secs() + 3600,
            account_id_b64: "AQAAAAAAAAA=".to_string(),
            user_id: "1".to_string(),
            online_id: "TestPlayer".to_string(),
            client_duid: "0000000700410080".to_string(),
        };
        assert!(tok.is_signed_in());
        assert!(!tok.needs_refresh());

        psn::save_stored_token_at(&path, &tok).expect("save token");
        let loaded = psn::load_stored_token_at(&path).expect("load token");
        assert_eq!(loaded, tok);

        let expired = StoredPsnToken {
            expires_at_unix: psn::unix_now_secs().saturating_sub(10),
            ..tok
        };
        assert!(expired.needs_refresh());
    }

    #[test]
    fn console_upsert_and_dropdown_merging() {
        let mut state = Ps5State::default();
        let data = RegisteredConsoleData {
            server_mac: [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff],
            server_nickname: "PS5 Pro".to_string(),
            rp_regist_key: *b"12345678\0\0\0\0\0\0\0\0",
            rp_key: [0x11; 16],
            rp_key_type: 2,
            console_pin: 4321,
        };
        let id = state.upsert_registered_console("192.168.1.42", "", "", &data);
        assert_eq!(state.config.consoles.len(), 1);
        assert_eq!(state.config.selected_console_id, id);
        assert!(state.config.consoles[0].has_local_registration());
        assert_eq!(state.config.consoles[0].console_pin, "4321");

        // Merge PSN device with same nickname updates psn_duid_hex without duplicating
        state.upsert_psn_devices_into_consoles(&[PsnRemoteDeviceInfo {
            device_name: "PS5 Pro".to_string(),
            device_uid_hex: "deadbeef".to_string(),
            remoteplay_enabled: true,
        }]);
        assert_eq!(state.config.consoles.len(), 1);
        assert!(state.config.consoles[0].has_psn_target());

        // Merge LAN discovery updates DHCP-changed IP address
        state.merge_discovered_hosts_into_consoles(&[DiscoveredHostInfo {
            state: Ps5HostPowerState::Ready,
            host_request_port: 9302,
            host_addr: "192.168.1.99".to_string(),
            host_name: "PS5 Pro".to_string(),
            host_id: "AABBCCDDEEFF".to_string(),
            host_type: "PS5".to_string(),
            system_version: "10.00".to_string(),
            running_app_titleid: String::new(),
            running_app_name: String::new(),
        }]);
        assert_eq!(state.config.consoles[0].host, "192.168.1.99");
    }

    #[cfg(feature = "ps5-remote-play")]
    #[test]
    fn libchiaki_ffi_init_and_duid_smoke_test() {
        ffi::init_lib().expect("libchiaki should initialize");
        let duid = ffi::generate_client_duid().expect("DUID should generate");
        assert!(duid.len() >= 32, "DUID too short: {duid}");
        assert!(duid.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn dualsense_monitor_starts_polls_and_drops_cleanly() {
        let monitor = DualSenseMonitor::start(None);
        std::thread::sleep(std::time::Duration::from_millis(30));
        let status = monitor.status();
        // If connected on this machine it must be a Sony DualSense / Edge; otherwise disconnected.
        if status.connected {
            assert!(is_supported_ps5_controller_vid_pid(
                status.vendor_id,
                status.product_id
            ));
        }
        drop(monitor);
    }

    #[test]
    fn ps5_keyboard_and_login_pin_event_state_machine() {
        let mut app = crate::app::AppState::default();
        let (ev_tx, ev_rx) = crossbeam_channel::unbounded();
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        app.ps5.is_active = true;
        app.ps5.connected_console_id = "ps5:test".to_string();
        app.ps5.config.consoles.push(Ps5ConsoleEntry {
            id: "ps5:test".to_string(),
            nickname: "Test PS5".to_string(),
            console_pin: "2468".to_string(),
            ..Default::default()
        });
        app.ps5.event_rx = Some(ev_rx);
        app.ps5.command_tx = Some(cmd_tx);

        let ctx = eframe::egui::Context::default();

        // Keyboard open -> text update -> remote close
        ev_tx
            .send(Ps5SessionEvent::KeyboardOpen("hello".to_string()))
            .unwrap();
        assert!(app.handle_ps5_events(&ctx));
        assert!(app.ps5.keyboard_prompt_open);
        assert_eq!(app.ps5.keyboard_text, "hello");
        assert!(app.ps5.is_modal_prompt_open());

        ev_tx
            .send(Ps5SessionEvent::KeyboardTextChange(
                "hello world".to_string(),
            ))
            .unwrap();
        assert!(app.handle_ps5_events(&ctx));
        assert_eq!(app.ps5.keyboard_text, "hello world");

        ev_tx.send(Ps5SessionEvent::KeyboardRemoteClose).unwrap();
        assert!(app.handle_ps5_events(&ctx));
        assert!(!app.ps5.keyboard_prompt_open);
        assert!(!app.ps5.is_modal_prompt_open());

        // First LoginPinRequested auto-submits saved 4-digit console_pin
        ev_tx
            .send(Ps5SessionEvent::LoginPinRequested { incorrect: false })
            .unwrap();
        assert!(app.handle_ps5_events(&ctx));
        assert!(!app.ps5.login_pin_prompt_open);
        match cmd_rx.try_recv().expect("auto-submitted PIN command") {
            Ps5Command::SetLoginPin(pin) => assert_eq!(pin, "2468"),
            other => panic!("unexpected command: {other:?}"),
        }

        // Subsequent incorrect PIN opens modal prompt
        ev_tx
            .send(Ps5SessionEvent::LoginPinRequested { incorrect: true })
            .unwrap();
        assert!(app.handle_ps5_events(&ctx));
        assert!(app.ps5.login_pin_prompt_open);
        assert!(app.ps5.login_pin_incorrect);
        assert!(app.ps5.is_modal_prompt_open());
    }

    #[test]
    fn ps5_streaming_presets_and_bitrate_clamping() {
        assert_eq!(Ps5Resolution::P360.dimensions(), (640, 360));
        assert_eq!(Ps5Resolution::P540.dimensions(), (960, 540));
        assert_eq!(Ps5Resolution::P720.dimensions(), (1280, 720));
        assert_eq!(Ps5Resolution::P1080.dimensions(), (1920, 1080));
        assert_eq!(Ps5Fps::Fps30.as_u32(), 30);
        assert_eq!(Ps5Fps::Fps60.as_u32(), 60);

        let mut cfg = Ps5Config::default();
        cfg.bitrate_kbps = 100;
        assert_eq!(cfg.clamped_bitrate_kbps(), PS5_MIN_BITRATE_KBPS);
        cfg.bitrate_kbps = 999_999;
        assert_eq!(cfg.clamped_bitrate_kbps(), PS5_MAX_BITRATE_KBPS);
    }

    #[test]
    fn ps5_protected_content_cant_display_manual_and_auto_go_home() {
        let mut app = crate::app::AppState::default();
        let (ev_tx, ev_rx) = crossbeam_channel::unbounded();
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        app.ps5.is_active = true;
        app.ps5.event_rx = Some(ev_rx);
        app.ps5.command_tx = Some(cmd_tx);
        let ctx = eframe::egui::Context::default();

        // Default (auto_go_home_on_cant_display = false): shows prompt without auto-sending GoHome
        assert!(!app.ps5.config.auto_go_home_on_cant_display);
        ev_tx.send(Ps5SessionEvent::CantDisplay(true)).unwrap();
        assert!(app.handle_ps5_events(&ctx));
        assert!(app.ps5.cant_display_warning);
        assert!(cmd_rx.try_recv().is_err());

        // Clear protected state
        ev_tx.send(Ps5SessionEvent::CantDisplay(false)).unwrap();
        assert!(app.handle_ps5_events(&ctx));
        assert!(!app.ps5.cant_display_warning);

        // Enable auto_go_home_on_cant_display: CantDisplay(true) triggers GoHome once per edge
        app.ps5.config.auto_go_home_on_cant_display = true;
        ev_tx.send(Ps5SessionEvent::CantDisplay(true)).unwrap();
        assert!(app.handle_ps5_events(&ctx));
        assert!(app.ps5.cant_display_warning);
        assert!(matches!(cmd_rx.try_recv(), Ok(Ps5Command::GoHome)));

        // Repeated CantDisplay(true) while already blocked must not spam GoHome
        ev_tx.send(Ps5SessionEvent::CantDisplay(true)).unwrap();
        assert!(app.handle_ps5_events(&ctx));
        assert!(cmd_rx.try_recv().is_err());

        // Returning to streamable screen re-arms auto-return for future protected screens
        ev_tx.send(Ps5SessionEvent::CantDisplay(false)).unwrap();
        assert!(app.handle_ps5_events(&ctx));
        ev_tx.send(Ps5SessionEvent::CantDisplay(true)).unwrap();
        assert!(app.handle_ps5_events(&ctx));
        assert!(matches!(cmd_rx.try_recv(), Ok(Ps5Command::GoHome)));
    }
}
