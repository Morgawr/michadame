//! Twitch OAuth: Device Code Grant flow, token validation/refresh and token storage.
//!
//! The token is stored in its own `twitch_token.json` (mode 0600) next to the main
//! config so it never ends up in the rotating config backups.

use crossbeam_channel::Receiver;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Client ID of the registered (public) Michadame Twitch application.
pub const DEFAULT_CLIENT_ID: &str = "mzzs95mdev74cqzysuvfnmv5ux6abj";
pub const SCOPES: &str = "chat:read chat:edit";

const DEVICE_URL: &str = "https://id.twitch.tv/oauth2/device";
const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const VALIDATE_URL: &str = "https://id.twitch.tv/oauth2/validate";
const REVOKE_URL: &str = "https://id.twitch.tv/oauth2/revoke";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredToken {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub login: String,
    #[serde(default)]
    pub user_id: String,
    pub client_id: String,
}

#[derive(Debug)]
pub enum AuthError {
    /// Token/credentials rejected by Twitch.
    Invalid(String),
    /// Could not reach Twitch.
    Network(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::Invalid(m) | AuthError::Network(m) => f.write_str(m),
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .build()
}

fn read_json(resp: ureq::Response) -> serde_json::Value {
    resp.into_string()
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// Returns `(status, body)` for both success and HTTP error statuses.
fn post_form(url: &str, form: &[(&str, &str)]) -> Result<(u16, serde_json::Value), AuthError> {
    match agent().post(url).send_form(form) {
        Ok(resp) => Ok((resp.status(), read_json(resp))),
        Err(ureq::Error::Status(code, resp)) => Ok((code, read_json(resp))),
        Err(e) => Err(AuthError::Network(e.to_string())),
    }
}

fn message_of(body: &serde_json::Value) -> String {
    body.get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown error")
        .to_string()
}

pub struct Validation {
    pub login: String,
    pub user_id: String,
}

pub fn validate(access_token: &str) -> Result<Validation, AuthError> {
    let result = agent()
        .get(VALIDATE_URL)
        .set("Authorization", &format!("OAuth {access_token}"))
        .call();
    match result {
        Ok(resp) => {
            let body = read_json(resp);
            let login = body.get("login").and_then(|v| v.as_str()).unwrap_or_default();
            if login.is_empty() {
                return Err(AuthError::Invalid("token has no user".into()));
            }
            Ok(Validation {
                login: login.to_string(),
                user_id: body
                    .get("user_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        }
        Err(ureq::Error::Status(401, _)) => Err(AuthError::Invalid("token expired or revoked".into())),
        Err(ureq::Error::Status(code, resp)) => Err(AuthError::Network(format!(
            "validate failed ({code}): {}",
            message_of(&read_json(resp))
        ))),
        Err(e) => Err(AuthError::Network(e.to_string())),
    }
}

struct Tokens {
    access_token: String,
    refresh_token: Option<String>,
}

fn parse_tokens(body: &serde_json::Value) -> Option<Tokens> {
    Some(Tokens {
        access_token: body.get("access_token")?.as_str()?.to_string(),
        refresh_token: body
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

fn refresh(client_id: &str, refresh_token: &str) -> Result<Tokens, AuthError> {
    let (status, body) = post_form(
        TOKEN_URL,
        &[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )?;
    if status == 200 {
        parse_tokens(&body).ok_or_else(|| AuthError::Network("malformed token response".into()))
    } else if status == 400 || status == 401 {
        Err(AuthError::Invalid(format!("refresh rejected: {}", message_of(&body))))
    } else {
        Err(AuthError::Network(format!("refresh failed ({status})")))
    }
}

/// Validates the token. Returns `Ok(Some(new))` if it had to be refreshed.
pub fn ensure_valid(token: &StoredToken) -> Result<Option<StoredToken>, AuthError> {
    match validate(&token.access_token) {
        Ok(_) => Ok(None),
        Err(AuthError::Invalid(reason)) => {
            let Some(rt) = token.refresh_token.as_deref() else {
                return Err(AuthError::Invalid(reason));
            };
            let tokens = refresh(&token.client_id, rt)?;
            let v = validate(&tokens.access_token)?;
            Ok(Some(StoredToken {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token.or_else(|| token.refresh_token.clone()),
                login: v.login,
                user_id: v.user_id,
                client_id: token.client_id.clone(),
            }))
        }
        Err(e) => Err(e),
    }
}

/// Best-effort token revocation on logout.
pub fn revoke_in_background(token: StoredToken) {
    std::thread::spawn(move || {
        let _ = post_form(
            REVOKE_URL,
            &[("client_id", &token.client_id), ("token", &token.access_token)],
        );
    });
}

#[derive(Debug)]
pub enum DeviceFlowEvent {
    Code {
        user_code: String,
        verification_uri: String,
        expires_in: Duration,
    },
    Success(StoredToken),
    Failed(String),
}

/// Runs the Device Code Grant flow on a background thread.
pub fn start_device_flow(
    client_id: String,
    cancel: Arc<AtomicBool>,
    ctx: egui::Context,
) -> Receiver<DeviceFlowEvent> {
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("twitch-auth".into())
        .spawn(move || {
            let result = device_flow(&client_id, &cancel, |event| {
                let _ = tx.send(event);
                ctx.request_repaint();
            });
            if let Some(event) = result {
                let _ = tx.send(event);
                ctx.request_repaint();
            }
        })
        .expect("spawn twitch auth thread");
    rx
}

fn device_flow(
    client_id: &str,
    cancel: &AtomicBool,
    mut emit: impl FnMut(DeviceFlowEvent),
) -> Option<DeviceFlowEvent> {
    let (status, body) = match post_form(DEVICE_URL, &[("client_id", client_id), ("scopes", SCOPES)]) {
        Ok(r) => r,
        Err(e) => return Some(DeviceFlowEvent::Failed(e.to_string())),
    };
    if status != 200 {
        return Some(DeviceFlowEvent::Failed(format!(
            "Twitch refused the login request ({status}): {}",
            message_of(&body)
        )));
    }
    let field = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let (Some(device_code), Some(user_code), Some(verification_uri)) =
        (field("device_code"), field("user_code"), field("verification_uri"))
    else {
        return Some(DeviceFlowEvent::Failed("malformed device code response".into()));
    };
    let expires_in = Duration::from_secs(body.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(1800));
    let mut interval = Duration::from_secs(body.get("interval").and_then(|v| v.as_u64()).unwrap_or(5).max(1));
    emit(DeviceFlowEvent::Code {
        user_code,
        verification_uri,
        expires_in,
    });

    let deadline = Instant::now() + expires_in;
    loop {
        let wake = Instant::now() + interval;
        while Instant::now() < wake {
            if cancel.load(Ordering::Acquire) {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if Instant::now() > deadline {
            return Some(DeviceFlowEvent::Failed("the login code expired".into()));
        }
        let (status, body) = match post_form(
            TOKEN_URL,
            &[
                ("client_id", client_id),
                ("scopes", SCOPES),
                ("device_code", &device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ],
        ) {
            Ok(r) => r,
            Err(_) => continue, // transient network error: keep polling
        };
        if status == 200 {
            let Some(tokens) = parse_tokens(&body) else {
                return Some(DeviceFlowEvent::Failed("malformed token response".into()));
            };
            return Some(match validate(&tokens.access_token) {
                Ok(v) => DeviceFlowEvent::Success(StoredToken {
                    access_token: tokens.access_token,
                    refresh_token: tokens.refresh_token,
                    login: v.login,
                    user_id: v.user_id,
                    client_id: client_id.to_string(),
                }),
                Err(e) => DeviceFlowEvent::Failed(e.to_string()),
            });
        }
        match message_of(&body).as_str() {
            "authorization_pending" => {}
            "slow_down" => interval += Duration::from_secs(5),
            other => return Some(DeviceFlowEvent::Failed(format!("login failed: {other}"))),
        }
    }
}

fn token_path() -> Option<PathBuf> {
    confy::get_configuration_file_path("michadame", None)
        .ok()
        .map(|p| p.with_file_name("twitch_token.json"))
}

pub fn load_token() -> Option<StoredToken> {
    let data = std::fs::read_to_string(token_path()?).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn save_token(token: &StoredToken) -> std::io::Result<()> {
    let path = token_path().ok_or_else(|| std::io::Error::other("no config directory"))?;
    save_token_at(&path, token)
}

fn save_token_at(path: &std::path::Path, token: &StoredToken) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp)?;
        file.write_all(serde_json::to_string_pretty(token)?.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

pub fn delete_token() {
    if let Some(path) = token_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_file_round_trip_is_private() {
        let path = std::env::temp_dir().join(format!(
            "michadame-twitch-token-{}-{}.json",
            std::process::id(),
            rand::random::<u32>()
        ));
        let token = StoredToken {
            access_token: "abc".into(),
            refresh_token: Some("def".into()),
            login: "someone".into(),
            user_id: "42".into(),
            client_id: DEFAULT_CLIENT_ID.into(),
        };
        save_token_at(&path, &token).unwrap();
        let loaded: StoredToken =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded, token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_file(&path).unwrap();
    }
}
