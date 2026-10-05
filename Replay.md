# Live replay buffer

Replay is opt-in and starts **disabled** on every launch. Start the capture stream,
open Controls (`M`), then check **Enable replay buffer** near the top. Replay
options are at the bottom of Controls. Defaults:

- 5-minute maximum history and a 1024 MiB total replay memory budget.
- Up to 512 MiB of that budget reserved for unencoded GPU/CPU work queues.
- Hardware H.264 (VAAPI) locked to 60 FPS CFR, 8 Mbit/s target with a 10 Mbit/s limit,
  stereo AAC-LC at 160 kbit/s, faststart-enabled MP4 files (`movflags=+faststart`).
- F5 / F6 / F7 / F8 / F9 save approximately 30 / 60 / 180 / 300 / 600 seconds.
- F10 saves the customizable duration (initially 120 seconds).
- Files go to `$HOME/Videos/Michadame`; folder and bindings are configurable.

Replay preferences (including RAM budget, work queue RAM, history duration, folder and
shortcuts) are saved when edited and restored on restart. Video saving uses a fixed,
widely-compatible profile across all export actions (Ctrl+C clipboard copy, F5–F10 save
shortcuts, and manual save). Enabling replay remains an explicit per-launch choice.

Shortcuts work in either focused Michadame window, outside text editing. They are
not desktop-global shortcuts. Bindings, custom clip duration and save buttons are
in the initially collapsed **Save shortcuts** section in Controls.
All durations share one buffer and clamp to the retained decodable A/V history.
The beginning rounds forward to a keyframe, so a clip can be about one second
shorter. It never includes future gameplay after the keypress. Saving waits for
queued button-time frames to be processed. An idle/stalled source uses a two-second
fallback; an active backlog may take up to 30 seconds before exporting the available
shorter clip or reporting insufficient history.

## Copy a clip to the Linux clipboard

With replay enabled, press **Ctrl+C** in the focused video window, then wait for
the **Copied … to clipboard** notification before pasting. This copies up to the
last fifteen seconds as an MP4 file attachment, with the same audio, rendered effects,
codec, crop and timestamps as a normal replay save. The clip can be shorter when
history is limited or its start must advance to a keyframe. It uses the existing
background export queue; copying and saving cannot run simultaneously. Plain
`C` still cycles CRT filters. Copying text in Controls is unaffected.

Press **Ctrl+Shift+C** instead to copy only the audio of the last 7 seconds as an
MP3 file (48 kHz stereo, 192 kbps CBR). The recorded audio is decoded and
re-encoded with FFmpeg's `libmp3lame`; no video keyframe is required, so the clip
is not shortened to a keyframe. Missing audio stays as silence at its original
position. The MP3 uses the same RAM-only `/tmp` directory, reservation and
background export queue as video copies, and only one clipboard clip, audio or
video, is kept at a time.

Linux browsers normally expose pasted video attachments through file references,
not arbitrary raw `video/mp4` bytes. This implementation uses arboard's native
Wayland data-control / X11 file-list support (`text/uri-list`). See
[Chromium's paste handling](https://raw.githubusercontent.com/chromium/chromium/main/third_party/blink/renderer/core/clipboard/data_object.cc)
and [arboard file lists](https://docs.rs/arboard/3.6.1/arboard/struct.Set.html#method.file_list).
No external `wl-copy` or `xclip` executable is required. A desktop without supported
clipboard access reports a copy failure.

The video is created in a private directory under `/tmp` with owner-only file
permissions. The application verifies tmpfs before writing video and refuses a
disk-backed `/tmp`; it does not fall back to the save folder or `$TMPDIR`.
The current clipboard file remains available after disabling replay, until the
next successful replay copy or normal application exit. Failed copies release
their files and reservations and retain the previous clip's backing file.
Do not replace the clip or exit until the receiving application has read it.
Clipboard history entries are not durable recordings; a history-exclusion hint
is supplied for managers that support it. Abrupt process termination can leave
temporary files until `/tmp` is cleaned. Like other tmpfs data, pages can be
swapped if the system permits it ([kernel documentation](https://www.kernel.org/doc/html/latest/filesystems/tmpfs.html)).

Clipboard output is capped at 256 MiB and charged to the replay RAM budget,
including the previous clipboard clip and in-flight export. A bounded seekable
writer enforces the reservation while muxing. Debug shows clipboard memory
separately from retained packets. Copying remuxes encoded packets without
re-encoding or changing quality. Website file-size limits and codec/paste support
still apply: fifteen seconds at an 8 Mbit/s video target is about 15 MB with audio,
well within Discord's upload limits. See
[Discord's attachment limits](https://support.discord.com/hc/en-us/articles/25444343291031-File-Attachments-FAQ).
Browser/website paste, sandboxed browser access to `/tmp`, and desktop clipboard
ownership need user validation; local tests do not access the live clipboard.

## Compression and player compatibility

All replay video saving and clipboard export features use a unified, highly-compatible
recording profile designed to play smoothly across Discord, web browsers, and iOS:

- **Hardware H.264 (`h264_vaapi`)**: Standard H.264 High Profile encoding via VAAPI,
  with VBR rate control set to an 8 Mbit/s target, 10 Mbit/s peak, and 20 Mbit buffer size.
  This produces crisp quality for gameplay and CRT shaders while keeping clip file sizes
  modest and well within chat and upload limits (e.g., 15s clipboard clip is ~15 MB).
- **Strict 60 FPS Constant Frame Rate (CFR)**: Capture timestamps are mapped onto a
  fixed 60 FPS timeline (`1/60` second grid) with an exact `(1, 15360)` stream timebase
  (256 ticks per frame with zero rounding error). Missing frames up to 1 second are filled
  by referencing the previous picture without re-encoding duplicate pixels; stalls longer
  than 1 second resume cleanly with an IDR keyframe without catch-up stutter.
- **Faststart MP4 container (`movflags=+faststart`)**: The MP4 `moov` atom (metadata index)
  is placed before media data (`mdat`), allowing instant playback in Discord, iOS Safari,
  and web players without waiting for the entire file to download or buffer.
- **Stereo AAC-LC audio**: 48 kHz stereo encoded with FFmpeg's native `aac` encoder at
  160 kbit/s (1024 samples per frame), universally supported on Apple and mobile platforms.
- **Native irregular dimensions and display crop**: Non-standard video dimensions (such as
  unscaled retro resolutions or custom aspect ratios) are preserved exactly as rendered.
  Internal hardware alignment padding uses standard MP4 container display crop metadata
  (`set_display_crop`) so the player displays only the intended active viewport without
  stretching or artificial black bars.

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

The video window's **Debug** overlay (press `D` while focused) shows available
RAM before enabling, projected remaining RAM, current
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
with AAC, exports through the real muxer, demuxes and decodes the result, and checks duration,
A/V start alignment, content and MP4 cropping. Other tests cover byte/time
limits, keyframe dependencies, truncated clips, fractional FPS, audio drift/gaps,
configuration compatibility, FIFO frame retention through simulated encoder stalls,
full-pool backpressure, buffer reuse, resize/cancellation, budget calculations,
backlogged save requests, nonblocking audio taps, and recovery after isolated
missing frames, bursts, >1-second stalls and queue saturation. The dropped-frame
media fixture checks retained pre-gap content, held-frame timing, resumed video,
audio sync and save snapshots. Audio regressions feed 1 ms stereo reads through
the real tap, resampler and AAC encoder/decoder with 100 ms servicing delays and
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
