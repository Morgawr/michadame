//! Twitch OAuth: Device Code Grant flow, token validation/refresh and token storage.
//!
//! The token is stored in its own `twitch_token.json` (mode 0600) next to the main
//! config so it never ends up in the rotating config backups.

use crossbeam_channel::Receiver;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Client ID of the registered (public) Michadame Twitch application.
pub const DEFAULT_CLIENT_ID: &str = "mzzs95mdev74cqzysuvfnmv5ux6abj";
pub const SCOPES: &str = "chat:read chat:edit channel:read:stream_key channel:manage:broadcast moderator:read:followers channel:read:subscriptions bits:read clips:edit";

const DEVICE_URL: &str = "https://id.twitch.tv/oauth2/device";
const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const VALIDATE_URL: &str = "https://id.twitch.tv/oauth2/validate";
const REVOKE_URL: &str = "https://id.twitch.tv/oauth2/revoke";
const STREAM_KEY_URL: &str = "https://api.twitch.tv/helix/streams/key";

/// Cache window for a successful `/oauth2/validate` check so concurrent or rapid
/// back-to-back callers (e.g. startup IRC, EventSub, stream info, live poller)
/// do not make redundant validation requests.
const VALIDATE_CACHE_TTL: Duration = Duration::from_secs(300);

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

impl StoredToken {
    pub fn effective_client_id(&self) -> &str {
        let trimmed = self.client_id.trim();
        if trimmed.is_empty() {
            DEFAULT_CLIENT_ID
        } else {
            trimmed
        }
    }
}

#[derive(Default)]
struct SharedTokenState {
    latest: Option<StoredToken>,
    validated_at: Option<Instant>,
    pending_ui_refresh: Option<StoredToken>,
    pending_ui_invalid: Option<String>,
}

static SHARED_TOKEN: OnceLock<Mutex<SharedTokenState>> = OnceLock::new();

fn shared_token() -> &'static Mutex<SharedTokenState> {
    SHARED_TOKEN.get_or_init(|| Mutex::new(SharedTokenState::default()))
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

fn ensure_valid_with<V, R, P>(
    state: &mut SharedTokenState,
    caller_token: &StoredToken,
    force_check: bool,
    force_refresh_now: bool,
    validate_fn: V,
    refresh_fn: R,
    persist_fn: P,
) -> Result<Option<StoredToken>, AuthError>
where
    V: Fn(&str) -> Result<Validation, AuthError>,
    R: Fn(&str, &str) -> Result<Tokens, AuthError>,
    P: FnOnce(&StoredToken),
{
    let (mut candidate, upgraded) = match &state.latest {
        Some(latest) if latest.access_token != caller_token.access_token => {
            (latest.clone(), true)
        }
        _ => (caller_token.clone(), false),
    };

    let need_refresh = force_refresh_now && !upgraded;
    let cache_fresh = !force_check
        && !need_refresh
        && state.latest.as_ref().map(|t| &t.access_token) == Some(&candidate.access_token)
        && state
            .validated_at
            .is_some_and(|t| t.elapsed() < VALIDATE_CACHE_TTL);

    if cache_fresh {
        return Ok(if upgraded { Some(candidate) } else { None });
    }

    if !need_refresh {
        match validate_fn(&candidate.access_token) {
            Ok(v) => {
                if !v.login.is_empty() {
                    candidate.login = v.login;
                }
                if !v.user_id.is_empty() {
                    candidate.user_id = v.user_id;
                }
                state.latest = Some(candidate.clone());
                state.validated_at = Some(Instant::now());
                state.pending_ui_invalid = None;
                return Ok(if upgraded { Some(candidate) } else { None });
            }
            Err(AuthError::Network(e)) => return Err(AuthError::Network(e)),
            Err(AuthError::Invalid(_)) => {
                // Fall through to refresh below
            }
        }
    }

    let Some(rt) = candidate
        .refresh_token
        .as_deref()
        .filter(|s| !s.is_empty())
    else {
        let msg = "token expired and no refresh token is available".to_string();
        state.latest = None;
        state.validated_at = None;
        state.pending_ui_refresh = None;
        state.pending_ui_invalid = Some(msg.clone());
        return Err(AuthError::Invalid(msg));
    };

    let client_id = candidate.effective_client_id().to_string();
    match refresh_fn(&client_id, rt) {
        Ok(tokens) => {
            let (login, user_id) = match validate_fn(&tokens.access_token) {
                Ok(v) => (v.login, v.user_id),
                Err(AuthError::Network(_)) if !candidate.login.is_empty() => {
                    (candidate.login.clone(), candidate.user_id.clone())
                }
                Err(e) => return Err(e),
            };
            let new_token = StoredToken {
                access_token: tokens.access_token,
                refresh_token: tokens
                    .refresh_token
                    .or_else(|| candidate.refresh_token.clone()),
                login,
                user_id,
                client_id,
            };
            persist_fn(&new_token);
            state.latest = Some(new_token.clone());
            state.validated_at = Some(Instant::now());
            state.pending_ui_refresh = Some(new_token.clone());
            state.pending_ui_invalid = None;
            Ok(Some(new_token))
        }
        Err(AuthError::Invalid(reason)) => {
            state.latest = None;
            state.validated_at = None;
            state.pending_ui_refresh = None;
            state.pending_ui_invalid = Some(reason.clone());
            Err(AuthError::Invalid(reason))
        }
        Err(AuthError::Network(e)) => Err(AuthError::Network(e)),
    }
}

