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

/// Returns `true` if `a` and `b` represent the same Twitch user account.
pub fn same_account(a: &StoredToken, b: &StoredToken) -> bool {
    if !a.user_id.is_empty() && !b.user_id.is_empty() {
        return a.user_id == b.user_id;
    }
    if !a.login.is_empty() && !b.login.is_empty() {
        return a.login.eq_ignore_ascii_case(&b.login);
    }
    a.access_token == b.access_token
}

/// Returns `true` if `token` matches the specified `user_id` / `login` account identity.
pub fn same_account_id(token: &StoredToken, user_id: &str, login: &str) -> bool {
    if !token.user_id.is_empty() && !user_id.is_empty() {
        return token.user_id == user_id;
    }
    if !token.login.is_empty() && !login.is_empty() {
        return token.login.eq_ignore_ascii_case(login);
    }
    token.user_id == user_id && token.login.eq_ignore_ascii_case(login)
}

#[derive(Default)]
struct SharedTokenState {
    auth_generation: u64,
    latest: Option<StoredToken>,
    validated_at: Option<Instant>,
    pending_ui_refresh: Option<(u64, StoredToken)>,
    pending_ui_invalid: Option<(u64, String)>,
    logged_out: bool,
}

impl SharedTokenState {
    fn can_commit(&self, start_gen: u64, expected_account: &StoredToken) -> bool {
        self.auth_generation == start_gen
            && !self.logged_out
            && self
                .latest
                .as_ref()
                .map_or(true, |l| same_account(l, expected_account))
    }
}

type ValidateHook = Arc<dyn Fn(&str) -> Result<Validation, AuthError> + Send + Sync>;
type RefreshHook = Arc<dyn Fn(&str, &str) -> Result<Tokens, AuthError> + Send + Sync>;
type PersistHook = Arc<dyn Fn(&StoredToken) + Send + Sync>;
type StreamKeyHook = Arc<dyn Fn(&StoredToken) -> Result<String, AuthError> + Send + Sync>;

#[derive(Clone, Default)]
struct TokenHooks {
    validate: Option<ValidateHook>,
    refresh: Option<RefreshHook>,
    persist: Option<PersistHook>,
    stream_key: Option<StreamKeyHook>,
    token_path_override: Option<PathBuf>,
}

/// Coordinates Twitch OAuth token state and background refreshes.
///
/// Synchronization architecture:
/// - `state` (`Mutex<SharedTokenState>`) is a fast, strictly in-memory lock held only for
///   nanoseconds to inspect or commit state. It is NEVER held during network HTTP requests
///   or disk persistence, so UI callers (`take_refreshed_token`, `take_invalidated_reason`,
///   `latest_token`, `clear`, `delete_token`, etc.) never block on network I/O.
/// - `refresh_lock` (`Mutex<()>`) serializes background validation and token refresh calls
///   (single-flight) so concurrent background workers reuse a single refresh without
///   re-consuming single-use refresh tokens.
/// - `disk_lock` (`Mutex<()>`) serializes on-disk credential file updates and verifies
///   that `auth_generation` and account identity still match before writing.
#[derive(Default)]
pub struct SharedToken {
    state: Mutex<SharedTokenState>,
    refresh_lock: Mutex<()>,
    disk_lock: Mutex<()>,
    hooks: Mutex<TokenHooks>,
}

static SHARED_TOKEN: OnceLock<Arc<SharedToken>> = OnceLock::new();

pub fn shared_token() -> &'static Arc<SharedToken> {
    SHARED_TOKEN.get_or_init(|| Arc::new(SharedToken::new()))
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
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
            let login = body
                .get("login")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
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
        Err(ureq::Error::Status(401, _)) => {
            Err(AuthError::Invalid("token expired or revoked".into()))
        }
        Err(ureq::Error::Status(code, resp)) => Err(AuthError::Network(format!(
            "validate failed ({code}): {}",
            message_of(&read_json(resp))
        ))),
        Err(e) => Err(AuthError::Network(e.to_string())),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
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
        Err(AuthError::Invalid(format!(
            "refresh rejected: {}",
            message_of(&body)
        )))
    } else {
        Err(AuthError::Network(format!("refresh failed ({status})")))
    }
}

