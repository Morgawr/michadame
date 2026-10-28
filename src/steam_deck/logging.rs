//! Deck notifications go to a persistent log, without covering the game or UI.
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir)
        .join("michadame-steam-deck")
}

struct Writer(Option<std::fs::File>);
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(bytes);
        if let Some(file) = &mut self.0 {
            file.write_all(bytes)?;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(file) = &mut self.0 {
            file.flush()?;
        }
        Ok(())
    }
}

pub fn initialize() {
    let directory = state_dir();
    let _ = std::fs::create_dir_all(&directory);
    let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700));
    let path = directory.join("michadame.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 5 * 1024 * 1024) {
        let _ = std::fs::rename(&path, directory.join("michadame.previous.log"));
    }
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || {
            Writer(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .mode(0o600)
                    .open(&path)
                    .ok(),
            )
        })
        .init();
}
