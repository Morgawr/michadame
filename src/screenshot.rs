//! Fullscreen screenshot (`S`) and realistic smartphone monitor photo (`Ctrl+S`) capture
//! and RAM-backed clipboard publishing.

use crate::devices::filter_type::CrtFilter;
use crate::replay::clipboard::Clipboard;
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use image::{codecs::jpeg::JpegEncoder, ColorType, RgbaImage};
use rand::Rng;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};

/// If the video paint callback hasn't picked up a screenshot request within this timeout,
/// the pending request is cleared so it cannot get stuck.
const CAPTURE_TIMEOUT: Duration = Duration::from_millis(1500);

/// JPEG quality for normal (`S`) screenshots: crisp visual quality with compact file size.
const NORMAL_SCREENSHOT_JPEG_QUALITY: u8 = 88;

/// Screenshot capture mode triggered by keyboard hotkeys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotMode {
    /// Normal clean fullscreen screenshot (`S`).
    Normal,
    /// Realistic low-quality smartphone photo of the monitor (`Ctrl+S`).
    PhonePhoto,
}

/// Snapshot of the active display, shader, and room lighting state at the moment a screenshot
/// is requested. Used by [`ScreenshotMode::PhonePhoto`] to synthesize the physical monitor,
/// bezel/toolbar, glass reflections, and room lighting.
#[derive(Clone, Debug)]
pub struct PhotoSceneContext {
    pub crt_filter: CrtFilter,
    pub dark_room: bool,
    pub retro_pc_frame_already_drawn: bool,
    pub crt_glass_already_drawn: bool,
    pub is_fullscreen: bool,
    pub window_title: String,
    pub warp: [f32; 2],
    pub corner_size: f32,
    pub border_crop: [f32; 4],
}

impl Default for PhotoSceneContext {
    fn default() -> Self {
        Self {
            crt_filter: CrtFilter::Off,
            dark_room: false,
            retro_pc_frame_already_drawn: false,
            crt_glass_already_drawn: false,
            is_fullscreen: false,
            window_title: "Michadame".to_string(),
            warp: [0.031, 0.041],
            corner_size: 0.0,
            border_crop: [0.0; 4],
        }
    }
}

/// Pending screenshot capture request waiting for the next video frame render.
#[derive(Clone, Debug)]
pub struct ScreenshotRequest {
    pub mode: ScreenshotMode,
    pub scene: PhotoSceneContext,
    pub requested_at: Instant,
}

/// Result of a background screenshot encode + clipboard publish task.
#[derive(Clone, Debug)]
pub enum ScreenshotEvent {
    Copied(ScreenshotMode),
    Failed(ScreenshotMode, String),
}

/// Manages pending screenshot requests and background clipboard worker notifications.
pub struct ScreenshotState {
    pending: Arc<Mutex<Option<ScreenshotRequest>>>,
    in_flight: Arc<AtomicBool>,
    tx: Sender<ScreenshotEvent>,
    rx: Receiver<ScreenshotEvent>,
}

impl Default for ScreenshotState {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self {
            pending: Arc::new(Mutex::new(None)),
            in_flight: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
        }
    }
}

impl ScreenshotState {
    /// Queues a screenshot capture for the next rendered video frame.
    /// Returns `false` if a screenshot capture or clipboard copy is already in progress.
    pub fn request(&mut self, mode: ScreenshotMode, scene: PhotoSceneContext) -> bool {
        if self.in_flight.load(Ordering::Acquire) {
            return false;
        }
        let mut guard = match self.pending.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        if guard.is_some() {
            return false;
        }
        *guard = Some(ScreenshotRequest {
            mode,
            scene,
            requested_at: Instant::now(),
        });
        true
    }

    /// Returns `true` if a screenshot request is waiting to be captured on the current frame.
    pub fn has_pending(&self) -> bool {
        self.pending.lock().map(|p| p.is_some()).unwrap_or(false)
    }

    /// Returns the mode of the currently pending screenshot request, if any.
    pub fn pending_mode(&self) -> Option<ScreenshotMode> {
        self.pending
            .lock()
            .ok()
            .and_then(|p| p.as_ref().map(|r| r.mode))
    }

    /// Returns a cloneable handle for the GPU paint callback in `draw_video_player`.
    pub fn capture_handle(&self) -> ScreenshotCaptureHandle {
        ScreenshotCaptureHandle {
            pending: self.pending.clone(),
            in_flight: self.in_flight.clone(),
            tx: self.tx.clone(),
        }
    }

    /// Polls completed screenshot events and expires timed-out pending requests.
    pub fn poll_events(&mut self) -> Vec<ScreenshotEvent> {
        if let Ok(mut guard) = self.pending.lock() {
            if let Some(req) = guard.as_ref() {
                if req.requested_at.elapsed() > CAPTURE_TIMEOUT {
                    let mode = req.mode;
                    *guard = None;
                    let _ = self.tx.send(ScreenshotEvent::Failed(
                        mode,
                        "Timed out waiting for video frame".to_string(),
                    ));
                }
            }
        }
        let mut events = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            events.push(ev);
        }
        events
    }
}

/// Cloneable handle passed into the video player's post-background `PaintCallback`.
#[derive(Clone)]
pub struct ScreenshotCaptureHandle {
    pending: Arc<Mutex<Option<ScreenshotRequest>>>,
    in_flight: Arc<AtomicBool>,
    tx: Sender<ScreenshotEvent>,
}

impl ScreenshotCaptureHandle {
    pub fn has_pending(&self) -> bool {
        self.pending.lock().map(|p| p.is_some()).unwrap_or(false)
    }

    pub fn pending_mode(&self) -> Option<ScreenshotMode> {
        self.pending
            .lock()
            .ok()
            .and_then(|p| p.as_ref().map(|r| r.mode))
    }

    pub fn take_pending(&self) -> Option<ScreenshotRequest> {
        self.pending.lock().ok()?.take()
    }

    /// Processes the captured RGBA frame on a background thread and publishes the encoded
    /// image directly to the RAM-backed system clipboard.
    pub fn save_to_clipboard(
        &self,
        request: ScreenshotRequest,
        shot: Option<(Vec<u8>, u32, u32)>,
        clipboard: Arc<Clipboard>,
        ctx: egui::Context,
    ) {
        let Some((pixels, width, height)) = shot else {
            let _ = self.tx.send(ScreenshotEvent::Failed(
                request.mode,
                "Failed to read framebuffer pixels".to_string(),
            ));
            ctx.request_repaint();
            return;
        };

        self.in_flight.store(true, Ordering::Release);
        let in_flight = self.in_flight.clone();
        let tx = self.tx.clone();
        let mode = request.mode;
        let scene = request.scene;
        let worker_ctx = ctx.clone();

        let spawned = std::thread::Builder::new()
            .name("screenshot-clipboard-copy".into())
            .spawn(move || {
                let result = match mode {
                    ScreenshotMode::Normal => encode_normal_jpeg(&pixels, width, height)
                        .and_then(|bytes| clipboard.publish_bytes("screenshot-", ".jpg", &bytes)),
                    ScreenshotMode::PhonePhoto => {
                        let mut rng = rand::thread_rng();
                        let params = PhotoCameraParams::random(&mut rng, &scene);
                        encode_phone_photo_jpeg(&pixels, width, height, &scene, &params)
                            .and_then(|bytes| clipboard.publish_bytes("IMG_", ".jpg", &bytes))
                    }
                };
                in_flight.store(false, Ordering::Release);
                match result {
                    Ok(_) => {
                        let _ = tx.send(ScreenshotEvent::Copied(mode));
                    }
                    Err(e) => {
                        let _ = tx.send(ScreenshotEvent::Failed(mode, e.to_string()));
                    }
                }
                worker_ctx.request_repaint();
                worker_ctx.request_repaint_of(egui::ViewportId::ROOT);
            });

        if let Err(e) = spawned {
            self.in_flight.store(false, Ordering::Release);
            let _ = self.tx.send(ScreenshotEvent::Failed(
                mode,
                format!("Failed to spawn screenshot worker: {e}"),
            ));
            ctx.request_repaint();
        }
    }
}

/// Checks whether `Ctrl+S` (smartphone photo) or `S` (normal screenshot) was pressed in `ctx`
/// (ignoring active text input) and consumes the key event so other viewports in the same
/// frame do not double-trigger it.
pub fn check_screenshot_hotkeys(ctx: &egui::Context) -> Option<ScreenshotMode> {
    if ctx.wants_keyboard_input() {
        return None;
    }
    ctx.input_mut(|input| {
        let mut found = None;
        input.events.retain(|event| {
            let mode = match event {
                egui::Event::Key {
                    key: egui::Key::S,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if (modifiers.ctrl || modifiers.command)
                    && !modifiers.alt
                    && !modifiers.shift =>
                {
                    Some(ScreenshotMode::PhonePhoto)
                }
                egui::Event::Key {
                    key: egui::Key::S,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if modifiers.is_none() => Some(ScreenshotMode::Normal),
                _ => None,
            };
            if mode.is_some() {
                found = found.or(mode);
            }
            mode.is_none()
        });
        found
    })
}

/// Encodes raw RGBA pixels (top-left origin) into a good-quality JPEG image buffer.
pub fn encode_normal_jpeg(pixels: &[u8], width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(width > 0 && height > 0, "Invalid screenshot dimensions");
    anyhow::ensure!(
        pixels.len() == (width as usize) * (height as usize) * 4,
        "Unexpected RGBA pixel buffer length"
    );
    let mut rgb = Vec::with_capacity((width as usize) * (height as usize) * 3);
    for px in pixels.chunks_exact(4) {
        rgb.push(px[0]);
        rgb.push(px[1]);
        rgb.push(px[2]);
    }
    let mut out = Vec::with_capacity((width as usize) * (height as usize) / 2);
    let mut encoder = JpegEncoder::new_with_quality(&mut out, NORMAL_SCREENSHOT_JPEG_QUALITY);
    encoder.encode(&rgb, width, height, ColorType::Rgb8.into())?;
    Ok(out)
}

/// Cached NEC PC-98 CRT bezel texture atlas (`assets/nec_pc98_bezel.png`, top-left origin).
fn bezel_atlas() -> &'static RgbaImage {
    static BEZEL: OnceLock<RgbaImage> = OnceLock::new();
    BEZEL.get_or_init(|| {
        image::load_from_memory(include_bytes!("../assets/nec_pc98_bezel.png"))
            .expect("Failed to load nec_pc98_bezel.png")
            .to_rgba8()
    })
}

/// Cached photographer silhouette reflection mask (`assets/crt_reflection_silhouette.png`, top-left origin).
fn silhouette_mask() -> &'static RgbaImage {
    static SILHOUETTE: OnceLock<RgbaImage> = OnceLock::new();
    SILHOUETTE.get_or_init(|| {
        image::load_from_memory(include_bytes!("../assets/crt_reflection_silhouette.png"))
            .expect("Failed to load crt_reflection_silhouette.png")
            .to_rgba8()
    })
}