impl SharedToken {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    pub fn set_test_hooks(
        &self,
        validate_hook: Option<ValidateHook>,
        refresh_hook: Option<RefreshHook>,
        persist_hook: Option<PersistHook>,
    ) {
        {
            let mut hooks = self.hooks.lock().unwrap_or_else(|p| p.into_inner());
            hooks.validate = validate_hook;
            hooks.refresh = refresh_hook;
            hooks.persist = persist_hook;
        }
        if let Ok(mut state) = self.state.lock() {
            state.validated_at = None;
        }
    }

    #[allow(dead_code)]
    pub fn set_stream_key_hook(&self, hook: Option<StreamKeyHook>) {
        let mut hooks = self.hooks.lock().unwrap_or_else(|p| p.into_inner());
        hooks.stream_key = hook;
    }

    #[allow(dead_code)]
    pub fn set_token_path_override(&self, path: Option<PathBuf>) {
        let mut hooks = self.hooks.lock().unwrap_or_else(|p| p.into_inner());
        hooks.token_path_override = path;
    }

    fn resolve_token_path(&self) -> Option<PathBuf> {
        if let Some(custom) = self
            .hooks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .token_path_override
            .clone()
        {
            return Some(custom);
        }
        token_path()
    }

    /// Returns the current authentication session generation.
    pub fn auth_generation(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .auth_generation
    }

    /// Advances the authentication generation without changing the stored token.
    pub fn advance_generation(&self) -> u64 {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.auth_generation = state.auth_generation.wrapping_add(1);
        state.pending_ui_refresh = None;
        state.pending_ui_invalid = None;
        state.auth_generation
    }

    /// Returns `true` if `token` matches the currently active account and `generation`
    /// matches the current `auth_generation`.
    pub fn is_token_current(&self, token: &StoredToken, generation: u64) -> bool {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.auth_generation == generation
            && !state.logged_out
            && state
                .latest
                .as_ref()
                .map_or(true, |l| same_account(l, token))
    }

    /// Seeds the active token if this `SharedToken` has not yet been initialized or logged out.
    pub fn sync_if_uninitialized(&self, token: &StoredToken) -> u64 {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.latest.is_none() && !state.logged_out {
            state.latest = Some(token.clone());
        } else if let Some(ref cur) = state.latest {
            if !same_account(cur, token) {
                state.auth_generation = state.auth_generation.wrapping_add(1);
                state.latest = Some(token.clone());
                state.validated_at = None;
                state.pending_ui_refresh = None;
                state.pending_ui_invalid = None;
                state.logged_out = false;
            }
        }
        state.auth_generation
    }

    /// Sets the active authenticated token in memory, advancing `auth_generation`
    /// and invalidating any in-flight operations from a previous session or account.
    pub fn set_active_token(&self, token: &StoredToken) -> u64 {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.auth_generation = state.auth_generation.wrapping_add(1);
        state.latest = Some(token.clone());
        state.validated_at = Some(Instant::now());
        state.pending_ui_refresh = None;
        state.pending_ui_invalid = None;
        state.logged_out = false;
        state.auth_generation
    }

    /// Clears the in-memory token state and advances `auth_generation`.
    pub fn clear(&self) -> u64 {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.auth_generation = state.auth_generation.wrapping_add(1);
        state.latest = None;
        state.validated_at = None;
        state.pending_ui_refresh = None;
        state.pending_ui_invalid = None;
        state.logged_out = true;
        state.auth_generation
    }

    /// Clears in-memory token state (advancing `auth_generation`) and removes the token file on disk.
    pub fn delete_token(&self) -> u64 {
        let gen = self.clear();
        if let Some(path) = self.resolve_token_path() {
            let _disk = self.disk_lock.lock().unwrap_or_else(|p| p.into_inner());
            let should_remove = {
                let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                state.logged_out || state.latest.is_none()
            };
            if should_remove {
                let _ = std::fs::remove_file(path);
            }
        }
        gen
    }

    /// Saves `token` as the active session token in memory and on disk.
    pub fn save_token(&self, token: &StoredToken) -> std::io::Result<u64> {
        let gen = self.set_active_token(token);
        let path = self
            .resolve_token_path()
            .ok_or_else(|| std::io::Error::other("no config directory"))?;
        let _disk = self.disk_lock.lock().unwrap_or_else(|p| p.into_inner());
        {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if !state.can_commit(gen, token) {
                return Ok(state.auth_generation);
            }
        }
        save_token_at(&path, token)?;
        Ok(gen)
    }

