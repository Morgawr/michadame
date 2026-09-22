# Live replay buffer

Replay is opt-in and starts **disabled** on every launch. Start the capture stream,
open Controls (`M`), then enable **Live replay buffer**. Defaults:

- 5-minute maximum history and a 1024 MiB total replay memory budget.
- Up to 512 MiB of that budget reserved for unencoded GPU/CPU work queues.
- Hardware AV1 through VAAPI, VBR target 30 Mbit/s with a 40 Mbit/s limit,
  stereo Opus at 192 kbit/s, MP4 files.
- F5 / F6 / F7 / F8 / F9 save approximately 30 / 60 / 180 / 300 / 600 seconds.
- F10 saves the customizable duration (initially 120 seconds).
- Files go to `$HOME/Videos/Michadame`; folder and bindings are configurable.

Replay preferences (including RAM budget, history, codec, compression mode,
bitrate limit, quality, folder and
shortcuts) are saved when edited and restored on restart. Enabling replay remains
an explicit per-launch choice.

Shortcuts work in either focused Michadame window, outside text editing. They are
not desktop-global shortcuts. Bindings, custom clip duration and save buttons are
in the initially collapsed **Save shortcuts** section in Controls.
All durations share one buffer and clamp to the retained decodable A/V history.
The beginning rounds forward to a keyframe, so a clip can be about one second
shorter. It never includes future gameplay after the keypress. Saving waits for
queued button-time frames to be processed. An idle/stalled source uses a two-second
fallback; an active backlog may take up to 30 seconds before exporting the available
shorter clip or reporting insufficient history.

## Compression and player compatibility

**Bitrate limited** is the default, including when loading settings saved before
this option existed. The configurable limit is 1–200 Mbit/s; the VBR target is
75% of that limit, with a two-second rate-control buffer. The driver can allow
short bursts, so this is not a per-packet size ceiling. Raising the limit retains
more detail but increases storage, memory bandwidth and decoding cost. Lowering
it can lose detail in complex scenes or fine CRT masks. Resolution, frame timing
and rendered effects remain unchanged. Unsupported VBR settings stop replay;
there is no silent fallback to unlimited bitrate or software encoding.

**Fixed quality** remains available without a bitrate limit. Its UI scale is
1–51, lower meaning higher quality. AV1 maps this to its native quantizer index
by multiplying by five (default 20 becomes 100); H.264/HEVC use the number directly
as QP. This is a range normalization, not equivalent quality across codecs.
Fixed-quality output can exceed player capabilities and declared stream level
limits. In bitrate-limited AV1, level and tier account for the configured peak,
padded dimensions and fractional frame rate, rather than just the target bitrate.

