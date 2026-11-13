use crate::config::VtuberConfig;
use glam::{Mat3, Quat, Vec2, Vec3};

/// Standard 52 Apple ARKit "Perfect Sync" blendshapes in canonical order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum ArkitBlendshape {
    EyeBlinkLeft = 0,
    EyeLookDownLeft = 1,
    EyeLookInLeft = 2,
    EyeLookOutLeft = 3,
    EyeLookUpLeft = 4,
    EyeSquintLeft = 5,
    EyeWideLeft = 6,
    EyeBlinkRight = 7,
    EyeLookDownRight = 8,
    EyeLookInRight = 9,
    EyeLookOutRight = 10,
    EyeLookUpRight = 11,
    EyeSquintRight = 12,
    EyeWideRight = 13,
    JawForward = 14,
    JawLeft = 15,
    JawRight = 16,
    JawOpen = 17,
    MouthClose = 18,
    MouthFunnel = 19,
    MouthPucker = 20,
    MouthLeft = 21,
    MouthRight = 22,
    MouthSmileLeft = 23,
    MouthSmileRight = 24,
    MouthFrownLeft = 25,
    MouthFrownRight = 26,
    MouthDimpleLeft = 27,
    MouthDimpleRight = 28,
    MouthStretchLeft = 29,
    MouthStretchRight = 30,
    MouthRollLower = 31,
    MouthRollUpper = 32,
    MouthShrugLower = 33,
    MouthShrugUpper = 34,
    MouthPressLeft = 35,
    MouthPressRight = 36,
    MouthLowerDownLeft = 37,
    MouthLowerDownRight = 38,
    MouthUpperUpLeft = 39,
    MouthUpperUpRight = 40,
    BrowDownLeft = 41,
    BrowDownRight = 42,
    BrowInnerUp = 43,
    BrowOuterUpLeft = 44,
    BrowOuterUpRight = 45,
    CheekPuff = 46,
    CheekSquintLeft = 47,
    CheekSquintRight = 48,
    NoseSneerLeft = 49,
    NoseSneerRight = 50,
    TongueOut = 51,
}

pub const ARKIT_BLENDSHAPE_COUNT: usize = 52;

pub const ARKIT_CANONICAL_NAMES: [&str; ARKIT_BLENDSHAPE_COUNT] = [
    "eyeBlinkLeft",
    "eyeLookDownLeft",
    "eyeLookInLeft",
    "eyeLookOutLeft",
    "eyeLookUpLeft",
    "eyeSquintLeft",
    "eyeWideLeft",
    "eyeBlinkRight",
    "eyeLookDownRight",
    "eyeLookInRight",
    "eyeLookOutRight",
    "eyeLookUpRight",
    "eyeSquintRight",
    "eyeWideRight",
    "jawForward",
    "jawLeft",
    "jawRight",
    "jawOpen",
    "mouthClose",
    "mouthFunnel",
    "mouthPucker",
    "mouthLeft",
    "mouthRight",
    "mouthSmileLeft",
    "mouthSmileRight",
    "mouthFrownLeft",
    "mouthFrownRight",
    "mouthDimpleLeft",
    "mouthDimpleRight",
    "mouthStretchLeft",
    "mouthStretchRight",
    "mouthRollLower",
    "mouthRollUpper",
    "mouthShrugLower",
    "mouthShrugUpper",
    "mouthPressLeft",
    "mouthPressRight",
    "mouthLowerDownLeft",
    "mouthLowerDownRight",
    "mouthUpperUpLeft",
    "mouthUpperUpRight",
    "browDownLeft",
    "browDownRight",
    "browInnerUp",
    "browOuterUpLeft",
    "browOuterUpRight",
    "cheekPuff",
    "cheekSquintLeft",
    "cheekSquintRight",
    "noseSneerLeft",
    "noseSneerRight",
    "tongueOut",
];

impl ArkitBlendshape {
    pub const ALL: [Self; ARKIT_BLENDSHAPE_COUNT] = [
        Self::EyeBlinkLeft,
        Self::EyeLookDownLeft,
        Self::EyeLookInLeft,
        Self::EyeLookOutLeft,
        Self::EyeLookUpLeft,
        Self::EyeSquintLeft,
        Self::EyeWideLeft,
        Self::EyeBlinkRight,
        Self::EyeLookDownRight,
        Self::EyeLookInRight,
        Self::EyeLookOutRight,
        Self::EyeLookUpRight,
        Self::EyeSquintRight,
        Self::EyeWideRight,
        Self::JawForward,
        Self::JawLeft,
        Self::JawRight,
        Self::JawOpen,
        Self::MouthClose,
        Self::MouthFunnel,
        Self::MouthPucker,
        Self::MouthLeft,
        Self::MouthRight,
        Self::MouthSmileLeft,
        Self::MouthSmileRight,
        Self::MouthFrownLeft,
        Self::MouthFrownRight,
        Self::MouthDimpleLeft,
        Self::MouthDimpleRight,
        Self::MouthStretchLeft,
        Self::MouthStretchRight,
        Self::MouthRollLower,
        Self::MouthRollUpper,
        Self::MouthShrugLower,
        Self::MouthShrugUpper,
        Self::MouthPressLeft,
        Self::MouthPressRight,
        Self::MouthLowerDownLeft,
        Self::MouthLowerDownRight,
        Self::MouthUpperUpLeft,
        Self::MouthUpperUpRight,
        Self::BrowDownLeft,
        Self::BrowDownRight,
        Self::BrowInnerUp,
        Self::BrowOuterUpLeft,
        Self::BrowOuterUpRight,
        Self::CheekPuff,
        Self::CheekSquintLeft,
        Self::CheekSquintRight,
        Self::NoseSneerLeft,
        Self::NoseSneerRight,
        Self::TongueOut,
    ];

