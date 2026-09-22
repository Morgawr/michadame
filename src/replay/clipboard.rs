use anyhow::{ensure, Context, Result};
use arboard::SetExtLinux;
use std::{
    fs::File,
    io::{self, Seek, SeekFrom, Write},
    os::fd::AsRawFd,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

const MAX_CLIP_BYTES: usize = 256 * 1024 * 1024;

#[derive(Default)]
pub struct Clipboard {
    bytes: Arc<AtomicUsize>,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    closed: bool,
    directory: Option<tempfile::TempDir>,
    current: Option<Published>,
}
struct Published {
    file: tempfile::NamedTempFile,
    reservation: Reservation,
    // X11 serves the selection while this connection remains alive.
    backend: Option<arboard::Clipboard>,
}
pub struct Reservation {
    owner: Arc<AtomicUsize>,
    pub limit: usize,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.owner.fetch_sub(self.limit, Ordering::AcqRel);
    }
}

impl Clipboard {
    pub fn bytes(&self) -> usize {
        self.bytes.load(Ordering::Acquire)
    }

    pub fn reserve(self: &Arc<Self>, limit: usize, budget: usize) -> Result<Reservation> {
        ensure!(
            limit <= MAX_CLIP_BYTES,
            "Clipboard clip exceeds 256 MiB; lower the replay bitrate"
        );
        self.bytes.fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
            old.checked_add(limit).filter(|total| *total <= budget)
        }).map_err(|_| anyhow::anyhow!("Not enough replay RAM for the clipboard clip; increase the RAM budget or lower the bitrate"))?;
        Ok(Reservation {
            owner: self.bytes.clone(),
            limit,
        })
    }

    pub fn file(&self) -> Result<tempfile::NamedTempFile> {
        let mut state = self.state.lock().unwrap();
        ensure!(!state.closed, "Clipboard export cancelled during shutdown");
        // Never fall back to a disk-backed TMPDIR or the recording directory.
        require_ram_filesystem(&File::open("/tmp")?)?;
        if state.directory.is_none() {
            state.directory = Some(
                tempfile::Builder::new()
                    .prefix("michadame-clipboard-")
                    .tempdir_in("/tmp")?,
            );
        }
        let file = tempfile::Builder::new()
            .prefix("replay-")
            .suffix(".mp4")
            .tempfile_in(state.directory.as_ref().unwrap().path())?;
        require_ram_filesystem(file.as_file())?;
        Ok(file)
    }

    pub fn publish(
        &self,
        file: tempfile::NamedTempFile,
        reservation: Reservation,
    ) -> Result<usize> {
        self.publish_with(file, reservation, |path| {
            // All display-server interaction runs on the export worker.
            let mut backend =
                arboard::Clipboard::new().context("Cannot connect to the Linux clipboard")?;
            backend
                .set()
                .exclude_from_history()
                .file_list(&[path])
                .context("Cannot offer the replay file to the Linux clipboard")?;
            Ok(Some(backend))
        })
    }

    fn publish_with(
        &self,
        file: tempfile::NamedTempFile,
        mut reservation: Reservation,
        offer: impl FnOnce(&std::path::Path) -> Result<Option<arboard::Clipboard>>,
    ) -> Result<usize> {
        let size = file.as_file().metadata()?.len() as usize;
        ensure!(
            size > 0 && size <= reservation.limit,
            "Invalid clipboard clip size"
        );
        ensure!(
            !self.state.lock().unwrap().closed,
            "Clipboard export cancelled during shutdown"
        );
        let backend = offer(file.path())?;
        let mut state = self.state.lock().unwrap();
        ensure!(!state.closed, "Clipboard export cancelled during shutdown");
        self.bytes
            .fetch_sub(reservation.limit - size, Ordering::AcqRel);
        reservation.limit = size;
        let old = state.current.replace(Published {
            file,
            reservation,
            backend,
        });
        drop(state);
        // Keep the last clip even if a clipboard manager takes selection ownership.
        // It is removed only when another replay replaces it or on application exit.
        drop(old);
        Ok(size)
    }

    pub fn close(&self) {
        let (current, directory) = {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            (state.current.take(), state.directory.take())
        };
        if let Some(Published {
            file,
            reservation,
            backend,
        }) = current
        {
            drop(file);
            drop(reservation);
            // A display server may be slow: never join its event thread on the UI.
            let _ = std::thread::Builder::new()
                .name("clipboard-cleanup".into())
                .spawn(move || drop(backend));
        }
        // Also unlinks an in-flight export; its open fd cannot leave a named file behind.
        drop(directory);
    }
}

fn require_ram_filesystem(file: &File) -> Result<()> {
    let mut fs = std::mem::MaybeUninit::<libc::statfs>::uninit();
    let result = unsafe { libc::fstatfs(file.as_raw_fd(), fs.as_mut_ptr()) };
    ensure!(
        result == 0,
        "Cannot inspect temporary filesystem: {}",
        io::Error::last_os_error()
    );
    let kind = unsafe { fs.assume_init() }.f_type;
    ensure!(
        kind == libc::TMPFS_MAGIC,
        "Clipboard video requires RAM-backed /tmp; no video was written to disk"
    );
    Ok(())
}

