//! Minimal Twitch IRC client: tag-aware line parser and a TLS connection worker.

use super::auth::{self, StoredToken};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

const HOST: &str = "irc.chat.twitch.tv";
const PORT: u16 = 6697;
/// Twitch sends a PING roughly every 5 minutes.
const IDLE_PING_AFTER: Duration = Duration::from_secs(300);
const IDLE_TIMEOUT: Duration = Duration::from_secs(360);
/// Twitch requires apps to re-validate tokens hourly.
const REVALIDATE_EVERY: Duration = Duration::from_secs(3600);

/// A parsed IRCv3 message.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IrcMessage {
    pub tags: HashMap<String, String>,
    pub prefix: Option<String>,
    pub command: String,
    pub params: Vec<String>,
}

impl IrcMessage {
    pub fn parse(line: &str) -> Option<Self> {
        let mut rest = line.trim_end_matches(['\r', '\n']);
        let mut tags = HashMap::new();
        if let Some(r) = rest.strip_prefix('@') {
            let (raw_tags, r) = r.split_once(' ')?;
            for kv in raw_tags.split(';') {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                tags.insert(k.to_string(), unescape_tag(v));
            }
            rest = r.trim_start_matches(' ');
        }
        let mut prefix = None;
        if let Some(r) = rest.strip_prefix(':') {
            let (p, r) = r.split_once(' ')?;
            prefix = Some(p.to_string());
            rest = r.trim_start_matches(' ');
        }
        let (command, mut rest) = rest.split_once(' ').unwrap_or((rest, ""));
        if command.is_empty() {
            return None;
        }
        let mut params = Vec::new();
        loop {
            rest = rest.trim_start_matches(' ');
            if rest.is_empty() {
                break;
            }
            if let Some(trailing) = rest.strip_prefix(':') {
                params.push(trailing.to_string());
                break;
            }
            match rest.split_once(' ') {
                Some((p, r)) => {
                    params.push(p.to_string());
                    rest = r;
                }
                None => {
                    params.push(rest.to_string());
                    break;
                }
            }
        }
        Some(Self {
            tags,
            prefix,
            command: command.to_string(),
            params,
        })
    }

    /// Non-empty tag value.
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// Nick portion of the `nick!user@host` prefix.
    pub fn nick(&self) -> Option<&str> {
        self.prefix
            .as_deref()
            .map(|p| p.split('!').next().unwrap_or(p))
    }

    pub fn trailing(&self) -> Option<&str> {
        self.params.last().map(String::as_str)
    }
}