/// Randomized camera pose, unsteady hand shake, room reflection, and sensor parameters
/// for synthesizing a realistic smartphone photo of the user's monitor (`Ctrl+S`).
#[derive(Clone, Debug)]
pub struct PhotoCameraParams {
    /// Wrist roll rotation in radians (~1.0° to 4.2° clockwise or counter-clockwise).
    pub roll_rad: f32,
    /// Horizontal perspective keystone yaw angle in radians.
    pub yaw_rad: f32,
    /// Vertical perspective keystone pitch angle in radians.
    pub pitch_rad: f32,
    /// Framing scale (> 1.0 zooms out slightly to show more monitor bezel/surroundings).
    pub zoom: f32,
    /// Off-center horizontal framing shift in normalized monitor units.
    pub pan_x: f32,
    /// Off-center vertical framing shift in normalized monitor units.
    pub pan_y: f32,
    /// Smartphone wide-angle lens barrel distortion coefficient.
    pub barrel_k1: f32,
    /// Unsteady hand motion-blur direction in radians.
    pub shake_angle_rad: f32,
    /// Unsteady hand motion-blur length in output pixels.
    pub shake_px: f32,
    /// Radial lens softness / slight autofocus imperfection.
    pub defocus_strength: f32,
    /// Auto-exposure multiplier.
    pub exposure_gain: f32,
    /// Smartphone auto-white-balance RGB channel multipliers (screens photograph slightly cool/cyan-blue).
    pub wb_rgb: [f32; 3],
    /// Spatial frequency of the sensor-vs-panel moiré interference pattern.
    pub moire_freq: f32,
    /// Orientation angle of the moiré interference pattern.
    pub moire_angle_rad: f32,
    /// Amplitude of the moiré interference pattern.
    pub moire_strength: f32,
    /// Vertical center `[0.0, 1.0]` of the 60Hz CRT rolling-shutter phosphor refresh band.
    pub shutter_band_y: f32,
    /// Strength of the CRT rolling-shutter band.
    pub shutter_band_strength: f32,
    /// Offset of the overhead room light / photographer reflection on the screen glass.
    pub light_offset: [f32; 2],
    /// Whether a subtle smartphone flash / warm room lamp reflection appears on the glass.
    pub flash_active: bool,
    /// Position of the flash / lamp reflection in normalized screen space `[-1.0, 1.0]^2`.
    pub flash_pos: [f32; 2],
    /// Intensity of the flash / lamp reflection.
    pub flash_intensity: f32,
    /// High-ISO luminance grain strength.
    pub iso_noise_lum: f32,
    /// Low-frequency YCbCr chroma blotch noise strength.
    pub iso_noise_chroma: f32,
    /// Smartphone lens corner vignetting strength.
    pub vignette_strength: f32,
    /// Lateral chromatic aberration strength near frame corners.
    pub chromatic_aberration: f32,
    /// JPEG compression quality (lower = more authentic 8x8 DCT block artifacts).
    pub jpeg_quality: u8,
    /// Per-photo deterministic noise seed.
    pub seed: u64,
}

impl PhotoCameraParams {
    /// Generates a randomized set of smartphone photo parameters tailored to `scene`,
    /// constrained so that the photo always looks casually hand-held and imperfectly aligned
    /// (showing bezel/toolbar/surroundings and slight tilt/crop) while keeping ~86-96% of
    /// the screen clearly readable inside the frame.
    pub fn random<R: Rng + ?Sized>(rng: &mut R, scene: &PhotoSceneContext) -> Self {
        let is_crt = scene.crt_filter != CrtFilter::Off;
        let dark = scene.dark_room;

        // 1. Hand-held wrist roll: never 0°, always between ±0.85° and ±2.6°
        let roll_deg = rng.gen_range(0.85f32..2.6f32) * if rng.gen_bool(0.5) { 1.0 } else { -1.0 };
        let roll_rad = roll_deg.to_radians();

        // 2. 3D perspective keystone (holding phone slightly off-axis horizontally & vertically)
        let yaw_deg = rng.gen_range(-4.0f32..4.0f32);
        let pitch_deg = rng.gen_range(-3.4f32..3.4f32);
        let yaw_rad = yaw_deg.to_radians();
        let pitch_rad = pitch_deg.to_radians();

        // 3. Tight, natural framing: shows the monitor's inner bevel and a slice of the bezel
        // on 1-3 sides while 1-2 opposite sides come close to (or slightly clip) the photo border,
        // never zooming out past the monitor bezel into the wall/room behind it.
        // (In CRT mode, the tube curvature already insets the aperture slightly inside [0,1]^2.)
        let zoom = if is_crt {
            rng.gen_range(1.012f32..1.042f32)
        } else {
            rng.gen_range(1.022f32..1.052f32)
        };
        let pan_angle = rng.gen_range(0.0f32..std::f32::consts::TAU);
        let pan_mag = rng.gen_range(0.018f32..0.032f32);
        let pan_x = (pan_angle.cos() * pan_mag).clamp(-0.028, 0.028);
        let pan_y = (pan_angle.sin() * pan_mag).clamp(-0.026, 0.026);

        // Flat LCD screens must keep straight edges in 3D perspective (zero barrel curvature).
        // Even in CRT mode, the CRT tube curvature comes from `scene.warp`, so camera lens
        // distortion is kept near zero like a 2015 smartphone ISP.
        let barrel_k1 = if is_crt {
            rng.gen_range(0.002f32..0.007f32)
        } else {
            0.0
        };

        // 4. Subtle unsteady hand micro-tremor (2015 smartphone with fast auto-shutter / OIS)
        // Kept tight enough that fine 480-line CRT scanlines and RGB phosphor triads remain crisp.
        let shake_angle_rad = rng.gen_range(0.0f32..std::f32::consts::TAU);
        let shake_px = if dark {
            rng.gen_range(0.22f32..0.42f32)
        } else {
            rng.gen_range(0.14f32..0.30f32)
        };
        let defocus_strength = rng.gen_range(0.06f32..0.16f32);

        // 5. Auto-exposure & white balance shift (phone cameras render emissive screens slightly cool/blue-cyan)
        let exposure_gain = if dark {
            rng.gen_range(1.03f32..1.14f32)
        } else {
            rng.gen_range(0.98f32..1.07f32)
        };
        let wb_rgb = [
            rng.gen_range(0.95f32..0.99f32),
            rng.gen_range(0.99f32..1.02f32),
            rng.gen_range(1.03f32..1.09f32),
        ];

        // 6. Camera sensor moiré & CRT rolling shutter band
        // CRT mode uses shadow-mask/scanline beat frequencies complementing the physical CRT scanline raster;
        // Flat LCD mode uses 2D perspective LCD subpixel-grid vs CMOS Bayer sensor beat frequencies.
        let moire_freq = if is_crt {
            rng.gen_range(135.0f32..205.0f32)
        } else {
            rng.gen_range(170.0f32..245.0f32)
        };
        let moire_angle_rad = roll_rad + rng.gen_range(-0.14f32..0.14f32);
        let moire_strength = if is_crt {
            rng.gen_range(0.018f32..0.036f32)
        } else {
            rng.gen_range(0.030f32..0.056f32)
        };

        let shutter_band_y = rng.gen_range(0.12f32..0.88f32);
        let shutter_band_strength = if is_crt {
            rng.gen_range(0.05f32..0.11f32)
        } else {
            0.0
        };

        // 7. Room glass reflection & optional phone flash / desk lamp glare
        let light_offset = [
            rng.gen_range(-0.14f32..0.14f32) - yaw_rad * 0.8,
            rng.gen_range(-0.10f32..0.10f32) + pitch_rad * 0.8,
        ];
        let flash_active = is_crt && !dark && rng.gen_bool(0.38);
        let flash_pos = [
            rng.gen_range(-0.38f32..0.38f32),
            rng.gen_range(-0.35f32..0.25f32),
        ];
        let flash_intensity = if flash_active {
            rng.gen_range(0.16f32..0.36f32)
        } else {
            0.0
        };

        // 8. 2015-era smartphone sensor ISO grain, subtle vignette, corner CA & JPEG quality
        let iso_noise_lum = if dark {
            rng.gen_range(0.014f32..0.024f32)
        } else {
            rng.gen_range(0.006f32..0.013f32)
        };
        let iso_noise_chroma = if dark {
            rng.gen_range(0.009f32..0.017f32)
        } else {
            rng.gen_range(0.004f32..0.009f32)
        };
        let vignette_strength = rng.gen_range(0.10f32..0.19f32);
        let chromatic_aberration = rng.gen_range(0.00035f32..0.00085f32);
        let jpeg_quality = rng.gen_range(80u8..=88u8);
        let seed = rng.gen::<u64>();

        Self {
            roll_rad,
            yaw_rad,
            pitch_rad,
            zoom,
            pan_x,
            pan_y,
            barrel_k1,
            shake_angle_rad,
            shake_px,
            defocus_strength,
            exposure_gain,
            wb_rgb,
            moire_freq,
            moire_angle_rad,
            moire_strength,
            shutter_band_y,
            shutter_band_strength,
            light_offset,
            flash_active,
            flash_pos,
            flash_intensity,
            iso_noise_lum,
            iso_noise_chroma,
            vignette_strength,
            chromatic_aberration,
            jpeg_quality,
            seed,
        }
    }

    /// Maps a normalized output photo coordinate `(nx, ny)` in `[-1.0, 1.0]^2`
    /// onto the physical monitor plane `(u, v)` where `[0.0, 1.0]^2` is the active screen.
    #[inline]
    pub fn project_photo_to_monitor_uv(&self, nx: f32, ny: f32, aspect: f32) -> (f32, f32) {
        // 1. Smartphone wide-angle lens barrel distortion in aspect-corrected sensor space
        let sx = nx * aspect;
        let sy = ny;
        let r2 = (sx * sx + sy * sy) / (aspect * aspect + 1.0).max(1.0);
        let radial = 1.0 + self.barrel_k1 * r2;
        let dx = nx * radial;
        let dy = ny * radial;

        // 2. Apply camera zoom and 2D wrist roll in aspect-correct space
        let cos_r = self.roll_rad.cos();
        let sin_r = self.roll_rad.sin();
        let rx_aspect = (dx * aspect * cos_r - dy * sin_r) / aspect.max(0.1);
        let ry_aspect = dx * aspect * sin_r + dy * cos_r;

        let zx = rx_aspect * self.zoom + self.pan_x * 2.0;
        let zy = ry_aspect * self.zoom + self.pan_y * 2.0;

        // 3. 3D perspective keystone homography (camera yaw & pitch relative to monitor plane z = 0)
        // Denominator models perspective foreshortening across the tilted monitor plane.
        let persp_denom =
            (1.0 + zx * self.yaw_rad.sin() * 0.58 + zy * self.pitch_rad.sin() * 0.58).max(0.25);
        let px = (zx / persp_denom) / self.yaw_rad.cos().max(0.5);
        let py = (zy / persp_denom) / self.pitch_rad.cos().max(0.5);

        // Convert from [-1, 1] monitor space to [0, 1] screen UV space
        let u = px * 0.5 + 0.5;
        let v = py * 0.5 + 0.5;
        (u, v)
    }
}

/// Fast deterministic 2D hash returning `[0.0, 1.0)`.
#[inline]
fn hash21(x: f32, y: f32) -> f32 {
    let mut px = (x * 123.34).fract();
    let mut py = (y * 456.21).fract();
    if px < 0.0 {
        px += 1.0;
    }
    if py < 0.0 {
        py += 1.0;
    }
    let dot = px * (px + 45.32) + py * (py + 45.32);
    px += dot;
    py += dot;
    let r = (px * py).fract();
    if r < 0.0 {
        r + 1.0
    } else {
        r
    }
}

/// Smooth 2D value noise in `[0.0, 1.0]`.
#[inline]
fn value_noise(x: f32, y: f32) -> f32 {
    let ix = x.floor();
    let iy = y.floor();
    let fx = x - ix;
    let fy = y - iy;
    let ux = fx * fx * (3.0 - 2.0 * fx);
    let uy = fy * fy * (3.0 - 2.0 * fy);
    let a = hash21(ix, iy);
    let b = hash21(ix + 1.0, iy);
    let c = hash21(ix, iy + 1.0);
    let d = hash21(ix + 1.0, iy + 1.0);
    let ab = a + (b - a) * ux;
    let cd = c + (d - c) * ux;
    ab + (cd - ab) * uy
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Bilinearly samples an RGBA buffer (`width x height`) at normalized `[0.0, 1.0]` UV coordinates,
/// returning `[r, g, b]` in `[0.0, 1.0]`.
#[inline]
fn sample_rgb_bilinear(pixels: &[u8], width: u32, height: u32, u: f32, v: f32) -> [f32; 3] {
    let uc = u.clamp(0.0, 1.0);
    let vc = v.clamp(0.0, 1.0);
    let x = uc * (width.saturating_sub(1) as f32);
    let y = vc * (height.saturating_sub(1) as f32);
    let x0 = (x.floor() as u32).min(width.saturating_sub(1));
    let y0 = (y.floor() as u32).min(height.saturating_sub(1));
    let x1 = (x0 + 1).min(width.saturating_sub(1));
    let y1 = (y0 + 1).min(height.saturating_sub(1));
    let fx = x - (x0 as f32);
    let fy = y - (y0 as f32);

    let idx = |px: u32, py: u32| -> usize { ((py * width + px) as usize) * 4 };
    let i00 = idx(x0, y0);
    let i10 = idx(x1, y0);
    let i01 = idx(x0, y1);
    let i11 = idx(x1, y1);

    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let p00 = pixels[i00 + c] as f32 * (1.0 / 255.0);
        let p10 = pixels[i10 + c] as f32 * (1.0 / 255.0);
        let p01 = pixels[i01 + c] as f32 * (1.0 / 255.0);
        let p11 = pixels[i11 + c] as f32 * (1.0 / 255.0);
        let top = p00 + (p10 - p00) * fx;
        let bot = p01 + (p11 - p01) * fx;
        out[c] = top + (bot - top) * fy;
    }
    out
}

