//! Deck notifications go to a persistent log, without covering the game or UI.
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir)
        .join("michadame-steam-deck")
}

#[derive(Clone)]
struct LogFileMakeWriter {
    file: Option<Arc<std::fs::File>>,
}

impl LogFileMakeWriter {
    fn open(path: &Path) -> Self {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .ok()
            .map(Arc::new);
        Self { file }
    }

    fn make_writer(&self) -> LogFileWriter {
        LogFileWriter {
            file: self.file.clone(),
        }
    }
}

struct LogFileWriter {
    file: Option<Arc<std::fs::File>>,
}

impl Write for LogFileWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(bytes);
        if let Some(file) = &self.file {
            (&**file).write_all(bytes)?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(file) = &self.file {
            (&**file).flush()?;
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
    let make_writer = LogFileMakeWriter::open(&path);
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || make_writer.make_writer())
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn log_file_make_writer_reuses_persistent_open_file() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("michadame.log");
        let make_writer = LogFileMakeWriter::open(&log_path);

        let mut w1 = make_writer.make_writer();
        let w2 = make_writer.make_writer();
        assert!(Arc::ptr_eq(
            w1.file.as_ref().unwrap(),
            w2.file.as_ref().unwrap()
        ));

        w1.write_all(b"line one\n").unwrap();
        let mut reader = std::fs::File::open(&log_path).unwrap();
        // Unlink path while open: subsequent writers must still write through the persistent fd
        // without reopening `michadame.log` per log line.
        std::fs::remove_file(&log_path).unwrap();
        let mut w3 = make_writer.make_writer();
        w3.write_all(b"line two\n").unwrap();
        w3.flush().unwrap();

        assert!(
            !log_path.exists(),
            "Writer must not re-create/re-open michadame.log on each log line"
        );

        let mut contents = String::new();
        reader.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "line one\nline two\n");
    }
}