fn persist_token_to_disk(token: &StoredToken) {
    if let Some(path) = token_path() {
        if let Err(e) = save_token_at(&path, token) {
            tracing::error!("Failed to save refreshed Twitch token: {e}");
        }
    }
}

/// Validates the token (using a short-lived shared cache to avoid redundant network
/// requests when multiple workers start together) and refreshes it if expired.
/// Returns `Ok(Some(new))` if it had to be refreshed or if another worker already refreshed it.
pub fn ensure_valid(token: &StoredToken) -> Result<Option<StoredToken>, AuthError> {
    if cfg!(test) {
        return Ok(None);
    }
    let mut state = shared_token()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_valid_with(
        &mut state,
        token,
        false,
        false,
        validate,
        refresh,
        persist_token_to_disk,
    )
}

/// Forces an immediate `/oauth2/validate` check against Twitch (bypassing the 5-minute
/// validation cache) and refreshes the token if expired. Used for periodic hourly checks
/// and after an IRC authentication failure.
pub fn revalidate(token: &StoredToken) -> Result<Option<StoredToken>, AuthError> {
    if cfg!(test) {
        return Ok(None);
    }
    let mut state = shared_token()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_valid_with(
        &mut state,
        token,
        true,
        false,
        validate,
        refresh,
        persist_token_to_disk,
    )
}

/// Immediately refreshes the token after a Helix endpoint returned HTTP 401 (or returns
/// the already-refreshed token if another thread just refreshed it).
pub fn force_refresh(token: &StoredToken) -> Result<StoredToken, AuthError> {
    if cfg!(test) {
        return Ok(token.clone());
    }
    let mut state = shared_token()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let opt = ensure_valid_with(
        &mut state,
        token,
        true,
        true,
        validate,
        refresh,
        persist_token_to_disk,
    )?;
    Ok(opt.unwrap_or_else(|| token.clone()))
}

/// Drains any token that was refreshed on a background thread so `TwitchState` can update its copy.
pub fn take_refreshed_token() -> Option<StoredToken> {
    shared_token()
        .lock()
        .ok()
        .and_then(|mut s| s.pending_ui_refresh.take())
}

/// Drains a permanent token invalidation error if a refresh token was rejected by Twitch.
pub fn take_invalidated_reason() -> Option<String> {
    shared_token()
        .lock()
        .ok()
        .and_then(|mut s| s.pending_ui_invalid.take())
}

/// Returns the latest valid/known token in shared state, if any.
pub fn latest_token() -> Option<StoredToken> {
    shared_token().lock().ok().and_then(|s| s.latest.clone())
}

/// Runs a periodic background revalidation/refresh so tokens stay fresh even if IRC is disabled.
pub fn revalidate_in_background(token: StoredToken, ctx: egui::Context) {
    let _ = std::thread::Builder::new()
        .name("twitch-token-revalidate".into())
        .spawn(move || {
            match revalidate(&token) {
                Ok(Some(_)) | Err(AuthError::Invalid(_)) => {
                    ctx.request_repaint();
                }
                _ => {}
            }
        });
}

