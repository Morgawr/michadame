//! Twitch clip creation and processing: triggers 20s live stream clips,
//! polls the Helix API until ready, and dispatches events.

use super::auth::StoredToken;
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use std::time::Duration;

const CLIPS_URL: &str = "https://api.twitch.tv/helix/clips";
const MAX_TITLE_CHARS: usize = 100;
/// Poll up to 10 times at 1.5s intervals (15 seconds total), matching Twitch's official
/// recommendation to assume failure if Get Clips does not return after 15 seconds.
const MAX_POLL_ATTEMPTS: usize = 10;
const POLL_INTERVAL: Duration = Duration::from_millis(1500);
/// Delay before triggering Twitch clip capture so viewers see context after clicking (post-click context).
pub const PRE_CLIP_BUFFER: Duration = Duration::from_secs(8);
/// Total length of the clip requested from Twitch in seconds (valid range: 5.0 to 60.0, default 30.0).
/// With an 8s post-click buffer, a 20.0s clip captures ~12s before the click and ~8s after.
pub const CLIP_DURATION_SECS: f32 = 20.0;

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
    Ready {
        title: String,
        clip_url: String,
        mined_created_at: Option<i64>,
    },
    Failed {
        title: String,
        error: String,
        #[allow(dead_code)]
        mined_created_at: Option<i64>,
    },
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
        mined_created_at: Option<i64>,
    ) {
        let tx = self.event_tx.clone();
        let clip_title = title.clone();
        let _ = std::thread::Builder::new()
            .name("twitch-clip".to_string())
            .spawn(move || {
                let result = create_and_poll_clip(&broadcaster_id, &title, &token);
                match result {
                    Ok(clip_url) => {
                        let _ = tx.send(ClipEvent::Ready { title: clip_title, clip_url, mined_created_at });
                    }
                    Err(error) => {
                        let _ = tx.send(ClipEvent::Failed { title: clip_title, error, mined_created_at });
                    }
                }
                ctx.request_repaint();
            });
    }
}

fn post_create_clip(
    agent: &ureq::Agent,
    broadcaster_id: &str,
    title: &str,
    token: &StoredToken,
) -> Result<String, String> {
    tracing::info!("Requesting Twitch clip for broadcaster '{broadcaster_id}' with title: '{title}'");
    let resp = agent
        .post(CLIPS_URL)
        .query("broadcaster_id", broadcaster_id)
        .query("title", title)
        .query("duration", &format!("{CLIP_DURATION_SECS:.1}"))
        .set("Client-Id", &token.client_id)
        .set("Authorization", &format!("Bearer {}", token.access_token))
        .send_bytes(&[]);

    let post_resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(401, resp)) => {
            let msg = resp.into_string().unwrap_or_default();
            tracing::error!("Twitch clip creation unauthorized (401): {msg}");
            if msg.contains("clips:edit") || msg.contains("Missing scope") {
                return Err(
                    "Missing permission: your login lacks 'clips:edit'. Please log out and log in again under the Twitch tab."
                        .into(),
                );
            }
            return Err(format!("Twitch unauthorized (401): {msg}"));
        }
        Err(ureq::Error::Status(404, resp)) => {
            let msg = resp.into_string().unwrap_or_default();
            tracing::error!("Twitch clip creation failed (404): {msg}");
            return Err("Twitch reports the channel is offline or cannot be clipped right now.".into());
        }
        Err(ureq::Error::Status(code, resp)) => {
            let msg = resp.into_string().unwrap_or_default();
            tracing::error!("Twitch clip creation failed ({code}): {msg}");
            return Err(format!("Twitch API error ({code}): {msg}"));
        }
        Err(e) => {
            tracing::error!("Twitch clip network error: {e}");
            return Err(format!("Network error: {e}"));
        }
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

    tracing::info!("Twitch clip created with ID '{clip_id}'. Polling for completion...");
    Ok(clip_id)
}