/// Unescape an IRCv3 tag value.
pub fn unescape_tag(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(':') => out.push(';'),
            Some('s') => out.push(' '),
            Some('\\') => out.push('\\'),
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionStatus {
    Disabled,
    Connecting,
    Connected { authenticated: bool },
    Reconnecting { secs: u64, reason: String },
}

#[derive(Debug)]
pub enum IrcEvent {
    Status(ConnectionStatus),
    Message(IrcMessage),
    /// The worker refreshed the OAuth token; persist it.
    TokenRefreshed(StoredToken),
    /// The token is no longer usable; the worker continues anonymously.
    TokenInvalid(String),
}

/// Owns a running connection; dropping it stops the worker thread.
pub struct WorkerHandle {
    pub channel: String,
    pub login: Option<String>,
    cmd_tx: Sender<String>,
    pub events: Receiver<IrcEvent>,
    stop: Arc<AtomicBool>,
}

impl WorkerHandle {
    pub fn send_privmsg(&self, text: &str) {
        let _ = self.cmd_tx.send(text.to_string());
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub fn spawn(channel: String, token: Option<StoredToken>, ctx: Option<egui::Context>) -> WorkerHandle {
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    let (ev_tx, ev_rx) = crossbeam_channel::unbounded();
    let stop = Arc::new(AtomicBool::new(false));
    let login = token.as_ref().map(|t| t.login.clone());
    let worker = Worker {
        channel: channel.clone(),
        token,
        stop: stop.clone(),
        cmd_rx,
        ev_tx,
        ctx,
    };
    std::thread::Builder::new()
        .name("twitch-irc".into())
        .spawn(move || worker.run())
        .expect("spawn twitch irc thread");
    WorkerHandle {
        channel,
        login,
        cmd_tx,
        events: ev_rx,
        stop,
    }
}

enum SessionEnd {
    Stopped,
    AuthFailed,
    Disconnected(String),
}

struct Worker {
    channel: String,
    token: Option<StoredToken>,
    stop: Arc<AtomicBool>,
    cmd_rx: Receiver<String>,
    ev_tx: Sender<IrcEvent>,
    ctx: Option<egui::Context>,
}

type TlsStream = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

impl Worker {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn emit(&self, event: IrcEvent) {
        let _ = self.ev_tx.send(event);
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    fn run(mut self) {
        let mut backoff = 1u64;
        let mut auth_failures = 0;
        while !self.stopped() {
            self.check_token();
            self.emit(IrcEvent::Status(ConnectionStatus::Connecting));
            let (end, joined) = self.session();
            let reason = match end {
                SessionEnd::Stopped => break,
                SessionEnd::AuthFailed => {
                    auth_failures += 1;
                    if auth_failures >= 2 {
                        self.token = None;
                        self.emit(IrcEvent::TokenInvalid(
                            "Twitch rejected the chat login".into(),
                        ));
                        auth_failures = 0;
                    }
                    "login rejected".to_string()
                }
                SessionEnd::Disconnected(reason) => reason,
            };
            if joined {
                backoff = 1;
            }
            if self.stopped() {
                break;
            }
            self.emit(IrcEvent::Status(ConnectionStatus::Reconnecting {
                secs: backoff,
                reason,
            }));
            let until = Instant::now() + Duration::from_secs(backoff);
            while Instant::now() < until && !self.stopped() {
                std::thread::sleep(Duration::from_millis(100));
            }
            backoff = (backoff * 2).min(30);
        }
        self.emit(IrcEvent::Status(ConnectionStatus::Disabled));
    }

    /// Validate (and refresh if needed) the token before using it.
    fn check_token(&mut self) {
        let Some(token) = self.token.clone() else {
            return;
        };
        match auth::ensure_valid(&token) {
            Ok(Some(refreshed)) => {
                self.token = Some(refreshed.clone());
                self.emit(IrcEvent::TokenRefreshed(refreshed));
            }
            Ok(None) => {}
            Err(auth::AuthError::Invalid(reason)) => {
                self.token = None;
                self.emit(IrcEvent::TokenInvalid(reason));
            }
            // Network trouble: try connecting anyway.
            Err(auth::AuthError::Network(_)) => {}
        }
    }

    fn session(&mut self) -> (SessionEnd, bool) {
        let mut stream = match connect_tls() {
            Ok(s) => s,
            Err(e) => return (SessionEnd::Disconnected(e.to_string()), false),
        };
        let nick = match &self.token {
            Some(t) => t.login.clone(),
            None => format!("justinfan{}", rand::random::<u32>() % 90000 + 10000),
        };
        let mut hello = String::from("CAP REQ :twitch.tv/tags twitch.tv/commands\r\n");
        if let Some(t) = &self.token {
            hello.push_str(&format!("PASS oauth:{}\r\n", t.access_token));
        }
        hello.push_str(&format!("NICK {nick}\r\nJOIN #{}\r\n", self.channel));
        if let Err(e) = write_raw(&mut stream, &hello) {
            return (SessionEnd::Disconnected(e.to_string()), false);
        }

        let authenticated = self.token.is_some();
        let mut joined = false;
        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut chunk = [0u8; 8192];
        let mut last_rx = Instant::now();
        let mut pinged = false;
        let mut last_validate = Instant::now();

        loop {
            if self.stopped() {
                let _ = write_raw(&mut stream, "QUIT\r\n");
                return (SessionEnd::Stopped, joined);
            }
            while let Ok(text) = self.cmd_rx.try_recv() {
                if !authenticated || !joined {
                    continue;
                }
                let line = format!("PRIVMSG #{} :{}\r\n", self.channel, sanitize_outgoing(&text));
                if let Err(e) = write_raw(&mut stream, &line) {
                    return (SessionEnd::Disconnected(e.to_string()), joined);
                }
            }

            match stream.read(&mut chunk) {
                Ok(0) => return (SessionEnd::Disconnected("connection closed".into()), joined),
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    last_rx = Instant::now();
                    pinged = false;
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return (SessionEnd::Disconnected(e.to_string()), joined),
            }

            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let raw: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&raw);
                let Some(msg) = IrcMessage::parse(&line) else {
                    continue;
                };
                match msg.command.as_str() {
                    "PING" => {
                        let reply = format!("PONG :{}\r\n", msg.trailing().unwrap_or("tmi.twitch.tv"));
                        if let Err(e) = write_raw(&mut stream, &reply) {
                            return (SessionEnd::Disconnected(e.to_string()), joined);
                        }
                    }
                    "PONG" => {}
                    "RECONNECT" => {
                        return (SessionEnd::Disconnected("server requested reconnect".into()), joined)
                    }
                    "NOTICE"
                        if msg.params.first().map(String::as_str) == Some("*")
                            && msg.trailing().map_or(false, |t| {
                                t.contains("Login authentication failed")
                                    || t.contains("Improperly formatted auth")
                            }) =>
                    {
                        return (SessionEnd::AuthFailed, joined);
                    }
                    "JOIN" => {
                        if msg.nick().map_or(false, |n| n.eq_ignore_ascii_case(&nick)) && !joined {
                            joined = true;
                            self.emit(IrcEvent::Status(ConnectionStatus::Connected { authenticated }));
                        }
                    }
                    _ => self.emit(IrcEvent::Message(msg)),
                }
            }

            let idle = last_rx.elapsed();
            if idle > IDLE_TIMEOUT {
                return (SessionEnd::Disconnected("connection timed out".into()), joined);
            }
            if idle > IDLE_PING_AFTER && !pinged {
                pinged = true;
                if let Err(e) = write_raw(&mut stream, "PING :tmi.twitch.tv\r\n") {
                    return (SessionEnd::Disconnected(e.to_string()), joined);
                }
            }

            if authenticated && last_validate.elapsed() > REVALIDATE_EVERY {
                last_validate = Instant::now();
                self.check_token();
                if self.token.is_none() {
                    // Token revoked: reconnect anonymously.
                    return (SessionEnd::Disconnected("login expired".into()), joined);
                }
            }
        }
    }
}

/// Strip control characters that would break the IRC line.
fn sanitize_outgoing(text: &str) -> String {
    let action = text.strip_prefix("/me ");
    let body: String = action
        .unwrap_or(text)
        .chars()
        .filter(|c| *c != '\r' && *c != '\n' && *c != '\u{1}')
        .take(500)
        .collect();
    match action {
        Some(_) => format!("\u{1}ACTION {body}\u{1}"),
        None => body,
    }
}

fn write_raw(stream: &mut TlsStream, data: &str) -> std::io::Result<()> {
    stream.write_all(data.as_bytes())?;
    stream.flush()
}

fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("ring provider supports default TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

fn connect_tls() -> std::io::Result<TlsStream> {
    let addr = (HOST, PORT)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other("could not resolve irc.chat.twitch.tv"))?;
    let mut tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
    tcp.set_nodelay(true)?;
    tcp.set_read_timeout(Some(Duration::from_secs(10)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(10)))?;
    let server_name = rustls::pki_types::ServerName::try_from(HOST).map_err(std::io::Error::other)?;
    let mut conn =
        rustls::ClientConnection::new(tls_config(), server_name).map_err(std::io::Error::other)?;
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp)?;
    }
    // Short read timeout so the loop can service outgoing messages and stop requests.
    tcp.set_read_timeout(Some(Duration::from_millis(200)))?;
    Ok(rustls::StreamOwned::new(conn, tcp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tagged_privmsg() {
        let line = "@badge-info=;color=#FF4500;display-name=Some\\sUser;emotes=25:0-4;id=abc-123 :someuser!someuser@someuser.tmi.twitch.tv PRIVMSG #chan :Kappa hello there\r\n";
        let msg = IrcMessage::parse(line).unwrap();
        assert_eq!(msg.command, "PRIVMSG");
        assert_eq!(msg.nick(), Some("someuser"));
        assert_eq!(msg.params, vec!["#chan", "Kappa hello there"]);
        assert_eq!(msg.tag("display-name"), Some("Some User"));
        assert_eq!(msg.tag("color"), Some("#FF4500"));
        assert_eq!(msg.tag("emotes"), Some("25:0-4"));
        assert_eq!(msg.tag("badge-info"), None);
    }

    #[test]
    fn parses_ping_and_untagged() {
        let msg = IrcMessage::parse("PING :tmi.twitch.tv").unwrap();
        assert_eq!(msg.command, "PING");
        assert_eq!(msg.trailing(), Some("tmi.twitch.tv"));
        assert!(msg.prefix.is_none());

        let msg = IrcMessage::parse(":tmi.twitch.tv CLEARCHAT #chan").unwrap();
        assert_eq!(msg.command, "CLEARCHAT");
        assert_eq!(msg.params, vec!["#chan"]);
        assert!(IrcMessage::parse("").is_none());
    }

    #[test]
    fn unescapes_tag_values() {
        assert_eq!(unescape_tag(r"a\sb\:c\\d\n"), "a b;c\\d\n");
        assert_eq!(unescape_tag(r"trailing\"), "trailing");
    }

    #[test]
    fn sanitizes_outgoing_lines() {
        assert_eq!(sanitize_outgoing("hi\r\nPRIVMSG #x :evil"), "hiPRIVMSG #x :evil");
        assert_eq!(sanitize_outgoing("/me waves"), "\u{1}ACTION waves\u{1}");
    }

    /// Live network check: `cargo test -- --ignored live_anonymous_join`.
    #[test]
    #[ignore]
    fn live_anonymous_join() {
        let handle = spawn("twitch".into(), None, None);
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(IrcEvent::Status(ConnectionStatus::Connected { authenticated })) =
                handle.events.recv_timeout(Duration::from_millis(500))
            {
                assert!(!authenticated);
                return;
            }
        }
        panic!("did not join within 20s");
    }
}
