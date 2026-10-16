use std::ffi::{c_char, c_int, c_ulong, c_void, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[repr(C)]
struct CGamescopeX11CursorMonitor {
    _unused: [u8; 0],
}

#[repr(C)]
pub struct CGamescopeX11CursorImage {
    pub width: u16,
    pub height: u16,
    pub xhot: u16,
    pub yhot: u16,
    pub cursor_serial: c_ulong,
    pub rgba_pixels: *const u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameCursorImage {
    pub width: u32,
    pub height: u32,
    pub xhot: u32,
    pub yhot: u32,
    pub cursor_serial: u64,
    pub rgba_pixels: Vec<u8>,
}

type CGamescopeX11CursorCb = unsafe extern "C" fn(
    is_hidden: c_int,
    image: *const CGamescopeX11CursorImage,
    user_data: *mut c_void,
);

extern "C" {
    fn gamescope_x11_cursor_monitor_start(
        display_name: *const c_char,
        cb: CGamescopeX11CursorCb,
        user_data: *mut c_void,
    ) -> *mut CGamescopeX11CursorMonitor;

    fn gamescope_x11_cursor_monitor_stop(monitor: *mut CGamescopeX11CursorMonitor);
}

struct CursorMonitorShared {
    is_cursor_hidden: AtomicBool,
    cursor_image: Mutex<Option<Arc<GameCursorImage>>>,
}

pub struct GamescopeX11CursorMonitor {
    monitor: *mut CGamescopeX11CursorMonitor,
    shared: Arc<CursorMonitorShared>,
    _callback_data: Box<Arc<CursorMonitorShared>>,
}

unsafe impl Send for GamescopeX11CursorMonitor {}
unsafe impl Sync for GamescopeX11CursorMonitor {}

unsafe extern "C" fn on_cursor_changed(
    is_hidden: c_int,
    image: *const CGamescopeX11CursorImage,
    user_data: *mut c_void,
) {
    if user_data.is_null() {
        return;
    }
    let shared = &*(user_data as *const Arc<CursorMonitorShared>);
    let hidden = is_hidden != 0;
    shared.is_cursor_hidden.store(hidden, Ordering::Release);

    if hidden {
        let mut guard = shared.cursor_image.lock().unwrap();
        *guard = None;
        tracing::info!("Gamescope X11 cursor visibility: hidden=true");
    } else if !image.is_null() {
        let img = &*image;
        if img.width > 0 && img.height > 0 && !img.rgba_pixels.is_null() {
            let count = (img.width as usize) * (img.height as usize) * 4;
            let slice = std::slice::from_raw_parts(img.rgba_pixels, count);
            let game_img = Arc::new(GameCursorImage {
                width: img.width as u32,
                height: img.height as u32,
                xhot: img.xhot as u32,
                yhot: img.yhot as u32,
                cursor_serial: img.cursor_serial as u64,
                rgba_pixels: slice.to_vec(),
            });
            let mut guard = shared.cursor_image.lock().unwrap();
            *guard = Some(game_img);
            tracing::info!(
                "Gamescope X11 cursor visibility: hidden=false, serial={}, size={}x{}, hotspot=({},{})",
                img.cursor_serial,
                img.width,
                img.height,
                img.xhot,
                img.yhot
            );
        }
    }
}

impl Drop for GamescopeX11CursorMonitor {
    fn drop(&mut self) {
        if !self.monitor.is_null() {
            unsafe {
                gamescope_x11_cursor_monitor_stop(self.monitor);
            }
            self.monitor = std::ptr::null_mut();
        }
    }
}

impl GamescopeX11CursorMonitor {
    pub fn start(display_name: &str) -> Option<Self> {
        let shared = Arc::new(CursorMonitorShared {
            is_cursor_hidden: AtomicBool::new(false),
            cursor_image: Mutex::new(None),
        });
        let callback_data = Box::new(shared.clone());

        let c_display = CString::new(display_name).ok()?;
        let monitor = unsafe {
            gamescope_x11_cursor_monitor_start(
                c_display.as_ptr(),
                on_cursor_changed,
                &*callback_data as *const Arc<CursorMonitorShared> as *mut c_void,
            )
        };

        if monitor.is_null() {
            None
        } else {
            Some(Self {
                monitor,
                shared,
                _callback_data: callback_data,
            })
        }
    }

    pub fn is_cursor_hidden(&self) -> bool {
        self.shared.is_cursor_hidden.load(Ordering::Acquire)
    }

    pub fn cursor_image(&self) -> Option<Arc<GameCursorImage>> {
        self.shared.cursor_image.lock().unwrap().clone()
    }

    #[cfg(test)]
    pub fn dummy_for_test(hidden: bool, image: Option<GameCursorImage>) -> Self {
        let shared = Arc::new(CursorMonitorShared {
            is_cursor_hidden: AtomicBool::new(hidden),
            cursor_image: Mutex::new(image.map(Arc::new)),
        });
        Self {
            monitor: std::ptr::null_mut(),
            shared: shared.clone(),
            _callback_data: Box::new(shared),
        }
    }

    #[cfg(test)]
    pub fn set_hidden_for_test(&self, hidden: bool) {
        self.shared.is_cursor_hidden.store(hidden, Ordering::Release);
    }

    #[cfg(test)]
    pub fn set_image_for_test(&self, image: Option<GameCursorImage>) {
        *self.shared.cursor_image.lock().unwrap() = image.map(Arc::new);
    }
}

/// Discovers the X11 display number allocated by Gamescope for its nested Xwayland instance.
pub fn find_gamescope_x_display() -> Option<String> {
    // 1. Explicit override via environment
    if let Ok(disp) = std::env::var("MICHADAME_GAMESCOPE_DISPLAY") {
        let s = disp.trim();
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    if let Ok(disp) = std::env::var("GAMESCOPE_DISPLAY") {
        let s = disp.trim();
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }

    // 2. Scan /proc for Xwayland processes whose parent process is gamescope
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let pid_str = file_name.to_string_lossy();
            if !pid_str.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }

            let proc_path = entry.path();
            let comm_path = proc_path.join("comm");
            let Ok(comm) = std::fs::read_to_string(&comm_path) else {
                continue;
            };
            if comm.trim() != "Xwayland" {
                continue;
            }

            // Check up to 4 ancestor levels in /proc/<pid>/stat for gamescope
            let Ok(mut curr_pid) = pid_str.parse::<u32>() else {
                continue;
            };
            let mut has_gamescope_ancestor = false;
            for _ in 0..4 {
                let stat_path = PathBuf::from(format!("/proc/{curr_pid}/stat"));
                let Ok(stat) = std::fs::read_to_string(&stat_path) else {
                    break;
                };
                let Some(rparen) = stat.rfind(')') else {
                    break;
                };
                let rest = &stat[rparen + 1..];
                let fields: Vec<&str> = rest.split_whitespace().collect();
                if fields.len() < 2 {
                    break;
                }
                let Ok(ppid) = fields[1].parse::<u32>() else {
                    break;
                };
                if ppid <= 1 {
                    break;
                }

                let parent_comm_path = PathBuf::from(format!("/proc/{ppid}/comm"));
                let is_parent_gamescope = std::fs::read_to_string(&parent_comm_path)
                    .map(|pcomm| pcomm.trim().contains("gamescope"))
                    .unwrap_or(false);

                let is_cmd_gamescope = if !is_parent_gamescope {
                    let parent_cmdline_path = PathBuf::from(format!("/proc/{ppid}/cmdline"));
                    std::fs::read(&parent_cmdline_path)
                        .map(|bytes| String::from_utf8_lossy(&bytes).contains("gamescope"))
                        .unwrap_or(false)
                } else {
                    true
                };

                if is_cmd_gamescope {
                    has_gamescope_ancestor = true;
                    break;
                }
                curr_pid = ppid;
            }

            if !has_gamescope_ancestor {
                continue;
            }

            // Extract display argument from Xwayland's cmdline
            let cmdline_path = proc_path.join("cmdline");
            if let Ok(bytes) = std::fs::read(&cmdline_path) {
                let parts: Vec<&[u8]> = bytes.split(|&b| b == 0).collect();
                for part in parts {
                    let s = String::from_utf8_lossy(part);
                    if s.starts_with(':') && !s[1..].is_empty() && s[1..].chars().all(|c| c.is_ascii_digit()) {
                        return Some(s.to_string());
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_gamescope_x_display_override() {
        std::env::set_var("MICHADAME_GAMESCOPE_DISPLAY", ":99");
        assert_eq!(find_gamescope_x_display(), Some(":99".to_string()));
        std::env::remove_var("MICHADAME_GAMESCOPE_DISPLAY");
    }

    #[test]
    fn test_c_cursor_image_layout() {
        assert_eq!(std::mem::size_of::<CGamescopeX11CursorImage>(), 24);
        assert_eq!(std::mem::align_of::<CGamescopeX11CursorImage>(), 8);
    }

    #[test]
    fn test_on_cursor_changed_callback() {
        let shared = Arc::new(CursorMonitorShared {
            is_cursor_hidden: AtomicBool::new(true),
            cursor_image: Mutex::new(None),
        });
        let user_data = &shared as *const Arc<CursorMonitorShared> as *mut c_void;

        let dummy_pixels = [255u8, 0, 0, 255, 0, 255, 0, 255]; // 2x1 pixels
        let c_img = CGamescopeX11CursorImage {
            width: 2,
            height: 1,
            xhot: 1,
            yhot: 0,
            cursor_serial: 42,
            rgba_pixels: dummy_pixels.as_ptr(),
        };

        // Notify visible
        unsafe {
            on_cursor_changed(0, &c_img, user_data);
        }

        assert!(!shared.is_cursor_hidden.load(Ordering::Acquire));
        let img = shared.cursor_image.lock().unwrap().clone().expect("cursor image");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 1);
        assert_eq!(img.xhot, 1);
        assert_eq!(img.yhot, 0);
        assert_eq!(img.cursor_serial, 42);
        assert_eq!(img.rgba_pixels, dummy_pixels.to_vec());

        // Notify hidden
        unsafe {
            on_cursor_changed(1, std::ptr::null(), user_data);
        }
        assert!(shared.is_cursor_hidden.load(Ordering::Acquire));
        assert!(shared.cursor_image.lock().unwrap().is_none());
    }
}
