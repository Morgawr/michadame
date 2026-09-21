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

/// Maps timestamped capture samples to a fixed 48 kHz timeline. Resampling and gap
/// repair affect only the recording, never the playback source.
struct AudioClock {
    pending: VecDeque<[f32; 2]>,
    next: i64,
    frame_start: i64,
    previous: Option<(f64, [f32; 2])>,
    clock_error: f64,
}
impl AudioClock {
    fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            next: 0,
            frame_start: 0,
            previous: None,
            clock_error: 0.,
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
            // ALSA read timestamps can jump while the PCM samples are perfectly
            // continuous. Only explicit capture/queue loss may insert silence or
            // discard overlapping samples. Smooth clock drift without splicing
            // the waveform at each read boundary.
            if block.continuous {
                start = expected_start;
                self.clock_error += (error - self.clock_error) * 0.02;
                let count = (block.len / channels).max(1) as f64;
                step *= 1. + (self.clock_error / count * 0.02).clamp(-0.001, 0.001);
            } else {
                self.clock_error = 0.;
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
    queue: &queue::WorkQueue,
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
    let mut pending_audio: Option<AudioBlock> = None;
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
        if let Some(request) = queue.allocation_request() {
            if request.generation == shared.generation.load(Ordering::Acquire) {
                session = None;
                history.clear();
                pending_save = None;
                if queue.prepare(request, &shared.generation, &shared.stop)? {
                    shared
                        .staging_bytes
                        .store(request.plan.overhead, Ordering::Release);
                    shared.processed_at.store(0, Ordering::Relaxed);
                }
            }
        }
        let surface_overhead = shared.staging_bytes.load(Ordering::Acquire);
        let mut limit = config
            .budget()
            .saturating_sub(surface_overhead)
            .saturating_sub(shared.saving_bytes.load(Ordering::Acquire));
        history.trim(limit);
        // FIFO: temporary encoder delays must not throw away queued pictures.
        if let Some(frame) = queue.receive(Duration::from_millis(5)) {
            if frame.generation != shared.generation.load(Ordering::Acquire) {
                queue.recycle(frame.generation, frame.rgba);
                continue;
            }
            if session.as_ref().is_none_or(|s| {
                (s.width, s.height, s.video.rate) != (frame.width, frame.height, frame.rate)
            }) {
                history.clear();
                pending_save = None;
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
                    generation: frame.generation,
                });
                limit = config
                    .budget()
                    .saturating_sub(surface_overhead)
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
            shared.processed_at.store(frame.at, Ordering::Release);
            queue.recycle(frame.generation, frame.rgba);
        }

