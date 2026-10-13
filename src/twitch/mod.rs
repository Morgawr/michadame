//! Twitch integration: chat connection state, messages and login.

pub mod alerts;
pub mod alert_audio;
pub mod auth;
pub mod broadcast;
pub mod clips;
pub mod emotes;
pub mod eventsub;
pub mod irc;
pub mod live;
pub mod preroll;
pub mod stream_info;

#[allow(unused_imports)]
pub use alerts::{ActiveAlert, AlertEvent, AlertKind, AlertTestKind, AlertsState};
pub use broadcast::{BroadcastHandle, BroadcastMetrics, BroadcastStatus};
pub use live::StreamStatus;
pub use preroll::PrerollPlayer;
#[allow(unused_imports)]
pub use stream_info::{StreamCategory, StreamInfo, StreamInfoState};

use crate::config::TwitchConfig;
use auth::{DeviceFlowEvent, StoredToken};
use crossbeam_channel::Receiver;
use eframe::egui;
use emotes::{EmoteCache, Fragment};
pub use irc::ConnectionStatus;
use irc::{IrcEvent, IrcMessage};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Messages fade out over this long before they expire.
pub const FADE_DURATION: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq)]
pub enum MessageKind {
    Chat,
    /// `/me` message.
    Action,
    /// Notices, subs/raids, moderation info.
    System,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub id: Option<String>,
    pub login: String,
    pub display_name: String,
    pub color: Option<egui::Color32>,
    pub fragments: Vec<Fragment>,
    pub kind: MessageKind,
    pub received: Instant,
    pub is_streamer: bool,
}

