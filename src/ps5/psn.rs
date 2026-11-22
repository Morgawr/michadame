//! PlayStation Network (PSN) OAuth2 login, token storage (`psn_token.json`, mode 0600),
//! automatic token refresh, and 8-byte little-endian Base64 Account-ID derivation.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const PSN_CLIENT_ID: &str = "ba495a24-818c-472b-b12d-ff231c1b5745";
pub const PSN_CLIENT_SECRET: &str = "mvaiZkRsAsI1IBkY";
pub const PSN_REDIRECT_URI: &str = "https://remoteplay.dl.playstation.net/remoteplay/redirect";
pub const PSN_SCOPES: &str = "psn:clientapp referenceDataService:countryConfig.read pushNotification:webSocket.desktop.connect sessionManager:remotePlaySession.system.update";
pub const PSN_TOKEN_URL: &str = "https://auth.api.sonyentertainmentnetwork.com/2.0/oauth/token";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoredPsnToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_unix: u64,
    pub account_id_b64: String,
    pub user_id: String,
    pub online_id: String,
    pub client_duid: String,
}

impl StoredPsnToken {
    pub fn is_signed_in(&self) -> bool {
        !self.access_token.trim().is_empty() && !self.refresh_token.trim().is_empty()
    }

    pub fn needs_refresh(&self) -> bool {
        let now = unix_now_secs();
        self.access_token.trim().is_empty() || now.saturating_add(60) >= self.expires_at_unix
    }
}

pub fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn psn_token_path() -> Option<PathBuf> {
    let cfg = confy::get_configuration_file_path("michadame", None).ok()?;
    Some(cfg.parent()?.join("psn_token.json"))
}

fn set_private_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

pub fn load_stored_token() -> Option<StoredPsnToken> {
    if cfg!(test) {
        return None;
    }
    let path = psn_token_path()?;
    load_stored_token_at(&path)
}

pub fn load_stored_token_at(path: &Path) -> Option<StoredPsnToken> {
    let raw = std::fs::read_to_string(path).ok()?;
    let tok: StoredPsnToken = serde_json::from_str(&raw).ok()?;
    if tok.is_signed_in() {
        Some(tok)
    } else {
        None
    }
}

pub fn save_stored_token(token: &StoredPsnToken) -> Result<()> {
    if cfg!(test) {
        return Ok(());
    }
    let path = psn_token_path().ok_or_else(|| anyhow!("Could not determine config directory"))?;
    save_stored_token_at(&path, token)
}

pub fn save_stored_token_at(path: &Path, token: &StoredPsnToken) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create config directory")?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(token).context("Failed to serialize PSN token")?;
    std::fs::write(&tmp, json).context("Failed to write temporary PSN token file")?;
    set_private_permissions(&tmp);
    std::fs::rename(&tmp, path).context("Failed to atomically save PSN token file")?;
    set_private_permissions(path);
    Ok(())
}

