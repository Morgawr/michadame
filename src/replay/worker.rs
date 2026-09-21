use super::{
    encoder::{AudioEncoder, VideoEncoder},
    ring::History,
    *,
};
use anyhow::{ensure, Result};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

struct Session {
    video: VideoEncoder,
    audio: AudioEncoder,
    width: u32,
    height: u32,
    epoch: i64,
    last_tick: i64,
    last_rgba: Vec<u8>,
    audio_clock: AudioClock,
    overhead: usize,
    generation: u64,
}
/// Maps timestamped capture samples to a fixed 48 kHz timeline. Resampling and gap
/// repair affect only the recording, never the playback source.
struct AudioClock {
    pending: VecDeque<[f32; 2]>,
    next: i64,
    frame_start: i64,
    previous: Option<(f64, [f32; 2])>,
}
impl AudioClock {
    fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            next: 0,
            frame_start: 0,
            previous: None,
        }
    }
    fn append(&mut self, block: &AudioBlock, epoch: i64) -> Result<()> {
        ensure!(block.rate > 0 && block.channels > 0, "Invalid audio format");
        let channels = block.channels as usize;
        let observed_start = (block.at - epoch) as f64 * 48000. / 1e6;
        let nominal_step = 48000. / block.rate as f64;
        let mut start = observed_start;
        let mut step = nominal_step;
        if let Some((previous, _)) = self.previous {
            let expected_start = previous + nominal_step;
            let error = observed_start - expected_start;
            // Filter block-arrival jitter, but track clock drift through a small
            // rate correction. Real missing blocks (>5ms) retain their gap.
            if error.abs() < 240. {
                start = expected_start;
                let count = (block.len / channels).max(1) as f64;
                step *= 1. + (error / count * 0.05).clamp(-0.005, 0.005);
            }
        }
        // The first block may arrive after video starts. Leave the real startup offset intact.
        if self.previous.is_none() && start > self.next as f64 {
            let first = start.round() as i64;
            self.next = first;
            self.frame_start = first;
        }
        for (i, input) in block.samples[..block.len]
            .chunks_exact(channels)
            .enumerate()
        {
            let at = start + i as f64 * step;
            let sample = [input[0], input[usize::from(channels > 1)]];
            if at < 0. {
                continue;
            }
            if let Some((previous_at, previous)) = self.previous {
                if at <= previous_at {
                    continue;
                }
                ensure!(
                    at - previous_at < 12000.,
                    "Audio capture gap; replay history reset"
                );
                while (self.next as f64) <= at {
                    let fraction = ((self.next as f64 - previous_at) / (at - previous_at))
                        .clamp(0., 1.) as f32;
                    // Missing blocks become silence, not time compression or a long held sample.
                    let output = if at - previous_at > step * 4. {
                        [0.; 2]
                    } else {
                        [
                            previous[0] + fraction * (sample[0] - previous[0]),
                            previous[1] + fraction * (sample[1] - previous[1]),
                        ]
                    };
                    self.pending.push_back(output);
                    self.next += 1;
                }
            }
            self.previous = Some((at, sample));
        }
        Ok(())
    }
    fn take(&mut self) -> Option<(i64, Vec<[f32; 2]>)> {
        if self.pending.len() < 960 {
            return None;
        }
        let at = self.frame_start;
        self.frame_start += 960;
        Some((at, self.pending.drain(..960).collect()))
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    config: ReplayConfig,
    video_rx: Receiver<VideoFrame>,
    recycled: Sender<Vec<u8>>,
    audio_rx: Receiver<AudioBlock>,
    audio_free: Sender<AudioBlock>,
    saves: Receiver<Save>,
    shared: &Arc<Shared>,
) -> Result<()> {
    // Best effort lower CPU priority for conversion/encoding, without changing app priority.
    unsafe {
        libc::setpriority(
            libc::PRIO_PROCESS,
            libc::syscall(libc::SYS_gettid) as u32,
            10,
        );
    }
    let mut session: Option<Session> = None;
    let mut history = History::new(config.history_seconds);
    let mut pending_save: Option<(Save, Instant)> = None;
    let mut export_thread: Option<std::thread::JoinHandle<()>> = None;
    let mut monitor = Instant::now();
    let mut last_input = Instant::now();
    let mut force_key = true;
    while !shared.stop.load(Ordering::Acquire) {
        if shared.generation.load(Ordering::Acquire) != session.as_ref().map_or(0, |s| s.generation)
        {
            session = None;
            history.clear();
            pending_save = None;
        }
        if let Some(thread) = export_thread.as_ref() {
            if thread.is_finished() {
                let _ = export_thread.take().unwrap().join();
            }
        }
        let surface_overhead = session.as_ref().map_or(64 * 1024 * 1024, |s| s.overhead);
        let mut limit = config
            .budget()
            .saturating_sub(surface_overhead)
            .saturating_sub(shared.saving_bytes.load(Ordering::Acquire));
        history.trim(limit);
        if let Ok(frame) = video_rx.recv_timeout(Duration::from_millis(5)) {
            if frame.generation != shared.generation.load(Ordering::Acquire) {
                let _ = recycled.try_send(frame.rgba);
                continue;
            }
            if now_us() - frame.at > 1_000_000 {
                let _ = recycled.try_send(frame.rgba);
                shared.dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if session.as_ref().is_none_or(|s| {
                (s.width, s.height, s.video.rate) != (frame.width, frame.height, frame.rate)
            }) {
                history.clear();
                pending_save = None;
                let overhead = config::overhead(frame.width, frame.height)
                    .ok_or_else(|| anyhow::anyhow!("Surface too large"))?;
                ensure!(
                    overhead < config.budget() / 2,
                    "Replay budget is too small for surface staging"
                );
                shared.message("Starting hardware replay encoder…");
                let video = VideoEncoder::new(&config, frame.width, frame.height, frame.rate)?;
                let audio = AudioEncoder::new()?;
                session = Some(Session {
                    video,
                    audio,
                    width: frame.width,
                    height: frame.height,
                    epoch: frame.at,
                    last_tick: -1,
                    last_rgba: Vec::new(),
                    audio_clock: AudioClock::new(),
                    overhead,
                    generation: frame.generation,
                });
                limit = config
                    .budget()
                    .saturating_sub(overhead)
                    .saturating_sub(shared.saving_bytes.load(Ordering::Acquire));
                force_key = true;
                shared.message("Buffering replay");
            }
            let s = session.as_mut().unwrap();
            let mut tick = s.video.rate.tick(frame.at - s.epoch);
            if s.last_tick >= 0 && s.video.rate.us(tick - s.last_tick) > 250_000 {
                // Keep the hardware session, but start a fresh independently decodable history.
                history.clear();
                pending_save = None;
                s.epoch = frame.at;
                s.last_tick = -1;
                s.audio = AudioEncoder::new()?;
                s.audio_clock = AudioClock::new();
                tick = 0;
                force_key = true;
                // Encoder PTS must remain monotonic even across capture gaps; use absolute timestamps below.
                shared.message("Replay history reset after a video gap");
            }
            if tick > s.last_tick {
                for missing in (s.last_tick + 1)..tick {
                    if !s.last_rgba.is_empty() {
                        let pts = s.epoch + s.video.rate.us(missing);
                        for packet in s.video.encode(&s.last_rgba, pts, force_key)? {
                            if packet.start >= s.epoch {
                                history.push(packet, limit);
                            }
                        }
                        force_key = false;
                    }
                }
                let pts = s.epoch + s.video.rate.us(tick);
                for packet in s.video.encode(&frame.rgba, pts, force_key)? {
                    if packet.start >= s.epoch {
                        history.push(packet, limit);
                    }
                }
                force_key = false;
                s.last_tick = tick;
                let old = std::mem::replace(&mut s.last_rgba, frame.rgba);
                let _ = recycled.try_send(old);
                last_input = Instant::now();
            } else {
                let _ = recycled.try_send(frame.rgba);
            }
        }
        // A fixed bound also prevents capture bursts from starving the video branch.
        for _ in 0..32 {
            let Ok(block) = audio_rx.try_recv() else {
                break;
            };
            if let Some(s) = session.as_mut() {
                if block.at >= s.epoch - 200_000 && block.at <= now_us() {
                    match s.audio_clock.append(&block, s.epoch) {
                        Ok(()) => {
                            while let Some((at, samples)) = s.audio_clock.take() {
                                for mut packet in s.audio.encode(&samples, at)? {
                                    // Opus lookahead is reflected in encoder PTS; preserve the offset.
                                    packet
                                        .packet
                                        .set_pts(packet.packet.pts().map(|t| t + s.epoch));
                                    packet
                                        .packet
                                        .set_dts(packet.packet.dts().map(|t| t + s.epoch));
                                    packet.start += s.epoch;
                                    packet.end += s.epoch;
                                    if packet.start >= s.epoch {
                                        history.push(packet, limit);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            history.clear();
                            session = None;
                            pending_save = None;
                            force_key = true;
                            shared.message(e.to_string());
                        }
                    }
                }
            }
            let _ = audio_free.try_send(block);
        }
        if last_input.elapsed() > Duration::from_secs(1) && session.is_some() {
            session = None;
            history.clear();
            pending_save = None;
            shared.message("Replay paused: waiting for rendered frames");
        }
        if session
            .as_ref()
            .is_some_and(|s| s.generation != shared.generation.load(Ordering::Acquire))
        {
            session = None;
            history.clear();
            pending_save = None;
        }
        if let Ok(save) = saves.try_recv() {
            if export_thread.is_some() || pending_save.is_some() {
                shared.message("A replay save is already pending");
            } else {
                pending_save = Some((save, Instant::now()));
            }
        }
        if let Some((save, requested)) = &pending_save {
            // Wait for asynchronously read-back/encoded packets through the keypress. A
            // stalled source gets a bounded timeout and a truthful shorter clip.
            if history.end().is_some_and(|end| end >= save.at)
                || requested.elapsed() > Duration::from_secs(2)
            {
                let packets = history.clip(save.seconds, save.at);
                if let (Some(packets), Some(s)) = (packets, session.as_ref()) {
                    let pinned: usize = packets.iter().map(|p| p.bytes).sum();
                    let video = s.video.parameters()?;
                    let audio = s.audio.parameters();
                    shared.saving_bytes.store(pinned, Ordering::Release);
                    history.trim(
                        config
                            .budget()
                            .saturating_sub(surface_overhead)
                            .saturating_sub(pinned),
                    );
                    let duration = (packets.iter().map(|p| p.end).max().unwrap() - packets[0].start)
                        as f64
                        / 1e6;
                    let directory = config.directory.clone();
                    let export_shared = shared.clone();
                    shared.status.lock().unwrap().saving = true;
                    let spawned = std::thread::Builder::new()
                        .name("replay-save".into())
                        .spawn(move || {
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    save_file(&directory, video, audio, packets)
                                }))
                                .unwrap_or_else(|_| Err(anyhow::anyhow!("Replay writer panicked")));
                            export_shared.saving_bytes.store(0, Ordering::Release);
                            let mut status = export_shared.status.lock().unwrap();
                            status.saving = false;
                            status.message = match result {
                                Ok(path) => format!("Saved {duration:.1}s: {}", path.display()),
                                Err(e) => format!("Replay save failed: {e:#}"),
                            };
                        });
                    match spawned {
                        Ok(thread) => export_thread = Some(thread),
                        Err(e) => {
                            shared.saving_bytes.store(0, Ordering::Release);
                            shared.status.lock().unwrap().saving = false;
                            return Err(e.into());
                        }
                    }
                } else {
                    shared.message("Not enough decodable video and audio to save yet");
                }
                pending_save = None;
            }
        }
        if monitor.elapsed() > Duration::from_millis(500) {
            let available = config::available_memory();
            if available.is_some_and(|n| n < config::SAFETY_RESERVE) {
                anyhow::bail!("Low available RAM; replay released to protect playback");
            }
            let mut status = shared.status.lock().unwrap();
            status.seconds = history.duration();
            status.bytes = history.bytes + shared.saving_bytes.load(Ordering::Acquire);
            status.overhead = session.as_ref().map_or(0, |s| s.overhead);
            status.available = available;
            status.surface = session.as_ref().map(|s| (s.width, s.height));
            status.codec = config.codec.encoder().into();
            status.dropped = shared.dropped.load(Ordering::Relaxed);
            monitor = Instant::now();
        }
    }
    // Dropping a JoinHandle detaches an active save. It owns only encoded packets,
    // and its retained memory remains visible through Shared until it finishes.
    drop(export_thread);
    history.clear();
    drop(session);
    let mut status = shared.status.lock().unwrap();
    status.seconds = 0.;
    status.bytes = shared.saving_bytes.load(Ordering::Acquire);
    status.overhead = 0;
    Ok(())
}
fn save_file(
    directory: &str,
    video: ffmpeg_next::codec::Parameters,
    audio: ffmpeg_next::codec::Parameters,
    packets: Vec<Arc<ring::Encoded>>,
) -> Result<std::path::PathBuf> {
    unsafe {
        libc::setpriority(
            libc::PRIO_PROCESS,
            libc::syscall(libc::SYS_gettid) as u32,
            10,
        );
        libc::syscall(libc::SYS_ioprio_set, 1, 0, 3 << 13);
    }
    let dir = std::path::Path::new(directory);
    std::fs::create_dir_all(dir)?;
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let path = dir.join(format!("replay-{time}.mkv"));
    let temporary = dir.join(format!(".replay-{time}.partial"));
    // Exclusive reservation; never overwrite an existing recording.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = encoder::export(&temporary, video, audio, packets);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(e);
    }
    // Hard-link publish is atomic and fails if destination exists.
    std::fs::hard_link(&temporary, &path)?;
    std::fs::remove_file(&temporary)?;
    Ok(path)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn block(at: i64, rate: u32, frames: usize) -> AudioBlock {
        let mut b = AudioBlock {
            samples: Box::new([0.; AUDIO_SAMPLES]),
            len: frames * 2,
            rate,
            channels: 2,
            at,
        };
        b.samples[..b.len].fill(0.5);
        b
    }
    #[test]
    fn audio_retains_start_offset_and_resamples() {
        let mut clock = AudioClock::new();
        clock.append(&block(100_000, 44100, 2205), 0).unwrap();
        let (pts, samples) = clock.take().unwrap();
        assert_eq!(pts, 4800);
        assert_eq!(samples.len(), 960);
        assert!(samples.iter().all(|v| (v[0] - 0.5).abs() < 1e-6));
    }
    #[test]
    fn long_audio_gap_is_not_silently_time_compressed() {
        let mut clock = AudioClock::new();
        clock.append(&block(0, 48000, 960), 0).unwrap();
        assert!(clock.append(&block(1_000_000, 48000, 960), 0).is_err());
    }
    #[test]
    fn short_missing_audio_has_silence_and_preserves_timeline() {
        let mut clock = AudioClock::new();
        clock.append(&block(0, 48000, 960), 0).unwrap();
        clock.append(&block(40_000, 48000, 960), 0).unwrap();
        assert!(clock.pending.iter().filter(|s| s[0] == 0.).count() >= 960);
    }
    #[test]
    fn jitter_and_clock_drift_do_not_accumulate_audio_desync() {
        let mut clock = AudioClock::new();
        let actual_rate = 48000. * 1.0001;
        for i in 0..1000 {
            let jitter = if i % 2 == 0 { 100. } else { -100. };
            let at = (i as f64 * 960. / actual_rate * 1e6 + jitter).max(0.) as i64;
            clock.append(&block(at, 48000, 960), 0).unwrap();
            while clock.take().is_some() {}
        }
        let expected = (960000. / actual_rate * 48000.) as i64;
        assert!(
            (clock.next - expected).abs() < 48,
            "Recording clock drift exceeded 1ms"
        );
    }
}