/// Best-effort token revocation on logout.
pub fn revoke_in_background(token: StoredToken) {
    std::thread::spawn(move || {
        let _ = post_form(
            REVOKE_URL,
            &[
                ("client_id", token.effective_client_id()),
                ("token", &token.access_token),
            ],
        );
    });
}

fn call_stream_key_api(token: &StoredToken) -> Result<ureq::Response, ureq::Error> {
    agent()
        .get(STREAM_KEY_URL)
        .query("broadcaster_id", &token.user_id)
        .set("Client-Id", token.effective_client_id())
        .set("Authorization", &format!("Bearer {}", token.access_token))
        .call()
}

/// Fetch the broadcaster's live stream key using Helix API.
pub fn fetch_stream_key(token: &StoredToken) -> Result<String, AuthError> {
    let mut active_token = match ensure_valid(token) {
        Ok(Some(refreshed)) => refreshed,
        Ok(None) => token.clone(),
        Err(AuthError::Invalid(e)) => return Err(AuthError::Invalid(e)),
        Err(AuthError::Network(_)) => token.clone(),
    };
    if active_token.user_id.is_empty() {
        return Err(AuthError::Invalid("User ID is missing from login token".into()));
    }
    let result = match call_stream_key_api(&active_token) {
        Err(ureq::Error::Status(401, resp)) => {
            let body = read_json(resp);
            let msg = message_of(&body);
            if msg.contains("Missing scope") || msg.contains("channel:read:stream_key") {
                return Err(AuthError::Invalid(
                    "Missing permission: your login was created before stream key support was added. Please click 'Log out' and 'Log in with Twitch' under 'Twitch Account' to re-authorize.".into(),
                ));
            }
            if let Ok(refreshed) = force_refresh(&active_token) {
                active_token = refreshed;
                call_stream_key_api(&active_token)
            } else {
                return Err(AuthError::Invalid(format!(
                    "Twitch login expired or unauthorized ({msg})"
                )));
            }
        }
        other => other,
    };
    match result {
        Ok(resp) => {
            let body = read_json(resp);
            let key = body
                .get("data")
                .and_then(|d| d.as_array())
                .and_then(|arr| arr.first())
                .and_then(|obj| obj.get("stream_key"))
                .and_then(|k| k.as_str())
                .unwrap_or_default();
            if key.is_empty() {
                return Err(AuthError::Network("Twitch did not return a stream key".into()));
            }
            Ok(key.to_string())
        }
        Err(ureq::Error::Status(401, resp)) => {
            let body = read_json(resp);
            let msg = message_of(&body);
            if msg.contains("Missing scope") || msg.contains("channel:read:stream_key") {
                Err(AuthError::Invalid(
                    "Missing permission: your login was created before stream key support was added. Please click 'Log out' and 'Log in with Twitch' under 'Twitch Account' to re-authorize.".into(),
                ))
            } else {
                Err(AuthError::Invalid(format!("Twitch login expired or unauthorized ({msg})")))
            }
        }
        Err(ureq::Error::Status(code, resp)) => Err(AuthError::Network(format!(
            "Helix API error {code}: {}",
            message_of(&read_json(resp))
        ))),
        Err(e) => Err(AuthError::Network(e.to_string())),
    }
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
    if cfg!(test) {
        return None;
    }
    confy::get_configuration_file_path("michadame", None)
        .ok()
        .map(|p| p.with_file_name("twitch_token.json"))
}

pub fn load_token() -> Option<StoredToken> {
    let data = std::fs::read_to_string(token_path()?).ok()?;
    let token: StoredToken = serde_json::from_str(&data).ok()?;
    if let Ok(mut state) = shared_token().lock() {
        if state.latest.as_ref().map(|t| &t.access_token) != Some(&token.access_token) {
            state.latest = Some(token.clone());
            state.validated_at = None;
            state.pending_ui_invalid = None;
        }
    }
    Some(token)
}