pub fn delete_stored_token() {
    if cfg!(test) {
        return;
    }
    if let Some(path) = psn_token_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Builds the Sony PSN OAuth2 login URL matching `chiaki-ng`'s remote play login flow.
pub fn build_psn_login_url(client_duid: &str) -> String {
    let encoded_scopes = PSN_SCOPES.replace(' ', "%20");
    format!(
        "https://auth.api.sonyentertainmentnetwork.com/2.0/oauth/authorize?\
service_entity=urn:service-entity:psn\
&response_type=code\
&client_id={PSN_CLIENT_ID}\
&redirect_uri={PSN_REDIRECT_URI}\
&scope={encoded_scopes}\
&request_locale=en_US\
&ui=pr\
&service_logo=ps\
&layout_type=popup\
&smcid=remoteplay\
&prompt=always\
&PlatformPrivacyWs1=minimal\
&duid={client_duid}&"
    )
}

/// Extracts the OAuth `code` query parameter from a pasted redirect URL, or returns the
/// trimmed raw input if the user pasted just the code itself.
pub fn extract_redirect_code(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = url::Url::parse(trimmed) {
        for (k, v) in parsed.query_pairs() {
            if k == "code" {
                let code = v.trim();
                if !code.is_empty() {
                    return Some(code.to_string());
                }
            }
        }
        return None;
    }
    if let Some(idx) = trimmed.find("code=") {
        let rest = &trimmed[idx + 5..];
        let code = rest.split('&').next().unwrap_or("").trim();
        if !code.is_empty() {
            return Some(code.to_string());
        }
        return None;
    }
    if !trimmed.contains(' ') && !trimmed.contains("://") {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn basic_auth_header() -> String {
    let creds = format!("{PSN_CLIENT_ID}:{PSN_CLIENT_SECRET}");
    format!("Basic {}", base64_encode(creds.as_bytes()))
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(20))
        .timeout_write(Duration::from_secs(15))
        .build()
}

fn parse_ureq_json(resp: Result<ureq::Response, ureq::Error>) -> Result<serde_json::Value> {
    match resp {
        Ok(r) => {
            let text = r
                .into_string()
                .context("Failed reading PSN HTTP response")?;
            serde_json::from_str(&text).context("Malformed JSON response from PSN")
        }
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            let detail = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    v.get("error_description")
                        .or_else(|| v.get("error"))
                        .and_then(|s| s.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| body.chars().take(160).collect());
            Err(anyhow!("PSN returned HTTP {code}: {detail}"))
        }
        Err(e) => Err(anyhow!("PSN network error: {e}")),
    }
}

/// Exchanges an OAuth redirect code for PSN tokens and fetches the user's Account-ID (Base64).
pub fn exchange_redirect_code(raw_input: &str, client_duid: &str) -> Result<StoredPsnToken> {
    let code = extract_redirect_code(raw_input).ok_or_else(|| {
        anyhow!("Could not find an authorization code in the pasted redirect URL.")
    })?;

    let agent = http_agent();
    let auth = basic_auth_header();

    let token_json = parse_ureq_json(
        agent
            .post(PSN_TOKEN_URL)
            .set("Authorization", &auth)
            .send_form(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("scope", PSN_SCOPES),
                ("redirect_uri", PSN_REDIRECT_URI),
            ]),
    )
    .context("PSN OAuth token exchange failed")?;

    let access_token = token_json
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("PSN token response missing access_token"))?
        .to_string();
    let refresh_token = token_json
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let expires_in = token_json
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(3600);
    let expires_at_unix = unix_now_secs().saturating_add(expires_in);

    // Fetch numeric user_id via GET /2.0/oauth/token/{access_token}
    let info_url = format!("{PSN_TOKEN_URL}/{access_token}");
    let user_json = parse_ureq_json(
        agent
            .get(&info_url)
            .set("Authorization", &auth)
            .set("Accept", "application/json")
            .call(),
    )
    .context("Failed to fetch PSN user_id from token info endpoint")?;

    let user_id = user_json
        .get("user_id")
        .and_then(|v| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("PSN token info response missing user_id"))?;
    let online_id = user_json
        .get("online_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let account_id_b64 = user_id_to_account_id_b64(&user_id)?;

    let stored = StoredPsnToken {
        access_token,
        refresh_token,
        expires_at_unix,
        account_id_b64,
        user_id,
        online_id,
        client_duid: client_duid.to_string(),
    };
    let _ = save_stored_token(&stored);
    Ok(stored)
}

/// Refreshes the PSN access token if it is expired or within 60 seconds of expiring.
pub fn ensure_fresh_psn_token(stored: &StoredPsnToken) -> Result<StoredPsnToken> {
    if !stored.needs_refresh() {
        return Ok(stored.clone());
    }
    if stored.refresh_token.trim().is_empty() {
        return Err(anyhow!(
            "PSN session expired and has no refresh token — please sign in to PSN again."
        ));
    }

    let agent = http_agent();
    let auth = basic_auth_header();
    let token_json = parse_ureq_json(
        agent
            .post(PSN_TOKEN_URL)
            .set("Authorization", &auth)
            .send_form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &stored.refresh_token),
                ("scope", PSN_SCOPES),
                ("redirect_uri", PSN_REDIRECT_URI),
            ]),
    )
    .context("Failed to refresh PSN OAuth token")?;

    let access_token = token_json
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("PSN token refresh response missing access_token"))?
        .to_string();
    let refresh_token = token_json
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| stored.refresh_token.clone());
    let expires_in = token_json
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(3600);

    let updated = StoredPsnToken {
        access_token,
        refresh_token,
        expires_at_unix: unix_now_secs().saturating_add(expires_in),
        account_id_b64: stored.account_id_b64.clone(),
        user_id: stored.user_id.clone(),
        online_id: stored.online_id.clone(),
        client_duid: stored.client_duid.clone(),
    };
    let _ = save_stored_token(&updated);
    Ok(updated)
}

