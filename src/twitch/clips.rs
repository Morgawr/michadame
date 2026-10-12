//! Twitch clip creation and processing: triggers 20s live stream clips,
//! polls the Helix API until ready, and dispatches events.

use super::auth::StoredToken;
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use std::time::Duration;

const CLIPS_URL: &str = "https://api.twitch.tv/helix/clips";
const MAX_TITLE_CHARS: usize = 100;
const MAX_POLL_ATTEMPTS: usize = 30;
const POLL_INTERVAL: Duration = Duration::from_millis(2500);

/// Formats a clip title: `"Word(Reading) - (tag)"` or `"Word - (tag)"` if reading
/// is empty or matches the word. Truncates to Twitch's 100 character limit.
pub fn format_clip_title(term: &str, reading: &str, tag: Option<&str>) -> String {
    let clean_term = term.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean_reading = reading.split_whitespace().collect::<Vec<_>>().join(" ");
    let word_part = if clean_reading.is_empty() || clean_reading == clean_term {
        clean_term
    } else {
        format!("{clean_term}({clean_reading})")
    };

    let full = match tag.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => {
            let clean_tag = t.split_whitespace().collect::<Vec<_>>().join(" ");
            format!("{word_part} - {clean_tag}")
        }
        None => word_part,
    };

    if full.chars().count() > MAX_TITLE_CHARS {
        full.chars().take(MAX_TITLE_CHARS).collect()
    } else {
        full
    }
}

#[derive(Clone, Debug)]
pub enum ClipEvent {
    Ready { clip_url: String },
    Failed { title: String, error: String },
}

pub struct ClipManager {
    event_tx: Sender<ClipEvent>,
    event_rx: Receiver<ClipEvent>,
}

impl Default for ClipManager {
    fn default() -> Self {
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        Self { event_tx, event_rx }
    }
}

impl ClipManager {
    /// Poll for finished clip jobs without blocking.
    pub fn poll(&self) -> Vec<ClipEvent> {
        self.event_rx.try_iter().collect()
    }

    #[cfg(test)]
    pub fn send_event_for_test(&self, event: ClipEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Triggers an asynchronous Twitch clip creation in a background thread.
    pub fn trigger_clip(
        &self,
        broadcaster_id: String,
        title: String,
        token: StoredToken,
        ctx: egui::Context,
    ) {
        let tx = self.event_tx.clone();
        let _ = std::thread::Builder::new()
            .name("twitch-clip".to_string())
            .spawn(move || {
                let result = create_and_poll_clip(&broadcaster_id, &title, &token);
                match result {
                    Ok(clip_url) => {
                        let _ = tx.send(ClipEvent::Ready { clip_url });
                    }
                    Err(error) => {
                        let _ = tx.send(ClipEvent::Failed { title, error });
                    }
                }
                ctx.request_repaint();
            });
    }
}

fn create_and_poll_clip(
    broadcaster_id: &str,
    title: &str,
    token: &StoredToken,
) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .build();

    // 1. POST /helix/clips?broadcaster_id=...&title=...&duration=20.0
    let resp = agent
        .post(CLIPS_URL)
        .query("broadcaster_id", broadcaster_id)
        .query("title", title)
        .query("duration", "20.0")
        .set("Client-Id", &token.client_id)
        .set("Authorization", &format!("Bearer {}", token.access_token))
        .call();

    let post_resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(401, resp)) => {
            let msg = resp.into_string().unwrap_or_default();
            if msg.contains("clips:edit") || msg.contains("Missing scope") {
                return Err(
                    "Missing permission: your login lacks 'clips:edit'. Please log out and log in again under the Twitch tab."
                        .into(),
                );
            }
            return Err(format!("Twitch unauthorized (401): {msg}"));
        }
        Err(ureq::Error::Status(code, resp)) => {
            let msg = resp.into_string().unwrap_or_default();
            return Err(format!("Twitch API error ({code}): {msg}"));
        }
        Err(e) => return Err(format!("Network error: {e}")),
    };

    let body: serde_json::Value = post_resp
        .into_string()
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .ok_or_else(|| "Failed to parse Twitch clip response JSON".to_string())?;

    let clip_id = body
        .get("data")
        .and_then(|d| d.as_array())
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Twitch clip response missing clip ID".to_string())?
        .to_string();

    // 2. Poll GET /helix/clips?id=<clip_id> until clip is processed
    let poll_agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build();

    for _ in 0..MAX_POLL_ATTEMPTS {
        std::thread::sleep(POLL_INTERVAL);

        let get_resp = poll_agent
            .get(CLIPS_URL)
            .query("id", &clip_id)
            .set("Client-Id", &token.client_id)
            .set("Authorization", &format!("Bearer {}", token.access_token))
            .call();

        match get_resp {
            Ok(r) => {
                let get_body: serde_json::Value = r
                    .into_string()
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or(serde_json::Value::Null);

                if let Some(data_array) = get_body.get("data").and_then(|d| d.as_array()) {
                    if let Some(first_clip) = data_array.first() {
                        let url = first_clip
                            .get("url")
                            .and_then(|u| u.as_str())
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("https://clips.twitch.tv/{clip_id}"));
                        return Ok(url);
                    }
                }
            }
            Err(ureq::Error::Status(404, _)) => {
                // Not ready yet, continue polling
            }
            Err(ureq::Error::Status(code, resp)) => {
                let msg = resp.into_string().unwrap_or_default();
                return Err(format!("Twitch clip check failed ({code}): {msg}"));
            }
            Err(e) => {
                tracing::warn!("Transient error polling clip {clip_id}: {e}");
            }
        }
    }

    Err(format!(
        "Timed out waiting for clip '{clip_id}' to be processed by Twitch."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_clip_title_with_reading_and_tag() {
        assert_eq!(
            format_clip_title("飽きる", "あきる", Some("二重影")),
            "飽きる(あきる) - 二重影"
        );
    }

    #[test]
    fn test_format_clip_title_without_tag() {
        assert_eq!(
            format_clip_title("飽きる", "あきる", None),
            "飽きる(あきる)"
        );
        assert_eq!(
            format_clip_title("飽きる", "あきる", Some("")),
            "飽きる(あきる)"
        );
        assert_eq!(
            format_clip_title("飽きる", "あきる", Some("   ")),
            "飽きる(あきる)"
        );
    }

    #[test]
    fn test_format_clip_title_kana_word() {
        assert_eq!(
            format_clip_title("スプーン", "スプーン", Some("二重影")),
            "スプーン - 二重影"
        );
        assert_eq!(
            format_clip_title("スプーン", "", Some("二重影")),
            "スプーン - 二重影"
        );
        assert_eq!(
            format_clip_title("スプーン", "スプーン", None),
            "スプーン"
        );
    }

    #[test]
    fn test_format_clip_title_whitespace_normalization() {
        assert_eq!(
            format_clip_title(" 食べる \n", " たべる ", Some(" タグ  名 ")),
            "食べる(たべる) - タグ 名"
        );
    }

    #[test]
    fn test_format_clip_title_truncation() {
        let long_tag = "a".repeat(120);
        let title = format_clip_title("単語", "たんご", Some(&long_tag));
        assert_eq!(title.chars().count(), 100);
        assert!(title.starts_with("単語(たんご) - aaaaa"));
    }
}