/// Builds a compact blurred ambient/bloom mipmap (`gw x gh`) of the source screen so that
/// phosphor halation, bezel bounce light, and room ambilight spill can be sampled in O(1) time.
fn build_ambient_grid(pixels: &[u8], width: u32, height: u32, gw: u32, gh: u32) -> Vec<[f32; 3]> {
    let mut grid = vec![[0.0f32; 3]; (gw * gh) as usize];
    for gy in 0..gh {
        let v0 = (gy as f32 + 0.25) / (gh as f32);
        let v1 = (gy as f32 + 0.75) / (gh as f32);
        for gx in 0..gw {
            let u0 = (gx as f32 + 0.25) / (gw as f32);
            let u1 = (gx as f32 + 0.75) / (gw as f32);
            let s00 = sample_rgb_bilinear(pixels, width, height, u0, v0);
            let s10 = sample_rgb_bilinear(pixels, width, height, u1, v0);
            let s01 = sample_rgb_bilinear(pixels, width, height, u0, v1);
            let s11 = sample_rgb_bilinear(pixels, width, height, u1, v1);
            grid[(gy * gw + gx) as usize] = [
                (s00[0] + s10[0] + s01[0] + s11[0]) * 0.25,
                (s00[1] + s10[1] + s01[1] + s11[1]) * 0.25,
                (s00[2] + s10[2] + s01[2] + s11[2]) * 0.25,
            ];
        }
    }

    // Two-pass separable box/tent blur for silky diffuse phosphor bloom & bezel bounce
    let mut temp = grid.clone();
    let radius = 3i32;
    for _pass in 0..2 {
        for y in 0..gh as i32 {
            for x in 0..gw as i32 {
                let mut sum = [0.0f32; 3];
                let mut w_sum = 0.0f32;
                for dx in -radius..=radius {
                    let wt = (radius + 1 - dx.abs()) as f32;
                    let sx = (x + dx).clamp(0, gw as i32 - 1) as u32;
                    let c = grid[(y as u32 * gw + sx) as usize];
                    sum[0] += c[0] * wt;
                    sum[1] += c[1] * wt;
                    sum[2] += c[2] * wt;
                    w_sum += wt;
                }
                temp[(y as u32 * gw + x as u32) as usize] =
                    [sum[0] / w_sum, sum[1] / w_sum, sum[2] / w_sum];
            }
        }
        for y in 0..gh as i32 {
            for x in 0..gw as i32 {
                let mut sum = [0.0f32; 3];
                let mut w_sum = 0.0f32;
                for dy in -radius..=radius {
                    let wt = (radius + 1 - dy.abs()) as f32;
                    let sy = (y + dy).clamp(0, gh as i32 - 1) as u32;
                    let c = temp[(sy * gw + x as u32) as usize];
                    sum[0] += c[0] * wt;
                    sum[1] += c[1] * wt;
                    sum[2] += c[2] * wt;
                    w_sum += wt;
                }
                grid[(y as u32 * gw + x as u32) as usize] =
                    [sum[0] / w_sum, sum[1] / w_sum, sum[2] / w_sum];
            }
        }
    }
    grid
}

#[inline]
fn sample_ambient_grid(grid: &[[f32; 3]], gw: u32, gh: u32, u: f32, v: f32) -> [f32; 3] {
    let uc = u.clamp(0.0, 1.0);
    let vc = v.clamp(0.0, 1.0);
    let x = uc * (gw.saturating_sub(1) as f32);
    let y = vc * (gh.saturating_sub(1) as f32);
    let x0 = (x.floor() as u32).min(gw.saturating_sub(1));
    let y0 = (y.floor() as u32).min(gh.saturating_sub(1));
    let x1 = (x0 + 1).min(gw.saturating_sub(1));
    let y1 = (y0 + 1).min(gh.saturating_sub(1));
    let fx = x - (x0 as f32);
    let fy = y - (y0 as f32);

    let c00 = grid[(y0 * gw + x0) as usize];
    let c10 = grid[(y0 * gw + x1) as usize];
    let c01 = grid[(y1 * gw + x0) as usize];
    let c11 = grid[(y1 * gw + x1) as usize];

    let mut out = [0.0f32; 3];
    for i in 0..3 {
        let top = c00[i] + (c10[i] - c00[i]) * fx;
        let bot = c01[i] + (c11[i] - c01[i]) * fx;
        out[i] = top + (bot - top) * fy;
    }
    out
}

/// Evaluates the twin-tube modular overhead fluorescent ceiling light reflection on curved glass.
#[inline]
fn eval_ceiling_troffer(
    center_x: f32,
    center_y: f32,
    half_len: f32,
    tube_sep: f32,
    rx: f32,
    ry: f32,
    gloss: f32,
) -> f32 {
    let dx = (rx - center_x).abs();
    let end_soft = 0.05 * (1.0 - 0.5 * gloss) + 0.014;
    let h_mask = (1.0 - (dx - (half_len - end_soft)) / end_soft).clamp(0.0, 1.0);
    let y_top = center_y + tube_sep * 0.5;
    let y_bot = center_y - tube_sep * 0.5;
    let sigma = 0.019 * (1.0 - 0.72 * gloss) + 0.006;
    let tube_top = (-0.5 * ((ry - y_top) / sigma).powi(2)).exp();
    let tube_bot = (-0.5 * ((ry - y_bot) / sigma).powi(2)).exp();
    let housing_sigma = tube_sep * 1.2;
    let housing_glow =
        (-0.5 * ((ry - center_y) / housing_sigma).powi(2)).exp() * (0.16 * (1.0 - 0.35 * gloss));
    (tube_top + tube_bot + housing_glow) * h_mask
}