    /// Matches custom VRM expression names (case-insensitive, ignoring `_` and `-`).
    pub fn from_name_relaxed(name: &str) -> Option<Self> {
        let normalized: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        ARKIT_CANONICAL_NAMES
            .iter()
            .position(|cand| {
                cand.chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .map(|c| c.to_ascii_lowercase())
                    .eq(normalized.chars())
            })
            .map(|idx| Self::ALL[idx])
    }
}

/// Canonical VRM 1.0 / 0.x expression presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum VrmPresetExpression {
    Happy = 0,
    Angry = 1,
    Sad = 2,
    Relaxed = 3,
    Surprised = 4,
    Aa = 5,
    Ih = 6,
    Ou = 7,
    Ee = 8,
    Oh = 9,
    Blink = 10,
    BlinkLeft = 11,
    BlinkRight = 12,
    LookUp = 13,
    LookDown = 14,
    LookLeft = 15,
    LookRight = 16,
    Neutral = 17,
}

pub const VRM_PRESET_COUNT: usize = 18;

impl VrmPresetExpression {
    pub fn from_vrm_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "happy" | "joy" => Some(Self::Happy),
            "angry" => Some(Self::Angry),
            "sad" | "sorrow" => Some(Self::Sad),
            "relaxed" | "fun" => Some(Self::Relaxed),
            "surprised" => Some(Self::Surprised),
            "aa" | "a" => Some(Self::Aa),
            "ih" | "i" => Some(Self::Ih),
            "ou" | "u" => Some(Self::Ou),
            "ee" | "e" => Some(Self::Ee),
            "oh" | "o" => Some(Self::Oh),
            "blink" => Some(Self::Blink),
            "blinkleft" | "blink_l" => Some(Self::BlinkLeft),
            "blinkright" | "blink_r" => Some(Self::BlinkRight),
            "lookup" => Some(Self::LookUp),
            "lookdown" => Some(Self::LookDown),
            "lookleft" => Some(Self::LookLeft),
            "lookright" => Some(Self::LookRight),
            "neutral" => Some(Self::Neutral),
            _ => None,
        }
    }
}

/// Raw face-tracking snapshot produced by the background ONNX tracker worker (~24-30 Hz),
/// expressed in un-mirrored anatomical subject coordinates:
/// - `pitch`: +chin down, -chin up (radians)
/// - `yaw`: +turn toward subject's left (viewer's right), -turn toward subject's right (radians)
/// - `roll`: +tilt top of head toward subject's left (viewer's right) (radians)
/// - `blink_subj_left` / `blink_subj_right`: anatomical subject left/right eye closure `[0..1]`
#[derive(Clone, Debug)]
pub struct TrackingRigFrame {
    pub tracked: bool,
    pub confidence: f32,
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
    pub translation: Vec3,
    pub gaze_subj: Vec2,
    pub blink_subj_left: f32,
    pub blink_subj_right: f32,
    pub eye_wide_subj_left: f32,
    pub eye_wide_subj_right: f32,
    pub mouth_open: f32,
    pub mouth_wide: f32,
    pub mouth_smile_subj_left: f32,
    pub mouth_smile_subj_right: f32,
    pub brow_subj_left: f32,
    pub brow_subj_right: f32,
    pub timestamp_secs: f64,
}

impl Default for TrackingRigFrame {
    fn default() -> Self {
        Self {
            tracked: false,
            confidence: 0.0,
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
            translation: Vec3::ZERO,
            gaze_subj: Vec2::ZERO,
            blink_subj_left: 0.0,
            blink_subj_right: 0.0,
            eye_wide_subj_left: 0.0,
            eye_wide_subj_right: 0.0,
            mouth_open: 0.0,
            mouth_wide: 0.0,
            mouth_smile_subj_left: 0.0,
            mouth_smile_subj_right: 0.0,
            brow_subj_left: 0.0,
            brow_subj_right: 0.0,
            timestamp_secs: 0.0,
        }
    }
}

/// Evaluated per-display-frame pose in Avatar Character Space (`+X` avatar left / viewer right,
/// `+Y` up, `+Z` forward toward camera).
#[derive(Clone, Debug)]
pub struct EvaluatedAvatarPose {
    pub spine_rot: Quat,
    pub chest_rot: Quat,
    pub upper_chest_rot: Quat,
    pub neck_rot: Quat,
    pub head_rot: Quat,
    pub left_eye_rot: Quat,
    pub right_eye_rot: Quat,
    pub left_shoulder_rot: Quat,
    pub right_shoulder_rot: Quat,
    /// Arm relaxation angle from horizontal T-pose down toward `-Y` (radians, ~0.68 rad).
    pub arm_relax_angle: f32,
    pub elbow_bend_angle: f32,
    pub hips_offset: Vec3,
    pub vrm_presets: [f32; VRM_PRESET_COUNT],
    pub arkit: [f32; ARKIT_BLENDSHAPE_COUNT],
}

impl Default for EvaluatedAvatarPose {
    fn default() -> Self {
        Self {
            spine_rot: Quat::IDENTITY,
            chest_rot: Quat::IDENTITY,
            upper_chest_rot: Quat::IDENTITY,
            neck_rot: Quat::IDENTITY,
            head_rot: Quat::IDENTITY,
            left_eye_rot: Quat::IDENTITY,
            right_eye_rot: Quat::IDENTITY,
            left_shoulder_rot: Quat::IDENTITY,
            right_shoulder_rot: Quat::IDENTITY,
            arm_relax_angle: 0.70,
            elbow_bend_angle: 0.12,
            hips_offset: Vec3::ZERO,
            vrm_presets: [0.0; VRM_PRESET_COUNT],
            arkit: [0.0; ARKIT_BLENDSHAPE_COUNT],
        }
    }
}

