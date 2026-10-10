//! Twitch integration: chat connection state, messages and login.

pub mod auth;
pub mod emotes;
pub mod irc;
pub mod live;

pub use live::StreamStatus;

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
    /// Last drawn overlay rect (video window points), if visible.
    pub overlay_rect: Option<egui::Rect>,
    pub emotes: EmoteCache,
    pub auth_flow: AuthFlow,
    /// Whether the channel is actually broadcasting (from the Helix API).
    pub stream_status: StreamStatus,
    /// Niconico comments currently on screen.
    pub comments: VecDeque<Comment>,

    self_display_name: Option<String>,
    self_color: Option<egui::Color32>,
    worker: Option<irc::WorkerHandle>,
    live_poller: Option<live::LivePoller>,
    auth_rx: Option<Receiver<DeviceFlowEvent>>,
    auth_cancel: Option<Arc<AtomicBool>>,
    token_loaded: bool,
    notifications: Vec<(bool, String)>,
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
            overlay_rect: None,
            emotes: EmoteCache::default(),
            auth_flow: AuthFlow::Idle,
            stream_status: StreamStatus::Unknown(String::new()),
            comments: VecDeque::new(),
            self_display_name: None,
            self_color: None,
            worker: None,
            live_poller: None,
            auth_rx: None,
            auth_cancel: None,
            token_loaded: false,
            notifications: Vec::new(),
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
        self.config = config;
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
        self.trim_messages(now);
        self.trim_comments(now);
        changed
    }

    /// Poll Helix for the real live status while the overlay is on. Needs a login
    /// token (public client); otherwise the status is reported as unknown.
    fn sync_live_poller(&mut self, ctx: &egui::Context) {
        let channel = self.config.channel.clone();
        if !self.config.chat_overlay_enabled || channel.is_empty() {
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
        let wanted = (self.config.chat_overlay_enabled || self.config.niconico_enabled) && !channel.is_empty();
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

    /// Store a chat message, firing it as a niconico comment if enabled.
    fn push_chat(&mut self, chat: ChatMessage, now: Instant) {
        if self.config.niconico_enabled && chat.kind != MessageKind::System {
            self.spawn_comment(&chat, now, &mut rand::thread_rng());
        }
        self.messages.push_back(chat);
    }

    pub fn spawn_comment(&mut self, chat: &ChatMessage, now: Instant, rng: &mut impl rand::Rng) {
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
                    self.push_chat(chat, now);
                }
            }
            "USERNOTICE" => {
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
        Some(ChatMessage {
            id: msg.tag("id").map(str::to_string),
            display_name: msg.tag("display-name").map(str::to_string).unwrap_or_else(|| login.clone()),
            login,
            color: parse_color(msg.tag("color")),
            fragments,
            kind,
            received: now,
        })
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
        let Some(worker) = &self.worker else {
            return;
        };
        worker.send_privmsg(&text);
        let login = self.token.as_ref().map(|t| t.login.clone()).unwrap_or_default();
        let (body, kind) = match text.strip_prefix("/me ") {
            Some(b) => (b, MessageKind::Action),
            None => (text.as_str(), MessageKind::Chat),
        };
        let chat = ChatMessage {
            id: None,
            display_name: self.self_display_name.clone().unwrap_or_else(|| login.clone()),
            login,
            color: self.self_color,
            fragments: emotes::split_by_known_names(body, &self.emotes.names),
            kind,
            received: now,
        };
        self.push_chat(chat, now);
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
        state.stream_status = StreamStatus::Live;
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
}
