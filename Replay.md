# Live replay buffer

Replay is opt-in and starts **disabled** on every launch. Start the capture stream,
open Controls (`M`), expand **Live replay buffer**, then enable it. Defaults:

- 5-minute maximum history and a 1024 MiB CPU memory budget.
- Hardware AV1 through VAAPI, quantizer 20, stereo Opus at 192 kbit/s, Matroska files.
- F5 / F6 / F7 / F8 / F9 save approximately 30 / 60 / 180 / 300 / 600 seconds.
- F10 saves the customizable duration (initially 120 seconds).
- Files go to `$HOME/Videos/Michadame`; folder and bindings are configurable.

Shortcuts work in either focused Michadame window, outside text editing. They are
not desktop-global shortcuts. Save buttons are also available in Controls.
All durations share one buffer and clamp to the retained decodable A/V history.
The beginning rounds forward to a keyframe, so a clip can be about one second
shorter. It never includes future gameplay after the keypress. Saving waits up to
two seconds for pending encoding; a stalled stream produces a shorter available
clip or an explicit insufficient-history message.

## Performance and memory behavior

The replay path reads the final rendered game framebuffer before egui overlays;
shader changes are baked into each captured image. The existing shader chain is
not rendered twice. Three PBOs use zero-timeout GPU fences. A bounded two-frame
queue feeds a lower-priority recording worker. It converts bottom-up RGB to NV12,
uploads to VAAPI, encodes, and retains compressed packets. This first version uses
asynchronous GPU readback and a CPU conversion/upload path, **not zero-copy GL to
VAAPI sharing**. GPU copies can still consume bandwidth or stall inside a driver.
Hardware performance is unvalidated; no zero-overhead guarantee is made.

The ALSA capture thread delivers playback samples first, then copies into a
separate pool of 32 preallocated recording blocks with `try_lock`/`try_send`.
The Rodio/CPAL playback callback is unchanged. Recording never reads from the
playback ring. When disabled, the audio tap performs no additional ALSA query.
Capture timestamps use the monotonic clock; negotiated fractional video rates are
kept in replay metadata. Audio timestamps account for ALSA queued samples.
A recording-only resampler filters arrival jitter and corrects drift. Short gaps
become silence/repeated video frames; long gaps reset history. USB/device latency
that the driver does not expose still needs empirical validation.

Encoded packet memory is capped after a conservative CPU staging/encoder
allowance (10 RGBA surfaces plus 64 MiB). Only one export runs at a time. Packets
retained by the writer are also charged, conservatively including shared packet
references in both histories. Saving can shorten the live history to stay within
the budget. Driver-owned GPU memory is additional, not a measured part of this
CPU budget. The budget is not a promise of a precise process RSS ceiling.

The controls show available RAM before enabling, projected remaining RAM, current
available RAM, allocated packet bytes, staging allowance, skipped recording work,
actual retained duration and an estimate from observed bitrate. Admission leaves
512 MiB of system headroom. Recording stops if available RAM falls below that
reserve. Swap is not counted as replay capacity. Fixed-quality compression has
variable bitrate, so a fixed RAM budget cannot guarantee a fixed history length.

Disabling releases the recording worker, history and GPU readback resources;
encoder teardown never joins on the UI thread. An export already in progress can
finish and retains its packets until then. Re-enabling waits until previous worker
and writer references are gone. There is no software video encoder fallback:
unsupported VAAPI settings stop replay and leave playback running. AV1, HEVC and
H.264 can be selected explicitly; settings that change the encoder require replay
to be disabled first.

## Surface changes and files

Window size/DPI/fullscreen changes invalidate history immediately. Recording
resumes after 300 ms of stable dimensions. The visible image remains at the
rendered surface size. Encoder surfaces are padded to 64×16 alignment and
Matroska pixel-crop metadata removes the padding, including odd window sizes.
Players must honor Matroska crop metadata. The pixels are not stretched.

Capture interruptions, long missing rendered-frame intervals (including minimized
windows) and restarting audio reset/suspend recording. Disabling or stopping the
stream cancels pending save requests; already-started exports finish independently.
Recordings contain capture-card audio, not system output or output-volume changes.
SDR RGB is converted to limited-range BT.709 matrix YUV with the rendered sRGB
transfer tagged. Hardware 4:2:0 AV1/HEVC/H.264 is lossy; colored CRT masks and fine
text need quality evaluation. Lower quantizers improve fidelity at larger sizes.

Files are first written as hidden `.partial` files. Success publishes a unique
`.mkv` name only after its trailer is written; existing recordings are not
replaced. The output folder must support hard links (normal Linux filesystems do).
An interrupted process can leave a partial file; it is not reported as a saved clip.

## Validation without launching Michadame

```
cargo test --locked -- --skip test_handle_device_scan_result_success --skip test_apply_config_hardware_settings
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
cargo build --release --locked
```

The two skipped legacy tests can traverse configuration paths that open
`/dev/video0`. The synthetic media test uses software FFV1 fixtures and Opus,
exports through the real muxer, demuxes and decodes the result, and checks duration,
A/V start alignment, content and Matroska cropping. Other tests cover byte/time
limits, keyframe dependencies, truncated clips, fractional FPS, audio drift/gaps,
configuration compatibility, and nonblocking audio taps. None of these replay
tests opens a capture device, a display or a hardware encoder.

## Manual validation by the user

This implementation is awaiting hardware validation. Suggested first pass:

1. Compare normal playback with replay disabled and enabled, starting at 1080p.
   Check visible smoothness and audio latency; inspect the replay status for
   VAAPI errors or skipped work. Confirm no playback change on codec failure.
2. Save a short clip containing an obvious audiovisual event. Check lip sync or
   a button/sound event near both ends, including a longer 5–10 minute session.
3. Toggle CRT, FFT, pixelation and upscalers while buffering. Confirm the clip
   contains the same changes and colors, with no dialogs or toasts recorded.
4. Resize, enter fullscreen and try an odd-sized window. Confirm history resets
   and the player's displayed image dimensions match the video surface.
5. Try longer shortcuts with a partly filled or memory-limited buffer. Confirm
   a valid shorter clip and accurate saved-duration notification.
6. Save repeatedly, disable while saving, and stop/restart capture. Confirm no
   hangs, bounded memory, and eventual memory release. Compare the most demanding
   shader/resolution combination off/on before accepting performance.

If CPU conversion/readback is too costly on the actual setup, the next step is
GL/VAAPI shared-surface interoperability. That requires driver-specific validation
and is intentionally not claimed by this first version.