/// Adaptive velocity-sensitive 1-Euro low-pass filter (`fc = min_cutoff + beta * |dx/dt|`).
/// Eliminates stationary landmark jitter while keeping blinks and fast head motion crisp.
#[derive(Clone, Debug)]
pub struct OneEuroFilter {
    min_cutoff: f32,
    beta: f32,
    d_cutoff: f32,
    x_prev: Option<f32>,
    dx_prev: f32,
}

impl OneEuroFilter {
    pub fn new(min_cutoff: f32, beta: f32) -> Self {
        Self {
            min_cutoff,
            beta,
            d_cutoff: 1.0,
            x_prev: None,
            dx_prev: 0.0,
        }
    }

    pub fn set_params(&mut self, min_cutoff: f32, beta: f32) {
        self.min_cutoff = min_cutoff.max(0.05);
        self.beta = beta.max(0.0);
    }

    fn smoothing_factor(dt: f32, cutoff: f32) -> f32 {
        let r = 2.0 * std::f32::consts::PI * cutoff * dt;
        r / (r + 1.0)
    }

    pub fn filter(&mut self, x: f32, dt: f32) -> f32 {
        if !x.is_finite() {
            return self.x_prev.unwrap_or(0.0);
        }
        let dt = dt.clamp(1.0 / 500.0, 0.25);
        let Some(x_prev) = self.x_prev else {
            self.x_prev = Some(x);
            self.dx_prev = 0.0;
            return x;
        };

        let dx = (x - x_prev) / dt;
        let a_d = Self::smoothing_factor(dt, self.d_cutoff);
        let dx_hat = a_d * dx + (1.0 - a_d) * self.dx_prev;
        let cutoff = self.min_cutoff + self.beta * dx_hat.abs();
        let a = Self::smoothing_factor(dt, cutoff);
        let x_hat = a * x + (1.0 - a) * x_prev;

        self.x_prev = Some(x_hat);
        self.dx_prev = dx_hat;
        x_hat
    }
}

/// Stateful animator blending live camera tracking, 1-Euro filtering, neutral calibration,
/// procedural idle breathing/blinking, and optional microphone lip-sync assist.
pub struct RigAnimator {
    last_time_secs: Option<f64>,
    tracking_weight: f32,
    pitch_f: OneEuroFilter,
    yaw_f: OneEuroFilter,
    roll_f: OneEuroFilter,
    tx_f: OneEuroFilter,
    ty_f: OneEuroFilter,
    tz_f: OneEuroFilter,
    gaze_x_f: OneEuroFilter,
    gaze_y_f: OneEuroFilter,
    blink_l_f: OneEuroFilter,
    blink_r_f: OneEuroFilter,
    mouth_open_f: OneEuroFilter,
    mouth_wide_f: OneEuroFilter,
    smile_l_f: OneEuroFilter,
    smile_r_f: OneEuroFilter,
    brow_l_f: OneEuroFilter,
    brow_r_f: OneEuroFilter,
    last_raw_head_quat: Quat,
}

impl Default for RigAnimator {
    fn default() -> Self {
        Self::new()
    }
}

impl RigAnimator {
    pub fn new() -> Self {
        Self {
            last_time_secs: None,
            tracking_weight: 0.0,
            pitch_f: OneEuroFilter::new(1.4, 0.35),
            yaw_f: OneEuroFilter::new(1.4, 0.35),
            roll_f: OneEuroFilter::new(1.4, 0.35),
            tx_f: OneEuroFilter::new(1.2, 0.20),
            ty_f: OneEuroFilter::new(1.2, 0.20),
            tz_f: OneEuroFilter::new(1.0, 0.15),
            gaze_x_f: OneEuroFilter::new(2.2, 0.60),
            gaze_y_f: OneEuroFilter::new(2.2, 0.60),
            blink_l_f: OneEuroFilter::new(3.5, 1.40),
            blink_r_f: OneEuroFilter::new(3.5, 1.40),
            mouth_open_f: OneEuroFilter::new(3.0, 1.10),
            mouth_wide_f: OneEuroFilter::new(2.2, 0.60),
            smile_l_f: OneEuroFilter::new(1.8, 0.40),
            smile_r_f: OneEuroFilter::new(1.8, 0.40),
            brow_l_f: OneEuroFilter::new(2.0, 0.50),
            brow_r_f: OneEuroFilter::new(2.0, 0.50),
            last_raw_head_quat: Quat::IDENTITY,
        }
    }

    pub fn current_raw_head_quat_array(&self) -> [f32; 4] {
        self.last_raw_head_quat.to_array()
    }