/// Evaluates the vintage CRT monitor bezel and 3D recessed inner bevel matching `fs_retro_frame.glsl`
/// (continuous bezel surface without branding, knobs, switches, LEDs, or outer wall background).
#[allow(clippy::too_many_arguments)]
fn eval_crt_bezel_and_room(
    u: f32,
    v: f32,
    d_ap_norm: f32,
    nx_ap: f32,
    ny_ap: f32,
    ambient_edge: [f32; 3],
    dark_room: bool,
    atlas: &RgbaImage,
) -> [f32; 3] {
    let (aw, ah) = atlas.dimensions();

    // 1. Dark rubber gasket sealing the CRT glass tube
    let gasket_w = 0.0048f32;
    let bevel_w = 0.039f32 * (1.0 + ny_ap.max(0.0) * 0.25);
    let t_bevel = ((d_ap_norm - gasket_w) / (bevel_w - gasket_w).max(0.001)).clamp(0.0, 1.0);

    // 2. Dual-scale stochastic ABS pebble grain sampled from `nec_pc98_bezel.png`
    // plus multi-octave organic aging/patina matching `fs_retro_frame.glsl`
    let tile_u1 = 0.51 + (u * 2.1).rem_euclid(1.0) * 0.22;
    let tile_v1 = 0.26 + (v * 2.1).rem_euclid(1.0) * 0.22;
    let tile_u2 = 0.51 + ((u * 1.45 + v * 0.95).rem_euclid(1.0)) * 0.22;
    let tile_v2 = 0.26 + ((-u * 0.95 + v * 1.45).rem_euclid(1.0)) * 0.22;
    let g1 = sample_rgb_bilinear(atlas.as_raw(), aw, ah, tile_u1, tile_v1);
    let g2 = sample_rgb_bilinear(atlas.as_raw(), aw, ah, tile_u2, tile_v2);
    let grain_rgb = [
        (g1[0] + g2[0]) * 0.5,
        (g1[1] + g2[1]) * 0.5,
        (g1[2] + g2[2]) * 0.5,
    ];

    let n_coarse = value_noise(u * 4.5, v * 4.5);
    let n_med = value_noise(u * 14.0, v * 14.0);
    let n_fine = value_noise(u * 48.0, v * 48.0);
    let n_micro = value_noise(u * 155.0, v * 155.0);
    let organic_stain = n_coarse * 0.50 + n_med * 0.35 + n_fine * 0.15;

    let base_putty = [0.67f32, 0.64f32, 0.57f32];
    let aged_yellow = [0.76f32, 0.70f32, 0.48f32];
    let tobacco_tar = [0.52f32, 0.40f32, 0.22f32];
    let crevice_soot = [0.14f32, 0.13f32, 0.11f32];

    let uv_exposure = ((1.0 - v.clamp(0.0, 1.0)).powf(1.3) * 0.75
        + (1.0 - u.clamp(0.0, 1.0)) * 0.25)
        .clamp(0.0, 1.0);
    let yellowing = (uv_exposure * 0.45 + organic_stain * 0.35 + 0.10).clamp(0.0, 1.0);
    let smoke_condense = ((organic_stain - 0.40) * 2.0).clamp(0.0, 1.0);
    let smoke_amount = smoke_condense * (0.50 + 0.50 * ny_ap.max(0.0)) * 0.24;

    let grain_scale = if dark_room { 0.85 } else { 1.0 };
    let grit_mod = (n_micro - 0.5) * 0.055 * grain_scale;
    let grain_ref = [0.60f32, 0.58f32, 0.50f32];
    let mut aged_plastic = [0.0f32; 3];
    for c in 0..3 {
        let p_yellow = base_putty[c] + (aged_yellow[c] - base_putty[c]) * yellowing;
        let p_smoke = p_yellow + (tobacco_tar[c] - p_yellow) * smoke_amount;
        let g_mod = (grain_rgb[c] - grain_ref[c]) * 0.35 * grain_scale;
        aged_plastic[c] = (p_smoke + g_mod + grit_mod).clamp(0.0, 1.0);
    }

    // 3. Steep inward-sloping 3D bevel normal & directional lighting matching `fs_retro_frame.glsl`
    // World space: +X right, +Y up (-v), +Z toward viewer; Light L = normalize(-0.28, 0.65, 0.70)
    let slope_angle = 0.62f32;
    let nbx = -nx_ap * slope_angle;
    let nby = ny_ap * slope_angle;
    let nbz = (1.0 - nbx * nbx - nby * nby).max(0.01).sqrt();
    let diffuse = (nbx * (-0.281) + nby * 0.653 + nbz * 0.703).clamp(0.0, 1.0);
    let cavity_ao = 0.45 + 0.55 * t_bevel.powf(0.75);

    let bevel_lighting = if dark_room {
        0.12 + diffuse * 0.12
    } else {
        (0.35 + diffuse * 0.85) * cavity_ao
    };
    let bottom_shelf = ny_ap.max(0.0) * t_bevel * t_bevel * if dark_room { 0.18 } else { 0.35 };

    // Specular crest ridge along the bottom/right bevel lip
    let crest_ridge = (-0.5 * ((t_bevel - 0.97) / 0.028).powi(2)).exp();
    let crest_dir = (nx_ap * 0.40 - ny_ap * 0.75).clamp(0.0, 1.0);
    let crest_mult = if dark_room { 0.35 } else { 1.0 };
    let crest_add = crest_ridge * crest_dir * crest_mult;

    // Crevice grime accumulation
    let seam_grime = (-0.5 * ((d_ap_norm - gasket_w) / 0.0032).powi(2)).exp() * 0.55;
    let cowl_grime =
        ((1.0 - t_bevel) * 0.35 + ny_ap.max(0.0) * (1.0 - t_bevel).powi(2) * 0.25).clamp(0.0, 0.60);
    let total_grime = if dark_room {
        (seam_grime * 0.20 + (1.0 - t_bevel) * 0.10).clamp(0.0, 0.30)
    } else {
        (seam_grime + cowl_grime * (0.60 + 0.40 * n_fine)).clamp(0.0, 0.80)
    };
    let grime_factor = if dark_room {
        total_grime * 0.15
    } else {
        total_grime
    };

    // 4. Near-field contact halation & physically-grounded inner bevel phosphor bounce
    let edge_lum = 0.2126 * ambient_edge[0] + 0.7152 * ambient_edge[1] + 0.0722 * ambient_edge[2];
    let halation_falloff = (-d_ap_norm.max(0.0) / 0.010).exp();
    let halation_ratio = if dark_room { 0.36 } else { 0.20 };

    // Cowl falloff strictly inside the 3D bevel (`d_ap_norm < bevel_w`), decaying smoothly to 0 at `t_bevel = 1.0`
    let cowl_falloff = if d_ap_norm < bevel_w {
        (1.0 - t_bevel).powf(2.2)
    } else {
        0.0
    };
    let vid_energy = (0.60 + 1.20 * edge_lum).clamp(0.60, 1.80);
    let bounce_mult = if dark_room { 0.38 } else { 0.50 };

    // Soft atmospheric night-mode halo (`fs_night_glow.glsl`) across the dark bezel when room lights are off
    let night_halo = if dark_room {
        (-d_ap_norm.max(0.0) * 18.0).exp() * 0.075 * edge_lum.powf(1.2)
    } else {
        0.0
    };

    // 5. Outer front chassis plastic & structural moldings
    let outer_vignette = (1.0 - (d_ap_norm - bevel_w).max(0.0) * 0.85).clamp(0.82, 1.0);
    let pillar_seam = if d_ap_norm > bevel_w {
        let dl = (u - (-0.056)).abs();
        let dr = (u - 1.056).abs();
        (-0.5 * (dl.min(dr) / 0.0018).powi(2)).exp() * 0.18
    } else {
        0.0
    };
    let chin_seam = if d_ap_norm > bevel_w && v > 1.045 {
        (-0.5 * ((v - 1.062) / 0.0020).powi(2)).exp() * 0.15
    } else {
        0.0
    };

    let t_bevel_blend = smoothstep(bevel_w - 0.0015, bevel_w + 0.0015, d_ap_norm);
    let crest_rgb = [0.14f32, 0.14f32, 0.11f32];
    let mut frame_plastic = [0.0f32; 3];

    for c in 0..3 {
        let plastic_bevel = if dark_room {
            aged_plastic[c] * 0.22
        } else {
            aged_plastic[c]
        };
        let soot = if dark_room {
            crevice_soot[c] * 0.20
        } else {
            crevice_soot[c]
        };
        let lit_bevel = plastic_bevel * (bevel_lighting + bottom_shelf) * (0.75 + 0.25 * t_bevel)
            + crest_rgb[c] * crest_add;
        let surface_plastic = lit_bevel + (soot - lit_bevel) * grime_factor;

        let bounce_tint = edge_lum * 0.60 + ambient_edge[c] * 0.40;
        let ambient_glow_c = bounce_tint * vid_energy * (bounce_mult * cowl_falloff);
        let surface_refl = if dark_room {
            aged_plastic[c] * 0.75 + 0.25
        } else {
            aged_plastic[c] * 0.85 + 0.15
        };
        let halation_c =
            (edge_lum * 0.40 + ambient_edge[c] * 0.60) * (halation_ratio * halation_falloff);
        let bevel_col = surface_plastic + ambient_glow_c * surface_refl + halation_c;

        let mut chassis_col =
            aged_plastic[c] * outer_vignette * (1.0 - pillar_seam) * (1.0 - chin_seam);
        if dark_room {
            chassis_col =
                chassis_col * 0.022 + (edge_lum * 0.35 + ambient_edge[c] * 0.65) * night_halo;
        }

        frame_plastic[c] = bevel_col + (chassis_col - bevel_col) * t_bevel_blend;
    }

    // 6. Velvety settled dust bed on the upward-facing bottom inner bevel shelf (`ny_ap > 0.15`)
    if ny_ap > 0.15 && d_ap_norm > gasket_w && d_ap_norm < bevel_w {
        let in_inlet = ((d_ap_norm - gasket_w) / 0.0022).clamp(0.0, 1.0)
            * ((bevel_w - d_ap_norm) / 0.0022).clamp(0.0, 1.0);
        let shelf_gravity = ny_ap.clamp(0.0, 1.0).powf(1.9);
        let shelf_profile =
            (t_bevel / 0.09).clamp(0.0, 1.0) * ((1.0 - t_bevel) / 0.16).clamp(0.0, 1.0).powf(0.55);
        let dust_density =
            in_inlet * shelf_gravity * shelf_profile * (0.65 + 0.25 * n_med + 0.10 * n_fine) * 1.45;
        if dust_density > 0.002 {
            let dust_ao = 0.45 + 0.55 * t_bevel.powf(0.55);
            let dust_albedo = [0.69f32, 0.67f32, 0.62f32];
            let dust_alpha = (dust_density * if dark_room { 0.65 } else { 0.82 }).clamp(0.0, 0.85);
            for c in 0..3 {
                let dust_room = if dark_room {
                    dust_albedo[c] * (0.06 + 0.08 * diffuse) * dust_ao
                } else {
                    dust_albedo[c] * (0.40 + 0.60 * diffuse) * dust_ao
                };
                let dust_crt = (edge_lum * 0.70 + ambient_edge[c] * 0.30)
                    * edge_lum.powf(1.3)
                    * if dark_room { 0.22 } else { 0.12 };
                let dust_lit = dust_room + dust_crt;
                frame_plastic[c] += (dust_lit - frame_plastic[c]) * dust_alpha;
            }
        }
    }

    // 7. Smooth transition from dark rubber gasket (`d_ap_norm < gasket_w`) to inner bevel plastic
    let t_gasket = smoothstep(gasket_w - 0.0010, gasket_w + 0.0010, d_ap_norm);
    let gasket_base = [0.038f32, 0.038f32, 0.042f32];
    let mut final_rgb = [0.0f32; 3];
    for c in 0..3 {
        let gasket_toned = if dark_room {
            gasket_base[c] * 0.58
        } else {
            gasket_base[c]
        };
        let halation_c =
            (edge_lum * 0.40 + ambient_edge[c] * 0.60) * (halation_ratio * halation_falloff);
        let gasket_lit = gasket_toned + halation_c;
        final_rgb[c] = (gasket_lit + (frame_plastic[c] - gasket_lit) * t_gasket).clamp(0.0, 1.0);
    }
    final_rgb
}

/// Evaluates a photorealistic modern flat-panel LCD/IPS monitor bezel with a recessed black
/// glass panel matrix border, 3D angled inner bezel chamfer, real dual-scale ABS micro-grain
/// texture from `nec_pc98_bezel.png` in matte dark charcoal/graphite, and real-time screen-edge
/// bounce lighting (no fake procedural titlebars, taskbars, stands, or wall background).
#[allow(clippy::too_many_arguments)]
fn eval_flat_lcd_bezel(
    u: f32,
    v: f32,
    d_ap_norm: f32,
    nx_ap: f32,
    ny_ap: f32,
    ambient_edge: [f32; 3],
    dark_room: bool,
    atlas: &RgbaImage,
) -> [f32; 3] {
    let (aw, ah) = atlas.dimensions();

    // 1. Recessed black glass panel matrix border ("BM" dead-zone around the active LCD pixel array)
    // followed by a sleek 3D inward-sloping modern monitor bezel chamfer (`bevel_w`).
    let matrix_border_w = 0.0042f32;
    let bevel_w = 0.021f32 * (1.0 + ny_ap.max(0.0) * 0.22);
    let t_bevel =
        ((d_ap_norm - matrix_border_w) / (bevel_w - matrix_border_w).max(0.001)).clamp(0.0, 1.0);

    // 2. Dual-scale stochastic ABS micro-pebble grain sampled from `nec_pc98_bezel.png`
    // combined with fine horizontal brushed/anodized micro-texture in dark matte charcoal/graphite.
    let tile_u1 = 0.51 + (u * 2.8).rem_euclid(1.0) * 0.22;
    let tile_v1 = 0.26 + (v * 2.8).rem_euclid(1.0) * 0.22;
    let tile_u2 = 0.51 + ((u * 1.9 + v * 1.2).rem_euclid(1.0)) * 0.22;
    let tile_v2 = 0.26 + ((-u * 1.2 + v * 1.9).rem_euclid(1.0)) * 0.22;
    let g1 = sample_rgb_bilinear(atlas.as_raw(), aw, ah, tile_u1, tile_v1);
    let g2 = sample_rgb_bilinear(atlas.as_raw(), aw, ah, tile_u2, tile_v2);
    let grain_rgb = [
        (g1[0] + g2[0]) * 0.5,
        (g1[1] + g2[1]) * 0.5,
        (g1[2] + g2[2]) * 0.5,
    ];

    let n_coarse = value_noise(u * 5.0, v * 5.0);
    let n_med = value_noise(u * 18.0, v * 18.0);
    let n_fine = value_noise(u * 62.0, v * 62.0);
    let n_brush = value_noise(u * 35.0, v * 210.0);

    // Base dark matte charcoal / graphite ABS albedo (#292b2f in lit room)
    let base_charcoal = [0.168f32, 0.174f32, 0.186f32];
    let uv_exposure = ((1.0 - v.clamp(0.0, 1.0)).powf(1.2) * 0.72
        + (1.0 - u.clamp(0.0, 1.0)) * 0.28)
        .clamp(0.0, 1.0);
    let grain_ref = [0.60f32, 0.58f32, 0.50f32];
    let grain_scale = if dark_room { 0.75 } else { 1.0 };
    let brush_mod = ((n_brush - 0.5) * 0.018 + (n_fine - 0.5) * 0.014) * grain_scale;

    let mut charcoal_plastic = [0.0f32; 3];
    for c in 0..3 {
        let g_mod = (grain_rgb[c] - grain_ref[c]) * 0.11 * grain_scale;
        let top_sheen = uv_exposure * 0.035 + (n_coarse - 0.5) * 0.012;
        charcoal_plastic[c] = (base_charcoal[c] + top_sheen + g_mod + brush_mod).clamp(0.02, 1.0);
    }

    // 3. 3D Inward-Sloping Bezel Step Normal & Directional Room Lighting
    let slope_angle = 0.68f32;
    let nbx = -nx_ap * slope_angle;
    let nby = ny_ap * slope_angle;
    let nbz = (1.0 - nbx * nbx - nby * nby).max(0.01).sqrt();
    let diffuse = (nbx * (-0.281) + nby * 0.653 + nbz * 0.703).clamp(0.0, 1.0);
    let cavity_ao = 0.42 + 0.58 * t_bevel.powf(0.75);

    let bevel_lighting = if dark_room {
        0.10 + diffuse * 0.10
    } else {
        (0.32 + diffuse * 0.88) * cavity_ao
    };
    let bottom_shelf = ny_ap.max(0.0) * t_bevel * t_bevel * if dark_room { 0.16 } else { 0.38 };

    // Crisp 3D specular catch-light ridge where the inner chamfer meets the flat front bezel face
    let crest_ridge = (-0.5 * ((t_bevel - 0.96) / 0.032).powi(2)).exp();
    let crest_dir = (nx_ap * 0.35 - ny_ap * 0.78 + 0.25).clamp(0.0, 1.0);
    let crest_mult = if dark_room { 0.30 } else { 1.0 };
    let crest_add = crest_ridge * crest_dir * crest_mult;

    // Subtle crevice shadow right at the recessed panel interface (`d_ap_norm ≈ matrix_border_w`)
    let seam_shadow = (-0.5 * ((d_ap_norm - matrix_border_w) / 0.0018).powi(2)).exp()
        * if dark_room { 0.25 } else { 0.48 };

    // 4. Real-Time LCD Screen-Edge Diffuse Bounce Light & Near-Field Halation
    let edge_lum = 0.2126 * ambient_edge[0] + 0.7152 * ambient_edge[1] + 0.0722 * ambient_edge[2];
    let halation_falloff = (-d_ap_norm.max(0.0) / 0.0085).exp();
    let halation_ratio = if dark_room { 0.32 } else { 0.16 };

    let cowl_falloff = if d_ap_norm < bevel_w {
        (1.0 - t_bevel).powf(2.2)
    } else {
        0.0
    };
    let vid_energy = (0.55 + 1.15 * edge_lum).clamp(0.55, 1.70);
    let bounce_mult = if dark_room { 0.40 } else { 0.52 };
    let night_halo = if dark_room {
        (-d_ap_norm.max(0.0) * 20.0).exp() * 0.065 * edge_lum.powf(1.2)
    } else {
        0.0
    };

    // 5. Outer front bezel face shading & subtle molded chin/frame seam
    let outer_vignette = (1.0 - (d_ap_norm - bevel_w).max(0.0) * 0.75).clamp(0.84, 1.0);
    let chin_seam = if d_ap_norm > bevel_w && v > 1.032 {
        (-0.5 * ((v - 1.044) / 0.0016).powi(2)).exp() * 0.16
    } else {
        0.0
    };

    let t_bevel_blend = smoothstep(bevel_w - 0.0012, bevel_w + 0.0012, d_ap_norm);
    let crest_rgb = [0.095f32, 0.100f32, 0.108f32];
    let mut frame_plastic = [0.0f32; 3];

    for c in 0..3 {
        let plastic_bevel = if dark_room {
            charcoal_plastic[c] * 0.24
        } else {
            charcoal_plastic[c]
        };
        let lit_bevel = (plastic_bevel * (bevel_lighting + bottom_shelf) * (0.76 + 0.24 * t_bevel)
            + crest_rgb[c] * crest_add)
            * (1.0 - seam_shadow);

        let bounce_tint = edge_lum * 0.55 + ambient_edge[c] * 0.45;
        let ambient_glow_c = bounce_tint * vid_energy * (bounce_mult * cowl_falloff);
        let surface_refl = charcoal_plastic[c] * 0.85 + 0.18;
        let halation_c =
            (edge_lum * 0.40 + ambient_edge[c] * 0.60) * (halation_ratio * halation_falloff);
        let bevel_col = lit_bevel + ambient_glow_c * surface_refl + halation_c;

        let mut chassis_col = charcoal_plastic[c] * outer_vignette * (1.0 - chin_seam);
        if dark_room {
            chassis_col =
                chassis_col * 0.11 + (edge_lum * 0.35 + ambient_edge[c] * 0.65) * night_halo;
        }

        frame_plastic[c] = bevel_col + (chassis_col - bevel_col) * t_bevel_blend;
    }

    // 6. Subtle settled micro-dust along the upward-facing bottom inner bezel lip (`ny_ap > 0.15`)
    if ny_ap > 0.15 && d_ap_norm > matrix_border_w && d_ap_norm < bevel_w {
        let in_inlet = ((d_ap_norm - matrix_border_w) / 0.0015).clamp(0.0, 1.0)
            * ((bevel_w - d_ap_norm) / 0.0015).clamp(0.0, 1.0);
        let shelf_gravity = ny_ap.clamp(0.0, 1.0).powf(2.0);
        let shelf_profile =
            (t_bevel / 0.12).clamp(0.0, 1.0) * ((1.0 - t_bevel) / 0.20).clamp(0.0, 1.0).powf(0.6);
        let dust_density =
            in_inlet * shelf_gravity * shelf_profile * (0.60 + 0.28 * n_med + 0.12 * n_fine) * 0.95;
        if dust_density > 0.002 {
            let dust_albedo = [0.46f32, 0.47f32, 0.49f32];
            let dust_alpha = (dust_density * if dark_room { 0.45 } else { 0.65 }).clamp(0.0, 0.65);
            for c in 0..3 {
                let dust_room = if dark_room {
                    dust_albedo[c] * (0.05 + 0.07 * diffuse)
                } else {
                    dust_albedo[c] * (0.35 + 0.55 * diffuse)
                };
                let dust_lit = dust_room + (edge_lum * 0.6 + ambient_edge[c] * 0.4) * 0.08;
                frame_plastic[c] += (dust_lit - frame_plastic[c]) * dust_alpha;
            }
        }
    }

    // 7. Smooth transition from the recessed black glass matrix border (`d_ap_norm < matrix_border_w`)
    let t_border = smoothstep(
        matrix_border_w - 0.0008,
        matrix_border_w + 0.0008,
        d_ap_norm,
    );
    let border_base = if dark_room {
        [0.014f32, 0.015f32, 0.018f32]
    } else {
        [0.028f32, 0.030f32, 0.035f32]
    };
    let mut final_rgb = [0.0f32; 3];
    for c in 0..3 {
        let halation_c =
            (edge_lum * 0.40 + ambient_edge[c] * 0.60) * (halation_ratio * halation_falloff);
        let border_lit = border_base[c] + halation_c;
        final_rgb[c] = (border_lit + (frame_plastic[c] - border_lit) * t_border).clamp(0.0, 1.0);
    }
    final_rgb
}