/// Converts a numeric PSN `user_id` string into the 8-byte little-endian Base64 `Account-ID`
/// expected by PS5 Remote Play registration and `libchiaki`.
pub fn user_id_to_account_id_b64(user_id: &str) -> Result<String> {
    let trimmed = user_id.trim();
    let parsed: u64 = trimmed
        .parse::<u64>()
        .or_else(|_| trimmed.parse::<i64>().map(|v| v as u64))
        .with_context(|| format!("Invalid numeric PSN user_id '{trimmed}'"))?;
    Ok(base64_encode(&parsed.to_le_bytes()))
}

/// Decodes a Base64-encoded PSN Account-ID into the exact 8-byte little-endian array.
pub fn decode_account_id_b64(b64: &str) -> Result<[u8; 8]> {
    let bytes = base64_decode(b64.trim())?;
    if bytes.len() != 8 {
        return Err(anyhow!(
            "PSN Account-ID must decode to exactly 8 bytes (got {} bytes).",
            bytes.len()
        ));
    }
    let mut out = [0u8; 8];
    out.copy_from_slice(&bytes);
    Ok(out)
}

pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(((input.len() + 2) / 3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub fn base64_decode(input: &str) -> Result<Vec<u8>> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }

    let cleaned: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    if cleaned.len() % 4 == 1 {
        return Err(anyhow!("Invalid Base64 length"));
    }

    let mut out = Vec::with_capacity((cleaned.len() * 3) / 4);
    let mut i = 0usize;
    while i < cleaned.len() {
        let c0 = cleaned[i];
        let c1 = *cleaned
            .get(i + 1)
            .ok_or_else(|| anyhow!("Truncated Base64"))?;
        let c2 = cleaned.get(i + 2).copied().unwrap_or(b'=');
        let c3 = cleaned.get(i + 3).copied().unwrap_or(b'=');

        let v0 = val(c0).ok_or_else(|| anyhow!("Invalid Base64 character"))? as u32;
        let v1 = val(c1).ok_or_else(|| anyhow!("Invalid Base64 character"))? as u32;

        if c2 == b'=' {
            if c3 != b'=' {
                return Err(anyhow!("Invalid Base64 padding"));
            }
            out.push(((v0 << 2) | (v1 >> 4)) as u8);
            break;
        }
        let v2 = val(c2).ok_or_else(|| anyhow!("Invalid Base64 character"))? as u32;
        if c3 == b'=' {
            let n = (v0 << 12) | (v1 << 6) | v2;
            out.push(((n >> 10) & 0xFF) as u8);
            out.push(((n >> 2) & 0xFF) as u8);
            break;
        }
        let v3 = val(c3).ok_or_else(|| anyhow!("Invalid Base64 character"))? as u32;
        let n = (v0 << 18) | (v1 << 12) | (v2 << 6) | v3;
        out.push(((n >> 16) & 0xFF) as u8);
        out.push(((n >> 8) & 0xFF) as u8);
        out.push((n & 0xFF) as u8);
        i += 4;
    }
    Ok(out)
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub fn hex_decode_fixed<const N: usize>(hex: &str) -> Result<[u8; N]> {
    let s = hex.trim();
    if s.len() != N * 2 {
        return Err(anyhow!(
            "Expected {} hex characters (got {})",
            N * 2,
            s.len()
        ));
    }
    let mut out = [0u8; N];
    for (idx, chunk) in s.as_bytes().chunks_exact(2).enumerate() {
        let pair = std::str::from_utf8(chunk).map_err(|_| anyhow!("Invalid hex UTF-8"))?;
        out[idx] =
            u8::from_str_radix(pair, 16).with_context(|| format!("Invalid hex byte '{pair}'"))?;
    }
    Ok(out)
}
