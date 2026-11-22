//! Safe Rust wrappers around `chiaki_bridge.h` (`libchiaki` + SDL2 DualSense).

use anyhow::{anyhow, Result};
#[cfg(feature = "ps5-remote-play")]
use std::ffi::CString;
use std::ffi::{c_char, c_int, c_void, CStr};

pub const EV_CONNECTED: i32 = 1;
pub const EV_QUIT: i32 = 2;
pub const EV_LOGIN_PIN_REQUEST: i32 = 3;
pub const EV_REGIST: i32 = 4;
pub const EV_NICKNAME: i32 = 5;
pub const EV_KEYBOARD_OPEN: i32 = 6;
pub const EV_KEYBOARD_TEXT_CHANGE: i32 = 7;
pub const EV_KEYBOARD_REMOTE_CLOSE: i32 = 8;
pub const EV_RUMBLE: i32 = 9;
pub const EV_TRIGGER_EFFECTS: i32 = 10;
pub const EV_LED_COLOR: i32 = 11;
pub const EV_PLAYER_INDEX: i32 = 12;
pub const EV_HAPTIC_INTENSITY: i32 = 13;
pub const EV_TRIGGER_INTENSITY: i32 = 14;
pub const EV_MOTION_RESET: i32 = 15;
pub const EV_HOLEPUNCH: i32 = 16;
pub const EV_FEC_FAILURE: i32 = 17;
pub const EV_CANT_DISPLAY: i32 = 18;

#[repr(C)]
pub struct MichadamePs5Session {
    _private: [u8; 0],
}