/// Evaluates a single ray at monitor plane coordinate `(u, v)` (where `[0.0, 1.0]^2` is the
/// active screen surface), combining screen pixels, CRT curvature/aperture or flat LCD panel,
/// glass reflections, moiré interference, phosphor bloom, and surrounding physical monitor bezel.
fn eval_monitor_plane_point(
    u: f32,
    v: f32,
    nx_sensor: f32,
    ny_sensor: f32,
    pixels: &[u8],
    width: u32,
    height: u32,
    ambient_grid: &[[f32; 3]],
    gw: u32,
    gh: u32,
    scene: &PhotoSceneContext,
    params: &PhotoCameraParams,
    atlas: &RgbaImage,
    silhouette: &RgbaImage,
) -> [f32; 3] {
    let is_crt = scene.crt_filter != CrtFilter::Off;
    let dark_room = scene.dark_room;
    let ambient_edge = sample_ambient_grid(
        ambient_grid,
        gw,
        gh,
        u.clamp(0.02, 0.98),
        v.clamp(0.02, 0.98),
    );

    if is_crt {
        // Apply CRT glass curvature & rounded aperture SDF matching `fs_retro_frame.glsl` exactly
        let pos_x = u * 2.0 - 1.0;
        let pos_y = v * 2.0 - 1.0;
        let has_warp = scene.warp[0] > 0.001 || scene.warp[1] > 0.001;
        let warp_x = if scene.crt_filter == CrtFilter::Lottes || has_warp {
            scene.warp[0].max(0.044)
        } else {
            0.024
        };
        let warp_y = if scene.crt_filter == CrtFilter::Lottes || has_warp {
            scene.warp[1].max(0.054)
        } else {
            0.030
        };
        let su = (pos_x * (1.0 + pos_y * pos_y * warp_x)) * 0.5 + 0.5;
        let sv = (pos_y * (1.0 + pos_x * pos_x * warp_y)) * 0.5 + 0.5;

        let r_corner = if scene.corner_size > 0.001 {
            scene.corner_size.max(0.035)
        } else {
            0.038
        };
        let qx = (su - 0.5).abs() - (0.5 - r_corner);
        let qy = (sv - 0.5).abs() - (0.5 - r_corner);
        let max_qx = qx.max(0.0);
        let max_qy = qy.max(0.0);
        let len_max_q = max_qx.hypot(max_qy);
        let d_ap = qx.max(qy).min(0.0) + len_max_q - r_corner;

        let (nx_dir, ny_dir) = if len_max_q > 1e-4 {
            (max_qx / len_max_q, max_qy / len_max_q)
        } else if qx > qy {
            (1.0, 0.0)
        } else {
            (0.0, 1.0)
        };
        let nx_ap = nx_dir * (su - 0.5).signum();
        let ny_ap = ny_dir * (sv - 0.5).signum();

        if d_ap >= 0.0 {
            return eval_crt_bezel_and_room(
                su,
                sv,
                d_ap,
                nx_ap,
                ny_ap,
                ambient_edge,
                dark_room,
                atlas,
            );
        }

        // Inside the CRT glass aperture:
        // Sample `pixels` directly with `sample_rgb_bilinear` (preserving the exact in-game CRT
        // scanline and phosphor detail already rendered by the GPU shader without vertical blurring),
        // then apply a subtle, high-density 480-line VGA/PC-98 CRT scanline & triad reinforcement
        // and curved sensor-vs-raster moiré ripple in curved tube space `(su, sv)`.
        let mut col = sample_rgb_bilinear(pixels, width, height, u, v);

        // 1. Bezel overhang drop-shadow onto the curved glass edge (if not already drawn by `retro_pc_frame`)
        if !scene.retro_pc_frame_already_drawn && !dark_room {
            let shadow_w = 0.024f32;
            if d_ap > -shadow_w {
                let falloff = ((d_ap + shadow_w) / shadow_w).clamp(0.0, 1.0);
                let dir_bias =
                    0.65 + 0.35 * ((-ny_ap * 0.5 + 0.5) * 0.65 + (-nx_ap * 0.5 + 0.5) * 0.35);
                let shadow_alpha = falloff.powf(1.6) * 0.58 * dir_bias;
                for c in 0..3 {
                    col[c] *= 1.0 - shadow_alpha;
                }
            }
        }

        // 2. Fine, Subtle High-Density CRT Horizontal Scanlines & Phosphor Triads in Curved Tube Space `(su, sv)`
        // Matches the fine ~480-line VGA/PC-98 CRT raster from the in-game CRT shader and real CRT monitor
        // photos: scanlines are delicate (~3px pitch at 1440p) with a gentle ~10-16% peak-to-trough dip
        // (`trough = 0.85..0.94`), never thick dark bars.
        let scan_count = if width.max(height) >= 320 {
            480.0f32
        } else {
            (height as f32 * 0.40).clamp(32.0, 56.0)
        };
        let scan_coord = sv * scan_count;
        let scan_phase = scan_coord.fract();
        // `cos_wave` is 1.0 at the center of the scanline beam (`scan_phase = 0.5`) and 0.0 in the gap (`0.0`/`1.0`)
        let cos_wave = 0.5 - 0.5 * (scan_phase * std::f32::consts::TAU).cos();
        let col_max = col[0].max(col[1]).max(col[2]).clamp(0.0, 1.0);

        // Bright highlights widen the Gaussian electron beam into adjacent scanline gaps (`beam_size` bloom),
        // while midtones have a delicate, natural ~8-12% scanline ripple matching the in-game shader.
        let beam = cos_wave.powf(1.02 - 0.32 * col_max);
        let trough = 0.90 + 0.05 * col_max;
        let peak = 1.025 - 0.01 * col_max;
        let scan_mult = trough + (peak - trough) * beam;

        // Fine vertical RGB phosphor triad pitch (`640` triads) with subtle modulation (`mask_str = 0.038`)
        let triad_count = if width.max(height) >= 320 {
            640.0f32
        } else {
            (width as f32 * 0.45).clamp(36.0, 80.0)
        };
        let triad_x = su * triad_count;
        let theta = triad_x * std::f32::consts::TAU;
        let mask_str = 0.038 * (1.0 - 0.45 * col_max * col_max);
        let mut mask_r = 1.0 + mask_str * theta.cos();
        let mut mask_g = 1.0 + mask_str * (theta - 2.0943951).cos();
        let mut mask_b = 1.0 + mask_str * (theta - 4.1887902).cos();

        if scene.crt_filter == CrtFilter::Lottes {
            // Subtle staggered slot-mask shadow bridges
            let slot_col = (triad_x * 2.0).floor() as i32;
            let slot_row =
                ((scan_coord + if (slot_col & 1) != 0 { 1.0 } else { 0.0 }) * 0.5).fract();
            let slot_bridge = 1.0
                - (1.0 - (slot_row * std::f32::consts::TAU).cos().abs())
                    * 0.025
                    * (1.0 - 0.45 * col_max);
            mask_r *= slot_bridge;
            mask_g *= slot_bridge;
            mask_b *= slot_bridge;
        } else {
            // Very faint Trinitron aperture-grille tension damper wires at sv = 0.30 and sv = 0.70
            let damper = 1.0
                - ((-0.5 * ((sv - 0.30) / 0.0010).powi(2)).exp()
                    + (-0.5 * ((sv - 0.70) / 0.0010).powi(2)).exp())
                    * 0.03;
            mask_r *= damper;
            mask_g *= damper;
            mask_b *= damper;
        }

        col[0] *= scan_mult * mask_r;
        col[1] *= scan_mult * mask_g;
        col[2] *= scan_mult * mask_b;

        // 3. CRT Phosphor Halation / Bloom
        let bloom_weight = (if dark_room { 0.32 } else { 0.15 }) * (0.75 + 0.25 * beam);
        let amb_lum =
            0.2126 * ambient_edge[0] + 0.7152 * ambient_edge[1] + 0.0722 * ambient_edge[2];
        for c in 0..3 {
            col[c] += ambient_edge[c] * (amb_lum * 0.65 + 0.15) * bloom_weight;
        }

        // 4. CRT 60Hz Rolling Shutter Phosphor Refresh Band (classic phone camera vs CRT effect)
        let band_dist = ((v - params.shutter_band_y) + (u - 0.5) * 0.06).abs();
        let refresh_boost = (-0.5 * (band_dist / 0.18).powi(2)).exp();
        let shutter_mult = 1.0 + params.shutter_band_strength * (refresh_boost * 1.4 - 0.45);
        col[0] *= shutter_mult * 0.98;
        col[1] *= shutter_mult;
        col[2] *= shutter_mult * (1.0 + 0.04 * refresh_boost);

        // 5. Curved CRT Scanline-vs-Sensor Moiré Ripple (like a real smartphone photo of a 480-line CRT)
        // Because the CRT scanlines follow curved tube coordinates `sv` while the smartphone CMOS sensor
        // rows follow straight tilted coordinates `ny_sensor`, their slight frequency difference creates
        // gentle curved horizontal/hyperbolic moiré wave bands across the screen.
        let cos_m = params.moire_angle_rad.cos();
        let sin_m = params.moire_angle_rad.sin();
        let mx = nx_sensor * cos_m - ny_sensor * sin_m;
        let my = nx_sensor * sin_m + ny_sensor * cos_m;
        let sensor_v = my * 0.5 + 0.5;
        let scanline_beat = ((sv - sensor_v * 0.94) * params.moire_freq * 2.1
            + (pos_x * pos_x - pos_y * pos_y * 0.35) * 7.5)
            .sin();
        let shadow_beat = (mx * params.moire_freq * 0.85 + (pos_x * pos_x + pos_y * pos_y) * 8.0)
            .sin()
            * (my * params.moire_freq * 0.35).cos();
        let lum = 0.2126 * col[0] + 0.7152 * col[1] + 0.0722 * col[2];
        let moire_mod = params.moire_strength * (0.35 + 0.65 * lum);
        let luma_ripple = 1.0 + scanline_beat * moire_mod * 1.15;
        col[0] *= luma_ripple * (1.0 + shadow_beat * moire_mod * 0.75);
        col[1] *= luma_ripple * (1.0 - shadow_beat * moire_mod * 0.50);
        col[2] *= luma_ripple * (1.0 + (shadow_beat * 1.7 + 1.1).sin() * moire_mod * 0.65);

        // 6. CRT Glass Reflections (Light Room vs Dark Room)
        let glass_scale = if scene.crt_glass_already_drawn {
            0.35
        } else {
            1.0
        };
        let nx_glass = pos_x * 0.24;
        let ny_glass = pos_y * 0.28;
        let fresnel = (pos_x.hypot(pos_y) * 0.65).clamp(0.0, 1.0).powi(3) * 0.14;

        if !dark_room {
            // Light Room: Overhead fluorescent ceiling fixture reflection + photographer silhouette
            // + ambient room light lifted blacks on phosphor coat (modulated by `beam` so carbon gaps stay darker)
            let rx = 2.0 * nx_glass + params.light_offset[0];
            let ry = -2.0 * ny_glass + params.light_offset[1];
            let troffer1 = eval_ceiling_troffer(-0.22, 0.42, 0.34, 0.046, rx, ry, 0.62);
            let troffer2 = eval_ceiling_troffer(0.20, 0.58, 0.25, 0.038, rx, ry, 0.62) * 0.35;
            let diffuse_room =
                (-0.5 * (((ry - 0.40) / 0.26).powi(2) + ((rx + 0.18) / 0.52).powi(2))).exp() * 0.09;

            // Sample photographer silhouette (`crt_reflection_silhouette.png`)
            let (sw, sh) = silhouette.dimensions();
            let sil_u = (u + nx_glass * 0.04 + params.pan_x * 0.5).clamp(0.0, 1.0);
            let sil_v = (v + ny_glass * 0.04 + params.pan_y * 0.5).clamp(0.0, 1.0);
            let sil_mask = sample_rgb_bilinear(silhouette.as_raw(), sw, sh, sil_u, sil_v)[0] * 0.78;

            let room_refl = ((troffer1 + troffer2) * 0.32 + diffuse_room + fresnel * 0.50)
                * (1.0 - 0.72 * sil_mask)
                * glass_scale
                * (0.72 + 0.28 * beam);

            // Dusty phosphor black-level lift in a lit room (lower on black carbon matrix between scanlines)
            let black_lift = 0.024 * (1.0 - 0.48 * sil_mask) * glass_scale * (0.42 + 0.58 * beam);
            col[0] = col[0] * (1.0 - 0.18 * sil_mask * glass_scale) + black_lift + room_refl * 0.90;
            col[1] = col[1] * (1.0 - 0.18 * sil_mask * glass_scale) + black_lift + room_refl * 0.95;
            col[2] = col[2] * (1.0 - 0.18 * sil_mask * glass_scale)
                + black_lift * 1.05
                + room_refl * 1.04;

            if params.flash_active {
                let fdx = (pos_x - params.flash_pos[0]) * 1.25;
                let fdy = pos_y - params.flash_pos[1];
                let fdist = fdx.hypot(fdy);
                let flash_glare = ((-0.5 * (fdist / 0.15).powi(2)).exp() * 0.42
                    + (-0.5 * (fdist / 0.42).powi(2)).exp() * 0.22)
                    * params.flash_intensity
                    * glass_scale;
                col[0] += flash_glare * 1.00;
                col[1] += flash_glare * 0.89;
                col[2] += flash_glare * 0.74;
            }
        } else {
            // Dark Room: Deep blacks, internal glass rim catch-light, and bright phosphor bloom
            let rim = fresnel * 0.25 * (amb_lum + 0.12) * glass_scale;
            col[0] += rim * 0.90;
            col[1] += rim * 0.95;
            col[2] += rim * 1.05;
        }

        col
    } else {
        // Flat LCD / IPS Monitor Mode (`CrtFilter::Off`):
        // Exact Euclidean rounded-rectangle SDF with crisp modern bezel corners (`r_corner = 0.0045`)
        // and straight, unwarped edges.
        let r_corner = 0.0045f32;
        let qx = (u - 0.5).abs() - (0.5 - r_corner);
        let qy = (v - 0.5).abs() - (0.5 - r_corner);
        let max_qx = qx.max(0.0);
        let max_qy = qy.max(0.0);
        let len_max_q = max_qx.hypot(max_qy);
        let d_ap = qx.max(qy).min(0.0) + len_max_q - r_corner;

        let (nx_dir, ny_dir) = if len_max_q > 1e-4 {
            (max_qx / len_max_q, max_qy / len_max_q)
        } else if qx > qy {
            (1.0, 0.0)
        } else {
            (0.0, 1.0)
        };
        let nx_ap = nx_dir * (u - 0.5).signum();
        let ny_ap = ny_dir * (v - 0.5).signum();

        if d_ap >= 0.0 {
            return eval_flat_lcd_bezel(u, v, d_ap, nx_ap, ny_ap, ambient_edge, dark_room, atlas);
        }

        let mut col = sample_rgb_bilinear(pixels, width, height, u, v);

        // 1. Raised inner bezel overhang contact shadow onto the recessed flat LCD panel
        if !dark_room {
            let shadow_w = 0.011f32;
            if d_ap > -shadow_w {
                let falloff = ((d_ap + shadow_w) / shadow_w).clamp(0.0, 1.0);
                let dir_bias =
                    0.60 + 0.40 * ((-ny_ap * 0.5 + 0.5) * 0.65 + (-nx_ap * 0.5 + 0.5) * 0.35);
                let shadow_alpha = falloff.powf(1.7) * 0.46 * dir_bias;
                for c in 0..3 {
                    col[c] *= 1.0 - shadow_alpha;
                }
            }
        }

        // 2. Smartphone camera lens / sensor bloom around bright emissive LCD regions
        let bloom_weight = if dark_room { 0.30 } else { 0.15 };
        let amb_lum =
            0.2126 * ambient_edge[0] + 0.7152 * ambient_edge[1] + 0.0722 * ambient_edge[2];
        for c in 0..3 {
            col[c] += ambient_edge[c] * (amb_lum * 0.60 + 0.12) * bloom_weight;
        }

        // 3. Flat IPS / LCD Backlight Black-Level Floor & Off-Axis Corner Glow
        let corner_x = (u - 0.5).abs() * 2.0;
        let corner_y = (v - 0.5).abs() * 2.0;
        let ips_corner = (corner_x * corner_y).powf(1.6);
        let view_angle_mag = (params.yaw_rad.hypot(params.pitch_rad) * 4.0).clamp(0.18, 0.75);
        let backlight_floor = if dark_room { 0.018 } else { 0.024 };
        let ips_glow = backlight_floor + 0.032 * ips_corner * view_angle_mag;
        let lum = 0.2126 * col[0] + 0.7152 * col[1] + 0.0722 * col[2];
        let dark_mask = (1.0 - lum).powi(2);
        col[0] += ips_glow * 0.88 * dark_mask;
        col[1] += ips_glow * 0.94 * dark_mask;
        col[2] += ips_glow * 1.12 * dark_mask;

        // 4. 2D Perspective-Grid LCD RGB Subpixel vs Camera Bayer Sensor Moiré Interference
        // The non-linear perspective foreshortening between monitor UV `(u, v)` and rotated
        // camera sensor coordinates `(mx, my)` creates authentic sweeping moiré fringes.
        let cos_m = params.moire_angle_rad.cos();
        let sin_m = params.moire_angle_rad.sin();
        let mx = nx_sensor * cos_m - ny_sensor * sin_m;
        let my = nx_sensor * sin_m + ny_sensor * cos_m;
        let persp_delta_x = (u - 0.5) * 2.0 - mx;
        let persp_delta_y = (v - 0.5) * 2.0 - my;
        let moire_x = (mx * params.moire_freq + persp_delta_x * params.moire_freq * 0.42).sin();
        let moire_y =
            (my * params.moire_freq * 0.85 + persp_delta_y * params.moire_freq * 0.36).cos();
        let beat_phase = moire_x * moire_y;
        let moire_mod = params.moire_strength * (0.32 + 0.68 * lum);
        col[0] *= 1.0 + beat_phase * moire_mod;
        col[1] *= 1.0 - beat_phase * moire_mod * 0.60;
        col[2] *= 1.0 + (beat_phase * 1.6 + 0.9).sin() * moire_mod * 0.80;

        // 5. Anti-Glare (AG) Polarizer Room Reflection & Photographer Silhouette in a Lit Room
        if !dark_room {
            let pos_x = u * 2.0 - 1.0;
            let pos_y = v * 2.0 - 1.0;
            let rx = pos_x * 0.18 + params.light_offset[0];
            let ry = -pos_y * 0.18 + params.light_offset[1];
            // Matte anti-glare coat (`gloss = 0.22`) spreads overhead room light into a soft diffuse glow
            let ag_troffer = eval_ceiling_troffer(-0.20, 0.38, 0.42, 0.065, rx, ry, 0.22) * 0.18;
            let ag_ambient = (-0.5 * (((ry - 0.32) / 0.38).powi(2) + ((rx + 0.15) / 0.62).powi(2)))
                .exp()
                * 0.055;

            let (sw, sh) = silhouette.dimensions();
            let sil_u = (u + pos_x * 0.015 + params.pan_x * 0.5).clamp(0.0, 1.0);
            let sil_v = (v + pos_y * 0.015 + params.pan_y * 0.5).clamp(0.0, 1.0);
            let sil_mask = sample_rgb_bilinear(silhouette.as_raw(), sw, sh, sil_u, sil_v)[0] * 0.65;

            let room_refl = (ag_troffer + ag_ambient) * (1.0 - 0.68 * sil_mask);
            let black_lift = 0.018 * (1.0 - 0.42 * sil_mask);
            col[0] = col[0] * (1.0 - 0.10 * sil_mask) + black_lift + room_refl * 0.92;
            col[1] = col[1] * (1.0 - 0.10 * sil_mask) + black_lift + room_refl * 0.96;
            col[2] = col[2] * (1.0 - 0.10 * sil_mask) + black_lift * 1.04 + room_refl * 1.04;
        }

        col
    }
}