The reported 19.37-second recording contained about 951 MB, averaging 392 Mbit/s
of video. Its AV1 quantizer was 20 on the 0–255 scale, and it declared level 5.1
Main tier, whose bitrate allowance is 40 Mbit/s. Packet timestamps increased
normally and audio/video packets were interleaved; software decoding found no
bitstream errors. The previous encoder configuration treated AV1 quantizers like
H.264/HEVC QP and allowed unlimited bitrate. See FFmpeg's
[AV1 VAAPI quantizer handling](https://raw.githubusercontent.com/FFmpeg/FFmpeg/master/libavcodec/vaapi_encode_av1.c)
and [AV1 level limits](https://raw.githubusercontent.com/FFmpeg/FFmpeg/master/libavcodec/av1_levels.c).
These changes affect new recordings. Remuxing an existing recording cannot reduce
its encoded bitrate. Actual hardware rate control and player compatibility still
need validation with a newly recorded clip.

## Performance and memory behavior

The replay path reads only the final aspect-fitted video rectangle before egui
overlays, excluding outer letterbox/pillarbox padding. For example, an unstretched
4:3 image in a 1920×1080 viewport records at 1440×1080. Bounds use the final shader
texture and horizontal stretch, in physical pixels; odd sizes round to the nearest
pixel. X/Y overscan offsets, CRT curvature, upscaling and other shader changes stay
baked into the image. The existing shader chain is not rendered twice.
A bounded GPU readback queue retains the already-shaded images; a bounded CPU
queue feeds the dedicated, lower-priority recording worker in FIFO order. Busy
encoders no longer cause older queued frames to be discarded. Completed GPU
transfers stay queued if no CPU buffer is free. New recording frames are skipped
only when capacity is exhausted; the live display never waits for queue space.

The configurable **Work queue RAM (MiB)** is part of the total budget, capped at
half of it to leave space for staging and encoded history. Equal GPU/CPU pools
hold up to 120 frames per stage. At default settings, 1920×1080 has 32 slots per
stage (about 506 MiB reserved, roughly one second combined at 60 fps); 3840×2160
has 8 slots per stage. GPU-only stalls have the GPU pool's capacity; the CPU
pool extends tolerance for encoder stalls. These are finite burst buffers, not
an assurance that sustained overload can be recorded without loss.

The worker allocates and touches reusable CPU buffers. The render callback uses
zero-timeout fences and copies at most two completed frames per draw, checking a
2 ms elapsed-work limit between copies so it can catch up without draining an
unbounded backlog in one draw. Individual driver calls can exceed that limit.
The worker converts bottom-up RGB to NV12, uploads to VAAPI, encodes, and retains
compressed packets. RGB conversion uses a bounded libswscale thread pool (half
the available CPUs, rounded down, clamped to 1–4 threads), created by the
lower-priority recording worker. It uses the
[frame conversion API](https://ffmpeg.org/doxygen/trunk/group__libsws.html),
which dispatches work to the pool; the old `sws_scale` call used only one thread.
RGBA queue storage is borrowed only during the synchronous conversion, with no
extra full-frame copy. The conversion preserves the previous BT.709 matrix,
range, chroma filtering, orientation and visible dimensions. This first version uses
asynchronous GPU readback and a CPU conversion/upload path, **not zero-copy GL to
VAAPI sharing**. GPU copies can still consume bandwidth or stall inside a driver.
Hardware performance is unvalidated; no zero-overhead guarantee is made.

The ALSA capture thread delivers playback samples first, then copies into a
separate pool of 256 preallocated recording blocks with `try_lock`/`try_send`.
Short reads are combined into approximately 40 ms blocks, providing 10.24 seconds
of queue capacity at 48 kHz stereo while the video encoder is busy. This batching
only delays recording work, never live playback. Actual capture recovery, format
changes and queue exhaustion explicitly mark discontinuities; timestamp jitter
alone cannot splice samples or insert silence into continuous captured sound.
The Rodio/CPAL playback callback is unchanged. Recording never reads from the
playback ring. When disabled, the audio tap performs no additional ALSA query.
Capture timestamps use the monotonic clock; negotiated fractional video rates are
kept in replay metadata. Audio timestamps account for ALSA queued samples.
A recording-only resampler filters arrival jitter and corrects drift. Dropped video
frames hold the preceding picture until the next received frame, using packet
duration and presentation timestamps instead of re-encoding duplicates. The
capture rate remains the nominal rate; gaps use variable frame durations. Recovery
encodes each retained picture once, in capture order, without generating duplicate
pictures. Increasing video timestamps are preserved at microsecond precision:
rounding them onto nominal FPS ticks previously discarded distinct frames when
timestamps jittered or actual FPS differed slightly from nominal FPS. Encoding a
queued burst cannot shorten its capture-time intervals. Audio processing stays near video media time during catch-up so newer
audio cannot evict queued older video from short histories. Audio keeps its own
capture timeline: short audio gaps become silence, and long gaps re-anchor only
the recording resampler without clearing the A/V history. USB/device latency
that the driver does not expose still needs empirical validation.

Encoded packet memory is capped after reserving both work pools and a conservative
staging/encoder allowance (four RGBA surfaces plus 64 MiB, including audio staging). Only one export runs at a time. Packets
retained by the writer are also charged, conservatively including shared packet
references in both histories. Saving can shorten the live history to stay within
the budget. PBO storage is conservatively charged even if the driver places it in
VRAM. Other driver-owned GPU memory is additional and not measured by this budget. The budget is not a promise of a precise process RSS ceiling.

The video window's **Debug** overlay (press `D` while focused, or click **D** at
the top-right) shows available RAM before enabling, projected remaining RAM, current
available RAM, allocated packet bytes, queue/staging reservation, GPU and CPU queue
occupancy, recording lag, separate video/audio drop counts, actual retained duration and an estimate from observed bitrate. Admission leaves
512 MiB of system headroom. Recording stops if available RAM falls below that
reserve. Swap is not counted as replay capacity. Fixed-quality compression has
variable bitrate, so a fixed RAM budget cannot guarantee a fixed history length.
The RAM admission warning and recording/save messages remain in Controls.
Debug is hidden by default and overlays the video without resizing it or appearing
in replay recordings. It also shows measured UI/received-video FPS, nominal source
FPS, decoded/window sizes, configured stream and shader options, and live audio
queue depth and estimated latency. Playback video queue drops are counted per
stream; playback underrun silence and clock-drift drops are counted per audio
stream. These counters describe application queues, not losses inside the capture
device or driver, and remain distinct from replay video/audio drops. Audio counters
use relaxed atomics once per refill or discontinuity, with no added buffering or
blocking in the playback callback. The detailed panel refreshes even during a stall.
Persistent **History resets / Last reset** fields distinguish format resets from
frame drops. **History emptied by limits** separately reports loss of the final
decodable group to the RAM or age cap; ordinary rolling eviction does not count.
Format resets also log their reason on the recording worker.
The video-drop total is cumulative for the enabled session. A separate recent
increment shows whether drops are still occurring. Average CPU conversion and
GPU upload/encode times per frame update alongside the nominal frame interval;
they exclude readback, audio and other worker work. If queues remain full,
recording throughput is insufficient: increasing queue memory only postpones
drops, even while the encoded-history buffer is mostly empty.

Disabling releases the recording worker, history and GPU readback resources;
encoder teardown never joins on the UI thread. An export already in progress can
finish and retains its packets until then. Re-enabling waits until previous worker
and writer references are gone. There is no software video encoder fallback:
unsupported VAAPI settings stop replay and leave playback running. AV1, HEVC and
H.264 can be selected explicitly; settings that change the encoder require replay
to be disabled first.

## Surface changes and files

Changes to the recorded image dimensions (including window size, DPI, fullscreen
or horizontal stretch) invalidate history only after 300 ms of stable, nonzero
dimensions. During resizing, readback pauses while the existing history remains
savable; returning to the original dimensions resumes without resetting. Temporary
zero-sized/minimized surfaces do not invalidate history. Changes only to image position or outer padding
preserve history when the recorded dimensions stay the same. The visible image
remains at the rendered surface size. Encoder surfaces are padded to 64×16 alignment and
MP4 clean-aperture metadata removes the padding, including odd window sizes.
Players must honor MP4 clean-aperture metadata. The pixels are not stretched.

Missing rendered-frame intervals (including minimized windows) pause video progress
without clearing history or cancelling saves. On resumption the preceding picture
holds over the gap and recording continues on the same timeline. Normal history
age/RAM eviction still applies; an indefinite outage cannot preserve history beyond
those limits. Late/out-of-order or missing video timestamps do not reset history:
old pictures are skipped until increasing timestamps resume, while queued work can
still drain. Repeated/stale work-pool allocation requests cannot clear queued frames
or encoded history. Committed size/rate changes reset recording. Disabling or stopping the
stream cancels pending save requests; already-started exports finish independently.
Recordings contain capture-card audio, not system output or output-volume changes.
SDR RGB is converted to limited-range BT.709 matrix YUV with the rendered sRGB
transfer tagged. Hardware 4:2:0 AV1/HEVC/H.264 is lossy; colored CRT masks and fine
text need quality evaluation. Lower quantizers improve fidelity at larger sizes.

Files are first written as hidden `.partial` files. Success publishes a unique
`.mp4` name only after its trailer is written; existing recordings are not
replaced. The output folder must support hard links (normal Linux filesystems do).
An interrupted process can leave a partial file; it is not reported as a saved clip.

## Validation without launching Michadame

```
cargo test --locked -- --skip test_handle_device_scan_result_success --skip test_apply_config_hardware_settings
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
cargo build --release --locked
```

Optional synthetic CPU-only benchmark at 3024×2160 (no display/capture/GPU access):

```
cargo test --release --locked benchmark_replay_conversion -- --ignored --nocapture
```

The local benchmark measured roughly 15.4 / 8.3 / 4.6 ms per frame at 1 / 2 / 4
conversion threads, versus a 16.7 ms frame interval at 60 fps. These numbers
exclude upload/encoding and do not establish hardware playback performance.
Regression tests compare visible Y/UV bytes against the original conversion at
1/2/4 threads, including odd dimensions and the reported recording size. MP4
fixtures verify exact timestamp/duration preservation through H.264, HEVC and
AV1 export with closely spaced frames. A 30-second timestamp-jitter fixture
checks that every valid frame survives scheduling. Encoder option tests parse
VBR/CQP settings for all three codecs using FFmpeg codec contexts without opening
an encoder or hardware device. Native quantizer mapping, AV1 level/tier selection
and legacy configuration defaults have regression coverage.

The two skipped legacy tests can traverse configuration paths that open
`/dev/video0`. The synthetic media test uses software H.264, HEVC and AV1 fixtures
with Opus, exports through the real muxer, demuxes and decodes the result, and checks duration,
A/V start alignment, content and MP4 cropping. Other tests cover byte/time
limits, keyframe dependencies, truncated clips, fractional FPS, audio drift/gaps,
configuration compatibility, FIFO frame retention through simulated encoder stalls,
full-pool backpressure, buffer reuse, resize/cancellation, budget calculations,
backlogged save requests, nonblocking audio taps, and recovery after isolated
missing frames, bursts, >1-second stalls and queue saturation. The dropped-frame
media fixture checks retained pre-gap content, held-frame timing, resumed video,
audio sync and save snapshots. Audio regressions feed 1 ms stereo reads through
the real tap, resampler and Opus encoder/decoder with 100 ms servicing delays and
8 ms timestamp jitter, checking waveform continuity, channel energy and drift.
Configuration tests use actual TOML files and reload settings into fresh state.
Reset regressions cover backwards timestamps, missing timestamps, minimized or
transient dimensions, one committed reset after resizing settles, and repeated
allocation requests. A ten-minute simulated 60 fps capture repeatedly saturates
the FIFO and injects transient inputs while retaining five minutes of packet
history. It exercises the capture policy, queue, video scheduler and packet ring
with synthetic packets; actual codec/muxing behavior has separate media tests.
None of these replay
tests opens a capture device, a display or a hardware encoder.

## Manual validation by the user

This implementation is awaiting hardware validation. Suggested first pass:

1. Compare normal playback with replay disabled and enabled, starting at 1080p.
   Check visible smoothness and audio latency; inspect the replay status for
   VAAPI errors, queue occupancy, recording lag and video/audio drop counters.
   Confirm no playback change on codec failure. Cause a brief load spike and check
   that queued work drains afterward, preserving the recording's frame sequence.
2. Save a short clip containing an obvious audiovisual event. Check lip sync or
   a button/sound event near both ends, including a longer 5–10 minute session.
3. Toggle CRT, FFT, pixelation and upscalers while buffering. Confirm the clip
   contains the same changes and colors, with no dialogs or toasts recorded.
4. Try 4:3 and widescreen video, horizontal stretch, and X/Y overscan offsets.
   Verify exports exclude outer padding and retain the displayed adjustments.
   Resize, enter fullscreen and try an odd-sized window. Confirm image dimension
   changes reset history and the player displays the rendered video size.
5. Try longer shortcuts with a partly filled or memory-limited buffer. Confirm
   a valid shorter clip and accurate saved-duration notification.
6. Save repeatedly, disable while saving, and stop/restart capture. Confirm no
   hangs, bounded memory, and eventual memory release. Compare the most demanding
   shader/resolution combination off/on before accepting performance.

If CPU conversion/readback is too costly on the actual setup, the next step is
GL/VAAPI shared-surface interoperability. That requires driver-specific validation
and is intentionally not claimed by this first version.
