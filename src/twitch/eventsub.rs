//! Twitch EventSub WebSocket client for real-time events (followers).

use super::alerts::{AlertEvent, AlertKind};
use super::auth::StoredToken;
use super::irc::tls_config;
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tungstenite::WebSocket;
use url::Url;

const EVENTSUB_WS_URL: &str = "wss://eventsub.wss.twitch.tv/ws";
const SUBSCRIPTION_URL: &str = "https://api.twitch.tv/helix/eventsub/subscriptions";
const DEFAULT_KEEPALIVE_TIMEOUT_SECS: f64 = 10.0;
const MAX_RECENT_MESSAGE_IDS: usize = 256;

type TlsStream = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

enum WsStream {
    Tls(TlsStream),
    Plain(TcpStream),
}

impl WsStream {
    fn set_read_timeout(&self, dur: Option<Duration>) -> std::io::Result<()> {
        match self {
            Self::Tls(tls) => tls.get_ref().set_read_timeout(dur),
            Self::Plain(tcp) => tcp.set_read_timeout(dur),
        }
    }
}

impl Read for WsStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Tls(tls) => tls.read(buf),
            Self::Plain(tcp) => tcp.read(buf),
        }
    }
}

impl Write for WsStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Tls(tls) => tls.write(buf),
            Self::Plain(tcp) => tcp.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Tls(tls) => tls.flush(),
            Self::Plain(tcp) => tcp.flush(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EventSubMsg {
    Alert(AlertEvent),
    TokenRefreshed(StoredToken),
    TokenInvalid(String),
    Revoked(String),
}

pub struct EventSubHandle {
    pub access_token: String,
    pub events: Receiver<EventSubMsg>,
    stop: Arc<AtomicBool>,
}

impl Drop for EventSubHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub fn spawn(token: StoredToken, ctx: Option<egui::Context>) -> EventSubHandle {
    spawn_with_config(
        token,
        ctx,
        EVENTSUB_WS_URL.to_string(),
        SUBSCRIPTION_URL.to_string(),
        Duration::from_secs(2),
    )
}

fn spawn_with_config(
    token: StoredToken,
    ctx: Option<egui::Context>,
    ws_url: String,
    subscription_url: String,
    initial_backoff: Duration,
) -> EventSubHandle {
    let (tx, rx) = crossbeam_channel::unbounded();
    let stop = Arc::new(AtomicBool::new(false));
    let access_token = token.access_token.clone();

    let worker = Worker {
        token,
        stop: stop.clone(),
        event_tx: tx,
        ctx,
        ws_url,
        subscription_url,
        initial_backoff,
        recent_message_ids: VecDeque::new(),
    };

    std::thread::Builder::new()
        .name("twitch-eventsub".into())
        .spawn(move || worker.run())
        .expect("spawn twitch eventsub thread");

    EventSubHandle {
        access_token,
        events: rx,
        stop,
    }
}

struct Worker {
    token: StoredToken,
    stop: Arc<AtomicBool>,
    event_tx: Sender<EventSubMsg>,
    ctx: Option<egui::Context>,
    ws_url: String,
    subscription_url: String,
    initial_backoff: Duration,
    recent_message_ids: VecDeque<String>,
}

fn parse_keepalive_deadline(welcome_json: &Value) -> Duration {
    let secs = welcome_json
        .pointer("/payload/session/keepalive_timeout_seconds")
        .and_then(|v| v.as_f64().or_else(|| v.as_u64().map(|n| n as f64)))
        .filter(|&s| s.is_finite() && s > 0.0)
        .unwrap_or(DEFAULT_KEEPALIVE_TIMEOUT_SECS);
    let grace = (secs * 0.2).clamp(0.05, 5.0);
    Duration::from_secs_f64(secs + grace)
}

impl Worker {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn emit(&self, event: EventSubMsg) {
        let _ = self.event_tx.send(event);
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    fn record_message_id(&mut self, msg_id: &str) -> bool {
        if msg_id.is_empty() {
            return true;
        }
        if self.recent_message_ids.iter().any(|seen| seen == msg_id) {
            return false;
        }
        if self.recent_message_ids.len() >= MAX_RECENT_MESSAGE_IDS {
            self.recent_message_ids.pop_front();
        }
        self.recent_message_ids.push_back(msg_id.to_string());
        true
    }

    /// Validate (and refresh if needed) the token before connecting/subscribing.
    /// Returns `false` if the token is permanently invalid and the worker should stop.
    fn check_token(&mut self) -> bool {
        match super::auth::ensure_valid(&self.token) {
            Ok(Some(refreshed)) => {
                self.token = refreshed.clone();
                self.emit(EventSubMsg::TokenRefreshed(refreshed));
                true
            }
            Ok(None) => true,
            Err(super::auth::AuthError::Invalid(reason)) => {
                self.emit(EventSubMsg::TokenInvalid(reason));
                false
            }
            Err(super::auth::AuthError::Network(_)) => true,
        }
    }

    fn run(mut self) {
        let mut backoff = self.initial_backoff;
        let max_backoff = Duration::from_secs(60);
        let mut current_url = self.ws_url.clone();
        let mut is_reconnect = false;

        while !self.stopped() {
            if !is_reconnect && !self.check_token() {
                break;
            }
            let res = self.session(&current_url, is_reconnect);
            match res {
                Ok(Some(new_url)) => {
                    backoff = self.initial_backoff;
                    current_url = new_url;
                    is_reconnect = true;
                }
                Ok(None) => {
                    backoff = self.initial_backoff;
                    current_url = self.ws_url.clone();
                    is_reconnect = false;
                }
                Err(err) => {
                    tracing::warn!(
                        "Twitch EventSub disconnected ({err}), reconnecting in {:?}",
                        backoff
                    );
                    current_url = self.ws_url.clone();
                    is_reconnect = false;
                    let until = Instant::now() + backoff;
                    while Instant::now() < until && !self.stopped() {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    backoff = (backoff * 2).min(max_backoff);
                }
            }
        }
    }

    fn handle_notification_or_revocation(&mut self, msg_type: &str, json: &Value, raw_text: &str) {
        let msg_id = json
            .pointer("/metadata/message_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        if !self.record_message_id(&msg_id) {
            tracing::debug!("Twitch EventSub: ignoring duplicate message_id {msg_id}");
            return;
        }

        match msg_type {
            "notification" => {
                let sub_type = json
                    .pointer("/metadata/subscription_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();

                if sub_type == "channel.follow" {
                    if let Some(user_name) = json
                        .pointer("/payload/event/user_name")
                        .or_else(|| json.pointer("/payload/event/user_login"))
                        .and_then(|v| v.as_str())
                    {
                        self.emit(EventSubMsg::Alert(AlertEvent {
                            id: msg_id,
                            kind: AlertKind::Follow {
                                user: user_name.to_string(),
                            },
                        }));
                    }
                }
            }
            "revocation" => {
                let sub_type = json
                    .pointer("/payload/subscription/type")
                    .or_else(|| json.pointer("/metadata/subscription_type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let status = json
                    .pointer("/payload/subscription/status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("revoked");
                let reason = format!("{sub_type} ({status})");
                tracing::warn!("Twitch EventSub subscription revoked: {reason} ({raw_text})");
                self.emit(EventSubMsg::Revoked(reason));
            }
            _ => {}
        }
    }

    /// Connects to `reconnect_url` and waits for its `session_welcome` while keeping
    /// `old_socket` open, draining any in-flight notifications from `old_socket`
    /// with message-ID deduplication before closing `old_socket`.
    fn handoff_reconnect(
        &mut self,
        old_socket: &mut WebSocket<WsStream>,
        reconnect_url: &str,
    ) -> Result<(WebSocket<WsStream>, Duration), String> {
        let mut new_socket = connect_ws(reconnect_url)?;
        let handoff_start = Instant::now();
        let welcome_timeout = Duration::from_secs(10);

        let new_keepalive_deadline = loop {
            if self.stopped() {
                let _ = new_socket.close(None);
                return Err("EventSub stopped during reconnect handoff".to_string());
            }
            if handoff_start.elapsed() > welcome_timeout {
                let _ = new_socket.close(None);
                return Err("Timed out waiting for session_welcome on reconnect_url".to_string());
            }

            // Poll any in-flight messages on old_socket while waiting for welcome on new_socket.
            let _ = old_socket
                .get_ref()
                .set_read_timeout(Some(Duration::from_millis(5)));
            if let Ok(old_msg) = old_socket.read() {
                match old_msg {
                    tungstenite::Message::Text(text) => {
                        if let Ok(json) = serde_json::from_str::<Value>(&text) {
                            let msg_type = json
                                .pointer("/metadata/message_type")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default();
                            if msg_type == "notification" || msg_type == "revocation" {
                                self.handle_notification_or_revocation(msg_type, &json, &text);
                            }
                        }
                    }
                    tungstenite::Message::Ping(data) => {
                        let _ = old_socket.send(tungstenite::Message::Pong(data));
                    }
                    _ => {}
                }
            }

            match new_socket.read() {
                Ok(tungstenite::Message::Text(text)) => {
                    if let Ok(json) = serde_json::from_str::<Value>(&text) {
                        let msg_type = json
                            .pointer("/metadata/message_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        match msg_type {
                            "session_welcome" => {
                                break parse_keepalive_deadline(&json);
                            }
                            "notification" | "revocation" => {
                                self.handle_notification_or_revocation(msg_type, &json, &text);
                            }
                            _ => {}
                        }
                    }
                }
                Ok(tungstenite::Message::Ping(data)) => {
                    let _ = new_socket.send(tungstenite::Message::Pong(data));
                }
                Ok(tungstenite::Message::Close(_)) => {
                    return Err("Reconnect socket closed before session_welcome".to_string());
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(ref e))
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => return Err(format!("Reconnect socket read error: {e}")),
            }
        };

        // Drain any remaining buffered notifications on old_socket before closing it.
        let _ = old_socket
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(10)));
        while let Ok(old_msg) = old_socket.read() {
            match old_msg {
                tungstenite::Message::Text(text) => {
                    if let Ok(json) = serde_json::from_str::<Value>(&text) {
                        let msg_type = json
                            .pointer("/metadata/message_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        if msg_type == "notification" || msg_type == "revocation" {
                            self.handle_notification_or_revocation(msg_type, &json, &text);
                        }
                    }
                }
                tungstenite::Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = old_socket.close(None);
        Ok((new_socket, new_keepalive_deadline))
    }

    fn session(&mut self, url_str: &str, is_reconnect: bool) -> Result<Option<String>, String> {
        let mut socket = connect_ws(url_str)?;
        let mut keepalive_deadline = Duration::from_secs_f64(DEFAULT_KEEPALIVE_TIMEOUT_SECS + 2.0);
        let mut last_message_at = Instant::now();

        while !self.stopped() {
            if last_message_at.elapsed() > keepalive_deadline {
                let _ = socket.close(None);
                return Err(format!(
                    "EventSub keepalive timeout exceeded ({:?})",
                    keepalive_deadline
                ));
            }

            let msg = match socket.read() {
                Ok(m) => m,
                Err(tungstenite::Error::Io(ref e))
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => return Err(e.to_string()),
            };

            last_message_at = Instant::now();

            match msg {
                tungstenite::Message::Text(text) => {
                    if let Ok(json) = serde_json::from_str::<Value>(&text) {
                        let msg_type = json
                            .pointer("/metadata/message_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();

                        match msg_type {
                            "session_welcome" => {
                                keepalive_deadline = parse_keepalive_deadline(&json);
                                if !is_reconnect {
                                    if let Some(id) =
                                        json.pointer("/payload/session/id").and_then(|v| v.as_str())
                                    {
                                        self.subscribe_followers(id)?;
                                    }
                                }
                            }
                            "session_keepalive" => {
                                // keepalive received; `last_message_at` was updated above
                            }
                            "session_reconnect" => {
                                if let Some(reconnect_url) = json
                                    .pointer("/payload/session/reconnect_url")
                                    .and_then(|v| v.as_str())
                                {
                                    let (new_socket, new_deadline) =
                                        self.handoff_reconnect(&mut socket, reconnect_url)?;
                                    socket = new_socket;
                                    keepalive_deadline = new_deadline;
                                    last_message_at = Instant::now();
                                }
                            }
                            "notification" | "revocation" => {
                                self.handle_notification_or_revocation(msg_type, &json, &text);
                            }
                            _ => {}
                        }
                    }
                }
                tungstenite::Message::Ping(data) => {
                    let _ = socket.send(tungstenite::Message::Pong(data));
                }
                tungstenite::Message::Close(_) => {
                    return Err("Connection closed by server".to_string());
                }
                _ => {}
            }
        }

        let _ = socket.close(None);
        Ok(None)
    }

    fn subscribe_followers(&mut self, session_id: &str) -> Result<(), String> {
        if self.token.user_id.is_empty() {
            tracing::warn!("Twitch EventSub: cannot subscribe without user_id");
            return Err("Missing user_id on token".to_string());
        }

        let body = serde_json::json!({
            "type": "channel.follow",
            "version": "2",
            "condition": {
                "broadcaster_user_id": self.token.user_id,
                "moderator_user_id": self.token.user_id
            },
            "transport": {
                "method": "websocket",
                "session_id": session_id
            }
        });

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .timeout(Duration::from_secs(15))
            .build();

        let body_str = body.to_string();
        let sub_url = self.subscription_url.clone();
        let post_sub = |tok: &StoredToken| {
            agent
                .post(&sub_url)
                .set("Client-Id", tok.effective_client_id())
                .set("Authorization", &format!("Bearer {}", tok.access_token))
                .set("Content-Type", "application/json")
                .send_string(&body_str)
        };

        let resp = match post_sub(&self.token) {
            Err(ureq::Error::Status(401, _)) => {
                if let Ok(refreshed) = super::auth::force_refresh(&self.token) {
                    self.token = refreshed.clone();
                    self.emit(EventSubMsg::TokenRefreshed(refreshed));
                    post_sub(&self.token)
                } else {
                    return Err("EventSub subscribe HTTP 401: token expired".to_string());
                }
            }
            other => other,
        };

        match resp {
            Ok(_) => {
                tracing::info!("Twitch EventSub: subscribed to channel.follow successfully");
                Ok(())
            }
            Err(ureq::Error::Status(409, _)) => {
                tracing::info!("Twitch EventSub: channel.follow subscription already exists");
                Ok(())
            }
            Err(ureq::Error::Status(code, resp)) => {
                let err_text = resp.into_string().unwrap_or_default();
                if code == 403 {
                    tracing::warn!(
                        "Twitch EventSub: follower alerts require 'moderator:read:followers' scope (HTTP 403: {err_text}). Log out and log in again to grant permissions."
                    );
                } else {
                    tracing::warn!("Twitch EventSub subscription failed (HTTP {code}): {err_text}");
                }
                Err(format!("EventSub subscribe HTTP {code}: {err_text}"))
            }
            Err(e) => {
                tracing::warn!("Twitch EventSub subscription request error: {e}");
                Err(format!("EventSub subscribe request error: {e}"))
            }
        }
    }
}

fn connect_ws(url_str: &str) -> Result<WebSocket<WsStream>, String> {
    let url = Url::parse(url_str).map_err(|e| format!("Invalid URL: {e}"))?;
    let use_tls = match url.scheme() {
        "wss" | "https" => true,
        "ws" | "http" => false,
        other => return Err(format!("Unsupported WebSocket scheme: {other}")),
    };
    let host = url.host_str().ok_or_else(|| "No host in URL".to_string())?;
    let default_port = if use_tls { 443 } else { 80 };
    let port = url.port_or_known_default().unwrap_or(default_port);

    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("DNS lookup failed: {e}"))?
        .next()
        .ok_or_else(|| format!("Could not resolve {host}"))?;

    let mut tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
        .map_err(|e| format!("TCP connect failed: {e}"))?;
    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
    let read_timeout = if cfg!(test) {
        Duration::from_millis(25)
    } else {
        Duration::from_millis(200)
    };
    tcp.set_read_timeout(Some(read_timeout))
        .map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;

    let stream = if use_tls {
        let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|e| format!("Invalid server name: {e}"))?;
        let mut conn = rustls::ClientConnection::new(tls_config(), server_name)
            .map_err(|e| format!("TLS client error: {e}"))?;
        while conn.is_handshaking() {
            conn.complete_io(&mut tcp)
                .map_err(|e| format!("TLS handshake error: {e}"))?;
        }
        WsStream::Tls(rustls::StreamOwned::new(conn, tcp))
    } else {
        WsStream::Plain(tcp)
    };

    let (socket, _) =
        tungstenite::client(url, stream).map_err(|e| format!("WebSocket handshake error: {e}"))?;
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;

    fn sample_token() -> StoredToken {
        StoredToken {
            access_token: "test_access".to_string(),
            refresh_token: Some("test_refresh".to_string()),
            login: "streamer".to_string(),
            user_id: "12345".to_string(),
            client_id: "test_client".to_string(),
        }
    }

    fn spawn_mock_subscription_http_server() -> (String, Arc<AtomicUsize>, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock sub server");
        listener
            .set_nonblocking(true)
            .expect("set_nonblocking sub server");
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let hits_clone = hits.clone();
        let stop_clone = stop.clone();

        std::thread::spawn(move || {
            while !stop_clone.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        hits_clone.fetch_add(1, Ordering::SeqCst);
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                        let mut buf = [0u8; 2048];
                        let _ = stream.read(&mut buf);
                        let resp =
                            b"HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                        let _ = stream.write_all(resp);
                        let _ = stream.flush();
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        (format!("http://{addr}/eventsub/subscriptions"), hits, stop)
    }

    fn welcome_frame(session_id: &str, keepalive_secs: f64) -> String {
        serde_json::json!({
            "metadata": {
                "message_id": format!("welcome-{session_id}"),
                "message_type": "session_welcome",
                "message_timestamp": "2026-04-20T00:00:00Z"
            },
            "payload": {
                "session": {
                    "id": session_id,
                    "status": "connected",
                    "keepalive_timeout_seconds": keepalive_secs,
                    "reconnect_url": Value::Null,
                    "connected_at": "2026-04-20T00:00:00Z"
                }
            }
        })
        .to_string()
    }

    fn keepalive_frame() -> String {
        serde_json::json!({
            "metadata": {
                "message_id": "ka-1",
                "message_type": "session_keepalive",
                "message_timestamp": "2026-04-20T00:00:01Z"
            },
            "payload": {}
        })
        .to_string()
    }

    fn follow_notification_frame(msg_id: &str, follower: &str) -> String {
        serde_json::json!({
            "metadata": {
                "message_id": msg_id,
                "message_type": "notification",
                "message_timestamp": "2026-04-20T00:00:02Z",
                "subscription_type": "channel.follow",
                "subscription_version": "2"
            },
            "payload": {
                "subscription": {
                    "id": "sub-1",
                    "type": "channel.follow"
                },
                "event": {
                    "user_id": "999",
                    "user_login": follower.to_ascii_lowercase(),
                    "user_name": follower
                }
            }
        })
        .to_string()
    }

    #[test]
    fn silence_after_welcome_triggers_keepalive_timeout_and_reconnects_f22() {
        let (sub_url, sub_hits, sub_stop) = spawn_mock_subscription_http_server();
        let ws_listener = TcpListener::bind("127.0.0.1:0").expect("bind ws server");
        let ws_addr = ws_listener.local_addr().unwrap();
        let ws_url = format!("ws://{ws_addr}/ws");

        let server_thread = std::thread::spawn(move || {
            // 1st connection: send welcome with a short 0.15s keepalive timeout, then stay silent.
            let (tcp1, _) = ws_listener.accept().expect("accept 1st ws connection");
            tcp1.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut ws1 = tungstenite::accept(tcp1).expect("handshake 1st ws");
            ws1.send(tungstenite::Message::Text(welcome_frame("sess-1", 0.15)))
                .expect("send welcome 1");

            // Do not send any keepalive; wait for client to time out and open a 2nd connection.
            let (tcp2, _) = ws_listener.accept().expect("accept 2nd ws connection");
            tcp2.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut ws2 = tungstenite::accept(tcp2).expect("handshake 2nd ws");
            ws2.send(tungstenite::Message::Text(welcome_frame("sess-2", 5.0)))
                .expect("send welcome 2");
            ws2.send(tungstenite::Message::Text(follow_notification_frame(
                "msg-after-reconnect",
                "ReconnectedFollower",
            )))
            .expect("send follow on 2nd connection");
            // Keep ws2 alive briefly until client reads notification.
            std::thread::sleep(Duration::from_millis(100));
            drop(ws1);
            drop(ws2);
        });

        let handle = spawn_with_config(
            sample_token(),
            None,
            ws_url,
            sub_url,
            Duration::from_millis(30),
        );

        let event = handle
            .events
            .recv_timeout(Duration::from_secs(3))
            .expect("receive follow alert after keepalive timeout reconnect");
        assert_eq!(
            event,
            EventSubMsg::Alert(AlertEvent {
                id: "msg-after-reconnect".to_string(),
                kind: AlertKind::Follow {
                    user: "ReconnectedFollower".to_string(),
                },
            })
        );
        assert!(
            sub_hits.load(Ordering::SeqCst) >= 2,
            "both initial and reconnected sessions should subscribe to followers"
        );

        drop(handle);
        sub_stop.store(true, Ordering::Release);
        server_thread
            .join()
            .expect("ws server thread cleanly finishes");
    }

    #[test]
    fn regular_keepalive_frames_preserve_session_past_single_timeout_f22() {
        let (sub_url, sub_hits, sub_stop) = spawn_mock_subscription_http_server();
        let ws_listener = TcpListener::bind("127.0.0.1:0").expect("bind ws server");
        let ws_addr = ws_listener.local_addr().unwrap();
        let ws_url = format!("ws://{ws_addr}/ws");

        let server_thread = std::thread::spawn(move || {
            let (tcp1, _) = ws_listener.accept().expect("accept ws connection");
            tcp1.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut ws1 = tungstenite::accept(tcp1).expect("handshake ws");
            // Keepalive timeout is 0.20s (deadline ~0.25s).
            ws1.send(tungstenite::Message::Text(welcome_frame("sess-ka", 0.20)))
                .expect("send welcome");

            // Send 3 keepalives spaced by 100ms (total 300ms > 250ms single deadline)
            for _ in 0..3 {
                std::thread::sleep(Duration::from_millis(100));
                ws1.send(tungstenite::Message::Text(keepalive_frame()))
                    .expect("send keepalive");
            }
            ws1.send(tungstenite::Message::Text(follow_notification_frame(
                "msg-after-keepalives",
                "LoyalFollower",
            )))
            .expect("send follow");
            std::thread::sleep(Duration::from_millis(100));
        });

        let handle = spawn_with_config(
            sample_token(),
            None,
            ws_url,
            sub_url,
            Duration::from_millis(30),
        );

        let event = handle
            .events
            .recv_timeout(Duration::from_secs(3))
            .expect("receive follow alert on same session kept alive by heartbeats");
        assert_eq!(
            event,
            EventSubMsg::Alert(AlertEvent {
                id: "msg-after-keepalives".to_string(),
                kind: AlertKind::Follow {
                    user: "LoyalFollower".to_string(),
                },
            })
        );
        assert_eq!(
            sub_hits.load(Ordering::SeqCst),
            1,
            "session should not have reconnected or re-subscribed when keepalives arrived on time"
        );

        drop(handle);
        sub_stop.store(true, Ordering::Release);
        server_thread
            .join()
            .expect("ws server thread cleanly finishes");
    }

    #[test]
    fn session_reconnect_keeps_old_socket_open_until_new_welcome_and_deduplicates_f22() {
        let (sub_url, sub_hits, sub_stop) = spawn_mock_subscription_http_server();
        let ws1_listener = TcpListener::bind("127.0.0.1:0").expect("bind ws1");
        let ws2_listener = TcpListener::bind("127.0.0.1:0").expect("bind ws2");
        let ws1_addr = ws1_listener.local_addr().unwrap();
        let ws2_addr = ws2_listener.local_addr().unwrap();
        let ws1_url = format!("ws://{ws1_addr}/ws");
        let ws2_url = format!("ws://{ws2_addr}/reconnect");

        let old_still_open_at_new_welcome = Arc::new(AtomicBool::new(false));
        let old_still_open_clone = old_still_open_at_new_welcome.clone();

        let server_thread = std::thread::spawn(move || {
            let (tcp1, _) = ws1_listener.accept().expect("accept ws1");
            tcp1.set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            let mut ws1 = tungstenite::accept(tcp1).expect("handshake ws1");
            ws1.send(tungstenite::Message::Text(welcome_frame("sess-old", 5.0)))
                .expect("send welcome on ws1");

            // Send a notification on ws1 before reconnect, then send session_reconnect
            ws1.send(tungstenite::Message::Text(follow_notification_frame(
                "dup-msg-1",
                "HandoffUser",
            )))
            .expect("send initial notification on ws1");

            let reconnect_frame = serde_json::json!({
                "metadata": {
                    "message_id": "reconn-1",
                    "message_type": "session_reconnect",
                    "message_timestamp": "2026-04-20T00:00:03Z"
                },
                "payload": {
                    "session": {
                        "id": "sess-old",
                        "status": "reconnecting",
                        "keepalive_timeout_seconds": Value::Null,
                        "reconnect_url": ws2_url,
                        "connected_at": "2026-04-20T00:00:00Z"
                    }
                }
            })
            .to_string();
            ws1.send(tungstenite::Message::Text(reconnect_frame))
                .expect("send session_reconnect on ws1");

            // Accept the new connection on ws2 while ws1 is still open
            let (tcp2, _) = ws2_listener.accept().expect("accept ws2");
            tcp2.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut ws2 = tungstenite::accept(tcp2).expect("handshake ws2");

            // Verify ws1 has NOT received a Close frame or EOF yet before ws2 sends session_welcome!
            let old_open_before_welcome = match ws1.read() {
                Ok(tungstenite::Message::Close(_)) => false,
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    false
                }
                Err(tungstenite::Error::Io(ref e))
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    true
                }
                Ok(_) => true,
                Err(_) => false,
            };
            old_still_open_clone.store(old_open_before_welcome, Ordering::SeqCst);

            // Now send session_welcome on ws2
            ws2.send(tungstenite::Message::Text(welcome_frame("sess-new", 5.0)))
                .expect("send welcome on ws2");

            // Replay duplicate message "dup-msg-1" on ws2 plus a brand-new message "new-msg-2"
            ws2.send(tungstenite::Message::Text(follow_notification_frame(
                "dup-msg-1",
                "HandoffUser",
            )))
            .expect("send duplicate notification on ws2");
            ws2.send(tungstenite::Message::Text(follow_notification_frame(
                "new-msg-2",
                "SecondUser",
            )))
            .expect("send new notification on ws2");

            std::thread::sleep(Duration::from_millis(100));
        });

        let handle = spawn_with_config(
            sample_token(),
            None,
            ws1_url,
            sub_url,
            Duration::from_millis(30),
        );

        let ev1 = handle
            .events
            .recv_timeout(Duration::from_secs(3))
            .expect("receive first alert");
        let ev2 = handle
            .events
            .recv_timeout(Duration::from_secs(3))
            .expect("receive second alert");

        assert_eq!(
            ev1,
            EventSubMsg::Alert(AlertEvent {
                id: "dup-msg-1".to_string(),
                kind: AlertKind::Follow {
                    user: "HandoffUser".to_string(),
                },
            })
        );
        assert_eq!(
            ev2,
            EventSubMsg::Alert(AlertEvent {
                id: "new-msg-2".to_string(),
                kind: AlertKind::Follow {
                    user: "SecondUser".to_string(),
                },
            })
        );
        assert!(
            old_still_open_at_new_welcome.load(Ordering::SeqCst),
            "old WebSocket connection must remain open until the new connection receives session_welcome"
        );
        assert_eq!(
            sub_hits.load(Ordering::SeqCst),
            1,
            "reconnect handoff must not re-POST subscriptions"
        );

        drop(handle);
        sub_stop.store(true, Ordering::Release);
        server_thread.join().expect("server thread finishes");
    }

    #[test]
    fn revocation_message_emits_revoked_event_f22() {
        let (sub_url, _sub_hits, sub_stop) = spawn_mock_subscription_http_server();
        let ws_listener = TcpListener::bind("127.0.0.1:0").expect("bind ws server");
        let ws_addr = ws_listener.local_addr().unwrap();
        let ws_url = format!("ws://{ws_addr}/ws");

        let server_thread = std::thread::spawn(move || {
            let (tcp, _) = ws_listener.accept().expect("accept ws");
            tcp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut ws = tungstenite::accept(tcp).expect("handshake ws");
            ws.send(tungstenite::Message::Text(welcome_frame("sess-rev", 5.0)))
                .expect("send welcome");

            let revocation = serde_json::json!({
                "metadata": {
                    "message_id": "rev-1",
                    "message_type": "revocation",
                    "message_timestamp": "2026-04-20T00:00:05Z",
                    "subscription_type": "channel.follow",
                    "subscription_version": "2"
                },
                "payload": {
                    "subscription": {
                        "id": "sub-1",
                        "status": "authorization_revoked",
                        "type": "channel.follow",
                        "version": "2"
                    }
                }
            })
            .to_string();
            ws.send(tungstenite::Message::Text(revocation))
                .expect("send revocation");
            std::thread::sleep(Duration::from_millis(100));
        });

        let handle = spawn_with_config(
            sample_token(),
            None,
            ws_url,
            sub_url,
            Duration::from_millis(30),
        );

        let ev = handle
            .events
            .recv_timeout(Duration::from_secs(3))
            .expect("receive revoked event");
        assert_eq!(
            ev,
            EventSubMsg::Revoked("channel.follow (authorization_revoked)".to_string())
        );

        drop(handle);
        sub_stop.store(true, Ordering::Release);
        server_thread.join().expect("server thread finishes");
    }
}