    /// Loads `token` from disk without holding the state lock during file I/O.
    pub fn load_token(&self) -> Option<StoredToken> {
        let path = self.resolve_token_path()?;
        let data = std::fs::read_to_string(path).ok()?;
        let token: StoredToken = serde_json::from_str(&data).ok()?;
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if !state.logged_out
            && state.latest.as_ref().map(|t| &t.access_token) != Some(&token.access_token)
        {
            state.latest = Some(token.clone());
            state.validated_at = None;
            state.pending_ui_invalid = None;
        }
        Some(token)
    }

    /// Returns the latest valid/known token in shared state, if any.
    pub fn latest_token(&self) -> Option<StoredToken> {
        self.state.lock().ok().and_then(|s| s.latest.clone())
    }

    /// Returns the current access token string in shared state, if any.
    #[allow(dead_code)]
    pub fn access_token(&self) -> Option<String> {
        self.state
            .lock()
            .ok()
            .and_then(|s| s.latest.as_ref().map(|t| t.access_token.clone()))
    }

    /// Drains any token that was refreshed on a background thread for the current generation.
    pub fn take_refreshed_token(&self) -> Option<StoredToken> {
        let mut state = self.state.lock().ok()?;
        let (gen, token) = state.pending_ui_refresh.take()?;
        if gen == state.auth_generation && !state.logged_out {
            Some(token)
        } else {
            None
        }
    }

    /// Drains a permanent token invalidation error for the current generation.
    pub fn take_invalidated_reason(&self) -> Option<String> {
        let mut state = self.state.lock().ok()?;
        let (gen, reason) = state.pending_ui_invalid.take()?;
        if gen == state.auth_generation && !state.logged_out {
            Some(reason)
        } else {
            None
        }
    }

    pub fn ensure_valid(&self, token: &StoredToken) -> Result<Option<StoredToken>, AuthError> {
        self.ensure_valid_inner(token, false, false)
    }

    pub fn revalidate(&self, token: &StoredToken) -> Result<Option<StoredToken>, AuthError> {
        self.ensure_valid_inner(token, true, false)
    }

    pub fn force_refresh(&self, token: &StoredToken) -> Result<StoredToken, AuthError> {
        let opt = self.ensure_valid_inner(token, true, true)?;
        Ok(opt.unwrap_or_else(|| token.clone()))
    }

    fn ensure_valid_inner(
        &self,
        caller_token: &StoredToken,
        force_check: bool,
        force_refresh_now: bool,
    ) -> Result<Option<StoredToken>, AuthError> {
        let hooks = self.hooks.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let default_login = caller_token.login.clone();
        let default_uid = caller_token.user_id.clone();
        let default_access = caller_token.access_token.clone();
        let default_rt = caller_token.refresh_token.clone();

        let validate_fn = |access: &str| -> Result<Validation, AuthError> {
            if let Some(ref h) = hooks.validate {
                h(access)
            } else if cfg!(test) {
                Ok(Validation {
                    login: default_login.clone(),
                    user_id: default_uid.clone(),
                })
            } else {
                validate(access)
            }
        };

        let refresh_fn = |cid: &str, rt: &str| -> Result<Tokens, AuthError> {
            if let Some(ref h) = hooks.refresh {
                h(cid, rt)
            } else if cfg!(test) {
                Ok(Tokens {
                    access_token: default_access.clone(),
                    refresh_token: default_rt.clone(),
                })
            } else {
                refresh(cid, rt)
            }
        };

        let persist_fn = |new_token: &StoredToken| {
            if let Some(ref h) = hooks.persist {
                h(new_token);
            } else if let Some(path) = self.resolve_token_path() {
                if let Err(e) = save_token_at(&path, new_token) {
                    tracing::error!("Failed to save refreshed Twitch token: {e}");
                }
            }
        };

        self.ensure_valid_with_fns(
            caller_token,
            force_check,
            force_refresh_now,
            validate_fn,
            refresh_fn,
            persist_fn,
        )
    }