    pub fn evaluate(
        &mut self,
        now_secs: f64,
        raw: Option<&TrackingRigFrame>,
        cfg: &VtuberConfig,
        mic_level_db: Option<f32>,
    ) -> EvaluatedAvatarPose {
        let dt = self
            .last_time_secs
            .map(|prev| (now_secs - prev) as f32)
            .unwrap_or(1.0 / 60.0)
            .clamp(1.0 / 500.0, 0.1);
        self.last_time_secs = Some(now_secs);

        // Tune 1-Euro filter cutoff from user's smoothing slider [0.0..0.98].
        let smooth = cfg.smoothing.clamp(0.0, 0.98);
        let pose_cutoff = (4.5 * (1.0 - smooth * 0.92)).max(0.25);
        let pose_beta = (0.85 * (1.0 - smooth * 0.75)).max(0.08);
        self.pitch_f.set_params(pose_cutoff, pose_beta);
        self.yaw_f.set_params(pose_cutoff, pose_beta);
        self.roll_f.set_params(pose_cutoff, pose_beta);

        let is_live = raw.is_some_and(|r| {
            r.tracked && r.confidence >= 0.45 && (now_secs - r.timestamp_secs) < 0.45
        });
        let target_weight = if is_live { 1.0 } else { 0.0 };
        let fade_rate = if is_live { 6.5 } else { 3.2 };
        self.tracking_weight += (target_weight - self.tracking_weight) * (fade_rate * dt).min(1.0);
        let w_track = self.tracking_weight.clamp(0.0, 1.0);

        // Procedural idle breathing & natural periodic blink
        let t = now_secs as f32;
        let breath_wave = (t * 1.65).sin();
        let idle_sway_yaw = (t * 0.55).sin() * 0.028;
        let idle_sway_pitch = (t * 0.82).cos() * 0.018;
        let idle_sway_roll = (t * 0.43 + 1.1).sin() * 0.016;
        let idle_blink = procedural_idle_blink(t);

        let (
            mut pitch,
            mut yaw,
            mut roll,
            mut trans,
            mut gaze,
            mut blink_avatar_l,
            mut blink_avatar_r,
            mut eye_wide_l,
            mut eye_wide_r,
            mut mouth_open,
            mut mouth_wide,
            mut smile_l,
            mut smile_r,
            mut brow_l,
            mut brow_r,
        ) = (
            idle_sway_pitch,
            idle_sway_yaw,
            idle_sway_roll,
            Vec3::ZERO,
            Vec2::ZERO,
            idle_blink,
            idle_blink,
            0.0f32,
            0.0f32,
            0.0f32,
            0.0f32,
            0.05f32,
            0.05f32,
            0.0f32,
            0.0f32,
        );

        if let Some(r) = raw {
            // Mirror mode:
            // - User's physical right (`subj_right`) is on the right side of the monitor (`+X` = Avatar Left).
            // - User's physical left (`subj_left`) is on the left side of the monitor (`-X` = Avatar Right).
            // - Turning toward subject's right (`yaw < 0`) turns the mirror image toward `+X` (`+yaw`).
            let mirror_sign = if cfg.mirror_tracking { -1.0 } else { 1.0 };
            let m_yaw = r.yaw * mirror_sign;
            let m_roll = r.roll * mirror_sign;
            let m_pitch = r.pitch;

            let raw_q = Quat::from_euler(glam::EulerRot::YXZ, m_yaw, m_pitch, -m_roll);
            self.last_raw_head_quat = raw_q;

            let calibrated_q = if let Some(nq) = cfg.neutral_head_quat {
                let neutral = Quat::from_array(nq).normalize();
                if neutral.is_finite() {
                    neutral.inverse() * raw_q
                } else {
                    raw_q
                }
            } else {
                raw_q
            };
            let (cal_yaw, cal_pitch, neg_cal_roll) = calibrated_q.to_euler(glam::EulerRot::YXZ);
            let cal_roll = -neg_cal_roll;

            let scale = cfg.movement_scale.clamp(0.0, 2.5);
            let f_pitch = self
                .pitch_f
                .filter(cal_pitch * scale, dt)
                .clamp(-0.70, 0.70);
            let f_yaw = self.yaw_f.filter(cal_yaw * scale, dt).clamp(-0.95, 0.95);
            let f_roll = self.roll_f.filter(cal_roll * scale, dt).clamp(-0.65, 0.65);

            let f_tx = self
                .tx_f
                .filter(r.translation.x * (-mirror_sign) * scale, dt)
                .clamp(-0.12, 0.12);
            let f_ty = self
                .ty_f
                .filter(r.translation.y * scale, dt)
                .clamp(-0.08, 0.08);
            let f_tz = self
                .tz_f
                .filter(r.translation.z * scale, dt)
                .clamp(-0.12, 0.12);

            let f_gx = self
                .gaze_x_f
                .filter(r.gaze_subj.x * mirror_sign, dt)
                .clamp(-1.0, 1.0);
            let f_gy = self.gaze_y_f.filter(r.gaze_subj.y, dt).clamp(-1.0, 1.0);

            let (raw_bl, raw_br, raw_wl, raw_wr, raw_sl, raw_sr, raw_brl, raw_brr) =
                if cfg.mirror_tracking {
                    (
                        r.blink_subj_right,
                        r.blink_subj_left,
                        r.eye_wide_subj_right,
                        r.eye_wide_subj_left,
                        r.mouth_smile_subj_right,
                        r.mouth_smile_subj_left,
                        r.brow_subj_right,
                        r.brow_subj_left,
                    )
                } else {
                    (
                        r.blink_subj_left,
                        r.blink_subj_right,
                        r.eye_wide_subj_left,
                        r.eye_wide_subj_right,
                        r.mouth_smile_subj_left,
                        r.mouth_smile_subj_right,
                        r.brow_subj_left,
                        r.brow_subj_right,
                    )
                };

            let blink_gain = cfg.blink_sensitivity.clamp(0.2, 2.5);
            let mut bl = self
                .blink_l_f
                .filter((raw_bl * blink_gain).clamp(0.0, 1.0), dt);
            let mut br = self
                .blink_r_f
                .filter((raw_br * blink_gain).clamp(0.0, 1.0), dt);
            // Synchronize near-simultaneous blinks to avoid slight asymmetric webcam noise,
            // while preserving deliberate winks (`|bl - br| > 0.32`).
            if (bl - br).abs() < 0.25 {
                let avg = 0.5 * (bl + br);
                bl = bl * 0.35 + avg * 0.65;
                br = br * 0.35 + avg * 0.65;
            }
            // Snap nearly-closed eyelids cleanly to 1.0 and nearly-open eyelids to 0.0
            bl = smoothstep_remap(bl, 0.08, 0.85);
            br = smoothstep_remap(br, 0.08, 0.85);

            let mouth_gain = cfg.mouth_sensitivity.clamp(0.2, 3.0);
            let mo = self
                .mouth_open_f
                .filter((r.mouth_open * mouth_gain).clamp(0.0, 1.0), dt);
            let mw = self.mouth_wide_f.filter(r.mouth_wide.clamp(-1.0, 1.0), dt);
            let sl = self.smile_l_f.filter(raw_sl.clamp(0.0, 1.0), dt);
            let sr = self.smile_r_f.filter(raw_sr.clamp(0.0, 1.0), dt);
            let brl = self.brow_l_f.filter(raw_brl.clamp(-1.0, 1.0), dt);
            let brr = self.brow_r_f.filter(raw_brr.clamp(-1.0, 1.0), dt);

            pitch = lerp(idle_sway_pitch, f_pitch, w_track);
            yaw = lerp(idle_sway_yaw, f_yaw, w_track);
            roll = lerp(idle_sway_roll, f_roll, w_track);
            trans = Vec3::new(f_tx, f_ty, f_tz) * w_track;
            gaze = Vec2::new(f_gx, f_gy) * w_track;
            blink_avatar_l = lerp(idle_blink, bl, w_track);
            blink_avatar_r = lerp(idle_blink, br, w_track);
            eye_wide_l = raw_wl.clamp(0.0, 1.0) * w_track;
            eye_wide_r = raw_wr.clamp(0.0, 1.0) * w_track;
            mouth_open = mo * w_track;
            mouth_wide = mw * w_track;
            smile_l = lerp(0.05, sl, w_track);
            smile_r = lerp(0.05, sr, w_track);
            brow_l = brl * w_track;
            brow_r = brr * w_track;
        }

        // Optional microphone lip-sync boost/fallback (`-48 dB`..`-12 dB` -> `0.0..1.0`).
        if cfg.mic_lipsync_assist {
            if let Some(db) = mic_level_db.filter(|v| v.is_finite()) {
                let mic_open = ((db + 46.0) / 30.0).clamp(0.0, 1.0);
                mouth_open = mouth_open.max(mic_open * 0.85);
            }
        }

        // Distribute rotation organically across spine -> chest -> neck -> head
        let spine_rot = Quat::from_euler(
            glam::EulerRot::YXZ,
            yaw * 0.08,
            pitch * 0.06 + breath_wave * 0.008,
            -roll * 0.08 - trans.x * 0.35,
        );
        let chest_rot = Quat::from_euler(
            glam::EulerRot::YXZ,
            yaw * 0.10,
            pitch * 0.08 + breath_wave * 0.014,
            -roll * 0.10 - trans.x * 0.45,
        );
        let upper_chest_rot = Quat::from_euler(
            glam::EulerRot::YXZ,
            yaw * 0.07,
            breath_wave * 0.010,
            -roll * 0.07,
        );
        let neck_rot =
            Quat::from_euler(glam::EulerRot::YXZ, yaw * 0.32, pitch * 0.35, -roll * 0.32);
        let head_rot =
            Quat::from_euler(glam::EulerRot::YXZ, yaw * 0.43, pitch * 0.51, -roll * 0.43);

        // Eye bone gaze (~10 deg horizontal, ~7 deg vertical)
        let eye_yaw = gaze.x * 0.18;
        let eye_pitch = -gaze.y * 0.12;
        let eye_rot = Quat::from_euler(glam::EulerRot::YXZ, eye_yaw, eye_pitch, 0.0);

        // Subtle shoulder breathing lift
        let shoulder_lift = breath_wave * 0.018;
        let left_shoulder_rot = Quat::from_axis_angle(Vec3::Z, shoulder_lift);
        let right_shoulder_rot = Quat::from_axis_angle(Vec3::Z, -shoulder_lift);

        // Populate VRM preset expressions & ARKit 52 weights
        let mut presets = [0.0f32; VRM_PRESET_COUNT];
        let mut arkit = [0.0f32; ARKIT_BLENDSHAPE_COUNT];

        presets[VrmPresetExpression::BlinkLeft as usize] = blink_avatar_l;
        presets[VrmPresetExpression::BlinkRight as usize] = blink_avatar_r;
        presets[VrmPresetExpression::Blink as usize] = blink_avatar_l.min(blink_avatar_r);

        // Vowel decomposition from (mouth_open, mouth_wide)
        let open = mouth_open.clamp(0.0, 1.0);
        let wide_pos = mouth_wide.max(0.0); // smile / wide
        let pucker = (-mouth_wide).max(0.0); // round / narrow

        let w_aa = (open * (1.0 - pucker * 0.65) * (1.0 - wide_pos * 0.35)).clamp(0.0, 1.0);
        let w_ih = ((1.0 - (open - 0.25).abs() * 2.5).max(0.0) * wide_pos * 1.1).clamp(0.0, 1.0)
            * open.min(0.45)
            * 2.2;
        let w_ou = (open.min(0.65) * pucker * 1.35).clamp(0.0, 1.0);
        let w_ee = ((1.0 - (open - 0.45).abs() * 2.0).max(0.0) * wide_pos * 0.9).clamp(0.0, 1.0);
        let w_oh = ((open - 0.35).max(0.0) * pucker * 1.3).clamp(0.0, 1.0);

        presets[VrmPresetExpression::Aa as usize] = w_aa.clamp(0.0, 1.0);
        presets[VrmPresetExpression::Ih as usize] = w_ih.clamp(0.0, 1.0);
        presets[VrmPresetExpression::Ou as usize] = w_ou.clamp(0.0, 1.0);
        presets[VrmPresetExpression::Ee as usize] = w_ee.clamp(0.0, 1.0);
        presets[VrmPresetExpression::Oh as usize] = w_oh.clamp(0.0, 1.0);

        let smile_avg = 0.5 * (smile_l + smile_r);
        presets[VrmPresetExpression::Relaxed as usize] =
            ((smile_avg - 0.25) * 0.85).clamp(0.0, 0.65);
        presets[VrmPresetExpression::Surprised as usize] =
            (0.5 * (eye_wide_l + eye_wide_r) * 0.7).clamp(0.0, 0.7);

        presets[VrmPresetExpression::LookRight as usize] = gaze.x.max(0.0);
        presets[VrmPresetExpression::LookLeft as usize] = (-gaze.x).max(0.0);
        presets[VrmPresetExpression::LookUp as usize] = gaze.y.max(0.0);
        presets[VrmPresetExpression::LookDown as usize] = (-gaze.y).max(0.0);

        // ARKit 52 weights (in Avatar left/right convention)
        arkit[ArkitBlendshape::EyeBlinkLeft as usize] = blink_avatar_l;
        arkit[ArkitBlendshape::EyeBlinkRight as usize] = blink_avatar_r;
        arkit[ArkitBlendshape::EyeWideLeft as usize] = eye_wide_l * (1.0 - blink_avatar_l);
        arkit[ArkitBlendshape::EyeWideRight as usize] = eye_wide_r * (1.0 - blink_avatar_r);
        arkit[ArkitBlendshape::EyeSquintLeft as usize] =
            (smile_l * 0.45 * (1.0 - blink_avatar_l)).clamp(0.0, 1.0);
        arkit[ArkitBlendshape::EyeSquintRight as usize] =
            (smile_r * 0.45 * (1.0 - blink_avatar_r)).clamp(0.0, 1.0);

        // Gaze: `+gaze.x` looks toward `+X` (Avatar's Left = OutLeft + InRight)
        let gx_pos = gaze.x.max(0.0);
        let gx_neg = (-gaze.x).max(0.0);
        let gy_pos = gaze.y.max(0.0);
        let gy_neg = (-gaze.y).max(0.0);
        arkit[ArkitBlendshape::EyeLookOutLeft as usize] = gx_pos;
        arkit[ArkitBlendshape::EyeLookInRight as usize] = gx_pos;
        arkit[ArkitBlendshape::EyeLookInLeft as usize] = gx_neg;
        arkit[ArkitBlendshape::EyeLookOutRight as usize] = gx_neg;
        arkit[ArkitBlendshape::EyeLookUpLeft as usize] = gy_pos;
        arkit[ArkitBlendshape::EyeLookUpRight as usize] = gy_pos;
        arkit[ArkitBlendshape::EyeLookDownLeft as usize] = gy_neg;
        arkit[ArkitBlendshape::EyeLookDownRight as usize] = gy_neg;

        arkit[ArkitBlendshape::JawOpen as usize] = open;
        arkit[ArkitBlendshape::MouthFunnel as usize] = (pucker * open * 1.2).clamp(0.0, 1.0);
        arkit[ArkitBlendshape::MouthPucker as usize] =
            (pucker * (1.0 - open * 0.5)).clamp(0.0, 1.0);
        arkit[ArkitBlendshape::MouthSmileLeft as usize] = smile_l;
        arkit[ArkitBlendshape::MouthSmileRight as usize] = smile_r;
        arkit[ArkitBlendshape::CheekSquintLeft as usize] = (smile_l * 0.5).clamp(0.0, 1.0);
        arkit[ArkitBlendshape::CheekSquintRight as usize] = (smile_r * 0.5).clamp(0.0, 1.0);

        let brow_up = 0.5 * (brow_l.max(0.0) + brow_r.max(0.0));
        arkit[ArkitBlendshape::BrowInnerUp as usize] = brow_up;
        arkit[ArkitBlendshape::BrowOuterUpLeft as usize] = brow_l.max(0.0);
        arkit[ArkitBlendshape::BrowOuterUpRight as usize] = brow_r.max(0.0);
        arkit[ArkitBlendshape::BrowDownLeft as usize] = (-brow_l).max(0.0);
        arkit[ArkitBlendshape::BrowDownRight as usize] = (-brow_r).max(0.0);

        EvaluatedAvatarPose {
            spine_rot,
            chest_rot,
            upper_chest_rot,
            neck_rot,
            head_rot,
            left_eye_rot: eye_rot,
            right_eye_rot: eye_rot,
            left_shoulder_rot,
            right_shoulder_rot,
            arm_relax_angle: 0.70 - breath_wave * 0.012,
            elbow_bend_angle: 0.12 + breath_wave * 0.010,
            hips_offset: Vec3::new(
                trans.x * 0.45,
                trans.y * 0.35 + breath_wave * 0.003,
                trans.z * 0.35,
            ),
            vrm_presets: presets,
            arkit,
        }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep_remap(x: f32, edge0: f32, edge1: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1e-5)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Natural procedural blink curve (periodic single blink every ~4.1s with occasional double blink).
fn procedural_idle_blink(t: f32) -> f32 {
    let period = 4.1;
    let phase = t.rem_euclid(period);
    let blink1 = (1.0 - ((phase - 0.12) / 0.075).abs()).clamp(0.0, 1.0);
    let cycle_idx = (t / period).floor() as i32;
    let blink2 = if cycle_idx % 3 == 1 {
        (1.0 - ((phase - 0.34) / 0.070).abs()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    blink1.max(blink2)
}

/// Solves 6-DOF head pose, iris gaze, independent blinks, mouth openness/form, and brow motion
/// from MediaPipe FaceMesh V2's 478 3D landmarks (`landmarks` in normalized image space:
/// `x` right, `y` down, `z` depth away from camera).
pub fn solve_rig_from_mediapipe_478(
    landmarks: &[Vec3],
    confidence: f32,
    timestamp_secs: f64,
) -> TrackingRigFrame {
    if landmarks.len() < 468 {
        return TrackingRigFrame::default();
    }

    // Convert from image-down `(x_right, y_down, z_away)` into right-handed camera view space
    // `(x_right, y_up, z_toward_camera)`.
    let cam_pt = |idx: usize| -> Vec3 {
        let p = landmarks[idx];
        Vec3::new(p.x, -p.y, -p.z)
    };

    // Skull anchors for 3D head orientation
    let p_right_cheek = cam_pt(234); // Subject's right cheek (left in raw image)
    let p_left_cheek = cam_pt(454); // Subject's left cheek (right in raw image)
    let p_forehead = cam_pt(10);
    let p_chin = cam_pt(152);
    let p_nose = cam_pt(1);
    let p_bridge = cam_pt(168);

    let x_axis = (p_left_cheek - p_right_cheek).normalize_or_zero();
    let raw_up = (p_forehead - p_chin).normalize_or_zero();
    // Forward vector points from skull toward nose (`+Z` when facing camera directly):
    // `x_axis` (`+X` right in raw image) x `raw_up` (`+Y` up) = `+Z` (toward camera).
    let z_axis = x_axis.cross(raw_up).normalize_or_zero();
    let y_axis = z_axis.cross(x_axis).normalize_or_zero();

    let basis = Mat3::from_cols(x_axis, y_axis, z_axis);
    let q = Quat::from_mat3(&basis).normalize();
    let (yaw, raw_pitch, neg_roll) = q.to_euler(glam::EulerRot::YXZ);
    // Notice in `(x_right, y_up, z_toward_cam)`:
    // - Thumb along `+X`: curls `+Y` toward `+Z` (chin UP -> forehead away, chin toward cam).
    //   wait: `+Y` toward `+Z` tilts top of head TOWARD camera (`+Z`) = nodding chin DOWN!
    // - Thumb along `+Y`: curls `+Z` toward `+X` (face turns toward `+X` = subject's left!).
    // - Thumb along `+Z`: curls `+X` toward `+Y` (subject's left side goes UP = head tilts toward subject's right).
    let pitch = raw_pitch;
    let roll = -neg_roll;

    // Head translation from face center & inter-cheek scale relative to neutral framing (~0.30 width)
    let face_center = 0.5 * (p_left_cheek + p_right_cheek);
    let face_span = (p_left_cheek - p_right_cheek).length().max(0.05);
    let translation = Vec3::new(
        (face_center.x - 0.5) * 0.45,
        (face_center.y + 0.5) * 0.45,
        (face_span - 0.28) * 0.65,
    );

    // Eye Aspect Ratio (EAR) for subject's right eye (`33, 133, 159, 145, 158, 153`)
    // and subject's left eye (`362, 263, 386, 374, 385, 380`).
    let r_outer = cam_pt(33);
    let r_inner = cam_pt(133);
    let r_width = (r_inner - r_outer).length().max(1e-4);
    let r_lid_v1 = (cam_pt(159) - cam_pt(145)).length();
    let r_lid_v2 = (cam_pt(158) - cam_pt(153)).length();
    let r_ear = 0.5 * (r_lid_v1 + r_lid_v2) / r_width;

    let l_inner = cam_pt(362);
    let l_outer = cam_pt(263);
    let l_width = (l_outer - l_inner).length().max(1e-4);
    let l_lid_v1 = (cam_pt(386) - cam_pt(374)).length();
    let l_lid_v2 = (cam_pt(385) - cam_pt(380)).length();
    let l_ear = 0.5 * (l_lid_v1 + l_lid_v2) / l_width;

    // Typical open eye EAR ~ 0.27..0.33; closed eye EAR ~ 0.08..0.13.
    let blink_subj_right = ((0.275 - r_ear) / (0.275 - 0.115)).clamp(0.0, 1.0);
    let blink_subj_left = ((0.275 - l_ear) / (0.275 - 0.115)).clamp(0.0, 1.0);
    let eye_wide_subj_right = ((r_ear - 0.32) / 0.10).clamp(0.0, 1.0);
    let eye_wide_subj_left = ((l_ear - 0.32) / 0.10).clamp(0.0, 1.0);

    // Iris gaze (landmarks 468 = subject right iris center, 473 = subject left iris center)
    let gaze_subj = if landmarks.len() >= 478 {
        let r_iris = cam_pt(468);
        let l_iris = cam_pt(473);
        let r_eye_dir = (r_inner - r_outer).normalize_or_zero();
        let l_eye_dir = (l_outer - l_inner).normalize_or_zero();
        let r_u = (r_iris - r_outer).dot(r_eye_dir) / r_width; // 0..1 from right-outer to right-inner
        let l_u = (l_iris - l_inner).dot(l_eye_dir) / l_width; // 0..1 from left-inner to left-outer

        let r_mid_y = 0.5 * (cam_pt(159).y + cam_pt(145).y);
        let l_mid_y = 0.5 * (cam_pt(386).y + cam_pt(374).y);
        let r_v = (r_iris.y - r_mid_y) / (r_width * 0.22).max(1e-4);
        let l_v = (l_iris.y - l_mid_y) / (l_width * 0.22).max(1e-4);

        let gx = (((0.5 * (r_u + l_u)) - 0.5) * 4.2).clamp(-1.0, 1.0);
        let gy = ((0.5 * (r_v + l_v)) * 2.4).clamp(-1.0, 1.0);
        Vec2::new(gx, gy)
    } else {
        Vec2::ZERO
    };

    // Mouth openness & form
    let inter_ocular = (l_outer - r_outer).length().max(1e-4);
    let lip_top = cam_pt(13);
    let lip_bot = cam_pt(14);
    let mouth_r = cam_pt(61);
    let mouth_l = cam_pt(291);

    let mouth_v = (lip_top - lip_bot).length();
    let mouth_h = (mouth_l - mouth_r).length().max(1e-4);
    let mar = mouth_v / inter_ocular;
    let mouth_open = ((mar - 0.035) / 0.34).clamp(0.0, 1.0);

    // Neutral mouth width / inter_ocular is ~0.50; <0.43 is pucker, >0.56 is wide/smile
    let width_ratio = mouth_h / inter_ocular;
    let mouth_wide = ((width_ratio - 0.50) / 0.13).clamp(-1.0, 1.0);

    // Smile: mouth corners lifting relative to lip center along head `y_axis`
    let lip_center = 0.5 * (lip_top + lip_bot);
    let r_corner_lift = (mouth_r - lip_center).dot(y_axis) / inter_ocular;
    let l_corner_lift = (mouth_l - lip_center).dot(y_axis) / inter_ocular;
    let mouth_smile_subj_right =
        ((r_corner_lift + 0.015) / 0.085 + mouth_wide.max(0.0) * 0.35).clamp(0.0, 1.0);
    let mouth_smile_subj_left =
        ((l_corner_lift + 0.015) / 0.085 + mouth_wide.max(0.0) * 0.35).clamp(0.0, 1.0);

    // Eyebrow lift/furrow relative to eye centers along head `y_axis`
    let r_eye_center = 0.5 * (r_outer + r_inner);
    let l_eye_center = 0.5 * (l_outer + l_inner);
    let r_brow_dist = (cam_pt(105) - r_eye_center).dot(y_axis) / inter_ocular;
    let l_brow_dist = (cam_pt(334) - l_eye_center).dot(y_axis) / inter_ocular;
    let brow_subj_right = ((r_brow_dist - 0.17) / 0.06).clamp(-1.0, 1.0);
    let brow_subj_left = ((l_brow_dist - 0.17) / 0.06).clamp(-1.0, 1.0);

    let _ = p_nose;
    let _ = p_bridge;

    TrackingRigFrame {
        tracked: true,
        confidence,
        pitch,
        yaw,
        roll,
        translation,
        gaze_subj,
        blink_subj_left,
        blink_subj_right,
        eye_wide_subj_left,
        eye_wide_subj_right,
        mouth_open,
        mouth_wide,
        mouth_smile_subj_left,
        mouth_smile_subj_right,
        brow_subj_left,
        brow_subj_right,
        timestamp_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arkit_name_matching_is_case_and_separator_insensitive() {
        assert_eq!(
            ArkitBlendshape::from_name_relaxed("eyeBlinkLeft"),
            Some(ArkitBlendshape::EyeBlinkLeft)
        );
        assert_eq!(
            ArkitBlendshape::from_name_relaxed("eye_blink_right"),
            Some(ArkitBlendshape::EyeBlinkRight)
        );
        assert_eq!(
            ArkitBlendshape::from_name_relaxed("Jaw-Open"),
            Some(ArkitBlendshape::JawOpen)
        );
        assert_eq!(ArkitBlendshape::from_name_relaxed("nonexistent"), None);
    }

    #[test]
    fn test_one_euro_filter_dampens_small_noise_and_tracks_large_step() {
        let mut f = OneEuroFilter::new(1.0, 0.5);
        let _ = f.filter(0.0, 1.0 / 60.0);
        let small = f.filter(0.01, 1.0 / 60.0);
        assert!(
            small < 0.005,
            "stationary micro-noise must be heavily damped"
        );

        let mut fast = 0.0;
        for _ in 0..8 {
            fast = f.filter(1.0, 1.0 / 60.0);
        }
        assert!(fast > 0.85, "large fast step must converge rapidly");
    }

    #[test]
    fn test_rig_animator_mirror_and_calibration() {
        let mut anim = RigAnimator::new();
        let mut cfg = VtuberConfig::default();
        cfg.smoothing = 0.0;

        let frame = TrackingRigFrame {
            tracked: true,
            confidence: 0.99,
            pitch: 0.25,
            yaw: -0.30, // subject turns right -> mirror turns +X (viewer's right)
            roll: 0.0,
            blink_subj_right: 1.0, // physical right eye (on right side of monitor)
            blink_subj_left: 0.0,
            mouth_open: 0.8,
            timestamp_secs: 1.0,
            ..Default::default()
        };

        let mut pose = EvaluatedAvatarPose::default();
        for i in 0..20 {
            let mut f = frame.clone();
            f.timestamp_secs = 1.0 + i as f64 / 60.0;
            pose = anim.evaluate(f.timestamp_secs, Some(&f), &cfg, None);
        }

        // In mirror mode, physical right eye wink drives Avatar's left eye (`+X` on screen).
        assert!(pose.vrm_presets[VrmPresetExpression::BlinkLeft as usize] > 0.8);
        assert!(pose.vrm_presets[VrmPresetExpression::BlinkRight as usize] < 0.2);
        assert!(pose.vrm_presets[VrmPresetExpression::Aa as usize] > 0.5);

        // Calibrating neutral pose at the current raw head orientation zeros out head/neck rotation.
        cfg.neutral_head_quat = Some(anim.current_raw_head_quat_array());
        for i in 20..50 {
            let mut f = frame.clone();
            f.timestamp_secs = 1.0 + i as f64 / 60.0;
            pose = anim.evaluate(f.timestamp_secs, Some(&f), &cfg, None);
        }
        let (hy, hp, hr) = pose.head_rot.to_euler(glam::EulerRot::YXZ);
        assert!(hy.abs() < 0.02 && hp.abs() < 0.02 && hr.abs() < 0.02);
    }
}