pub fn save_token(token: &StoredToken) -> std::io::Result<()> {
    let path = token_path().ok_or_else(|| std::io::Error::other("no config directory"))?;
    let mut state = shared_token()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    save_token_at(&path, token)?;
    state.latest = Some(token.clone());
    state.validated_at = Some(Instant::now());
    state.pending_ui_invalid = None;
    Ok(())
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
    if let Ok(mut state) = shared_token().lock() {
        state.latest = None;
        state.validated_at = None;
        state.pending_ui_refresh = None;
        state.pending_ui_invalid = None;
    }
    if let Some(path) = token_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

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

    #[test]
    fn concurrent_callers_share_single_token_refresh_without_invalidating_rotated_token() {
        let expired_token = StoredToken {
            access_token: "expired_access".into(),
            refresh_token: Some("single_use_rt_1".into()),
            login: "morgstream".into(),
            user_id: "89884001".into(),
            client_id: DEFAULT_CLIENT_ID.into(),
        };
        let mut state = SharedTokenState {
            latest: Some(expired_token.clone()),
            validated_at: None,
            pending_ui_refresh: None,
            pending_ui_invalid: None,
        };

        let validate_calls = AtomicUsize::new(0);
        let refresh_calls = AtomicUsize::new(0);
        let persisted = Mutex::new(Vec::<StoredToken>::new());

        let validate_fn = |access: &str| -> Result<Validation, AuthError> {
            validate_calls.fetch_add(1, Ordering::SeqCst);
            if access == "fresh_access_2" {
                Ok(Validation {
                    login: "morgstream".into(),
                    user_id: "89884001".into(),
                })
            } else {
                Err(AuthError::Invalid("token expired or revoked".into()))
            }
        };

        // Simulate Twitch's single-use refresh token rotation: only the first call with
        // `single_use_rt_1` succeeds; a second call with `single_use_rt_1` would fail with Invalid!
        let refresh_fn = |_cid: &str, rt: &str| -> Result<Tokens, AuthError> {
            let call_num = refresh_calls.fetch_add(1, Ordering::SeqCst);
            if call_num == 0 && rt == "single_use_rt_1" {
                Ok(Tokens {
                    access_token: "fresh_access_2".into(),
                    refresh_token: Some("single_use_rt_2".into()),
                })
            } else {
                Err(AuthError::Invalid(
                    "refresh rejected: Invalid refresh token".into(),
                ))
            }
        };

        // 1. First worker (e.g. IRC or stream_info) validates & refreshes the expired token
        let res1 = ensure_valid_with(
            &mut state,
            &expired_token,
            false,
            false,
            validate_fn,
            refresh_fn,
            |tok| persisted.lock().unwrap().push(tok.clone()),
        )
        .expect("first caller should refresh token");
        let refreshed = res1.expect("should return Some(new_token)");
        assert_eq!(refreshed.access_token, "fresh_access_2");
        assert_eq!(refreshed.refresh_token.as_deref(), Some("single_use_rt_2"));
        assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);
        assert_eq!(persisted.lock().unwrap().len(), 1);

        // 2. Second worker (e.g. EventSub) still holds a clone of `expired_token` and calls ensure_valid.
        // It must upgrade to `fresh_access_2` from shared state without calling refresh_fn again
        // or making a redundant validate call.
        let calls_before = validate_calls.load(Ordering::SeqCst);
        let res2 = ensure_valid_with(
            &mut state,
            &expired_token,
            false,
            false,
            validate_fn,
            refresh_fn,
            |tok| persisted.lock().unwrap().push(tok.clone()),
        )
        .expect("second caller should receive already-refreshed token");
        assert_eq!(res2, Some(refreshed.clone()));
        assert_eq!(refresh_calls.load(Ordering::SeqCst), 1, "must not reuse consumed refresh token");
        assert_eq!(validate_calls.load(Ordering::SeqCst), calls_before, "cached validation reused");

        // 3. Even if a third worker got HTTP 401 with `expired_token` and calls force_refresh,
        // it sees `state.latest` already moved to `fresh_access_2` and upgrades without refreshing again.
        let res3 = ensure_valid_with(
            &mut state,
            &expired_token,
            true,
            true,
            validate_fn,
            refresh_fn,
            |tok| persisted.lock().unwrap().push(tok.clone()),
        )
        .expect("force_refresh with stale token should return already-upgraded token");
        assert_eq!(res3, Some(refreshed));
        assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);
    }
}