    pub fn ensure_valid_with_fns<V, R, P>(
        &self,
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
        // Single-flight lock serializes network validation/refresh across background workers
        // while leaving `self.state` completely unlocked during network & disk I/O.
        let _single_flight = self
            .refresh_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let (mut candidate, upgraded, start_gen, need_refresh) = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            if state.logged_out {
                return Err(AuthError::Invalid("session logged out".into()));
            }

            let (candidate, upgraded) = match &state.latest {
                Some(latest) => {
                    if !same_account(latest, caller_token) {
                        return Err(AuthError::Invalid("account changed".into()));
                    }
                    if latest.access_token != caller_token.access_token {
                        (latest.clone(), true)
                    } else {
                        (caller_token.clone(), false)
                    }
                }
                None => (caller_token.clone(), false),
            };

            let need_refresh = force_refresh_now && !upgraded;
            let cache_fresh = (!force_check || upgraded)
                && !need_refresh
                && state.latest.as_ref().map(|t| &t.access_token) == Some(&candidate.access_token)
                && state
                    .validated_at
                    .is_some_and(|t| t.elapsed() < VALIDATE_CACHE_TTL);

            if cache_fresh {
                return Ok(if upgraded { Some(candidate) } else { None });
            }

            (candidate, upgraded, state.auth_generation, need_refresh)
        };