/// Enforce the reserved byte limit while muxing, including seeks to write MP4 tables.
pub struct LimitedFile {
    file: File,
    limit: u64,
    position: u64,
}
impl LimitedFile {
    pub fn new(file: File, limit: usize) -> Self {
        Self {
            file,
            limit: limit as u64,
            position: 0,
        }
    }
}
impl Write for LimitedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() as u64 > self.limit.saturating_sub(self.position) {
            return Err(io::Error::other(
                "Clipboard clip exceeded its RAM reservation",
            ));
        }
        let written = self.file.write(buf)?;
        self.position += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
impl Seek for LimitedFile {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let next = match from {
            SeekFrom::Start(at) => i128::from(at),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.file.metadata()?.len()) + i128::from(offset),
        };
        if !(0..=i128::from(self.limit)).contains(&next) {
            return Err(io::Error::other(
                "Clipboard seek exceeds its RAM reservation",
            ));
        }
        self.position = self.file.seek(SeekFrom::Start(next as u64))?;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservations_release_on_failure_and_enforce_combined_budget() {
        let clipboard = Arc::new(Clipboard::default());
        let first = clipboard.reserve(100, 200).unwrap();
        assert!(clipboard.reserve(101, 200).is_err());
        assert!(clipboard.reserve(MAX_CLIP_BYTES + 1, usize::MAX).is_err());
        let second = clipboard.reserve(100, 200).unwrap();
        assert_eq!(clipboard.bytes(), 200);
        drop(first);
        drop(second);
        assert_eq!(clipboard.bytes(), 0);
    }

    #[test]
    fn failed_copy_preserves_previous_file_and_replacement_releases_it() {
        if require_ram_filesystem(&File::open("/tmp").unwrap()).is_err() {
            return;
        }
        let clipboard = Arc::new(Clipboard::default());
        let mut first = clipboard.file().unwrap();
        first.write_all(b"clip").unwrap();
        let original = first.path().to_owned();
        clipboard
            .publish_with(first, clipboard.reserve(8, 12).unwrap(), |_| Ok(None))
            .unwrap();
        assert_eq!(clipboard.bytes(), 4);
        let mut failed = clipboard.file().unwrap();
        failed.write_all(b"fail").unwrap();
        let failed_path = failed.path().to_owned();
        assert!(clipboard
            .publish_with(
                failed,
                clipboard.reserve(8, 12).unwrap(),
                |_| anyhow::bail!("no clipboard")
            )
            .is_err());
        assert!(original.exists());
        assert!(!failed_path.exists());
        assert_eq!(clipboard.bytes(), 4);
        let mut second = clipboard.file().unwrap();
        second.write_all(b"clip2").unwrap();
        let replacement = second.path().to_owned();
        clipboard
            .publish_with(second, clipboard.reserve(8, 12).unwrap(), |_| Ok(None))
            .unwrap();
        assert!(!original.exists());
        assert!(replacement.exists());
        assert_eq!(clipboard.bytes(), 5);
        let mut racing = clipboard.file().unwrap();
        racing.write_all(b"late").unwrap();
        assert!(clipboard
            .publish_with(racing, clipboard.reserve(7, 12).unwrap(), |_| {
                clipboard.close();
                Ok(None)
            })
            .is_err());
        assert!(!replacement.exists());
        assert_eq!(clipboard.bytes(), 0);
    }

    #[test]
    fn ram_files_are_private_bounded_and_removed_on_shutdown() {
        use std::os::unix::fs::PermissionsExt;
        let clipboard = Clipboard::default();
        // memfd always supplies a RAM-backed test fd, even on hosts with disk /tmp.
        let fd = unsafe { libc::memfd_create(c"replay-test".as_ptr(), libc::MFD_CLOEXEC) };
        assert!(fd >= 0);
        use std::os::fd::FromRawFd;
        let file = unsafe { File::from_raw_fd(fd) };
        require_ram_filesystem(&file).unwrap();
        let mut limited = LimitedFile::new(file, 4);
        limited.write_all(b"1234").unwrap();
        assert!(limited.write_all(b"5").is_err());
        limited.seek(SeekFrom::Start(0)).unwrap();
        limited.write_all(b"abcd").unwrap();
        assert!(limited.seek(SeekFrom::Start(5)).is_err());
        assert_eq!(limited.file.metadata().unwrap().len(), 4);
        if require_ram_filesystem(&File::open("/tmp").unwrap()).is_err() {
            assert!(clipboard.file().is_err());
            return;
        }
        let pending = clipboard.file().unwrap();
        let path = pending.path().to_owned();
        assert_eq!(
            pending.as_file().metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        clipboard.close();
        assert!(!path.exists());
        assert!(clipboard.file().is_err());
    }
}