        // A fixed bound also prevents capture bursts from starving the video branch.
        for _ in 0..32 {
            if session.is_none() {
                break;
            }
            let Some(block) = next_audio_block(
                &mut pending_audio,
                &audio_rx,
                shared.processed_at.load(Ordering::Acquire),
                queue.len() > 0 || shared.gpu_pending.load(Ordering::Relaxed) > 0,
            ) else {
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
                shared.message("Waiting for queued frames before saving replay…");
            }
        }
        if let Some((save, requested)) = &pending_save {
            // Wait for asynchronously read-back/encoded packets through the keypress. A
            // stalled source gets a bounded timeout and a truthful shorter clip.
            if history.end().is_some_and(|end| end >= save.at)
                || save_wait_expired(
                    requested.elapsed(),
                    save.at,
                    shared.processed_at.load(Ordering::Acquire),
                    queue.len(),
                    shared.gpu_pending.load(Ordering::Relaxed),
                )
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
            status.overhead = shared.staging_bytes.load(Ordering::Acquire);
            status.available = available;
            status.surface = session.as_ref().map(|s| (s.width, s.height));
            status.codec = config.codec.encoder().into();
            status.video_dropped = shared.video_dropped.load(Ordering::Relaxed);
            status.audio_dropped = shared.audio_dropped.load(Ordering::Relaxed);

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
fn next_audio_block(
    pending: &mut Option<AudioBlock>,
    audio: &Receiver<AudioBlock>,
    video_at: i64,
    backlogged: bool,
) -> Option<AudioBlock> {
    let block = pending.take().or_else(|| audio.try_recv().ok())?;
    // Keep A/V history near the same media time while video catches up;
    // newer audio must not evict queued older video from short histories.
    if backlogged && block.at > video_at + 50_000 {
        *pending = Some(block);
        None
    } else {
        Some(block)
    }
}
fn save_wait_expired(
    elapsed: Duration,
    requested_at: i64,
    processed_at: i64,
    cpu_pending: usize,
    gpu_pending: usize,
) -> bool {
    // A busy queue may still contain the button-time picture. Allow catch-up,
    // while bounding waits if a driver/stream never makes further progress.
    elapsed > Duration::from_secs(30)
        || (elapsed > Duration::from_secs(2)
            && (processed_at >= requested_at || (cpu_pending == 0 && gpu_pending == 0)))
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
    let path = dir.join(format!("replay-{time}.mp4"));
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
            continuous: false,
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
        for gap_us in [1000, 20_000] {
            clock.append(&block(0, 48000, 960), 0).unwrap();
            clock
                .append(&block(20_000 + gap_us, 48000, 960), 0)
                .unwrap();
            assert!(
                clock.pending.iter().filter(|s| s[0] == 0.).count() >= gap_us as usize * 48 / 1000
            );
            clock = AudioClock::new();
        }
    }
    #[test]
    fn jitter_and_clock_drift_do_not_accumulate_audio_desync() {
        let mut clock = AudioClock::new();
        let actual_rate = 48000. * 1.0001;
        for i in 0..1000 {
            let jitter = if i % 2 == 0 { 100. } else { -100. };
            let at = (i as f64 * 960. / actual_rate * 1e6 + jitter).max(0.) as i64;
            let mut input = block(at, 48000, 960);
            input.continuous = i > 0;
            clock.append(&input, 0).unwrap();
            while clock.take().is_some() {}
        }
        let expected = (960000. / actual_rate * 48000.) as i64;
        assert!(
            (clock.next - expected).abs() < 48,
            "Recording clock drift exceeded 1ms"
        );
    }
    #[test]
    fn short_reads_and_timestamp_jitter_keep_the_recorded_waveform_continuous() {
        use ffmpeg_next as ff;
        ff::init().unwrap();
        let (tap, rx, free, dropped) = super::super::tests::test_tap(32);
        let mut clock = AudioClock::new();
        let mut encoder = AudioEncoder::new().unwrap();
        let mut decoder = ff::codec::context::Context::from_parameters(encoder.parameters())
            .unwrap()
            .decoder()
            .audio()
            .unwrap();
        unsafe {
            (*decoder.as_mut_ptr()).pkt_timebase = ff::Rational(1, 1_000_000).into();
        }
        let mut recorded = Vec::<[f32; 2]>::new();
        let mut decoded = Vec::<[f32; 2]>::new();
        let epoch = 1_000_000;
        let mut next_pts = 0;
        for read in 0..2000 {
            let mut samples = [0.; 96];
            for (i, pair) in samples.chunks_exact_mut(2).enumerate() {
                let time = (read * 48 + i) as f64 / 48000.;
                pair[0] = (time * 440. * std::f64::consts::TAU).sin() as f32 * 0.4;
                pair[1] = (time * 997. * std::f64::consts::TAU).sin() as f32 * 0.3;
            }
            // Delayed/early timestamp observations must never be interpreted as
            // missing PCM. Model video work blocking audio service for 100ms.
            let jitter = if read == 0 {
                0
            } else if read / 40 % 2 == 0 {
                -8000
            } else {
                8000
            };
            tap.submit(&samples, 48000, 2, epoch + read as i64 * 1000 + jitter);
            if read % 100 != 99 {
                continue;
            }
            while let Ok(input) = rx.try_recv() {
                assert!(!clock.append_recovering(&input, epoch).unwrap());
                while let Some((at, samples)) = clock.take() {
                    assert_eq!(at, next_pts);
                    next_pts += 960;
                    recorded.extend_from_slice(&samples);
                    for packet in encoder.encode(&samples, at).unwrap() {
                        decoder.send_packet(&packet.packet).unwrap();
                        let mut frame = ff::frame::Audio::empty();
                        while decoder.receive_frame(&mut frame).is_ok() {
                            match frame.format() {
                                ff::format::Sample::F32(ff::format::sample::Type::Planar) => {
                                    decoded.extend(
                                        frame
                                            .plane::<f32>(0)
                                            .iter()
                                            .zip(frame.plane::<f32>(1))
                                            .map(|(l, r)| [*l, *r]),
                                    );
                                }
                                ff::format::Sample::F32(ff::format::sample::Type::Packed) => {
                                    decoded.extend(
                                        frame.plane::<(f32, f32)>(0).iter().map(|(l, r)| [*l, *r]),
                                    );
                                }
                                other => panic!("Unexpected decoder format {other:?}"),
                            }
                        }
                    }
                }
                free.send(input).unwrap();
            }
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        assert!(decoded.len() > 94000);
        for waveform in [&recorded, &decoded] {
            // Skip Opus startup lookahead. Check every 5ms for holes and each
            // sample boundary for splices, in both independently generated channels.
            for window in waveform[960..].chunks_exact(240) {
                for (channel, minimum) in [(0, 0.20), (1, 0.15)] {
                    let rms =
                        (window.iter().map(|v| v[channel].powi(2)).sum::<f32>() / 240.).sqrt();
                    assert!(rms > minimum, "Silent/skipped recording interval: {rms}");
                }
            }
            for pair in waveform[960..].windows(2) {
                assert!(
                    (pair[1][0] - pair[0][0]).abs() < 0.04,
                    "Left channel splice"
                );
                assert!(
                    (pair[1][1] - pair[0][1]).abs() < 0.06,
                    "Right channel splice"
                );
            }
        }
        assert!((clock.next - 96000).abs() < 48, "Audio drift exceeded 1ms");
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
    fn audio_waiting_for_video_catchup_keeps_all_samples_and_the_original_timeline() {
        let (tx, rx) = crossbeam_channel::bounded(128);
        for i in 0..100 {
            let mut input = block(i * 40_000, 48000, 1920);
            input.continuous = i > 0;
            tx.send(input).unwrap();
        }
        let mut pending = None;
        let mut clock = AudioClock::new();
        let mut received = 0;
        for video_tick in 0..240 {
            let video_at = video_tick * 1_000_000 / 60;
            while let Some(input) = next_audio_block(&mut pending, &rx, video_at, true) {
                assert_eq!(input.at, received * 40_000);
                assert!(input.at <= video_at + 50_000);
                assert!(!clock.append_recovering(&input, 0).unwrap());
                while let Some((_, samples)) = clock.take() {
                    assert!(samples.iter().all(|s| *s == [0.5, 0.5]));
                }
                received += 1;
            }
        }
        assert_eq!(received, 100);
        assert!(pending.is_none() && rx.is_empty());
        assert_eq!(clock.next, 4 * 48000);
    }
    #[test]
    fn save_waits_for_queued_button_time_frames_but_cannot_wait_forever() {
        let elapsed = Duration::from_secs(3);
        assert!(!save_wait_expired(elapsed, 1000, 900, 8, 0));
        assert!(!save_wait_expired(elapsed, 1000, 900, 0, 8));
        assert!(save_wait_expired(elapsed, 1000, 1001, 8, 8));
        assert!(save_wait_expired(elapsed, 1000, 900, 0, 0));
        assert!(save_wait_expired(Duration::from_secs(31), 1000, 900, 8, 8));
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
