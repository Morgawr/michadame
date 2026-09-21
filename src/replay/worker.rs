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
    schedule: VideoSchedule,
    audio_clock: AudioClock,
    overhead: usize,
    generation: u64,
}
/// One encode per accepted picture, regardless of how many capture ticks were
/// missed. Packet durations hold the previous picture across the missing ticks.
/// Keyframes follow elapsed media time, not the reduced count of encoded frames.
pub(super) struct VideoSchedule {
    epoch: i64,
    rate: config::Rate,
    last_tick: i64,
    last_key_at: Option<i64>,
}
impl VideoSchedule {
    pub(super) fn new(epoch: i64, rate: config::Rate) -> Self {
        Self {
            epoch,
            rate,
            last_tick: -1,
            last_key_at: None,
        }
    }
    pub(super) fn accept(&mut self, at: i64) -> Option<(i64, bool)> {
        let tick = self.rate.tick(at - self.epoch);
        if tick <= self.last_tick {
            return None;
        }
        let pts = self.epoch + self.rate.us(tick);
        let key = self.last_key_at.is_none_or(|last| pts - last >= 1_000_000);
        self.last_tick = tick;
        if key {
            self.last_key_at = Some(pts);
        }
        Some((pts, key))
    }
}

/// Drain only the queue snapshot, so producers cannot keep this worker here
/// indefinitely. The bounded queue holds at most two pictures.
fn newest_frame(
    mut frame: VideoFrame,
    queued: &Receiver<VideoFrame>,
    recycled: &Sender<Vec<u8>>,
    dropped: &AtomicU64,
) -> VideoFrame {
    for _ in 0..queued.len() {
        let Ok(newer) = queued.try_recv() else {
            break;
        };
        let _ = recycled.try_send(frame.rgba);
        dropped.fetch_add(1, Ordering::Relaxed);
        frame = newer;
    }
    frame
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
                ensure!(at - previous_at < 12000., "Audio capture gap");
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
    fn append_recovering(&mut self, block: &AudioBlock, epoch: i64) -> Result<bool> {
        if self.append(block, epoch).is_ok() {
            return Ok(false);
        }
        // Re-anchor only the recording resampler. The existing encoder and A/V
        // history remain valid; PTS preserves the gap instead of moving audio early.
        *self = Self::new();
        self.append(block, epoch)?;
        Ok(true)
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
    let mut waiting_for_video = false;
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
            let frame = newest_frame(frame, &video_rx, &recycled, &shared.dropped);
            if frame.generation != shared.generation.load(Ordering::Acquire) {
                let _ = recycled.try_send(frame.rgba);
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
                    schedule: VideoSchedule::new(frame.at, frame.rate),
                    audio_clock: AudioClock::new(),
                    overhead,
                    generation: frame.generation,
                });
                limit = config
                    .budget()
                    .saturating_sub(overhead)
                    .saturating_sub(shared.saving_bytes.load(Ordering::Acquire));
                shared.message("Buffering replay");
            }
            let s = session.as_mut().unwrap();
            if let Some((pts, key)) = s.schedule.accept(frame.at) {
                for packet in s.video.encode(&frame.rgba, pts, key)? {
                    if packet.start >= s.epoch {
                        history.push(packet, limit);
                    }
                }
                last_input = Instant::now();
                if waiting_for_video {
                    shared.message("Replay resumed; earlier history preserved");
                }
                waiting_for_video = false;
            }
            let _ = recycled.try_send(frame.rgba);
        }

        // A fixed bound also prevents capture bursts from starving the video branch.
        for _ in 0..32 {
            let Ok(block) = audio_rx.try_recv() else {
                break;
            };
            if let Some(s) = session.as_mut() {
                if block.at >= s.epoch - 200_000 && block.at <= now_us() {
                    if s.audio_clock.append_recovering(&block, s.epoch)? {
                        shared.message(
                            "Audio capture gap; continuing with earlier history preserved",
                        );
                    }
                    while let Some((at, samples)) = s.audio_clock.take() {
                        for mut packet in s.audio.encode(&samples, at)? {
                            // Preserve the encoder's Opus lookahead offset.
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
            }
            let _ = audio_free.try_send(block);
        }
        if last_input.elapsed() > Duration::from_secs(1) && session.is_some() && !waiting_for_video
        {
            // No new frames is a pause, not a new recording epoch. Saved history
            // and pending requests remain usable, within normal age/byte limits.
            waiting_for_video = true;
            shared.message("Waiting for rendered frames; earlier replay history preserved");
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
    #[test]
    fn dropped_frames_and_long_pauses_do_not_restart_video_time_or_create_catchup_work() {
        let rate = config::Rate::new(60000, 1001);
        let epoch = 10_000_000;
        let mut schedule = VideoSchedule::new(epoch, rate);
        let ticks = [0, 1, 3, 7, 31, 160, 161]; // isolated, burst, >250ms, >1s
        let accepted: Vec<_> = ticks
            .into_iter()
            .map(|t| schedule.accept(epoch + rate.us(t)).unwrap())
            .collect();
        assert_eq!(accepted.len(), ticks.len());
        for ((pts, _), tick) in accepted.iter().zip(ticks) {
            assert_eq!(*pts, epoch + rate.us(tick));
        }
        assert!(accepted[5].1, "Recovered frame needs a time-based keyframe");
        assert!(!accepted[6].1);
        assert!(
            schedule.accept(epoch + rate.us(160)).is_none(),
            "Late pictures cannot move timestamps backward"
        );
    }
    #[test]
    fn saturated_queue_recovers_with_newest_picture_and_recycles_old_buffers() {
        let frame = |at| VideoFrame {
            rgba: vec![at as u8],
            width: 1,
            height: 1,
            at,
            rate: config::Rate::new(60, 1),
            generation: 1,
        };
        let (tx, rx) = crossbeam_channel::bounded(2);
        let (recycle, returned) = crossbeam_channel::bounded(3);
        let dropped = AtomicU64::new(0);
        tx.send(frame(2)).unwrap();
        tx.send(frame(3)).unwrap();
        let latest = newest_frame(frame(1), &rx, &recycle, &dropped);
        assert_eq!(latest.at, 3);
        assert_eq!(latest.rgba, [3]);
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
        assert_eq!(returned.try_recv().unwrap(), [1]);
        assert_eq!(returned.try_recv().unwrap(), [2]);
        assert!(rx.is_empty());
    }
    #[test]
    fn audio_recovery_keeps_the_original_epoch_without_allocating_a_long_silence() {
        let mut clock = AudioClock::new();
        assert!(!clock.append_recovering(&block(0, 48000, 1920), 0).unwrap());
        let (before, _) = clock.take().unwrap();
        assert_eq!(before, 0);
        assert!(clock
            .append_recovering(&block(60_000_000, 48000, 1920), 0)
            .unwrap());
        let (after, _) = clock.take().unwrap();
        assert_eq!(after, 60 * 48000);
        assert!(clock.pending.len() < 1920);
    }
}
