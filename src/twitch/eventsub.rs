//! Twitch EventSub WebSocket client for real-time events (followers).

use super::alerts::{AlertEvent, AlertKind};
use super::auth::StoredToken;
use super::irc::tls_config;
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use serde_json::Value;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tungstenite::WebSocket;
use url::Url;

const EVENTSUB_WS_URL: &str = "wss://eventsub.wss.twitch.tv/ws";
const SUBSCRIPTION_URL: &str = "https://api.twitch.tv/helix/eventsub/subscriptions";

type TlsStream = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

pub struct EventSubHandle {
    pub access_token: String,
    pub events: Receiver<AlertEvent>,
    stop: Arc<AtomicBool>,
}

impl Drop for EventSubHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub fn spawn(token: StoredToken, ctx: Option<egui::Context>) -> EventSubHandle {
    let (tx, rx) = crossbeam_channel::unbounded();
    let stop = Arc::new(AtomicBool::new(false));
    let access_token = token.access_token.clone();

    let worker = Worker {
        token,
        stop: stop.clone(),
        event_tx: tx,
        ctx,
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
    event_tx: Sender<AlertEvent>,
    ctx: Option<egui::Context>,
}

impl Worker {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn emit(&self, event: AlertEvent) {
        let _ = self.event_tx.send(event);
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    fn run(self) {
        let mut backoff = 2u64;
        let mut current_url = EVENTSUB_WS_URL.to_string();

        while !self.stopped() {
            let res = self.session(&current_url);
            match res {
                Ok(new_url) => {
                    backoff = 2;
                    current_url = new_url.unwrap_or_else(|| EVENTSUB_WS_URL.to_string());
                }
                Err(err) => {
                    tracing::warn!("Twitch EventSub disconnected ({err}), reconnecting in {backoff}s");
                    current_url = EVENTSUB_WS_URL.to_string();
                    let until = Instant::now() + Duration::from_secs(backoff);
                    while Instant::now() < until && !self.stopped() {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    backoff = (backoff * 2).min(60);
                }
            }
        }
    }

    fn session(&self, url_str: &str) -> Result<Option<String>, String> {
        let mut socket = connect_ws_tls(url_str)?;

        while !self.stopped() {
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

            match msg {
                tungstenite::Message::Text(text) => {
                    if let Ok(json) = serde_json::from_str::<Value>(&text) {
                        let msg_type = json
                            .pointer("/metadata/message_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();

                        match msg_type {
                            "session_welcome" => {
                                if let Some(id) = json.pointer("/payload/session/id").and_then(|v| v.as_str()) {
                                    self.subscribe_followers(id);
                                }
                            }
                            "session_keepalive" => {
                                // keepalive received
                            }
                            "session_reconnect" => {
                                if let Some(reconnect_url) =
                                    json.pointer("/payload/session/reconnect_url").and_then(|v| v.as_str())
                                {
                                    return Ok(Some(reconnect_url.to_string()));
                                }
                            }
                            "notification" => {
                                let sub_type = json
                                    .pointer("/metadata/subscription_type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default();
                                let msg_id = json
                                    .pointer("/metadata/message_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string();

                                if sub_type == "channel.follow" {
                                    if let Some(user_name) = json
                                        .pointer("/payload/event/user_name")
                                        .or_else(|| json.pointer("/payload/event/user_login"))
                                        .and_then(|v| v.as_str())
                                    {
                                        self.emit(AlertEvent {
                                            id: msg_id,
                                            kind: AlertKind::Follow {
                                                user: user_name.to_string(),
                                            },
                                            timestamp: Instant::now(),
                                        });
                                    }
                                }
                            }
                            "revocation" => {
                                tracing::warn!("Twitch EventSub subscription revoked: {text}");
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

    fn subscribe_followers(&self, session_id: &str) {
        if self.token.user_id.is_empty() {
            tracing::warn!("Twitch EventSub: cannot subscribe without user_id");
            return;
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

        let client_id = if self.token.client_id.is_empty() {
            super::auth::DEFAULT_CLIENT_ID
        } else {
            &self.token.client_id
        };

        let body_str = body.to_string();
        let resp = ureq::post(SUBSCRIPTION_URL)
            .set("Client-Id", client_id)
            .set("Authorization", &format!("Bearer {}", self.token.access_token))
            .set("Content-Type", "application/json")
            .send_string(&body_str);

        match resp {
            Ok(_) => {
                tracing::info!("Twitch EventSub: subscribed to channel.follow successfully");
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
            }
            Err(e) => {
                tracing::warn!("Twitch EventSub subscription request error: {e}");
            }
        }
    }
}

fn connect_ws_tls(url_str: &str) -> Result<WebSocket<TlsStream>, String> {
    let url = Url::parse(url_str).map_err(|e| format!("Invalid URL: {e}"))?;
    let host = url.host_str().ok_or_else(|| "No host in URL".to_string())?;
    let port = url.port_or_known_default().unwrap_or(443);

    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("DNS lookup failed: {e}"))?
        .next()
        .ok_or_else(|| format!("Could not resolve {host}"))?;

    let mut tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
        .map_err(|e| format!("TCP connect failed: {e}"))?;
    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
    tcp.set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;

    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| format!("Invalid server name: {e}"))?;
    let mut conn = rustls::ClientConnection::new(tls_config(), server_name)
        .map_err(|e| format!("TLS client error: {e}"))?;
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp)
            .map_err(|e| format!("TLS handshake error: {e}"))?;
    }
    let tls_stream = rustls::StreamOwned::new(conn, tcp);

    let (socket, _) = tungstenite::client(url, tls_stream)
        .map_err(|e| format!("WebSocket handshake error: {e}"))?;
    Ok(socket)
}