fn poll_clip_ready(
    agent: &ureq::Agent,
    clip_id: &str,
    token: &StoredToken,
) -> Result<Option<String>, String> {
    for _ in 0..MAX_POLL_ATTEMPTS {
        std::thread::sleep(POLL_INTERVAL);

        let get_resp = agent
            .get(CLIPS_URL)
            .query("id", clip_id)
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
                        tracing::info!("Twitch clip '{clip_id}' is ready: {url}");
                        return Ok(Some(url));
                    }
                }
            }
            Err(ureq::Error::Status(404, _)) => {
                // Not ready yet, continue polling
            }
            Err(ureq::Error::Status(code, resp)) => {
                let msg = resp.into_string().unwrap_or_default();
                tracing::error!("Twitch clip check failed ({code}): {msg}");
                return Err(format!("Twitch clip check failed ({code}): {msg}"));
            }
            Err(e) => {
                tracing::warn!("Transient error polling clip {clip_id}: {e}");
            }
        }
    }
    Ok(None)
}

/// Triggers a fire-and-forget warm-up clip request to initialize Twitch's
/// clipping pipeline (preempting known Twitch issue #935 where the first clip fails).
pub fn trigger_warmup_clip(broadcaster_id: String, token: StoredToken) {
    let _ = std::thread::Builder::new()
        .name("twitch-warmup-clip".to_string())
        .spawn(move || {
            let agent = ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(10))
                .build();
            tracing::info!(
                "Sending background Twitch stream warm-up clip for broadcaster '{broadcaster_id}' (preempting issue #935)..."
            );
            let resp = agent
                .post(CLIPS_URL)
                .query("broadcaster_id", &broadcaster_id)
                .query("title", "michadame-stream-warmup")
                .set("Client-Id", &token.client_id)
                .set("Authorization", &format!("Bearer {}", token.access_token))
                .send_bytes(&[]);
            match resp {
                Ok(_) => {
                    tracing::info!("Twitch stream warm-up clip request accepted by Twitch.");
                }
                Err(e) => {
                    tracing::debug!("Twitch stream warm-up clip response: {e}");
                }
            }
        });
}

fn create_and_poll_clip(
    broadcaster_id: &str,
    title: &str,
    token: &StoredToken,
) -> Result<String, String> {
    // Wait for the post-click buffer so the clip includes context after the word was clicked.
    tracing::info!("Waiting {PRE_CLIP_BUFFER:?} buffer before requesting clip for '{title}'...");
    std::thread::sleep(PRE_CLIP_BUFFER);

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build();

    let clip_id = post_create_clip(&agent, broadcaster_id, title, token)?;
    if let Some(url) = poll_clip_ready(&agent, &clip_id, token)? {
        return Ok(url);
    }

    tracing::error!("Timed out waiting for clip '{clip_id}' to be processed by Twitch.");
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

    #[test]
    fn test_post_clip_sends_content_length() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            stream
                .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\n\r\n{\"data\":[{\"id\":\"TestSlug123\",\"edit_url\":\"https://clips.twitch.tv/TestSlug123/edit\"}]}")
                .unwrap();
            req
        });

        let agent = ureq::AgentBuilder::new().build();
        let resp = agent
            .post(&format!("http://127.0.0.1:{port}/helix/clips"))
            .query("broadcaster_id", "89884001")
            .query("title", "頻繁(ひんぱん) - 二重影")
            .query("duration", &format!("{CLIP_DURATION_SECS:.1}"))
            .set("Client-Id", "test-client-id")
            .set("Authorization", "Bearer test-token")
            .send_bytes(&[]);

        assert!(resp.is_ok());
        let req_str = handle.join().unwrap();
        assert!(req_str.starts_with("POST /helix/clips?"));
        assert!(req_str.contains("Content-Length: 0"));
        assert!(req_str.contains("Client-Id: test-client-id"));
        assert!(req_str.contains("Authorization: Bearer test-token"));
        assert!(req_str.contains("duration=20.0"));
    }
}