#[repr(C)]
pub struct MichadameDualSense {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawDiscoveredHost {
    pub state: c_int,
    pub host_request_port: u16,
    pub host_addr: [c_char; 64],
    pub host_name: [c_char; 128],
    pub host_id: [c_char; 32],
    pub host_type: [c_char; 32],
    pub system_version: [c_char; 32],
    pub running_app_titleid: [c_char; 64],
    pub running_app_name: [c_char; 128],
    pub is_ps5: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawPsnDevice {
    pub device_name: [c_char; 64],
    pub device_uid_hex: [c_char; 65],
    pub remoteplay_enabled: bool,
    pub is_ps5: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawRegistResult {
    pub server_mac: [u8; 6],
    pub server_nickname: [c_char; 64],
    pub rp_regist_key: [u8; 16],
    pub rp_key: [u8; 16],
    pub rp_key_type: u32,
    pub console_pin: u32,
    pub target: c_int,
}

impl Default for RawRegistResult {
    fn default() -> Self {
        Self {
            server_mac: [0; 6],
            server_nickname: [0; 64],
            rp_regist_key: [0; 16],
            rp_key: [0; 16],
            rp_key_type: 0,
            console_pin: 0,
            target: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawDualSenseInfo {
    pub connected: bool,
    pub is_edge: bool,
    pub is_wireless: bool,
    pub haptics_audio_connected: bool,
    pub vendor_id: u16,
    pub product_id: u16,
    pub name: [c_char; 128],
}

impl Default for RawDualSenseInfo {
    fn default() -> Self {
        Self {
            connected: false,
            is_edge: false,
            is_wireless: false,
            haptics_audio_connected: false,
            vendor_id: 0,
            product_id: 0,
            name: [0; 128],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawPs5Event {
    pub r#type: c_int,
    pub quit_reason: c_int,
    pub quit_is_error: bool,
    pub flag: bool,
    pub rumble_left: u8,
    pub rumble_right: u8,
    pub trigger_type_left: u8,
    pub trigger_type_right: u8,
    pub trigger_left: [u8; 10],
    pub trigger_right: [u8; 10],
    pub led_color: [u8; 3],
    pub player_index: u8,
    pub intensity: c_int,
    pub frame_index: i32,
    pub text: *const c_char,
    pub regist: RawRegistResult,
}

pub type VideoCb = unsafe extern "C" fn(*const u8, usize, i32, bool, *mut c_void) -> bool;
pub type AudioSettingsCb = unsafe extern "C" fn(u32, u32, *mut c_void);
pub type AudioFrameCb = unsafe extern "C" fn(*const i16, usize, *mut c_void);
pub type HapticsFrameCb = unsafe extern "C" fn(*const u8, usize, *mut c_void);
pub type EventCb = unsafe extern "C" fn(*const RawPs5Event, *mut c_void);
pub type LogCb = unsafe extern "C" fn(c_int, *const c_char, *mut c_void);
pub type DiscoveredHostCb = unsafe extern "C" fn(*const RawDiscoveredHost, *mut c_void);
pub type PsnDeviceCb = unsafe extern "C" fn(*const RawPsnDevice, *mut c_void);

#[repr(C)]
pub struct RawSessionConfig {
    pub host: *const c_char,
    pub regist_key: [u8; 16],
    pub morning: [u8; 16],
    pub psn_oauth_token: *const c_char,
    pub psn_duid_hex: *const c_char,
    pub psn_account_id: [u8; 8],
    pub auto_regist: bool,
    pub width: u32,
    pub height: u32,
    pub max_fps: u32,
    pub bitrate_kbps: u32,
    pub codec: c_int,
    pub enable_keyboard: bool,
    pub enable_dualsense: bool,
    pub video_cb: Option<VideoCb>,
    pub audio_settings_cb: Option<AudioSettingsCb>,
    pub audio_frame_cb: Option<AudioFrameCb>,
    pub haptics_frame_cb: Option<HapticsFrameCb>,
    pub event_cb: Option<EventCb>,
    pub log_cb: Option<LogCb>,
    pub user: *mut c_void,
}

#[cfg(feature = "ps5-remote-play")]
extern "C" {
    pub fn michadame_ps5_lib_init() -> c_int;
    pub fn michadame_ps5_generate_client_duid(out_buf: *mut c_char, out_cap: usize) -> c_int;
    pub fn michadame_ps5_discover_once(
        timeout_ms: u32,
        cb: Option<DiscoveredHostCb>,
        user: *mut c_void,
    ) -> c_int;
    pub fn michadame_ps5_wakeup(host: *const c_char, regist_key: *const u8) -> c_int;
    pub fn michadame_ps5_psn_list_devices(
        psn_oauth_token: *const c_char,
        cb: Option<PsnDeviceCb>,
        user: *mut c_void,
        err_buf: *mut c_char,
        err_cap: usize,
    ) -> c_int;
    pub fn michadame_ps5_register_local(
        host: *const c_char,
        broadcast: bool,
        psn_account_id: *const u8,
        pin: u32,
        console_pin: u32,
        out_result: *mut RawRegistResult,
        err_buf: *mut c_char,
        err_cap: usize,
    ) -> c_int;
    pub fn michadame_ps5_session_create(
        cfg: *const RawSessionConfig,
        err_buf: *mut c_char,
        err_cap: usize,
    ) -> *mut MichadamePs5Session;
    pub fn michadame_ps5_session_connect_psn_and_start(
        handle: *mut MichadamePs5Session,
        psn_duid_hex: *const c_char,
        err_buf: *mut c_char,
        err_cap: usize,
    ) -> c_int;
    pub fn michadame_ps5_session_start_local(
        handle: *mut MichadamePs5Session,
        err_buf: *mut c_char,
        err_cap: usize,
    ) -> c_int;
    pub fn michadame_ps5_session_stop(handle: *mut MichadamePs5Session);
    pub fn michadame_ps5_session_destroy(handle: *mut MichadamePs5Session);
    pub fn michadame_ps5_session_set_login_pin(
        handle: *mut MichadamePs5Session,
        pin: *const c_char,
    ) -> c_int;
    pub fn michadame_ps5_session_keyboard_set_text(
        handle: *mut MichadamePs5Session,
        text: *const c_char,
    ) -> c_int;
    pub fn michadame_ps5_session_keyboard_accept(handle: *mut MichadamePs5Session) -> c_int;
    pub fn michadame_ps5_session_keyboard_reject(handle: *mut MichadamePs5Session) -> c_int;
    pub fn michadame_ps5_session_go_home(handle: *mut MichadamePs5Session) -> c_int;
    pub fn michadame_ps5_session_goto_bed(handle: *mut MichadamePs5Session) -> c_int;
    pub fn michadame_ps5_session_request_idr(handle: *mut MichadamePs5Session) -> c_int;

    pub fn michadame_ds_create() -> *mut MichadameDualSense;
    pub fn michadame_ds_destroy(ds: *mut MichadameDualSense);
    pub fn michadame_ds_poll(
        ds: *mut MichadameDualSense,
        active_session: *mut MichadamePs5Session,
        out_info: *mut RawDualSenseInfo,
    );
    pub fn michadame_ds_set_rumble(ds: *mut MichadameDualSense, left: u8, right: u8);
    pub fn michadame_ds_set_trigger_effects(
        ds: *mut MichadameDualSense,
        type_left: u8,
        left: *const u8,
        type_right: u8,
        right: *const u8,
    );
    pub fn michadame_ds_set_led(ds: *mut MichadameDualSense, r: u8, g: u8, b: u8);
    pub fn michadame_ds_set_player_index(ds: *mut MichadameDualSense, player_index: u8);
    pub fn michadame_ds_set_haptic_intensity(ds: *mut MichadameDualSense, intensity: c_int);
    pub fn michadame_ds_set_trigger_intensity(ds: *mut MichadameDualSense, intensity: c_int);
    pub fn michadame_ds_reset_motion(ds: *mut MichadameDualSense);
    pub fn michadame_ds_clear_effects(ds: *mut MichadameDualSense);
    pub fn michadame_ds_start_haptics(ds: *mut MichadameDualSense);
    pub fn michadame_ds_stop_haptics(ds: *mut MichadameDualSense);
    pub fn michadame_ds_push_haptics(ds: *mut MichadameDualSense, buf: *const u8, buf_size: usize);
}

pub fn is_backend_compiled() -> bool {
    cfg!(feature = "ps5-remote-play")
}

pub fn c_char_buf_to_string(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub unsafe fn c_ptr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

pub fn format_mac(mac: &[u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

pub fn init_lib() -> Result<()> {
    #[cfg(feature = "ps5-remote-play")]
    {
        let rc = unsafe { michadame_ps5_lib_init() };
        if rc == 0 {
            Ok(())
        } else {
            Err(anyhow!("libchiaki init failed with code {rc}"))
        }
    }
    #[cfg(not(feature = "ps5-remote-play"))]
    {
        Err(anyhow!(
            "PS5 Remote Play support was disabled at compile time (build without --no-default-features)."
        ))
    }
}

pub fn generate_client_duid() -> Result<String> {
    #[cfg(feature = "ps5-remote-play")]
    {
        let mut buf = [0 as c_char; 64];
        let rc = unsafe { michadame_ps5_generate_client_duid(buf.as_mut_ptr(), buf.len()) };
        if rc == 0 {
            Ok(c_char_buf_to_string(&buf))
        } else {
            Err(anyhow!("Failed to generate PSN client DUID (code {rc})"))
        }
    }
    #[cfg(not(feature = "ps5-remote-play"))]
    {
        // Deterministic-format fallback DUID (prefix 0000000700410080 + 16 random bytes hex)
        use rand::RngCore;
        let mut random = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut random);
        let mut out = String::from("0000000700410080");
        for b in random {
            out.push_str(&format!("{b:02x}"));
        }
        Ok(out)
    }
}

#[derive(Clone, Debug)]
pub struct DiscoveredHostInfo {
    pub state: super::Ps5HostPowerState,
    pub host_request_port: u16,
    pub host_addr: String,
    pub host_name: String,
    pub host_id: String,
    pub host_type: String,
    pub system_version: String,
    pub running_app_titleid: String,
    pub running_app_name: String,
}

pub fn discover_local_ps5s(timeout_ms: u32) -> Result<Vec<DiscoveredHostInfo>> {
    #[cfg(feature = "ps5-remote-play")]
    {
        unsafe extern "C" fn collect(host: *const RawDiscoveredHost, user: *mut c_void) {
            if host.is_null() || user.is_null() {
                return;
            }
            let h = &*host;
            if !h.is_ps5 {
                return;
            }
            let list = &mut *(user as *mut Vec<DiscoveredHostInfo>);
            let state = match h.state {
                1 => super::Ps5HostPowerState::Ready,
                2 => super::Ps5HostPowerState::Standby,
                _ => super::Ps5HostPowerState::Unknown,
            };
            list.push(DiscoveredHostInfo {
                state,
                host_request_port: h.host_request_port,
                host_addr: c_char_buf_to_string(&h.host_addr),
                host_name: c_char_buf_to_string(&h.host_name),
                host_id: c_char_buf_to_string(&h.host_id),
                host_type: c_char_buf_to_string(&h.host_type),
                system_version: c_char_buf_to_string(&h.system_version),
                running_app_titleid: c_char_buf_to_string(&h.running_app_titleid),
                running_app_name: c_char_buf_to_string(&h.running_app_name),
            });
        }

        let mut results: Vec<DiscoveredHostInfo> = Vec::new();
        let rc = unsafe {
            michadame_ps5_discover_once(
                timeout_ms,
                Some(collect),
                (&mut results as *mut Vec<DiscoveredHostInfo>).cast::<c_void>(),
            )
        };
        if rc != 0 {
            return Err(anyhow!("PS5 LAN discovery failed (code {rc})"));
        }
        Ok(results)
    }
    #[cfg(not(feature = "ps5-remote-play"))]
    {
        let _ = timeout_ms;
        Err(anyhow!(
            "PS5 Remote Play was disabled at compile time (ps5-remote-play feature off)."
        ))
    }
}

pub fn send_wakeup(host: &str, regist_key: &[u8; 16]) -> Result<()> {
    #[cfg(feature = "ps5-remote-play")]
    {
        let c_host = CString::new(host).map_err(|_| anyhow!("Invalid host string"))?;
        let rc = unsafe { michadame_ps5_wakeup(c_host.as_ptr(), regist_key.as_ptr()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(anyhow!("Failed to send PS5 wakeup packet (code {rc})"))
        }
    }
    #[cfg(not(feature = "ps5-remote-play"))]
    {
        let _ = (host, regist_key);
        Err(anyhow!("PS5 Remote Play was disabled at compile time."))
    }
}

#[derive(Clone, Debug)]
pub struct PsnRemoteDeviceInfo {
    pub device_name: String,
    pub device_uid_hex: String,
    pub remoteplay_enabled: bool,
}

pub fn list_psn_ps5_devices(oauth_token: &str) -> Result<Vec<PsnRemoteDeviceInfo>> {
    #[cfg(feature = "ps5-remote-play")]
    {
        unsafe extern "C" fn collect(dev: *const RawPsnDevice, user: *mut c_void) {
            if dev.is_null() || user.is_null() {
                return;
            }
            let d = &*dev;
            if !d.is_ps5 {
                return;
            }
            let list = &mut *(user as *mut Vec<PsnRemoteDeviceInfo>);
            list.push(PsnRemoteDeviceInfo {
                device_name: c_char_buf_to_string(&d.device_name),
                device_uid_hex: c_char_buf_to_string(&d.device_uid_hex),
                remoteplay_enabled: d.remoteplay_enabled,
            });
        }

        let c_tok = CString::new(oauth_token).map_err(|_| anyhow!("Invalid PSN OAuth token"))?;
        let mut results: Vec<PsnRemoteDeviceInfo> = Vec::new();
        let mut err_buf = [0 as c_char; 256];
        let rc = unsafe {
            michadame_ps5_psn_list_devices(
                c_tok.as_ptr(),
                Some(collect),
                (&mut results as *mut Vec<PsnRemoteDeviceInfo>).cast::<c_void>(),
                err_buf.as_mut_ptr(),
                err_buf.len(),
            )
        };
        if rc != 0 {
            let msg = c_char_buf_to_string(&err_buf);
            return Err(anyhow!(if msg.is_empty() {
                format!("PSN device list failed (code {rc})")
            } else {
                msg
            }));
        }
        Ok(results)
    }
    #[cfg(not(feature = "ps5-remote-play"))]
    {
        let _ = oauth_token;
        Err(anyhow!("PS5 Remote Play was disabled at compile time."))
    }
}

#[derive(Clone, Debug)]
pub struct RegisteredConsoleData {
    pub server_mac: [u8; 6],
    pub server_nickname: String,
    pub rp_regist_key: [u8; 16],
    pub rp_key: [u8; 16],
    pub rp_key_type: u32,
    pub console_pin: u32,
}

impl From<&RawRegistResult> for RegisteredConsoleData {
    fn from(raw: &RawRegistResult) -> Self {
        Self {
            server_mac: raw.server_mac,
            server_nickname: c_char_buf_to_string(&raw.server_nickname),
            rp_regist_key: raw.rp_regist_key,
            rp_key: raw.rp_key,
            rp_key_type: raw.rp_key_type,
            console_pin: raw.console_pin,
        }
    }
}

pub fn register_local_ps5(
    host: &str,
    broadcast: bool,
    psn_account_id: &[u8; 8],
    pin: u32,
    console_pin: u32,
) -> Result<RegisteredConsoleData> {
    #[cfg(feature = "ps5-remote-play")]
    {
        let c_host = CString::new(host).map_err(|_| anyhow!("Invalid PS5 host string"))?;
        let mut raw = RawRegistResult::default();
        let mut err_buf = [0 as c_char; 256];
        let rc = unsafe {
            michadame_ps5_register_local(
                c_host.as_ptr(),
                broadcast,
                psn_account_id.as_ptr(),
                pin,
                console_pin,
                &mut raw,
                err_buf.as_mut_ptr(),
                err_buf.len(),
            )
        };
        if rc != 0 {
            let msg = c_char_buf_to_string(&err_buf);
            return Err(anyhow!(if msg.is_empty() {
                format!("PS5 registration failed (code {rc})")
            } else {
                msg
            }));
        }
        Ok(RegisteredConsoleData::from(&raw))
    }
    #[cfg(not(feature = "ps5-remote-play"))]
    {
        let _ = (host, broadcast, psn_account_id, pin, console_pin);
        Err(anyhow!("PS5 Remote Play was disabled at compile time."))
    }
}
