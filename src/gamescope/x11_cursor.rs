use std::ffi::{c_char, c_int, c_void, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[repr(C)]
struct CGamescopeX11CursorMonitor {
    _unused: [u8; 0],
}

type CGamescopeX11CursorCb = unsafe extern "C" fn(is_hidden: c_int, user_data: *mut c_void);

extern "C" {
    fn gamescope_x11_cursor_monitor_start(
        display_name: *const c_char,
        cb: CGamescopeX11CursorCb,
        user_data: *mut c_void,
    ) -> *mut CGamescopeX11CursorMonitor;

    fn gamescope_x11_cursor_monitor_stop(monitor: *mut CGamescopeX11CursorMonitor);
}

pub struct GamescopeX11CursorMonitor {
    monitor: *mut CGamescopeX11CursorMonitor,
    is_cursor_hidden: Arc<AtomicBool>,
    _callback_data: Box<Arc<AtomicBool>>,
}

unsafe impl Send for GamescopeX11CursorMonitor {}
unsafe impl Sync for GamescopeX11CursorMonitor {}

unsafe extern "C" fn on_cursor_changed(is_hidden: c_int, user_data: *mut c_void) {
    if !user_data.is_null() {
        let flag = &*(user_data as *const Arc<AtomicBool>);
        let hidden = is_hidden != 0;
        flag.store(hidden, Ordering::Release);
        tracing::info!("Gamescope X11 cursor visibility: hidden={hidden}");
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
        let is_cursor_hidden = Arc::new(AtomicBool::new(false));
        let callback_data = Box::new(is_cursor_hidden.clone());

        let c_display = CString::new(display_name).ok()?;
        let monitor = unsafe {
            gamescope_x11_cursor_monitor_start(
                c_display.as_ptr(),
                on_cursor_changed,
                &*callback_data as *const Arc<AtomicBool> as *mut c_void,
            )
        };

        if monitor.is_null() {
            None
        } else {
            Some(Self {
                monitor,
                is_cursor_hidden,
                _callback_data: callback_data,
            })
        }
    }

    pub fn is_cursor_hidden(&self) -> bool {
        self.is_cursor_hidden.load(Ordering::Acquire)
    }

    pub fn cursor_hidden_flag(&self) -> Arc<AtomicBool> {
        self.is_cursor_hidden.clone()
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

            // Check parent PID in /proc/<pid>/stat
            let stat_path = proc_path.join("stat");
            let Ok(stat) = std::fs::read_to_string(&stat_path) else {
                continue;
            };
            let Some(rparen) = stat.rfind(')') else {
                continue;
            };
            let rest = &stat[rparen + 1..];
            let fields: Vec<&str> = rest.split_whitespace().collect();
            if fields.len() < 2 {
                continue;
            }
            let Ok(ppid) = fields[1].parse::<u32>() else {
                continue;
            };

            let parent_comm_path = PathBuf::from(format!("/proc/{ppid}/comm"));
            let is_parent_gamescope = std::fs::read_to_string(&parent_comm_path)
                .map(|pcomm| pcomm.trim().contains("gamescope"))
                .unwrap_or(false);

            if !is_parent_gamescope {
                let parent_cmdline_path = PathBuf::from(format!("/proc/{ppid}/cmdline"));
                let is_cmd_gamescope = std::fs::read(&parent_cmdline_path)
                    .map(|bytes| String::from_utf8_lossy(&bytes).contains("gamescope"))
                    .unwrap_or(false);
                if !is_cmd_gamescope {
                    continue;
                }
            }

            // Extract display argument from Xwayland's cmdline
            let cmdline_path = proc_path.join("cmdline");
            if let Ok(bytes) = std::fs::read(&cmdline_path) {
                let parts: Vec<&[u8]> = bytes.split(|&b| b == 0).collect();
                for part in parts {
                    let s = String::from_utf8_lossy(part);
                    if s.starts_with(':') && s[1..].chars().all(|c| c.is_ascii_digit()) {
                        return Some(s.to_string());
                    }
                }
            }
        }
    }

    // 3. Fallback: Search /tmp/.X11-unix/X* for socket numbers different from host DISPLAY
    let host_disp_num = std::env::var("DISPLAY")
        .ok()
        .and_then(|d| d.trim_start_matches(':').split('.').next().map(|s| s.to_string()));

    if let Ok(entries) = std::fs::read_dir("/tmp/.X11-unix") {
        let mut candidates = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(num_str) = name.strip_prefix('X') {
                if num_str.chars().all(|c| c.is_ascii_digit()) {
                    if host_disp_num.as_deref() != Some(num_str) {
                        if let Ok(num) = num_str.parse::<u32>() {
                            candidates.push(num);
                        }
                    }
                }
            }
        }
        candidates.sort_by(|a, b| b.cmp(a));
        if let Some(num) = candidates.first() {
            return Some(format!(":{num}"));
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
}