impl ChatMessage {
    fn system(text: impl Into<String>, now: Instant) -> Self {
        Self {
            id: None,
            login: String::new(),
            display_name: String::new(),
            color: None,
            fragments: vec![Fragment::Text(text.into())],
            kind: MessageKind::System,
            received: now,
            is_streamer: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AuthFlow {
    Idle,
    Requesting,
    AwaitingUser {
        user_code: String,
        verification_uri: String,
        expires_at: Instant,
    },
}

/// A chat message flying across the video (niconico style).
#[derive(Clone, Debug)]
pub struct Comment {
    pub id: Option<String>,
    pub login: String,
    pub fragments: Vec<Fragment>,
    pub spawned: Instant,
    /// Top of the comment as a fraction of the free vertical space (0 = top).
    pub y_norm: f32,
    /// Time to travel from fully off the right edge to fully off the left edge.
    pub duration: Duration,
}

impl Comment {
    /// 0 when entering on the right, 1 when fully gone on the left.
    pub fn progress(&self, now: Instant) -> f32 {
        now.saturating_duration_since(self.spawned).as_secs_f32() / self.duration.as_secs_f32().max(0.1)
    }
}

pub struct TwitchState {
    pub config: TwitchConfig,
    /// Editable buffers for the settings tab.
    pub channel_input: String,
    pub client_id_input: String,
    /// Settings changed by sliders, saved once the pointer is released.
    pub config_dirty: bool,

    pub token: Option<StoredToken>,
    pub status: ConnectionStatus,
    pub messages: VecDeque<ChatMessage>,
    pub input: String,
    /// Hidden from the live view with the `T` hotkey (session only).
    pub overlay_hidden: bool,
    /// Whether the text input box should request keyboard focus on the next frame.
    pub focus_input_requested: bool,
    /// Last drawn overlay rect (video window points), if visible.
    pub overlay_rect: Option<egui::Rect>,
    pub emotes: EmoteCache,
    pub auth_flow: AuthFlow,
    /// Whether the channel is actually broadcasting (from the Helix API).
    pub stream_status: StreamStatus,
    /// Niconico comments currently on screen.
    pub comments: VecDeque<Comment>,

    // Twitch live streaming client state
    pub broadcast: Option<BroadcastHandle>,
    pub stream_key_input: String,
    pub show_stream_key: bool,
    pub broadcast_last_status: Option<BroadcastStatus>,

    // Stream info (Title & Category)
    pub stream_info: StreamInfoState,
    pub stream_info_loaded: bool,
    pub studio_active: bool,
    pub alerts: AlertsState,
    pub alert_player: alert_audio::AlertSoundPlayer,
    pub audio_tap: Option<Arc<crate::replay::AudioTap>>,
    pub clips: clips::ClipManager,

    // Pre-roll video player
    pub preroll: Option<PrerollPlayer>,
    pub preroll_file_dialog_receiver: Option<Receiver<std::path::PathBuf>>,

    self_display_name: Option<String>,
    self_color: Option<egui::Color32>,
    worker: Option<irc::WorkerHandle>,
    eventsub: Option<eventsub::EventSubHandle>,
    live_poller: Option<live::LivePoller>,
    auth_rx: Option<Receiver<DeviceFlowEvent>>,
    auth_cancel: Option<Arc<AtomicBool>>,
    token_loaded: bool,
    notifications: Vec<(bool, String)>,
    pub ready_mined_clips: Vec<(i64, String)>,
    was_stream_live: bool,
    warmup_clip_deadline: Option<Instant>,
}

impl Default for TwitchState {
    fn default() -> Self {
        Self {
            config: TwitchConfig::default(),
            channel_input: String::new(),
            client_id_input: String::new(),
            config_dirty: false,
            token: None,
            status: ConnectionStatus::Disabled,
            messages: VecDeque::new(),
            input: String::new(),
            // Start hidden each session; the user opens it with `T`.
            overlay_hidden: true,
            focus_input_requested: false,
            overlay_rect: None,
            emotes: EmoteCache::default(),
            auth_flow: AuthFlow::Idle,
            stream_status: StreamStatus::Unknown(String::new()),
            comments: VecDeque::new(),
            broadcast: None,
            stream_key_input: String::new(),
            show_stream_key: false,
            broadcast_last_status: None,
            stream_info: StreamInfoState::default(),
            stream_info_loaded: false,
            studio_active: false,
            alerts: AlertsState::default(),
            alert_player: alert_audio::AlertSoundPlayer::default(),
            audio_tap: None,
            clips: clips::ClipManager::default(),
            preroll: None,
            preroll_file_dialog_receiver: None,
            self_display_name: None,
            self_color: None,
            worker: None,
            eventsub: None,
            live_poller: None,
            auth_rx: None,
            auth_cancel: None,
            token_loaded: false,
            notifications: Vec::new(),
            ready_mined_clips: Vec::new(),
            was_stream_live: false,
            warmup_clip_deadline: None,
        }
    }
}

impl TwitchState {
    /// Updates the local alert sound playback volume to follow the stream master volume.
    pub fn set_master_volume(&self, volume: f32) {
        self.alert_player.set_volume(volume);
    }

    /// Plays the retro alert chime both locally on PC and into the broadcast / replay audio stream.
    pub fn play_alert_sound(&self) {
        self.alert_player.play_local();
        if let Some(tap) = &self.audio_tap {
            tap.play_alert(self.alert_player.chime_samples());
        }
    }
}

/// Normalize user input (`Name`, `#name`, `twitch.tv/name`, full URL) to a channel login.
pub fn normalize_channel(input: &str) -> String {
    let mut s = input.trim();
    for prefix in ["https://", "http://"] {
        if let Some(r) = s.strip_prefix(prefix) {
            s = r;
        }
    }
    for prefix in ["www.", "m."] {
        if let Some(r) = s.strip_prefix(prefix) {
            s = r;
        }
    }
    if let Some(r) = s.strip_prefix("twitch.tv/") {
        s = r;
    }
    let s = s.split(['/', '?', '#']).find(|p| !p.is_empty()).unwrap_or("");
    s.trim_start_matches('@')
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(25)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Parse an IRC `#RRGGBB` color tag.
pub fn parse_color(tag: Option<&str>) -> Option<egui::Color32> {
    let hex = tag?.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some(egui::Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

impl TwitchState {
    pub fn apply_config(&mut self, config: TwitchConfig) {
        self.channel_input = config.channel.clone();
        self.client_id_input = config.client_id.clone();
        self.stream_key_input = config.stream_key.clone();
        self.config = config;
    }

    pub fn is_broadcasting(&self) -> bool {
        self.broadcast.as_ref().map_or(false, |b| b.is_live()) || self.preroll.is_some()
    }

    /// Returns true if Michadame is broadcasting or Twitch reports the channel as Live.
    pub fn is_stream_live(&self) -> bool {
        self.is_broadcasting() || matches!(self.stream_status, StreamStatus::Live { .. })
    }

    /// Resolves the broadcaster Twitch user ID for the current live stream.
    pub fn live_broadcaster_id(&self) -> Option<String> {
        if let StreamStatus::Live { broadcaster_id: Some(ref id), .. } = self.stream_status {
            return Some(id.clone());
        }
        if let Some(token) = &self.token {
            if !token.user_id.is_empty() {
                return Some(token.user_id.clone());
            }
        }
        None
    }

    /// Initiates an asynchronous Twitch clip creation, optionally bound to a mined word.
    pub fn request_clip(
        &self,
        title: String,
        mined_created_at: Option<i64>,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let Some(token) = &self.token else {
            return Err("Twitch login required to create clips".into());
        };
        let Some(broadcaster_id) = self.live_broadcaster_id() else {
            return Err("Cannot determine live stream broadcaster ID".into());
        };
        self.clips
            .trigger_clip(broadcaster_id, title, token.clone(), ctx.clone(), mined_created_at);
        Ok(())
    }

    /// Drains Twitch clips that have completed and were bound to a mined word timestamp.
    pub fn take_ready_mined_clips(&mut self) -> Vec<(i64, String)> {
        std::mem::take(&mut self.ready_mined_clips)
    }

    pub fn broadcast_metrics(&self) -> Option<Arc<BroadcastMetrics>> {
        self.broadcast.as_ref().map(|b| b.metrics.clone())
    }

    pub fn broadcast_status(&self) -> BroadcastStatus {
        self.broadcast
            .as_ref()
            .map(|b| b.status())
            .unwrap_or(BroadcastStatus::Offline)
    }

    pub fn preroll_remaining_secs(&self) -> Option<f64> {
        self.preroll.as_ref().map(|p| p.remaining_secs())
    }

    pub fn preroll_fade_alpha(&self) -> f32 {
        self.preroll.as_ref().map(|p| p.fade_alpha()).unwrap_or(0.0)
    }

    pub fn is_preroll_in_fade_in(&self) -> bool {
        self.preroll.as_ref().map(|p| p.is_in_fade_in_phase()).unwrap_or(false)
    }

    pub fn preroll_frame(&self) -> Option<Arc<crate::video::types::RawFrame>> {
        self.preroll.as_ref().and_then(|p| p.latest_frame.clone())
    }

    pub fn viewer_count(&self) -> Option<u32> {
        match self.stream_status {
            StreamStatus::Live { viewer_count, .. } => viewer_count,
            _ => None,
        }
    }

    pub fn start_broadcast(
        &mut self,
        audio_tap: Arc<crate::replay::AudioTap>,
        render_device: &str,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        if self.broadcast.is_some() {
            return Err("Broadcast is already active".into());
        }
        let stream_key = self.config.stream_key.trim();
        if stream_key.is_empty() {
            let err = "No stream key set. Please enter or fetch your Twitch stream key.".to_string();
            self.notify_error(&err);
            return Err(err);
        }

        let preroll_path = if self.config.preroll_enabled {
            let trimmed = self.config.preroll_video_path.trim();
            if trimmed.is_empty() {
                let err = "Pre-roll video is enabled, but no video file has been selected.".to_string();
                self.notify_error(&err);
                return Err(err);
            }
            let path = std::path::PathBuf::from(trimmed);
            if !path.exists() {
                let err = format!("Pre-roll video file not found: {:?}", path);
                self.notify_error(&err);
                return Err(err);
            }
            Some(path)
        } else {
            None
        };

        let config = broadcast::BroadcastConfig {
            video_bitrate_kbps: self.config.video_bitrate_kbps,
            audio_bitrate_kbps: self.config.audio_bitrate_kbps,
            video_encoder: self.config.video_encoder,
            encoder_preset: self.config.encoder_preset.clone(),
            ingest_server: self.config.ingest_server.clone(),
            stream_key: stream_key.to_string(),
            render_device: render_device.to_string(),
        };
        match broadcast::start_broadcast(config, audio_tap.clone()) {
            Ok(handle) => {
                self.broadcast = Some(handle);
                if let Some(path) = preroll_path {
                    let ctx_clone = ctx.clone();
                    let repaint = Arc::new(move || ctx_clone.request_repaint());
                    match PrerollPlayer::start(path, audio_tap, repaint) {
                        Ok(player) => {
                            self.preroll = Some(player);
                            self.notify_info("Starting pre-roll video broadcast…");
                        }
                        Err(e) => {
                            let msg = format!("Failed to start pre-roll video: {e:#}");
                            self.notify_error(&msg);
                        }
                    }
                } else {
                    self.notify_info("Connecting to Twitch live stream…");
                }
                Ok(())
            }
            Err(e) => {
                let msg = format!("Failed to start broadcast: {e:#}");
                self.notify_error(&msg);
                Err(msg)
            }
        }
    }

    pub fn stop_broadcast(&mut self) {
        if let Some(mut player) = self.preroll.take() {
            player.stop();
        }
        if let Some(mut handle) = self.broadcast.take() {
            handle.stop();
            self.broadcast_last_status = None;
            self.notify_info("Twitch broadcast stopped");
        }
    }

    pub fn open_preroll_file_dialog(&mut self, ctx: &egui::Context) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.preroll_file_dialog_receiver = Some(rx);
        let ctx_clone = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("preroll-file-picker".to_string())
            .spawn(move || {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter(
                        "Video files",
                        &[
                            "mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "m4v", "ts", "mpg",
                            "mpeg", "vob", "ogv", "3gp",
                        ],
                    )
                    .add_filter("All files", &["*"])
                    .set_title("Select Pre-Roll Video File")
                    .pick_file()
                {
                    let _ = tx.send(path);
                    ctx_clone.request_repaint();
                }
            });
    }

    pub fn try_fetch_stream_key(&mut self) {
        let Some(token) = &self.token else {
            self.notify_error("Log in to Twitch first to fetch your stream key");
            return;
        };
        match auth::fetch_stream_key(token) {
            Ok(key) => {
                self.config.stream_key = key.clone();
                self.stream_key_input = key;
                self.config_dirty = true;
                self.notify_info("Twitch stream key fetched from account!");
            }
            Err(e) => {
                self.notify_error(format!("Could not fetch stream key: {e}"));
            }
        }
    }

    pub fn client_id(&self) -> String {
        let id = self.config.client_id.trim();
        if id.is_empty() {
            auth::DEFAULT_CLIENT_ID.to_string()
        } else {
            id.to_string()
        }
    }

    pub fn can_send(&self) -> bool {
        self.worker.is_some()
            && self.token.is_some()
            && self.status == ConnectionStatus::Connected { authenticated: true }
    }

    pub fn overlay_contains(&self, pos: egui::Pos2) -> bool {
        self.overlay_rect.map_or(false, |r| r.contains(pos))
    }

    pub fn enqueue_alert(&mut self, event: AlertEvent) {
        if !self.config.alerts_enabled {
            return;
        }
        let allow = match &event.kind {
            AlertKind::Follow { .. } => self.config.alert_follows,
            AlertKind::Subscription { .. } => self.config.alert_subs,
            AlertKind::Bits { amount, .. } => {
                self.config.alert_bits && *amount >= self.config.alert_min_bits
            }
            AlertKind::Raid { .. } => self.config.alert_raids,
        };
        if allow {
            self.alerts.enqueue(event);
        }
    }

    pub fn take_notifications(&mut self) -> Vec<(bool, String)> {
        std::mem::take(&mut self.notifications)
    }

    fn notify_error(&mut self, msg: impl Into<String>) {
        self.notifications.push((true, msg.into()));
    }

    fn notify_info(&mut self, msg: impl Into<String>) {
        self.notifications.push((false, msg.into()));
    }

    /// Per-frame update. Returns true if a repaint is useful.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = false;
        if !self.token_loaded {
            self.token_loaded = true;
            self.token = auth::load_token();
        }
        changed |= self.poll_auth_flow(ctx);
        self.sync_connection(ctx);
        if self.token.is_some() && !self.stream_info_loaded {
            self.stream_info_loaded = true;
            if let Some(token) = &self.token {
                self.stream_info.fetch_channel(token, ctx);
            }
        }
        changed |= self.stream_info.poll(ctx);

        if let Some(handle) = &self.broadcast {
            let status = handle.status();
            if let BroadcastStatus::Error(ref err) = status {
                self.notify_error(format!("Broadcast error: {err}"));
                self.broadcast = None;
                self.broadcast_last_status = None;
                if let Some(mut player) = self.preroll.take() {
                    player.stop();
                }
                changed = true;
            } else if self.broadcast_last_status.as_ref() != Some(&status) {
                self.broadcast_last_status = Some(status);
                changed = true;
            }
        }

        if let Some(rx) = &self.preroll_file_dialog_receiver {
            if let Ok(path) = rx.try_recv() {
                self.config.preroll_video_path = path.to_string_lossy().to_string();
                self.config_dirty = true;
                changed = true;
            }
        }

        if let Some(player) = &mut self.preroll {
            let _ = player.poll_frame();
            if player.is_completed() {
                self.preroll = None;
                self.notify_info("Pre-roll finished: stream is now live!");
            }
            changed = true;
        }

        if self.broadcast.is_none() && self.preroll.is_some() {
            if let Some(mut player) = self.preroll.take() {
                player.stop();
            }
            changed = true;
        }

        let events: Vec<IrcEvent> = self
            .worker
            .as_ref()
            .map(|w| w.events.try_iter().collect())
            .unwrap_or_default();
        let now = Instant::now();
        for event in events {
            changed = true;
            match event {
                IrcEvent::Status(status) => self.status = status,
                IrcEvent::Message(msg) => self.handle_irc(&msg, now),
                IrcEvent::TokenRefreshed(token) => {
                    if let Err(e) = auth::save_token(&token) {
                        tracing::error!("Failed to save refreshed Twitch token: {e}");
                    }
                    self.token = Some(token);
                }
                IrcEvent::TokenInvalid(reason) => {
                    self.token = None;
                    auth::delete_token();
                    self.notify_error(format!("Twitch login expired ({reason}). Log in again to chat."));
                }
            }
        }
        changed |= self.emotes.poll(ctx);
        self.sync_live_poller(ctx);
        if let Some(poller) = &self.live_poller {
            if let Some(status) = poller.results.try_iter().last() {
                self.stream_status = status;
                changed = true;
            }
        }

        let is_live_now = self.is_stream_live();
        if is_live_now && !self.was_stream_live {
            self.was_stream_live = true;
            let deadline = now + Duration::from_secs(10);
            tracing::info!(
                "Twitch stream detected as live. Scheduling warm-up clip in 10s (preempting first-clip bug #935)..."
            );
            self.warmup_clip_deadline = Some(deadline);
            ctx.request_repaint_after(Duration::from_secs(10));
        } else if !is_live_now && self.was_stream_live {
            self.was_stream_live = false;
            self.warmup_clip_deadline = None;
        }

        if let Some(deadline) = self.warmup_clip_deadline {
            if now >= deadline {
                self.warmup_clip_deadline = None;
                if let (Some(token), Some(broadcaster_id)) = (&self.token, self.live_broadcaster_id()) {
                    clips::trigger_warmup_clip(broadcaster_id, token.clone());
                }
            } else {
                ctx.request_repaint_after(deadline.saturating_duration_since(now));
            }
        }
        self.sync_eventsub(ctx);
        if let Some(es) = &self.eventsub {
            let new_events: Vec<AlertEvent> = es.events.try_iter().collect();
            if !new_events.is_empty() {
                for ev in new_events {
                    self.enqueue_alert(ev);
                }
                changed = true;
            }
        }
        let default_alert_duration =
            Duration::from_secs_f32(self.config.alerts_duration_secs.clamp(1.0, 30.0));
        if self.alerts.update(now, default_alert_duration).is_some() {
            ctx.request_repaint();
            changed = true;
        }
        if self.config.alerts_enabled && self.alerts.take_just_activated() {
            self.play_alert_sound();
        }

        for event in self.clips.poll() {
            changed = true;
            match event {
                clips::ClipEvent::Ready { title, clip_url, mined_created_at } => {
                    self.send_chat_message(&clip_url);
                    self.notify_info(format!("Twitch clip ready for '{title}'!"));
                    if let Some(created_at) = mined_created_at {
                        self.ready_mined_clips.push((created_at, clip_url));
                    }
                }
                clips::ClipEvent::Failed { title, error, .. } => {
                    tracing::error!("Twitch clip creation failed for '{title}': {error}");
                    self.notify_error(format!("Failed to create clip for '{title}': {error}"));
                }
            }
        }

        self.trim_messages(now);
        self.trim_comments(now);
        changed
    }

    fn sync_eventsub(&mut self, ctx: &egui::Context) {
        let wanted = self.config.alerts_enabled && self.config.alert_follows && self.token.is_some();
        if !wanted {
            self.eventsub = None;
            return;
        }
        let Some(token) = &self.token else {
            self.eventsub = None;
            return;
        };
        let up_to_date = self
            .eventsub
            .as_ref()
            .map_or(false, |es| es.access_token == token.access_token);
        if !up_to_date {
            self.eventsub = Some(eventsub::spawn(token.clone(), Some(ctx.clone())));
        }
    }

    /// Poll Helix for the real live status while the overlay is on. Needs a login
    /// token (public client); otherwise the status is reported as unknown.
    fn sync_live_poller(&mut self, ctx: &egui::Context) {
        let channel = self.config.channel.clone();
        let wanted = (self.config.chat_overlay_enabled
            || self.is_broadcasting()
            || self.studio_active
            || self.config.share_mined_words_to_chat)
            && !channel.is_empty();
        if !wanted {
            self.live_poller = None;
            self.stream_status = StreamStatus::Unknown(String::new());
            return;
        }
        let Some(token) = self.token.clone() else {
            self.live_poller = None;
            self.stream_status = StreamStatus::Unknown("log in to Twitch to see live status".into());
            return;
        };
        let up_to_date = self
            .live_poller
            .as_ref()
            .map_or(false, |p| p.channel == channel && p.access_token == token.access_token);
        if !up_to_date {
            let channel_changed = self.live_poller.as_ref().map_or(true, |p| p.channel != channel);
            if channel_changed {
                self.stream_status = StreamStatus::Unknown("checking…".into());
            }
            self.live_poller = Some(live::spawn(channel, token, ctx.clone()));
        }
    }

    /// Start/stop/restart the IRC worker to match the settings.
    fn sync_connection(&mut self, ctx: &egui::Context) {
        let channel = self.config.channel.clone();
        let wanted = (self.config.chat_overlay_enabled
            || self.config.niconico_enabled
            || self.config.share_mined_words_to_chat)
            && !channel.is_empty();
        let login = self.token.as_ref().map(|t| t.login.clone());
        let up_to_date = self
            .worker
            .as_ref()
            .map_or(false, |w| w.channel == channel && w.login == login);
        if wanted && !up_to_date {
            let channel_changed = self.worker.as_ref().map_or(true, |w| w.channel != channel);
            if channel_changed {
                self.messages.clear();
                self.comments.clear();
                self.messages
                    .push_back(ChatMessage::system(format!("Joining #{channel}…"), Instant::now()));
            }
            self.worker = Some(irc::spawn(channel, self.token.clone(), Some(ctx.clone())));
            self.status = ConnectionStatus::Connecting;
        } else if !wanted && self.worker.is_some() {
            self.worker = None;
            self.comments.clear();
            self.status = ConnectionStatus::Disabled;
        }
    }

    /// Check whether a chat message is from the streamer/broadcaster.
    pub fn is_streamer(&self, chat: &ChatMessage) -> bool {
        if chat.is_streamer {
            return true;
        }
        if chat.login.is_empty() {
            return false;
        }
        let channel = if !self.config.channel.is_empty() {
            normalize_channel(&self.config.channel)
        } else {
            normalize_channel(&self.channel_input)
        };
        if !channel.is_empty() && chat.login.eq_ignore_ascii_case(&channel) {
            return true;
        }
        if let Some(token) = &self.token {
            if !token.login.is_empty() && chat.login.eq_ignore_ascii_case(&token.login) {
                return true;
            }
        }
        if let Some(worker) = &self.worker {
            if let Some(login) = &worker.login {
                if !login.is_empty() && chat.login.eq_ignore_ascii_case(login) {
                    return true;
                }
            }
        }
        if let Some(self_name) = &self.self_display_name {
            if !self_name.is_empty()
                && (chat.login.eq_ignore_ascii_case(self_name)
                    || chat.display_name.eq_ignore_ascii_case(self_name))
            {
                return true;
            }
        }
        false
    }

    /// Store a chat message, firing it as a niconico comment if enabled.
    fn push_chat(&mut self, chat: ChatMessage, now: Instant) {
        if self.config.niconico_enabled && chat.kind != MessageKind::System && !self.is_streamer(&chat) {
            self.spawn_comment(&chat, now, &mut rand::thread_rng());
        }
        self.messages.push_back(chat);
    }

    pub fn spawn_comment(&mut self, chat: &ChatMessage, now: Instant, rng: &mut impl rand::Rng) {
        if self.is_streamer(chat) {
            return;
        }
        let has_content = chat.fragments.iter().any(|f| match f {
            Fragment::Text(t) => !t.trim().is_empty(),
            Fragment::Emote { .. } => true,
        });
        if !has_content {
            return;
        }
        let base = self.config.niconico_duration_secs.clamp(2.0, 30.0);
        let duration = Duration::from_secs_f32(base * rng.gen_range(0.75..1.3));
        // Pick the candidate row least crowded by comments still entering
        // from the right, so new comments rarely sit on top of each other.
        let entering: Vec<f32> = self
            .comments
            .iter()
            .filter(|c| c.progress(now) < 0.4)
            .map(|c| c.y_norm)
            .collect();
        let band = self.config.niconico_size_pct.clamp(0.02, 0.3) * 1.15;
        let mut best = (usize::MAX, 0.0f32);
        for _ in 0..8 {
            let y: f32 = rng.gen_range(0.0..=1.0);
            let clashes = entering.iter().filter(|&&o| (o - y).abs() < band).count();
            if clashes < best.0 {
                best = (clashes, y);
                if clashes == 0 {
                    break;
                }
            }
        }
        self.comments.push_back(Comment {
            id: chat.id.clone(),
            login: chat.login.clone(),
            fragments: chat.fragments.clone(),
            spawned: now,
            y_norm: best.1,
            duration,
        });
        let max = self.config.niconico_max_comments as usize;
        while max > 0 && self.comments.len() > max {
            self.comments.pop_front();
        }
    }

    /// Drop comments that have fully left the screen (or all, if disabled).
    pub fn trim_comments(&mut self, now: Instant) {
        if !self.config.niconico_enabled {
            self.comments.clear();
            return;
        }
        self.comments.retain(|c| c.progress(now) < 1.0);
    }

    pub fn handle_irc(&mut self, msg: &IrcMessage, now: Instant) {
        match msg.command.as_str() {
            "PRIVMSG" => {
                if let Some(chat) = self.chat_from_privmsg(msg, now) {
                    if self.config.alerts_enabled && self.config.alert_bits {
                        if let Some(bits_str) = msg.tag("bits") {
                            if let Ok(bits) = bits_str.parse::<u32>() {
                                if bits >= self.config.alert_min_bits {
                                    let raw_text = msg.params.get(1).map(String::as_str).unwrap_or("");
                                    let text = raw_text
                                        .strip_prefix("\u{1}ACTION ")
                                        .map(|t| t.trim_end_matches('\u{1}'))
                                        .unwrap_or(raw_text)
                                        .trim()
                                        .to_string();
                                    self.enqueue_alert(AlertEvent {
                                        id: msg.tag("id").map(str::to_string).unwrap_or_else(|| {
                                            format!("bits-{}-{}", chat.login, now.elapsed().as_nanos())
                                        }),
                                        kind: AlertKind::Bits {
                                            user: chat.display_name.clone(),
                                            amount: bits,
                                            message: if text.is_empty() { None } else { Some(text) },
                                        },
                                    });
                                }
                            }
                        }
                    }
                    self.push_chat(chat, now);
                }
            }
            "USERNOTICE" => {
                if self.config.alerts_enabled {
                    let msg_id = msg.tag("msg-id").unwrap_or_default();
                    match msg_id {
                        "sub" | "resub" => {
                            if self.config.alert_subs {
                                let user = msg
                                    .tag("display-name")
                                    .or_else(|| msg.tag("login"))
                                    .unwrap_or("Someone")
                                    .to_string();
                                let tier = match msg.tag("msg-param-sub-plan") {
                                    Some("2000") => "Tier 2".to_string(),
                                    Some("3000") => "Tier 3".to_string(),
                                    Some("Prime") => "Prime".to_string(),
                                    _ => "Tier 1".to_string(),
                                };
                                let months = msg
                                    .tag("msg-param-cumulative-months")
                                    .and_then(|m| m.parse::<u32>().ok())
                                    .unwrap_or(1);
                                let message = msg.trailing().filter(|m| !m.trim().is_empty()).map(str::to_string);
                                self.enqueue_alert(AlertEvent {
                                    id: msg.tag("id").map(str::to_string).unwrap_or_else(|| {
                                        format!("sub-{user}-{}", now.elapsed().as_nanos())
                                    }),
                                    kind: AlertKind::Subscription {
                                        user,
                                        tier,
                                        months,
                                        is_gift: false,
                                        gifter: None,
                                        message,
                                    },
                                });
                            }
                        }
                        "subgift" | "submysterygift" => {
                            if self.config.alert_subs {
                                let gifter = msg
                                    .tag("display-name")
                                    .or_else(|| msg.tag("login"))
                                    .unwrap_or("An anonymous gifter")
                                    .to_string();
                                let recipient = msg
                                    .tag("msg-param-recipient-display-name")
                                    .or_else(|| msg.tag("msg-param-recipient-user-name"))
                                    .map(str::to_string);
                                let count = msg
                                    .tag("msg-param-mass-gift-count")
                                    .and_then(|c| c.parse::<u32>().ok());
                                let tier = match msg.tag("msg-param-sub-plan") {
                                    Some("2000") => "Tier 2".to_string(),
                                    Some("3000") => "Tier 3".to_string(),
                                    _ => "Tier 1".to_string(),
                                };
                                self.enqueue_alert(AlertEvent {
                                    id: msg.tag("id").map(str::to_string).unwrap_or_else(|| {
                                        format!("gift-{gifter}-{}", now.elapsed().as_nanos())
                                    }),
                                    kind: AlertKind::Subscription {
                                        user: recipient.unwrap_or_else(|| {
                                            format!("{} community members", count.unwrap_or(1))
                                        }),
                                        tier,
                                        months: 1,
                                        is_gift: true,
                                        gifter: Some(gifter),
                                        message: None,
                                    },
                                });
                            }
                        }
                        "raid" => {
                            if self.config.alert_raids {
                                let raider = msg
                                    .tag("msg-param-displayName")
                                    .or_else(|| msg.tag("msg-param-login"))
                                    .unwrap_or("A raider")
                                    .to_string();
                                let viewers = msg
                                    .tag("msg-param-viewerCount")
                                    .and_then(|v| v.parse::<u32>().ok())
                                    .unwrap_or(0);
                                self.enqueue_alert(AlertEvent {
                                    id: msg.tag("id").map(str::to_string).unwrap_or_else(|| {
                                        format!("raid-{raider}-{}", now.elapsed().as_nanos())
                                    }),
                                    kind: AlertKind::Raid {
                                        user: raider,
                                        viewers,
                                    },
                                });
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(system) = msg.tag("system-msg") {
                    self.messages.push_back(ChatMessage::system(system, now));
                }
                // Resub/announcement messages may carry user text as well.
                if msg.params.len() >= 2 {
                    if let Some(chat) = self.chat_from_privmsg(msg, now) {
                        self.push_chat(chat, now);
                    }
                }
            }
            "CLEARCHAT" => match msg.params.get(1) {
                Some(login) => {
                    let login = login.to_ascii_lowercase();
                    self.messages.retain(|m| m.login != login);
                    self.comments.retain(|c| c.login != login);
                }
                None => {
                    self.messages.clear();
                    self.comments.clear();
                    self.messages
                        .push_back(ChatMessage::system("Chat was cleared by a moderator", now));
                }
            },
            "CLEARMSG" => {
                if let Some(target) = msg.tag("target-msg-id") {
                    self.messages.retain(|m| m.id.as_deref() != Some(target));
                    self.comments.retain(|c| c.id.as_deref() != Some(target));
                }
            }
            "NOTICE" => {
                if let Some(text) = msg.trailing() {
                    self.messages.push_back(ChatMessage::system(text, now));
                }
            }
            "GLOBALUSERSTATE" | "USERSTATE" => {
                if let Some(name) = msg.tag("display-name") {
                    self.self_display_name = Some(name.to_string());
                }
                if let Some(color) = parse_color(msg.tag("color")) {
                    self.self_color = Some(color);
                }
            }
            _ => {}
        }
    }

    fn chat_from_privmsg(&mut self, msg: &IrcMessage, now: Instant) -> Option<ChatMessage> {
        let raw = msg.params.get(1)?;
        let (text, kind) = match raw
            .strip_prefix("\u{1}ACTION ")
            .map(|t| t.trim_end_matches('\u{1}'))
        {
            Some(t) => (t, MessageKind::Action),
            None => (raw.as_str(), MessageKind::Chat),
        };
        let login = msg
            .tag("login")
            .or_else(|| msg.nick())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let fragments = emotes::split_fragments(text, msg.tag("emotes"));
        self.emotes.learn(&fragments);
        let channel_param = msg
            .params
            .first()
            .map(|p| p.trim_start_matches('#'))
            .unwrap_or("");
        let is_broadcaster_badge = msg.tag("badges").map_or(false, |badges| {
            badges.split(',').any(|b| b.starts_with("broadcaster/"))
        });
        let is_room_owner = match (msg.tag("room-id"), msg.tag("user-id")) {
            (Some(room), Some(user)) => !room.is_empty() && room == user,
            _ => false,
        };
        let channel = if !self.config.channel.is_empty() {
            normalize_channel(&self.config.channel)
        } else {
            normalize_channel(&self.channel_input)
        };
        let is_streamer = is_broadcaster_badge
            || is_room_owner
            || (!login.is_empty() && !channel_param.is_empty() && login.eq_ignore_ascii_case(channel_param))
            || (!login.is_empty() && !channel.is_empty() && login.eq_ignore_ascii_case(&channel))
            || self
                .token
                .as_ref()
                .map_or(false, |t| !t.login.is_empty() && !login.is_empty() && login.eq_ignore_ascii_case(&t.login));

        Some(ChatMessage {
            id: msg.tag("id").map(str::to_string),
            display_name: msg.tag("display-name").map(str::to_string).unwrap_or_else(|| login.clone()),
            login,
            color: parse_color(msg.tag("color")),
            fragments,
            kind,
            received: now,
            is_streamer,
        })
    }

    /// Sends an arbitrary chat message over IRC and echoes it locally in chat.
    pub fn send_chat_message(&mut self, text: &str) -> bool {
        let text = text.trim();
        if text.is_empty() || !self.can_send() {
            return false;
        }
        let Some(worker) = &self.worker else {
            return false;
        };
        worker.send_privmsg(text);
        let now = Instant::now();
        let login = self.token.as_ref().map(|t| t.login.clone()).unwrap_or_default();
        let (body, kind) = match text.strip_prefix("/me ") {
            Some(b) => (b, MessageKind::Action),
            None => (text, MessageKind::Chat),
        };
        let chat = ChatMessage {
            id: None,
            display_name: self.self_display_name.clone().unwrap_or_else(|| login.clone()),
            login,
            color: self.self_color,
            fragments: emotes::split_by_known_names(body, &self.emotes.names),
            kind,
            received: now,
            is_streamer: true,
        };
        self.push_chat(chat, now);
        true
    }

    /// Send the current input line and echo it locally.
    pub fn send_input(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || !self.can_send() {
            return;
        }
        self.input.clear();
        let now = Instant::now();
        if text.starts_with('/') && !text.starts_with("/me ") {
            self.messages.push_back(ChatMessage::system(
                "Only /me is supported here; other chat commands aren't available over IRC.",
                now,
            ));
            return;
        }
        self.send_chat_message(&text);
    }

    pub fn trim_messages(&mut self, now: Instant) {
        let max = self.config.max_messages.max(1) as usize;
        while self.messages.len() > max {
            self.messages.pop_front();
        }
        if let Some(lifetime) = self.lifetime() {
            self.messages
                .retain(|m| now.saturating_duration_since(m.received) < lifetime);
        }
    }

    pub fn lifetime(&self) -> Option<Duration> {
        (self.config.message_lifetime_secs > 0)
            .then(|| Duration::from_secs(self.config.message_lifetime_secs as u64))
    }

    /// Opacity multiplier for a message (fades out before expiring).
    pub fn message_alpha(&self, msg: &ChatMessage, now: Instant) -> f32 {
        let Some(lifetime) = self.lifetime() else {
            return 1.0;
        };
        let remaining = lifetime.saturating_sub(now.saturating_duration_since(msg.received));
        let fade = FADE_DURATION.min(lifetime);
        if fade.is_zero() {
            return 1.0;
        }
        (remaining.as_secs_f32() / fade.as_secs_f32()).clamp(0.0, 1.0)
    }

    /// When the overlay next needs repainting for fade/expiry, if ever.
    pub fn next_fade_repaint(&self, now: Instant) -> Option<Duration> {
        let lifetime = self.lifetime()?;
        let oldest = self.messages.front()?;
        let fade_start = oldest.received + lifetime.saturating_sub(FADE_DURATION);
        if now >= fade_start {
            Some(Duration::from_millis(33))
        } else {
            Some(fade_start - now)
        }
    }

    // ---- Login -----------------------------------------------------------

    pub fn start_login(&mut self, ctx: &egui::Context) {
        self.cancel_login();
        let cancel = Arc::new(AtomicBool::new(false));
        self.auth_rx = Some(auth::start_device_flow(self.client_id(), cancel.clone(), ctx.clone()));
        self.auth_cancel = Some(cancel);
        self.auth_flow = AuthFlow::Requesting;
    }

    pub fn cancel_login(&mut self) {
        if let Some(cancel) = self.auth_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.auth_rx = None;
        self.auth_flow = AuthFlow::Idle;
    }

    pub fn logout(&mut self) {
        if let Some(token) = self.token.take() {
            auth::revoke_in_background(token);
        }
        auth::delete_token();
        self.self_display_name = None;
        self.self_color = None;
        self.stream_info = StreamInfoState::default();
        self.stream_info_loaded = false;
    }

    fn poll_auth_flow(&mut self, ctx: &egui::Context) -> bool {
        let Some(rx) = &self.auth_rx else {
            return false;
        };
        let events: Vec<DeviceFlowEvent> = rx.try_iter().collect();
        let changed = !events.is_empty();
        for event in events {
            match event {
                DeviceFlowEvent::Code {
                    user_code,
                    verification_uri,
                    expires_in,
                } => {
                    open_in_browser(ctx, &verification_uri);
                    self.auth_flow = AuthFlow::AwaitingUser {
                        user_code,
                        verification_uri,
                        expires_at: Instant::now() + expires_in,
                    };
                }
                DeviceFlowEvent::Success(token) => {
                    if let Err(e) = auth::save_token(&token) {
                        self.notify_error(format!("Could not save Twitch login: {e}"));
                    }
                    self.notify_info(format!("Logged in to Twitch as {}.", token.login));
                    self.token = Some(token);
                    self.stream_info_loaded = false;
                    self.auth_rx = None;
                    self.auth_cancel = None;
                    self.auth_flow = AuthFlow::Idle;
                }
                DeviceFlowEvent::Failed(reason) => {
                    self.notify_error(format!("Twitch login failed: {reason}"));
                    self.auth_rx = None;
                    self.auth_cancel = None;
                    self.auth_flow = AuthFlow::Idle;
                }
            }
        }
        changed
    }
}

pub fn open_in_browser(ctx: &egui::Context, url: &str) {
    if std::process::Command::new("xdg-open").arg(url).spawn().is_err() {
        ctx.open_url(egui::OpenUrl::new_tab(url));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_channel_names() {
        assert_eq!(normalize_channel("  SomeStreamer "), "somestreamer");
        assert_eq!(normalize_channel("#some_streamer"), "some_streamer");
        assert_eq!(normalize_channel("@name"), "name");
        assert_eq!(normalize_channel("https://www.twitch.tv/Foo_Bar/videos"), "foo_bar");
        assert_eq!(normalize_channel("twitch.tv/foo?x=1"), "foo");
        assert_eq!(normalize_channel("bad name!"), "badname");
        assert_eq!(normalize_channel(""), "");
    }

    #[test]
    fn parses_colors() {
        assert_eq!(parse_color(Some("#FF4500")), Some(egui::Color32::from_rgb(255, 69, 0)));
        assert_eq!(parse_color(Some("")), None);
        assert_eq!(parse_color(None), None);
        assert_eq!(parse_color(Some("#12345")), None);
    }

    fn irc(line: &str) -> IrcMessage {
        IrcMessage::parse(line).unwrap()
    }

    #[test]
    fn handles_chat_moderation_and_actions() {
        let mut state = TwitchState::default();
        let now = Instant::now();
        state.handle_irc(
            &irc("@display-name=Alice;color=#00FF00;emotes=25:6-10;id=m1 :alice!alice@alice.tmi.twitch.tv PRIVMSG #c :hello Kappa"),
            now,
        );
        state.handle_irc(
            &irc("@display-name=Bob;id=m2 :bob!bob@bob.tmi.twitch.tv PRIVMSG #c :\u{1}ACTION waves\u{1}"),
            now,
        );
        state.handle_irc(&irc("@id=m3 :alice!alice@alice.tmi.twitch.tv PRIVMSG #c :again"), now);
        assert_eq!(state.messages.len(), 3);
        let first = &state.messages[0];
        assert_eq!(first.display_name, "Alice");
        assert_eq!(first.color, Some(egui::Color32::from_rgb(0, 255, 0)));
        assert_eq!(
            first.fragments,
            vec![
                Fragment::Text("hello ".into()),
                Fragment::Emote { id: "25".into(), name: "Kappa".into() }
            ]
        );
        assert_eq!(state.emotes.names.get("Kappa").map(String::as_str), Some("25"));
        assert_eq!(state.messages[1].kind, MessageKind::Action);
        assert_eq!(state.messages[1].fragments, vec![Fragment::Text("waves".into())]);

        // A deleted message disappears.
        state.handle_irc(&irc("@target-msg-id=m2 :tmi.twitch.tv CLEARMSG #c :waves"), now);
        assert_eq!(state.messages.len(), 2);
        // A timeout removes that user's messages.
        state.handle_irc(&irc(":tmi.twitch.tv CLEARCHAT #c :alice"), now);
        assert!(state.messages.is_empty());
        // A full clear leaves a system notice.
        state.handle_irc(&irc("@id=m4 :bob!bob@bob.tmi.twitch.tv PRIVMSG #c :hi"), now);
        state.handle_irc(&irc(":tmi.twitch.tv CLEARCHAT #c"), now);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].kind, MessageKind::System);
    }

    #[test]
    fn trims_by_count_and_lifetime() {
        let mut state = TwitchState::default();
        state.config.max_messages = 3;
        state.config.message_lifetime_secs = 60;
        let start = Instant::now();
        for i in 0..5u64 {
            state
                .messages
                .push_back(ChatMessage::system(format!("{i}"), start + Duration::from_secs(i * 20)));
        }
        state.trim_messages(start + Duration::from_secs(80));
        // Count cap keeps t=40,60,80; all are younger than the 60s lifetime.
        assert_eq!(state.messages.len(), 3);
        state.trim_messages(start + Duration::from_secs(105));
        // t=40 expired (65s old), t=60 and t=80 remain.
        assert_eq!(state.messages.len(), 2);

        let msg = state.messages.front().unwrap().clone(); // t=60
        assert_eq!(state.message_alpha(&msg, start + Duration::from_secs(100)), 1.0);
        let a = state.message_alpha(&msg, start + Duration::from_secs(117) + Duration::from_millis(500));
        assert!((a - 0.5).abs() < 0.01);

        state.config.message_lifetime_secs = 0;
        assert_eq!(state.message_alpha(&msg, start + Duration::from_secs(10_000)), 1.0);
        assert!(state.next_fade_repaint(start).is_none());
    }

    #[test]
    fn live_status_is_unknown_without_login() {
        let ctx = egui::Context::default();
        let mut state = TwitchState::default();
        state.token_loaded = true;
        state.config.channel = "someone".into();
        state.config.chat_overlay_enabled = true;
        state.sync_live_poller(&ctx);
        assert!(state.live_poller.is_none());
        assert!(matches!(state.stream_status, StreamStatus::Unknown(ref r) if r.contains("log in")));
        // Even if a stale "live" result was around, disabling clears it.
        state.stream_status = StreamStatus::Live { viewer_count: None, broadcaster_id: None };
        state.config.chat_overlay_enabled = false;
        state.sync_live_poller(&ctx);
        assert!(matches!(state.stream_status, StreamStatus::Unknown(_)));
    }

    #[test]
    fn cannot_send_without_login_or_connection() {
        let mut state = TwitchState::default();
        state.input = "hello".into();
        state.send_input();
        assert!(state.messages.is_empty());
        assert_eq!(state.input, "hello");
    }

    #[test]
    fn niconico_comments_spawn_expire_and_obey_moderation() {
        let mut state = TwitchState::default();
        let now = Instant::now();
        // Disabled: no comments.
        state.handle_irc(&irc("@id=a0 :alice!alice@alice.tmi.twitch.tv PRIVMSG #c :hi"), now);
        assert!(state.comments.is_empty());

        state.config.niconico_enabled = true;
        state.handle_irc(&irc("@id=a1 :alice!alice@alice.tmi.twitch.tv PRIVMSG #c :hello"), now);
        state.handle_irc(&irc("@id=b1 :bob!bob@bob.tmi.twitch.tv PRIVMSG #c :yo"), now);
        state.handle_irc(&irc("@id=b2 :bob!bob@bob.tmi.twitch.tv PRIVMSG #c :   "), now);
        state.handle_irc(&irc(":tmi.twitch.tv NOTICE #c :system stuff"), now);
        assert_eq!(state.comments.len(), 2, "blank and system messages don't fly");
        let c = &state.comments[0];
        assert_eq!(c.fragments, vec![Fragment::Text("hello".into())]);
        assert!((0.0..=1.0).contains(&c.y_norm));
        let base = state.config.niconico_duration_secs;
        assert!(c.duration.as_secs_f32() >= base * 0.75 - 0.01 && c.duration.as_secs_f32() <= base * 1.3);

        state.handle_irc(&irc("@target-msg-id=a1 :tmi.twitch.tv CLEARMSG #c :hello"), now);
        assert_eq!(state.comments.len(), 1);
        state.handle_irc(&irc(":tmi.twitch.tv CLEARCHAT #c :bob"), now);
        assert!(state.comments.is_empty());

        state.handle_irc(&irc("@id=a2 :alice!alice@alice.tmi.twitch.tv PRIVMSG #c :again"), now);
        state.trim_comments(now + Duration::from_secs(1));
        assert_eq!(state.comments.len(), 1);
        state.trim_comments(now + Duration::from_secs(60));
        assert!(state.comments.is_empty(), "comments leave once off screen");

        // Unlimited by default.
        for i in 0..200 {
            state.handle_irc(&irc(&format!("@id=x{i} :x!x@x.tmi.twitch.tv PRIVMSG #c :spam {i}")), now);
        }
        assert_eq!(state.comments.len(), 200);
        // A cap drops the oldest.
        state.config.niconico_max_comments = 50;
        state.handle_irc(&irc("@id=last :x!x@x.tmi.twitch.tv PRIVMSG #c :last"), now);
        assert_eq!(state.comments.len(), 50);
        assert_eq!(state.comments.back().unwrap().id.as_deref(), Some("last"));
        state.config.niconico_enabled = false;
        state.trim_comments(now);
        assert!(state.comments.is_empty());
    }

    #[test]
    fn niconico_comments_avoid_rows_in_use() {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let mut state = TwitchState::default();
        state.config.niconico_enabled = true;
        let now = Instant::now();
        let msg = ChatMessage::system("x", now);
        let mut overlaps = 0;
        for _ in 0..5 {
            state.spawn_comment(&msg, now, &mut rng);
        }
        let ys: Vec<f32> = state.comments.iter().map(|c| c.y_norm).collect();
        for i in 0..ys.len() {
            for j in (i + 1)..ys.len() {
                if (ys[i] - ys[j]).abs() < state.config.niconico_size_pct {
                    overlaps += 1;
                }
            }
        }
        assert_eq!(overlaps, 0, "{ys:?}");
    }

    #[test]
    fn niconico_comments_skip_streamer_chat() {
        let mut state = TwitchState::default();
        state.config.niconico_enabled = true;
        state.config.channel = "coolstreamer".into();
        let now = Instant::now();

        // 1. Regular viewer chat spawns a comment and records a message.
        state.handle_irc(&irc("@id=v1 :viewer!viewer@viewer.tmi.twitch.tv PRIVMSG #coolstreamer :hello"), now);
        assert_eq!(state.comments.len(), 1);
        assert_eq!(state.messages.len(), 1);

        // 2. Chat from the channel broadcaster itself (same login as channel) is skipped for comments.
        state.handle_irc(&irc("@id=s1 :coolstreamer!coolstreamer@coolstreamer.tmi.twitch.tv PRIVMSG #coolstreamer :streamer says hi"), now);
        assert_eq!(state.comments.len(), 1, "streamer message must not spawn a niconico comment");
        assert_eq!(state.messages.len(), 2, "streamer message is still recorded in chat overlay");

        // 3. Chat with broadcaster badge is skipped even if channel name differed.
        state.handle_irc(&irc("@id=b1;badges=broadcaster/1,subscriber/12 :someone!someone@someone.tmi.twitch.tv PRIVMSG #coolstreamer :broadcaster badge"), now);
        assert_eq!(state.comments.len(), 1);
        assert_eq!(state.messages.len(), 3);

        // 4. Chat where user-id matches room-id is skipped.
        state.handle_irc(&irc("@id=r1;room-id=999;user-id=999 :host!host@host.tmi.twitch.tv PRIVMSG #coolstreamer :host message"), now);
        assert_eq!(state.comments.len(), 1);
        assert_eq!(state.messages.len(), 4);

        // 5. Chat from token login is skipped.
        state.token = Some(StoredToken {
            access_token: "test".into(),
            refresh_token: None,
            login: "tokenuser".into(),
            user_id: "123".into(),
            client_id: "cid".into(),
        });
        state.handle_irc(&irc("@id=t1 :tokenuser!tokenuser@tokenuser.tmi.twitch.tv PRIVMSG #coolstreamer :from authenticated account"), now);
        assert_eq!(state.comments.len(), 1);
        assert_eq!(state.messages.len(), 5);

        // 6. Direct spawn_comment call on a streamer message is also skipped.
        let streamer_msg = ChatMessage {
            id: Some("direct".into()),
            login: "coolstreamer".into(),
            display_name: "CoolStreamer".into(),
            color: None,
            fragments: vec![Fragment::Text("direct call".into())],
            kind: MessageKind::Chat,
            received: now,
            is_streamer: true,
        };
        state.spawn_comment(&streamer_msg, now, &mut rand::thread_rng());
        assert_eq!(state.comments.len(), 1, "direct spawn_comment on streamer message must be skipped");
    }

    #[test]
    fn video_alerts_parsed_from_irc_and_filter_settings() {
        let mut state = TwitchState::default();
        let now = Instant::now();

        // 1. Alerts disabled by default -> nothing enqueued
        state.handle_irc(
            &irc("@bits=100;display-name=CheerGuy :cheerguy!cheerguy@tmi.twitch.tv PRIVMSG #c :cheer100 awesome!"),
            now,
        );
        assert!(state.alerts.queue.is_empty());

        // 2. Enable alerts
        state.config.alerts_enabled = true;
        state.handle_irc(
            &irc("@id=b1;bits=100;display-name=CheerGuy :cheerguy!cheerguy@tmi.twitch.tv PRIVMSG #c :cheer100 awesome!"),
            now,
        );
        assert_eq!(state.alerts.queue.len(), 1);
        let alert = state.alerts.queue.pop_front().unwrap();
        assert_eq!(
            alert.kind,
            AlertKind::Bits {
                user: "CheerGuy".to_string(),
                amount: 100,
                message: Some("cheer100 awesome!".to_string()),
            }
        );

        // 3. Min bits filter
        state.config.alert_min_bits = 500;
        state.handle_irc(
            &irc("@id=b2;bits=250;display-name=SmallCheer :sc!sc@tmi.twitch.tv PRIVMSG #c :cheer250"),
            now,
        );
        assert!(state.alerts.queue.is_empty(), "cheer below min bits ignored");

        state.handle_irc(
            &irc("@id=b3;bits=500;display-name=BigCheer :bc!bc@tmi.twitch.tv PRIVMSG #c :cheer500"),
            now,
        );
        assert_eq!(state.alerts.queue.len(), 1);
        state.alerts.queue.clear();
        state.config.alert_min_bits = 1;

        // 4. Sub and Resub
        state.handle_irc(
            &irc("@id=s1;msg-id=sub;display-name=SubViewer;msg-param-sub-plan=1000 :tmi.twitch.tv USERNOTICE #c :Hyped!"),
            now,
        );
        assert_eq!(state.alerts.queue.len(), 1);
        let alert = state.alerts.queue.pop_front().unwrap();
        assert_eq!(
            alert.kind,
            AlertKind::Subscription {
                user: "SubViewer".to_string(),
                tier: "Tier 1".to_string(),
                months: 1,
                is_gift: false,
                gifter: None,
                message: Some("Hyped!".to_string()),
            }
        );

        // 5. Gift Sub
        state.handle_irc(
            &irc("@id=g1;msg-id=subgift;display-name=GenerousGifter;msg-param-recipient-display-name=LuckyRecipient;msg-param-sub-plan=2000 :tmi.twitch.tv USERNOTICE #c"),
            now,
        );
        assert_eq!(state.alerts.queue.len(), 1);
        let alert = state.alerts.queue.pop_front().unwrap();
        assert_eq!(
            alert.kind,
            AlertKind::Subscription {
                user: "LuckyRecipient".to_string(),
                tier: "Tier 2".to_string(),
                months: 1,
                is_gift: true,
                gifter: Some("GenerousGifter".to_string()),
                message: None,
            }
        );

        // 6. Raid
        state.handle_irc(
            &irc("@id=r1;msg-id=raid;msg-param-displayName=FellowStreamer;msg-param-viewerCount=75 :tmi.twitch.tv USERNOTICE #c"),
            now,
        );
        assert_eq!(state.alerts.queue.len(), 1);
        let alert = state.alerts.queue.pop_front().unwrap();
        assert_eq!(
            alert.kind,
            AlertKind::Raid {
                user: "FellowStreamer".to_string(),
                viewers: 75,
            }
        );

        // 7. Individual type toggles
        state.config.alert_raids = false;
        state.handle_irc(
            &irc("@id=r2;msg-id=raid;msg-param-displayName=AnotherRaider;msg-param-viewerCount=10 :tmi.twitch.tv USERNOTICE #c"),
            now,
        );
        assert!(state.alerts.queue.is_empty(), "raid alerts disabled");
    }

    #[test]
    fn test_alert_sound_dispatch_on_activation() {
        let mut state = TwitchState::default();
        state.config.alerts_enabled = true;
        let tap = Arc::new(crate::replay::AudioTap::default());
        state.audio_tap = Some(tap.clone());

        state.alerts.trigger_test(AlertTestKind::Follow);
        assert!(state.alerts.active.is_none());
        assert_eq!(state.alerts.queue.len(), 1);

        // When poll runs, alert is activated and play_alert_sound is called
        let ctx = egui::Context::default();
        state.poll(&ctx);
        assert!(state.alerts.active.is_some());
    }

    #[test]
    fn test_live_broadcaster_id_resolution() {
        let mut state = TwitchState::default();
        assert_eq!(state.live_broadcaster_id(), None);

        // From StoredToken
        state.token = Some(StoredToken {
            access_token: "tok".into(),
            refresh_token: None,
            login: "streamer".into(),
            user_id: "999".into(),
            client_id: "cid".into(),
        });
        assert_eq!(state.live_broadcaster_id(), Some("999".into()));

        // From StreamStatus::Live overrides
        state.stream_status = StreamStatus::Live {
            viewer_count: Some(10),
            broadcaster_id: Some("888".into()),
        };
        assert_eq!(state.live_broadcaster_id(), Some("888".into()));
    }

    #[test]
    fn test_poll_clip_events_dispatches_chat_and_error() {
        let ctx = egui::Context::default();
        let mut state = TwitchState::default();
        state.token_loaded = true;
        state.config.channel = "streamer".into();
        state.config.chat_overlay_enabled = true;
        state.status = ConnectionStatus::Connected { authenticated: true };
        let token = StoredToken {
            access_token: "tok".into(),
            refresh_token: None,
            login: "streamer".into(),
            user_id: "999".into(),
            client_id: "cid".into(),
        };
        state.token = Some(token);
        let (worker, cmd_rx) = irc::WorkerHandle::mock("streamer".into(), Some("streamer".into()));
        state.worker = Some(worker);

        // Trigger Ready clip event with mined word binding
        state.clips.send_event_for_test(clips::ClipEvent::Ready {
            title: "TestClip".into(),
            clip_url: "https://clips.twitch.tv/TestClip123".into(),
            mined_created_at: Some(12345),
        });

        state.poll(&ctx);
        // Clip url was posted to chat and sent to IRC worker
        assert_eq!(cmd_rx.try_recv().unwrap(), "https://clips.twitch.tv/TestClip123");
        assert_eq!(state.messages.len(), 1);
        let msg = state.messages.back().unwrap();
        assert_eq!(msg.fragments, vec![Fragment::Text("https://clips.twitch.tv/TestClip123".into())]);
        // And ready_mined_clips was queued for the word bank
        assert_eq!(state.take_ready_mined_clips(), vec![(12345, "https://clips.twitch.tv/TestClip123".into())]);
        // And in-app confirmation popup was displayed
        let notifs = state.take_notifications();
        assert_eq!(notifs.len(), 1);
        assert!(!notifs[0].0); // is_error == false (info toast)
        assert!(notifs[0].1.contains("Twitch clip ready"));

        // Trigger Failed clip event
        state.clips.send_event_for_test(clips::ClipEvent::Failed {
            title: "飽きる(あきる)".into(),
            error: "Something went wrong".into(),
            mined_created_at: Some(12345),
        });

        state.poll(&ctx);
        // Error notification WAS generated
        let notifs = state.take_notifications();
        assert_eq!(notifs.len(), 1);
        assert!(notifs[0].0); // is_error == true
        assert!(notifs[0].1.contains("Something went wrong"));
    }

    #[test]
    fn test_warmup_clip_scheduled_on_stream_live() {
        let ctx = egui::Context::default();
        let mut state = TwitchState::default();
        state.token_loaded = true;
        state.config.channel = "morgstream".into();
        state.config.chat_overlay_enabled = true;
        let token = StoredToken {
            access_token: "tok".into(),
            refresh_token: None,
            login: "morgstream".into(),
            user_id: "12345".into(),
            client_id: "cid".into(),
        };
        state.token = Some(token.clone());

        let (tx, rx) = crossbeam_channel::unbounded();
        state.live_poller = Some(live::LivePoller::mock(
            "morgstream".into(),
            token.access_token.clone(),
            rx,
        ));

        assert!(!state.is_stream_live());
        assert!(state.warmup_clip_deadline.is_none());

        // Channel goes live via poller
        tx.send(StreamStatus::Live {
            viewer_count: Some(5),
            broadcaster_id: Some("12345".into()),
        })
        .unwrap();
        state.poll(&ctx);
        assert!(state.is_stream_live());
        assert!(state.was_stream_live);
        assert!(state.warmup_clip_deadline.is_some());

        // Channel goes offline via poller -> timer is cleared
        tx.send(StreamStatus::Offline).unwrap();
        state.poll(&ctx);
        assert!(!state.is_stream_live());
        assert!(!state.was_stream_live);
        assert!(state.warmup_clip_deadline.is_none());
    }
}