/// Renders the complete smartphone photo of the monitor into an RGBA8 buffer `(pixels, out_w, out_h)`.
pub fn render_phone_photo_rgba(
    pixels: &[u8],
    width: u32,
    height: u32,
    scene: &PhotoSceneContext,
    params: &PhotoCameraParams,
) -> (Vec<u8>, u32, u32) {
    // For real window captures (longest side >= 320px), render at 2015-smartphone sensor
    // resolution (1920..=2400 on the longest side) so the photo is crisp and high-resolution.
    // Small synthetic buffers (< 320px, e.g. unit tests) preserve their exact dimensions.
    let longest = width.max(height);
    let (out_w, out_h) = if longest >= 320 {
        let target_long = longest.clamp(1920, 2400);
        if width >= height {
            let h = ((height as u64 * target_long as u64) / width.max(1) as u64).max(1) as u32;
            (target_long, h)
        } else {
            let w = ((width as u64 * target_long as u64) / height.max(1) as u64).max(1) as u32;
            (w, target_long)
        }
    } else {
        (width.max(1), height.max(1))
    };

    let aspect = (out_w as f32) / (out_h as f32);
    let gw = 64u32;
    let gh = 48u32;
    let ambient_grid = build_ambient_grid(pixels, width, height, gw, gh);
    let atlas = bezel_atlas();
    let silhouette = silhouette_mask();

    // Pass 1: Synthesize 3D perspective scene (monitor screen + glass + bezel/toolbar + room)
    let mut scene_buf = vec![[0.0f32; 3]; (out_w * out_h) as usize];
    for y in 0..out_h {
        let ny = (y as f32 + 0.5) / (out_h as f32) * 2.0 - 1.0;
        for x in 0..out_w {
            let nx = (x as f32 + 0.5) / (out_w as f32) * 2.0 - 1.0;
            let (u, v) = params.project_photo_to_monitor_uv(nx, ny, aspect);
            scene_buf[(y * out_w + x) as usize] = eval_monitor_plane_point(
                u,
                v,
                nx,
                ny,
                pixels,
                width,
                height,
                &ambient_grid,
                gw,
                gh,
                scene,
                params,
                atlas,
                silhouette,
            );
        }
    }

    // Pass 2: Sharp Center-Weighted OIS Hand Micro-Tremor + Corner Chromatic Aberration + 2015 Smartphone ISP
    // Strong center weight (0.78) preserves fine 480-line CRT scanlines and RGB phosphor triads
    // while two asymmetric ghost taps (0.12, 0.10) add natural handheld micro-tremor softness.
    let shake_dx = params.shake_angle_rad.cos() * params.shake_px;
    let shake_dy = params.shake_angle_rad.sin() * params.shake_px;
    let tap_offsets: [(f32, f32, f32); 3] =
        [(-0.55, -0.35, 0.10), (0.0, 0.0, 0.78), (0.65, 0.45, 0.12)];

    let sample_scene_bilinear = |fx: f32, fy: f32| -> [f32; 3] {
        let cx = fx.clamp(0.0, (out_w - 1) as f32);
        let cy = fy.clamp(0.0, (out_h - 1) as f32);
        let x0 = cx.floor() as u32;
        let y0 = cy.floor() as u32;
        let x1 = (x0 + 1).min(out_w - 1);
        let y1 = (y0 + 1).min(out_h - 1);
        let tx = cx - x0 as f32;
        let ty = cy - y0 as f32;
        let c00 = scene_buf[(y0 * out_w + x0) as usize];
        let c10 = scene_buf[(y0 * out_w + x1) as usize];
        let c01 = scene_buf[(y1 * out_w + x0) as usize];
        let c11 = scene_buf[(y1 * out_w + x1) as usize];
        [
            (c00[0] + (c10[0] - c00[0]) * tx) * (1.0 - ty) + (c01[0] + (c11[0] - c01[0]) * tx) * ty,
            (c00[1] + (c10[1] - c00[1]) * tx) * (1.0 - ty) + (c01[1] + (c11[1] - c01[1]) * tx) * ty,
            (c00[2] + (c10[2] - c00[2]) * tx) * (1.0 - ty) + (c01[2] + (c11[2] - c01[2]) * tx) * ty,
        ]
    };

    let seed_offset = ((params.seed & 0xFFFF) as f32) * 0.137;
    let mut out_rgba = vec![0u8; (out_w * out_h * 4) as usize];

    for y in 0..out_h {
        let ny = (y as f32 + 0.5) / (out_h as f32) * 2.0 - 1.0;
        for x in 0..out_w {
            let nx = (x as f32 + 0.5) / (out_w as f32) * 2.0 - 1.0;
            let r2 = nx * nx + ny * ny;

            // Subtle lateral chromatic aberration near frame corners
            let ca_px = params.chromatic_aberration * r2 * (out_w as f32);
            let ca_x = nx * ca_px;
            let ca_y = ny * ca_px;

            // Gentle handheld micro-tremor blur + slight corner softness
            let corner_blur = 1.0 + params.defocus_strength * r2 * 0.35;
            let mut acc = [0.0f32; 3];
            for &(t_par, t_perp, wt) in &tap_offsets {
                let ox =
                    (shake_dx * t_par - shake_dy * 0.15 * (t_perp * t_perp - 0.3)) * corner_blur;
                let oy =
                    (shake_dy * t_par + shake_dx * 0.15 * (t_perp * t_perp - 0.3)) * corner_blur;
                let sr = sample_scene_bilinear(x as f32 + ox + ca_x, y as f32 + oy + ca_y)[0];
                let sg = sample_scene_bilinear(x as f32 + ox, y as f32 + oy)[1];
                let sb = sample_scene_bilinear(x as f32 + ox - ca_x, y as f32 + oy - ca_y)[2];
                acc[0] += sr * wt;
                acc[1] += sg * wt;
                acc[2] += sb * wt;
            }

            // Smartphone auto-exposure & white-balance shift
            let mut r = acc[0] * params.exposure_gain * params.wb_rgb[0];
            let mut g = acc[1] * params.exposure_gain * params.wb_rgb[1];
            let mut b = acc[2] * params.exposure_gain * params.wb_rgb[2];

            // 2015 Smartphone ISP tone curve (natural contrast with gentle highlight roll-off)
            let isp_curve = |v: f32| -> f32 {
                let c = v.max(0.0);
                let mapped = (c * (1.06 + 0.22 * c)) / (1.0 + 0.26 * c * c);
                let s = mapped.clamp(0.0, 1.0);
                s * s * (3.0 - 2.0 * s) * 0.22 + s * 0.78
            };
            r = isp_curve(r);
            g = isp_curve(g);
            b = isp_curve(b);

            // Subtle lens vignetting
            let vig = (1.0 - params.vignette_strength * r2 * 0.50).clamp(0.65, 1.0);
            r *= vig;
            g *= vig;
            b *= vig;

            // Fine 2015-smartphone sensor grain (high spatial frequency, no big muddy blotches)
            let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let shadow_noise_boost = 1.25 - 0.55 * lum.clamp(0.0, 1.0);
            let luma_grain = (hash21(x as f32 + seed_offset, y as f32 + seed_offset * 1.7) - 0.5)
                * 2.0
                * params.iso_noise_lum
                * shadow_noise_boost;
            let cb_grain = (value_noise(x as f32 * 0.75 + seed_offset, y as f32 * 0.75) - 0.5)
                * 2.0
                * params.iso_noise_chroma
                * shadow_noise_boost;
            let cr_grain = (value_noise(x as f32 * 0.75 + 91.7, y as f32 * 0.75 + seed_offset)
                - 0.5)
                * 2.0
                * params.iso_noise_chroma
                * shadow_noise_boost;

            let rf = (r + luma_grain + cr_grain).clamp(0.0, 1.0);
            let gf = (g + luma_grain - 0.35 * cb_grain - 0.45 * cr_grain).clamp(0.0, 1.0);
            let bf = (b + luma_grain + cb_grain).clamp(0.0, 1.0);

            let out_idx = ((y * out_w + x) as usize) * 4;
            out_rgba[out_idx] = (rf * 255.0).round() as u8;
            out_rgba[out_idx + 1] = (gf * 255.0).round() as u8;
            out_rgba[out_idx + 2] = (bf * 255.0).round() as u8;
            out_rgba[out_idx + 3] = 255;
        }
    }

    (out_rgba, out_w, out_h)
}

