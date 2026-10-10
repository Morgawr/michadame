//! Polls the Twitch Helix API for whether the channel is actually live.
//!
//! Helix requires an access token; Michadame is a public client (no secret), so
//! this only runs when the user is logged in. Otherwise the status is `Unknown`.

use super::auth::StoredToken;
use crossbeam_channel::Receiver;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const STREAMS_URL: &str = "https://api.twitch.tv/helix/streams";
const POLL_EVERY: Duration = Duration::from_secs(60);
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq)]
pub enum StreamStatus {
    /// Not known (not logged in, request failed, ...). Carries the reason.
    Unknown(String),
    Live,
    Offline,
}

/// Interpret a Helix `/streams` response body.
pub fn parse_streams_response(body: &serde_json::Value) -> Option<StreamStatus> {
    let data = body.get("data")?.as_array()?;
    let live = data
        .iter()
        .any(|s| s.get("type").and_then(|t| t.as_str()) == Some("live"));
    Some(if live {
        StreamStatus::Live
    } else {
        StreamStatus::Offline
    })
}

fn query(channel: &str, token: &StoredToken) -> StreamStatus {
    let result = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .build()
        .get(STREAMS_URL)
        .query("user_login", channel)
        .set("Client-Id", &token.client_id)
        .set("Authorization", &format!("Bearer {}", token.access_token))
        .call();
    match result {
        Ok(resp) => {
            let body = resp
                .into_string()
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Null);
            parse_streams_response(&body)
                .unwrap_or_else(|| StreamStatus::Unknown("unexpected response from Twitch".into()))
        }
        Err(ureq::Error::Status(401, _)) => StreamStatus::Unknown("Twitch login expired".into()),
        Err(ureq::Error::Status(code, _)) => StreamStatus::Unknown(format!("Twitch API error {code}")),
        Err(e) => StreamStatus::Unknown(e.to_string()),
    }
}

/// Owns a polling thread; dropping it stops the thread.
pub struct LivePoller {
    pub channel: String,
    pub access_token: String,
    pub results: Receiver<StreamStatus>,
    stop: Arc<AtomicBool>,
}

impl Drop for LivePoller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub fn spawn(channel: String, token: StoredToken, ctx: egui::Context) -> LivePoller {
    let (tx, rx) = crossbeam_channel::unbounded();
    let stop = Arc::new(AtomicBool::new(false));
    let poller = LivePoller {
        channel: channel.clone(),
        access_token: token.access_token.clone(),
        results: rx,
        stop: stop.clone(),
    };
    let _ = std::thread::Builder::new()
        .name("twitch-live".into())
        .spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let status = query(&channel, &token);
                let wait = match status {
                    StreamStatus::Unknown(_) => RETRY_AFTER_ERROR,
                    _ => POLL_EVERY,
                };
                if tx.send(status).is_err() {
                    break;
                }
                ctx.request_repaint_of(egui::ViewportId::ROOT);
                let until = Instant::now() + wait;
                while Instant::now() < until && !stop.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        });
    poller
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_live_offline_and_garbage() {
        let live = serde_json::json!({"data": [{"type": "live", "user_login": "x"}], "pagination": {}});
        assert_eq!(parse_streams_response(&live), Some(StreamStatus::Live));
        let offline = serde_json::json!({"data": [], "pagination": {}});
        assert_eq!(parse_streams_response(&offline), Some(StreamStatus::Offline));
        // Anything other than type "live" is not considered live.
        let other = serde_json::json!({"data": [{"type": ""}]});
        assert_eq!(parse_streams_response(&other), Some(StreamStatus::Offline));
        assert_eq!(parse_streams_response(&serde_json::json!({"error": "x"})), None);
    }
}