        // Network validation runs WITHOUT holding `self.state`!
        if !need_refresh {
            match validate_fn(&candidate.access_token) {
                Ok(v) => {
                    if !v.login.is_empty() {
                        candidate.login = v.login;
                    }
                    if !v.user_id.is_empty() {
                        candidate.user_id = v.user_id;
                    }
                    let mut state = self
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if !state.can_commit(start_gen, &candidate) {
                        return Err(AuthError::Invalid(
                            "authentication session invalidated".into(),
                        ));
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

        // Re-check session generation before attempting a refresh rotation
        {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !state.can_commit(start_gen, &candidate) {
                return Err(AuthError::Invalid(
                    "authentication session invalidated".into(),
                ));
            }
        }

        let Some(rt) = candidate.refresh_token.as_deref().filter(|s| !s.is_empty()) else {
            let msg = "token expired and no refresh token is available".to_string();
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.can_commit(start_gen, &candidate) {
                state.latest = None;
                state.validated_at = None;
                state.pending_ui_refresh = None;
                state.pending_ui_invalid = Some((start_gen, msg.clone()));
            }
            return Err(AuthError::Invalid(msg));
        };

        let client_id = candidate.effective_client_id().to_string();
        // Network refresh & post-refresh validation run WITHOUT holding `self.state`!
        match refresh_fn(&client_id, rt) {
            Ok(tokens) => {
                let (login, user_id) = match validate_fn(&tokens.access_token) {
                    Ok(v) => (
                        if v.login.is_empty() {
                            candidate.login.clone()
                        } else {
                            v.login
                        },
                        if v.user_id.is_empty() {
                            candidate.user_id.clone()
                        } else {
                            v.user_id
                        },
                    ),
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
                if !same_account(&candidate, &new_token) {
                    return Err(AuthError::Invalid(
                        "refreshed token account mismatch".into(),
                    ));
                }

                // Commit in-memory state only if session generation and account identity still match
                {
                    let mut state = self
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if !state.can_commit(start_gen, &new_token) {
                        return Err(AuthError::Invalid(
                            "authentication session invalidated".into(),
                        ));
                    }
                    state.latest = Some(new_token.clone());
                    state.validated_at = Some(Instant::now());
                    state.pending_ui_refresh = Some((start_gen, new_token.clone()));
                    state.pending_ui_invalid = None;
                }

                // Persist to disk outside `self.state` lock, guarded by `disk_lock` and generation re-check
                {
                    let _disk = self
                        .disk_lock
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let still_valid = {
                        let state = self
                            .state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        state.can_commit(start_gen, &new_token)
                    };
                    if still_valid {
                        persist_fn(&new_token);
                    } else {
                        return Err(AuthError::Invalid(
                            "authentication session invalidated".into(),
                        ));
                    }
                }

                Ok(Some(new_token))
            }
            Err(AuthError::Invalid(reason)) => {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if state.can_commit(start_gen, &candidate) {
                    state.latest = None;
                    state.validated_at = None;
                    state.pending_ui_refresh = None;
                    state.pending_ui_invalid = Some((start_gen, reason.clone()));
                }
                Err(AuthError::Invalid(reason))
            }
            Err(AuthError::Network(e)) => Err(AuthError::Network(e)),
        }
    }

    /// Fetches the broadcaster's live stream key using this `SharedToken` store.
    pub fn fetch_stream_key(&self, token: &StoredToken) -> Result<String, AuthError> {
        let start_gen = self.sync_if_uninitialized(token);
        if !self.is_token_current(token, start_gen) {
            return Err(AuthError::Invalid(
                "authentication session invalidated".into(),
            ));
        }

        let mut active_token = match self.ensure_valid(token) {
            Ok(Some(refreshed)) => refreshed,
            Ok(None) => token.clone(),
            Err(AuthError::Invalid(e)) => return Err(AuthError::Invalid(e)),
            Err(AuthError::Network(_)) => token.clone(),
        };

        if !self.is_token_current(&active_token, start_gen) {
            return Err(AuthError::Invalid(
                "authentication session invalidated".into(),
            ));
        }

        if active_token.user_id.is_empty() {
            return Err(AuthError::Invalid(
                "User ID is missing from login token".into(),
            ));
        }

        let custom_hook = self
            .hooks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stream_key
            .clone();
        if let Some(hook) = custom_hook {
            let key = hook(&active_token)?;
            if !self.is_token_current(&active_token, start_gen) {
                return Err(AuthError::Invalid(
                    "authentication session invalidated".into(),
                ));
            }
            return Ok(key);
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
                if let Ok(refreshed) = self.force_refresh(&active_token) {
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
        let key = match result {
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
                    return Err(AuthError::Network(
                        "Twitch did not return a stream key".into(),
                    ));
                }
                key.to_string()
            }
            Err(ureq::Error::Status(401, resp)) => {
                let body = read_json(resp);
                let msg = message_of(&body);
                if msg.contains("Missing scope") || msg.contains("channel:read:stream_key") {
                    return Err(AuthError::Invalid(
                        "Missing permission: your login was created before stream key support was added. Please click 'Log out' and 'Log in with Twitch' under 'Twitch Account' to re-authorize.".into(),
                    ));
                } else {
                    return Err(AuthError::Invalid(format!(
                        "Twitch login expired or unauthorized ({msg})"
                    )));
                }
            }
            Err(ureq::Error::Status(code, resp)) => {
                return Err(AuthError::Network(format!(
                    "Helix API error {code}: {}",
                    message_of(&read_json(resp))
                )))
            }
            Err(e) => return Err(AuthError::Network(e.to_string())),
        };

        if !self.is_token_current(&active_token, start_gen) {
            return Err(AuthError::Invalid(
                "authentication session invalidated".into(),
            ));
        }
        Ok(key)
    }
}

#[allow(dead_code)]
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
    let store = SharedToken {
        state: Mutex::new(std::mem::take(state)),
        refresh_lock: Mutex::new(()),
        disk_lock: Mutex::new(()),
        hooks: Mutex::new(TokenHooks::default()),
    };
    let res = store.ensure_valid_with_fns(
        caller_token,
        force_check,
        force_refresh_now,
        validate_fn,
        refresh_fn,
        persist_fn,
    );
    *state = store.state.into_inner().unwrap_or_default();
    res
}

/// Validates the token (using a short-lived shared cache to avoid redundant network
/// requests when multiple workers start together) and refreshes it if expired.
/// Returns `Ok(Some(new))` if it had to be refreshed or if another worker already refreshed it.
pub fn ensure_valid(token: &StoredToken) -> Result<Option<StoredToken>, AuthError> {
    shared_token().ensure_valid(token)
}

/// Forces an immediate `/oauth2/validate` check against Twitch (bypassing the 5-minute
/// validation cache) and refreshes the token if expired. Used for periodic hourly checks
/// and after an IRC authentication failure.
pub fn revalidate(token: &StoredToken) -> Result<Option<StoredToken>, AuthError> {
    shared_token().revalidate(token)
}

/// Immediately refreshes the token after a Helix endpoint returned HTTP 401 (or returns
/// the already-refreshed token if another thread just refreshed it).
pub fn force_refresh(token: &StoredToken) -> Result<StoredToken, AuthError> {
    shared_token().force_refresh(token)
}

/// Drains any token that was refreshed on a background thread so `TwitchState` can update its copy.
#[allow(dead_code)]
pub fn take_refreshed_token() -> Option<StoredToken> {
    shared_token().take_refreshed_token()
}

/// Drains a permanent token invalidation error if a refresh token was rejected by Twitch.
#[allow(dead_code)]
pub fn take_invalidated_reason() -> Option<String> {
    shared_token().take_invalidated_reason()
}

/// Returns the latest valid/known token in shared state, if any.
#[allow(dead_code)]
pub fn latest_token() -> Option<StoredToken> {
    shared_token().latest_token()
}

/// Returns the current authentication session generation.
#[allow(dead_code)]
pub fn auth_generation() -> u64 {
    shared_token().auth_generation()
}

/// Runs a periodic background revalidation/refresh so tokens stay fresh even if IRC is disabled.
pub fn revalidate_in_background(token: StoredToken, ctx: egui::Context) {
    revalidate_in_background_with(shared_token().clone(), token, ctx);
}

pub fn revalidate_in_background_with(
    store: Arc<SharedToken>,
    token: StoredToken,
    ctx: egui::Context,
) {
    let _ = std::thread::Builder::new()
        .name("twitch-token-revalidate".into())
        .spawn(move || match store.revalidate(&token) {
            Ok(Some(_)) | Err(AuthError::Invalid(_)) => {
                ctx.request_repaint();
            }
            _ => {}
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
#[allow(dead_code)]
pub fn fetch_stream_key(token: &StoredToken) -> Result<String, AuthError> {
    shared_token().fetch_stream_key(token)
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
    let (status, body) =
        match post_form(DEVICE_URL, &[("client_id", client_id), ("scopes", SCOPES)]) {
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
    let (Some(device_code), Some(user_code), Some(verification_uri)) = (
        field("device_code"),
        field("user_code"),
        field("verification_uri"),
    ) else {
        return Some(DeviceFlowEvent::Failed(
            "malformed device code response".into(),
        ));
    };
    let expires_in = Duration::from_secs(
        body.get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(1800),
    );
    let mut interval = Duration::from_secs(
        body.get("interval")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .max(1),
    );
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

#[allow(dead_code)]
pub fn load_token() -> Option<StoredToken> {
    shared_token().load_token()
}

#[allow(dead_code)]
pub fn save_token(token: &StoredToken) -> std::io::Result<()> {
    shared_token().save_token(token).map(|_| ())
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

#[allow(dead_code)]
pub fn delete_token() {
    shared_token().delete_token();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Barrier;

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
            auth_generation: 1,
            latest: Some(expired_token.clone()),
            validated_at: None,
            pending_ui_refresh: None,
            pending_ui_invalid: None,
            logged_out: false,
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
        assert_eq!(
            refresh_calls.load(Ordering::SeqCst),
            1,
            "must not reuse consumed refresh token"
        );
        assert_eq!(
            validate_calls.load(Ordering::SeqCst),
            calls_before,
            "cached validation reused"
        );

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

    #[test]
    fn ui_queries_and_logout_do_not_block_while_validation_and_refresh_are_stalled_f09() {
        let store = Arc::new(SharedToken::new());
        let expired = StoredToken {
            access_token: "expired_tok".into(),
            refresh_token: Some("rt_1".into()),
            login: "alice".into(),
            user_id: "1001".into(),
            client_id: DEFAULT_CLIENT_ID.into(),
        };
        store.set_active_token(&expired);
        // Force validation by clearing validated_at via revalidate/ensure_valid with expired
        let enter_refresh = Arc::new(Barrier::new(2));
        let release_refresh = Arc::new(Barrier::new(2));

        let enter_clone = enter_refresh.clone();
        let release_clone = release_refresh.clone();

        store.set_test_hooks(
            Some(Arc::new(|access: &str| {
                if access == "fresh_tok" {
                    Ok(Validation {
                        login: "alice".into(),
                        user_id: "1001".into(),
                    })
                } else {
                    Err(AuthError::Invalid("expired".into()))
                }
            })),
            Some(Arc::new(move |_cid: &str, _rt: &str| {
                enter_clone.wait();
                release_clone.wait();
                Ok(Tokens {
                    access_token: "fresh_tok".into(),
                    refresh_token: Some("rt_2".into()),
                })
            })),
            None,
        );

        let worker_store = store.clone();
        let worker_tok = expired.clone();
        let worker = std::thread::spawn(move || worker_store.revalidate(&worker_tok));

        // Wait until worker is blocked inside the refresh network call
        enter_refresh.wait();

        // While worker is blocked in network I/O, UI-facing methods must return immediately!
        let ui_start = Instant::now();
        for _ in 0..50 {
            assert!(store.take_refreshed_token().is_none());
            assert!(store.take_invalidated_reason().is_none());
            assert_eq!(
                store
                    .latest_token()
                    .as_ref()
                    .map(|t| t.access_token.as_str()),
                Some("expired_tok")
            );
            assert_eq!(store.access_token().as_deref(), Some("expired_tok"));
        }
        assert!(
            ui_start.elapsed() < Duration::from_millis(50),
            "UI-facing SharedToken methods blocked during network refresh: {:?}",
            ui_start.elapsed()
        );

        // Release the refresh and verify the refreshed token is committed and drained by UI
        release_refresh.wait();
        let res = worker.join().unwrap().expect("refresh should succeed");
        assert_eq!(
            res.as_ref().map(|t| t.access_token.as_str()),
            Some("fresh_tok")
        );
        let ui_refreshed = store
            .take_refreshed_token()
            .expect("UI should receive refreshed token");
        assert_eq!(ui_refreshed.access_token, "fresh_tok");
    }

    #[test]
    fn concurrent_workers_single_flight_refresh_via_public_sync_path_f09() {
        let store = Arc::new(SharedToken::new());
        let expired = StoredToken {
            access_token: "expired_1".into(),
            refresh_token: Some("single_use_rt_1".into()),
            login: "streamer".into(),
            user_id: "777".into(),
            client_id: DEFAULT_CLIENT_ID.into(),
        };
        store.set_active_token(&expired);
        // Clear `validated_at` so `ensure_valid` must validate
        {
            store.state.lock().unwrap().validated_at = None;
        }

        let refresh_calls = Arc::new(AtomicUsize::new(0));
        let refresh_calls_hook = refresh_calls.clone();
        let start_barrier = Arc::new(Barrier::new(4));

        store.set_test_hooks(
            Some(Arc::new(|access: &str| {
                if access == "fresh_2" {
                    Ok(Validation {
                        login: "streamer".into(),
                        user_id: "777".into(),
                    })
                } else {
                    Err(AuthError::Invalid("expired".into()))
                }
            })),
            Some(Arc::new(move |_cid: &str, rt: &str| {
                let n = refresh_calls_hook.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(25));
                if n == 0 && rt == "single_use_rt_1" {
                    Ok(Tokens {
                        access_token: "fresh_2".into(),
                        refresh_token: Some("single_use_rt_2".into()),
                    })
                } else {
                    Err(AuthError::Invalid("rotated refresh token reused".into()))
                }
            })),
            None,
        );

        let mut handles = Vec::new();
        for i in 0..4 {
            let s = store.clone();
            let tok = expired.clone();
            let b = start_barrier.clone();
            handles.push(std::thread::spawn(move || {
                b.wait();
                if i == 3 {
                    s.force_refresh(&tok)
                } else {
                    s.ensure_valid(&tok).map(|opt| opt.unwrap_or(tok))
                }
            }));
        }

        for h in handles {
            let out = h
                .join()
                .unwrap()
                .expect("all concurrent callers must succeed");
            assert_eq!(out.access_token, "fresh_2");
            assert_eq!(out.refresh_token.as_deref(), Some("single_use_rt_2"));
        }
        assert_eq!(
            refresh_calls.load(Ordering::SeqCst),
            1,
            "single-flight lock must serialize refresh so single-use token is rotated only once"
        );
    }

    #[test]
    fn logout_and_account_switch_reject_delayed_auth_results_and_preserve_disk_f10() {
        let path = std::env::temp_dir().join(format!(
            "michadame-twitch-f10-{}-{}.json",
            std::process::id(),
            rand::random::<u32>()
        ));
        let store = Arc::new(SharedToken::new());
        store.set_token_path_override(Some(path.clone()));

        let token_a = StoredToken {
            access_token: "tok_a_old".into(),
            refresh_token: Some("rt_a_1".into()),
            login: "account_a".into(),
            user_id: "111".into(),
            client_id: DEFAULT_CLIENT_ID.into(),
        };
        let token_b = StoredToken {
            access_token: "tok_b_active".into(),
            refresh_token: Some("rt_b_1".into()),
            login: "account_b".into(),
            user_id: "222".into(),
            client_id: DEFAULT_CLIENT_ID.into(),
        };

        store.save_token(&token_a).unwrap();
        assert!(path.exists());

        let enter_a = Arc::new(Barrier::new(2));
        let release_a = Arc::new(Barrier::new(2));
        let enter_a_clone = enter_a.clone();
        let release_a_clone = release_a.clone();

        store.set_test_hooks(
            Some(Arc::new(|access: &str| {
                if access == "tok_a_refreshed" {
                    Ok(Validation {
                        login: "account_a".into(),
                        user_id: "111".into(),
                    })
                } else {
                    Err(AuthError::Invalid("expired".into()))
                }
            })),
            Some(Arc::new(move |_cid: &str, _rt: &str| {
                enter_a_clone.wait();
                release_a_clone.wait();
                Ok(Tokens {
                    access_token: "tok_a_refreshed".into(),
                    refresh_token: Some("rt_a_2".into()),
                })
            })),
            None,
        );

        let worker_store = store.clone();
        let worker_tok_a = token_a.clone();
        let worker_a = std::thread::spawn(move || worker_store.revalidate(&worker_tok_a));

        // Wait for Account A's refresh to be in flight
        enter_a.wait();

        // User logs out of Account A and logs into Account B while A's refresh is in flight
        store.delete_token();
        assert!(!path.exists(), "delete_token must remove credential file");
        store.save_token(&token_b).unwrap();

        // Release Account A's delayed refresh result
        release_a.wait();
        let res_a = worker_a.join().unwrap();
        assert!(
            matches!(res_a, Err(AuthError::Invalid(_))),
            "stale refresh from Account A must be rejected after switching to Account B"
        );

        // Verify Account B remains active in memory and on disk
        assert_eq!(store.latest_token(), Some(token_b.clone()));
        assert!(store.take_refreshed_token().is_none());
        assert!(store.take_invalidated_reason().is_none());
        let on_disk: StoredToken =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk, token_b);

        // Now test logout with NO subsequent login while a refresh is in flight
        let enter_b = Arc::new(Barrier::new(2));
        let release_b = Arc::new(Barrier::new(2));
        let enter_b_clone = enter_b.clone();
        let release_b_clone = release_b.clone();

        store.set_test_hooks(
            Some(Arc::new(|access: &str| {
                if access == "tok_b_refreshed" {
                    Ok(Validation {
                        login: "account_b".into(),
                        user_id: "222".into(),
                    })
                } else {
                    Err(AuthError::Invalid("expired".into()))
                }
            })),
            Some(Arc::new(move |_cid: &str, _rt: &str| {
                enter_b_clone.wait();
                release_b_clone.wait();
                Ok(Tokens {
                    access_token: "tok_b_refreshed".into(),
                    refresh_token: Some("rt_b_2".into()),
                })
            })),
            None,
        );

        let worker_store_b = store.clone();
        let worker_tok_b = token_b.clone();
        let worker_b = std::thread::spawn(move || worker_store_b.revalidate(&worker_tok_b));

        enter_b.wait();
        store.delete_token();
        assert!(!path.exists(), "credential file must be removed on logout");

        release_b.wait();
        let res_b = worker_b.join().unwrap();
        assert!(matches!(res_b, Err(AuthError::Invalid(_))));
        assert!(store.latest_token().is_none());
        assert!(store.take_refreshed_token().is_none());
        assert!(store.take_invalidated_reason().is_none());
        assert!(
            !path.exists(),
            "rejected old refresh work must not recreate deleted credential file"
        );
    }
}