/// Synthesizes a smartphone photo of the monitor (`Ctrl+S`) and encodes it as a JPEG image
/// with realistic phone compression quality.
pub fn encode_phone_photo_jpeg(
    pixels: &[u8],
    width: u32,
    height: u32,
    scene: &PhotoSceneContext,
    params: &PhotoCameraParams,
) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(width > 0 && height > 0, "Invalid screenshot dimensions");
    anyhow::ensure!(
        pixels.len() == (width as usize) * (height as usize) * 4,
        "Unexpected RGBA pixel buffer length"
    );

    let (rgba, out_w, out_h) = render_phone_photo_rgba(pixels, width, height, scene, params);

    // Convert RGBA8 -> RGB8 for JPEG encoding
    let mut rgb = Vec::with_capacity((out_w * out_h * 3) as usize);
    for px in rgba.chunks_exact(4) {
        rgb.push(px[0]);
        rgb.push(px[1]);
        rgb.push(px[2]);
    }

    let mut jpeg_bytes = Vec::with_capacity((out_w * out_h) as usize / 2);
    let mut encoder = JpegEncoder::new_with_quality(&mut jpeg_bytes, params.jpeg_quality);
    encoder.encode(&rgb, out_w, out_h, ColorType::Rgb8.into())?;
    Ok(jpeg_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn test_screenshot_hotkeys_s_and_ctrl_s() {
        let ctx = egui::Context::default();

        // 1. Plain `S` triggers Normal screenshot and consumes the key event
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::S,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(input, |ctx| {
            assert_eq!(check_screenshot_hotkeys(ctx), Some(ScreenshotMode::Normal));
            assert_eq!(check_screenshot_hotkeys(ctx), None);
        });

        // Release `S` before next key-down so egui does not mark the next press as `repeat: true`
        let mut release_input = egui::RawInput::default();
        release_input.events.push(egui::Event::Key {
            key: egui::Key::S,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(release_input, |_| {});

        // 2. `Ctrl+S` triggers PhonePhoto screenshot (and does NOT fall through to Normal)
        let mut input = egui::RawInput::default();
        input.modifiers = egui::Modifiers::CTRL;
        input.events.push(egui::Event::Key {
            key: egui::Key::S,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        });
        let _ = ctx.run(input, |ctx| {
            assert_eq!(
                check_screenshot_hotkeys(ctx),
                Some(ScreenshotMode::PhonePhoto)
            );
            assert_eq!(check_screenshot_hotkeys(ctx), None);
        });

        let mut release_input = egui::RawInput::default();
        release_input.events.push(egui::Event::Key {
            key: egui::Key::S,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(release_input, |_| {});

        // 3. `Shift+S` or `Alt+S` does not trigger either mode
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::S,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        });
        let _ = ctx.run(input, |ctx| {
            assert_eq!(check_screenshot_hotkeys(ctx), None);
        });
    }

    #[test]
    fn test_encode_normal_jpeg_roundtrip() {
        let width = 32;
        let height = 24;
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        for (i, px) in pixels.chunks_exact_mut(4).enumerate() {
            px[0] = (i % 255) as u8;
            px[1] = ((i * 3) % 255) as u8;
            px[2] = ((i * 7) % 255) as u8;
            px[3] = 255;
        }
        let jpg = encode_normal_jpeg(&pixels, width, height).unwrap();
        let decoded = image::load_from_memory(&jpg).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (width, height));
    }

    #[test]
    fn test_phone_camera_params_viewability_and_framing_bounds() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let scene = PhotoSceneContext {
            crt_filter: CrtFilter::Lottes,
            dark_room: false,
            ..Default::default()
        };

        for _ in 0..100 {
            let params = PhotoCameraParams::random(&mut rng, &scene);
            assert!(
                params.roll_rad.abs() >= 0.8f32.to_radians(),
                "Photo should always have noticeable hand-held rotation"
            );

            // Sample a 50x50 grid across the photo frame and measure what fraction of the
            // photo shows the monitor screen vs surrounding bezel/toolbar, and how much of
            // the screen is retained inside the photo.
            let grid_n = 50;
            let mut photo_screen_pixels = 0usize;
            let mut min_u = 1.0f32;
            let mut max_u = 0.0f32;
            let mut min_v = 1.0f32;
            let mut max_v = 0.0f32;

            for gy in 0..grid_n {
                let ny = (gy as f32 + 0.5) / (grid_n as f32) * 2.0 - 1.0;
                for gx in 0..grid_n {
                    let nx = (gx as f32 + 0.5) / (grid_n as f32) * 2.0 - 1.0;
                    let (u, v) = params.project_photo_to_monitor_uv(nx, ny, 4.0 / 3.0);
                    if (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v) {
                        photo_screen_pixels += 1;
                    }
                    // Ensure we never project far outside the bezel into a room/wall area
                    assert!(
                        (-0.115..=1.115).contains(&u) && (-0.115..=1.115).contains(&v),
                        "Projected UV ({u:.3}, {v:.3}) went too far outside the monitor bezel"
                    );
                    min_u = min_u.min(u);
                    max_u = max_u.max(u);
                    min_v = min_v.min(v);
                    max_v = max_v.max(v);
                }
            }

            let total_samples = (grid_n * grid_n) as f32;
            let bezel_fraction = 1.0 - (photo_screen_pixels as f32 / total_samples);
            assert!(
                bezel_fraction >= 0.025 && bezel_fraction <= 0.22,
                "Expected surrounding bezel/toolbar to occupy 2.5%..22% of photo frame, got {:.1}%",
                bezel_fraction * 100.0
            );

            // Verify that the vast majority of the screen is covered inside the photo
            let covered_w = (max_u.min(1.0) - min_u.max(0.0)).clamp(0.0, 1.0);
            let covered_h = (max_v.min(1.0) - min_v.max(0.0)).clamp(0.0, 1.0);
            assert!(
                covered_w * covered_h >= 0.88,
                "Expected at least 88% of the screen to remain in the picture, got {:.1}%",
                covered_w * covered_h * 100.0
            );
        }
    }

    #[test]
    fn test_phone_photo_randomization_and_room_modes() {
        let width = 160;
        let height = 120;
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                let idx = ((y * width + x) as usize) * 4;
                pixels[idx] = 60;
                pixels[idx + 1] = 140;
                pixels[idx + 2] = 210;
                pixels[idx + 3] = 255;
            }
        }

        let mut rng = rand::rngs::StdRng::seed_from_u64(12345);
        let crt_light_scene = PhotoSceneContext {
            crt_filter: CrtFilter::Lottes,
            dark_room: false,
            ..Default::default()
        };
        let p1 = PhotoCameraParams::random(&mut rng, &crt_light_scene);
        let p2 = PhotoCameraParams::random(&mut rng, &crt_light_scene);

        let (img1, w1, h1) = render_phone_photo_rgba(&pixels, width, height, &crt_light_scene, &p1);
        let (img2, w2, h2) = render_phone_photo_rgba(&pixels, width, height, &crt_light_scene, &p2);
        assert_eq!((w1, h1), (width, height));
        assert_eq!((w2, h2), (width, height));
        assert_ne!(
            img1, img2,
            "Two randomized Ctrl+S photos must not be identical"
        );

        // Compare Light Room vs Dark Room with identical camera pose
        let crt_dark_scene = PhotoSceneContext {
            crt_filter: CrtFilter::Lottes,
            dark_room: true,
            ..Default::default()
        };
        let mut fixed_params = p1.clone();
        fixed_params.exposure_gain = 1.0;
        let (light_rgba, _, _) =
            render_phone_photo_rgba(&pixels, width, height, &crt_light_scene, &fixed_params);
        let (dark_rgba, _, _) =
            render_phone_photo_rgba(&pixels, width, height, &crt_dark_scene, &fixed_params);
        assert_ne!(light_rgba, dark_rgba);

        // Compare CRT mode vs Flat LCD mode
        let lcd_light_scene = PhotoSceneContext {
            crt_filter: CrtFilter::Off,
            dark_room: false,
            ..Default::default()
        };
        let lcd_params = PhotoCameraParams::random(&mut rng, &lcd_light_scene);
        assert_eq!(
            lcd_params.barrel_k1, 0.0,
            "Flat LCD mode must have zero barrel curvature"
        );
        assert_eq!(
            lcd_params.shutter_band_strength, 0.0,
            "Flat LCD mode must not have CRT rolling-shutter band"
        );
        assert!(
            !lcd_params.flash_active,
            "Flat LCD mode must not have CRT curved glass flash glare"
        );
        let (lcd_rgba, _, _) =
            render_phone_photo_rgba(&pixels, width, height, &lcd_light_scene, &fixed_params);
        assert_ne!(light_rgba, lcd_rgba);

        // Verify that a 640x480 window capture upscales to 1920x1440 (2015-smartphone resolution)
        let win_w = 640u32;
        let win_h = 480u32;
        let win_pixels = vec![120u8; (win_w * win_h * 4) as usize];
        let (_, hi_w, hi_h) =
            render_phone_photo_rgba(&win_pixels, win_w, win_h, &lcd_light_scene, &lcd_params);
        assert_eq!((hi_w, hi_h), (1920, 1440));

        // Verify JPEG encoding produces valid decodable JPEG
        let jpeg = encode_phone_photo_jpeg(&pixels, width, height, &crt_light_scene, &p1).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!(decoded.width(), width);
        assert_eq!(decoded.height(), height);
    }

    #[test]
    fn test_crt_phone_photo_renders_visible_scanlines() {
        let width = 160;
        let height = 120;
        let pixels = vec![190u8; (width * height * 4) as usize];
        let mut rng = rand::rngs::StdRng::seed_from_u64(777);

        for (filter, dark_room) in [
            (CrtFilter::Lottes, false),
            (CrtFilter::Lottes, true),
            (CrtFilter::Halo, false),
            (CrtFilter::Halo, true),
        ] {
            let scene = PhotoSceneContext {
                crt_filter: filter,
                dark_room,
                ..Default::default()
            };
            let mut params = PhotoCameraParams::random(&mut rng, &scene);
            params.iso_noise_lum = 0.0;
            params.iso_noise_chroma = 0.0;

            let (rgba, out_w, out_h) =
                render_phone_photo_rgba(&pixels, width, height, &scene, &params);

            // Inspect a vertical slice down the center of the screen (`y` in 25%..75% of `out_h`)
            // and verify subtle, natural scanline peak-to-trough modulation is present without harsh dark bars.
            let cx = out_w / 2;
            let mut max_step_delta = 0.0f32;
            let mut direction_changes = 0usize;
            let mut prev_delta = 0.0f32;

            for y in (out_h / 4 + 1)..(out_h * 3 / 4) {
                let idx = ((y * out_w + cx) as usize) * 4;
                let lum = 0.2126 * (rgba[idx] as f32)
                    + 0.7152 * (rgba[idx + 1] as f32)
                    + 0.0722 * (rgba[idx + 2] as f32);
                let prev_idx = (((y - 1) * out_w + cx) as usize) * 4;
                let prev_lum = 0.2126 * (rgba[prev_idx] as f32)
                    + 0.7152 * (rgba[prev_idx + 1] as f32)
                    + 0.0722 * (rgba[prev_idx + 2] as f32);
                let delta = lum - prev_lum;
                max_step_delta = max_step_delta.max(delta.abs());
                if delta.abs() > 0.8 {
                    if (delta > 0.0 && prev_delta < 0.0) || (delta < 0.0 && prev_delta > 0.0) {
                        direction_changes += 1;
                    }
                    prev_delta = delta;
                }
            }

            assert!(
                (3.0..=28.0).contains(&max_step_delta),
                "Expected subtle, natural adjacent-scanline contrast (3..28 luma levels), got {max_step_delta:.1} for {filter:?} dark_room={dark_room}",
            );
            assert!(
                direction_changes >= 18,
                "Expected fine alternating horizontal scanline peaks and troughs, got {direction_changes} reversals for {filter:?} dark_room={dark_room}"
            );
        }
    }

    #[test]
    fn test_screenshot_state_and_clipboard_publish() {
        let mut state = ScreenshotState::default();
        let clipboard = Arc::new(Clipboard::default());
        let ctx = egui::Context::default();

        // 1. Queue Normal screenshot
        assert!(state.request(ScreenshotMode::Normal, PhotoSceneContext::default()));
        assert!(state.has_pending());
        assert_eq!(state.pending_mode(), Some(ScreenshotMode::Normal));
        // Duplicate request while pending is rejected
        assert!(!state.request(ScreenshotMode::PhonePhoto, PhotoSceneContext::default()));

        let handle = state.capture_handle();
        let req = handle.take_pending().expect("expected pending request");
        assert!(!state.has_pending());

        let width = 32;
        let height = 24;
        let pixels = vec![180u8; (width * height * 4) as usize];

        // Verify RAM-backed clipboard temp-file publishing directly via publish_bytes_with
        let jpg_bytes = encode_normal_jpeg(&pixels, width, height).unwrap();
        let published_len = clipboard
            .publish_bytes_with("screenshot-", ".jpg", &jpg_bytes, |path| {
                assert!(path.exists());
                assert_eq!(path.extension().and_then(|s| s.to_str()), Some("jpg"));
                Ok(None)
            })
            .unwrap();
        assert_eq!(published_len, jpg_bytes.len());

        handle.save_to_clipboard(
            req,
            Some((pixels.clone(), width, height)),
            clipboard.clone(),
            ctx.clone(),
        );

        // Wait briefly for worker thread to complete (Copied if X11/Wayland active, or Failed if headless)
        let mut finished_normal = false;
        for _ in 0..100 {
            for ev in state.poll_events() {
                if matches!(
                    ev,
                    ScreenshotEvent::Copied(ScreenshotMode::Normal)
                        | ScreenshotEvent::Failed(ScreenshotMode::Normal, _)
                ) {
                    finished_normal = true;
                }
            }
            if finished_normal {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(finished_normal, "Expected Normal screenshot worker event");

        // 2. Queue PhonePhoto screenshot
        let scene = PhotoSceneContext {
            crt_filter: CrtFilter::Halo,
            dark_room: true,
            ..Default::default()
        };
        assert!(state.request(ScreenshotMode::PhonePhoto, scene));
        let req2 = handle
            .take_pending()
            .expect("expected pending PhonePhoto request");
        handle.save_to_clipboard(req2, Some((pixels, width, height)), clipboard, ctx);

        let mut finished_phone = false;
        for _ in 0..100 {
            for ev in state.poll_events() {
                if matches!(
                    ev,
                    ScreenshotEvent::Copied(ScreenshotMode::PhonePhoto)
                        | ScreenshotEvent::Failed(ScreenshotMode::PhonePhoto, _)
                ) {
                    finished_phone = true;
                }
            }
            if finished_phone {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            finished_phone,
            "Expected PhonePhoto screenshot worker event"
        );
    }
}
