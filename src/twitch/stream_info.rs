//! Twitch stream information: title and game category editing,
//! autocomplete search with box art thumbnails, follower counts, and genre tags.

use crate::twitch::auth::StoredToken;
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CHANNELS_URL: &str = "https://api.twitch.tv/helix/channels";
const GQL_URL: &str = "https://gql.twitch.tv/gql";
const GQL_CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";

fn read_json(resp: ureq::Response) -> serde_json::Value {
    resp.into_string()
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null)
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct StreamCategory {
    pub id: String,
    pub name: String,
    pub box_art_url: String,
    pub followers: Option<u64>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct StreamInfo {
    pub title: String,
    pub category: Option<StreamCategory>,
}

pub enum InfoEvent {
    ChannelFetched(Result<StreamInfo, String>),
    ChannelUpdated(Result<StreamInfo, String>),
    CategoriesSearched {
        query: String,
        results: Result<Vec<StreamCategory>, String>,
    },
}

enum BoxArtEntry {
    Loading,
    Ready(egui::TextureHandle),
    Failed,
}

pub struct BoxArtCache {
    entries: HashMap<String, BoxArtEntry>,
    job_tx: Option<Sender<String>>,
    result_tx: Sender<(String, Result<egui::ColorImage, String>)>,
    result_rx: Receiver<(String, Result<egui::ColorImage, String>)>,
}

impl Default for BoxArtCache {
    fn default() -> Self {
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        Self {
            entries: HashMap::new(),
            job_tx: None,
            result_tx,
            result_rx,
        }
    }
}

impl BoxArtCache {
    pub fn texture(&self, url: &str) -> Option<&egui::TextureHandle> {
        match self.entries.get(url) {
            Some(BoxArtEntry::Ready(t)) => Some(t),
            _ => None,
        }
    }

    pub fn request(&mut self, url: &str, ctx: &egui::Context) {
        if url.is_empty() || self.entries.contains_key(url) {
            return;
        }
        self.entries.insert(url.to_string(), BoxArtEntry::Loading);

        let tx = self.job_tx.get_or_insert_with(|| {
            let (job_tx, job_rx) = crossbeam_channel::unbounded::<String>();
            for i in 0..2 {
                let job_rx = job_rx.clone();
                let result_tx = self.result_tx.clone();
                let ctx = ctx.clone();
                let _ = std::thread::Builder::new()
                    .name(format!("twitch-boxart-{i}"))
                    .spawn(move || {
                        while let Ok(url) = job_rx.recv() {
                            let res = fetch_image(&url);
                            if result_tx.send((url, res)).is_err() {
                                break;
                            }
                            ctx.request_repaint();
                        }
                    });
            }
            job_tx
        });
        let _ = tx.send(url.to_string());
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = false;
        while let Ok((url, result)) = self.result_rx.try_recv() {
            let entry = match result {
                Ok(image) => {
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut hasher = DefaultHasher::new();
                    url.hash(&mut hasher);
                    let id = hasher.finish();
                    BoxArtEntry::Ready(ctx.load_texture(
                        format!("twitch-boxart-{id:016x}"),
                        image,
                        egui::TextureOptions::LINEAR,
                    ))
                }
                Err(e) => {
                    tracing::warn!("Failed to load box art {url}: {e}");
                    BoxArtEntry::Failed
                }
            };
            self.entries.insert(url, entry);
            changed = true;
        }
        changed
    }
}

fn fetch_image(url: &str) -> Result<egui::ColorImage, String> {
    let formatted_url = if url.contains("{width}x{height}") {
        url.replace("{width}x{height}", "52x72")
    } else {
        url.to_string()
    };
    let resp = ureq::get(&formatted_url)
        .set("User-Agent", "Michadame/1.0 (Linux; x86_64)")
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| e.to_string())?;

    let mut bytes = Vec::new();
    resp.into_reader()
        .take(2 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;

    let img = image::load_from_memory(&bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

pub struct StreamInfoState {
    pub title_input: String,
    pub category: Option<StreamCategory>,
    pub original_title: String,
    pub original_category: Option<StreamCategory>,

    pub search_input: String,
    pub search_results: Vec<StreamCategory>,
    pub search_debounce: Option<Instant>,
    pub last_searched_query: String,

    pub is_searching: bool,
    pub is_saving: bool,
    pub is_loading: bool,

    pub status_message: Option<(bool, String)>,
    pub box_art_cache: BoxArtCache,

    event_tx: Sender<InfoEvent>,
    event_rx: Receiver<InfoEvent>,
    search_task_cancel: Option<Arc<AtomicBool>>,
}

impl Default for StreamInfoState {
    fn default() -> Self {
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        Self {
            title_input: String::new(),
            category: None,
            original_title: String::new(),
            original_category: None,
            search_input: String::new(),
            search_results: Vec::new(),
            search_debounce: None,
            last_searched_query: String::new(),
            is_searching: false,
            is_saving: false,
            is_loading: false,
            status_message: None,
            box_art_cache: BoxArtCache::default(),
            event_tx,
            event_rx,
            search_task_cancel: None,
        }
    }
}

impl StreamInfoState {
    pub fn has_changes(&self) -> bool {
        self.title_input != self.original_title || self.category != self.original_category
    }

    pub fn fetch_channel(&mut self, token: &StoredToken, ctx: &egui::Context) {
        if self.is_loading {
            return;
        }
        self.is_loading = true;
        self.status_message = None;

        let token = token.clone();
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();

        std::thread::Builder::new()
            .name("twitch-channel-fetch".into())
            .spawn(move || {
                let res = fetch_channel_info(&token);
                let _ = tx.send(InfoEvent::ChannelFetched(res));
                ctx.request_repaint();
            })
            .expect("spawn channel fetch thread");
    }

    pub fn save_channel(&mut self, token: &StoredToken, ctx: &egui::Context) {
        if self.is_saving {
            return;
        }
        self.is_saving = true;
        self.status_message = None;

        let token = token.clone();
        let title = self.title_input.clone();
        let category = self.category.clone();
        let tx = self.event_tx.clone();
        let ctx = ctx.clone();

        std::thread::Builder::new()
            .name("twitch-channel-update".into())
            .spawn(move || {
                let res = update_channel_info(&token, &title, category.as_ref());
                let _ = tx.send(InfoEvent::ChannelUpdated(res));
                ctx.request_repaint();
            })
            .expect("spawn channel update thread");
    }

    pub fn queue_search(&mut self, query: String, ctx: &egui::Context) {
        if let Some(cancel) = &self.search_task_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.search_task_cancel = Some(cancel.clone());
        self.is_searching = true;
        self.last_searched_query = query.clone();

        let tx = self.event_tx.clone();
        let ctx = ctx.clone();

        std::thread::Builder::new()
            .name("twitch-category-search".into())
            .spawn(move || {
                let res = search_categories_gql(&query);
                if !cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(InfoEvent::CategoriesSearched {
                        query,
                        results: res,
                    });
                    ctx.request_repaint();
                }
            })
            .expect("spawn category search thread");
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = false;
        changed |= self.box_art_cache.poll(ctx);

        // Handle debounce for search keystrokes
        if let Some(t) = self.search_debounce {
            if t.elapsed() >= Duration::from_millis(250) {
                self.search_debounce = None;
                if self.search_input != self.last_searched_query {
                    self.queue_search(self.search_input.clone(), ctx);
                }
            }
        }

        while let Ok(event) = self.event_rx.try_recv() {
            changed = true;
            match event {
                InfoEvent::ChannelFetched(Ok(info)) => {
                    self.title_input = info.title.clone();
                    self.original_title = info.title;
                    self.category = info.category.clone();
                    self.original_category = info.category;
                    self.is_loading = false;
                }
                InfoEvent::ChannelFetched(Err(e)) => {
                    self.status_message = Some((true, e));
                    self.is_loading = false;
                }
                InfoEvent::ChannelUpdated(Ok(info)) => {
                    self.original_title = info.title.clone();
                    self.original_category = info.category.clone();
                    self.status_message =
                        Some((false, "Stream info updated successfully!".into()));
                    self.is_saving = false;
                }
                InfoEvent::ChannelUpdated(Err(e)) => {
                    self.status_message = Some((true, e));
                    self.is_saving = false;
                }
                InfoEvent::CategoriesSearched { query, results } => {
                    if query == self.search_input {
                        self.search_results = results.unwrap_or_default();
                        self.is_searching = false;
                    }
                }
            }
        }

        changed
    }
}

pub fn search_categories_gql(query: &str) -> Result<Vec<StreamCategory>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let query_esc = q.replace('\\', "\\\\").replace('"', "\\\"");
    let gql_query = format!(
        r#"query CategorySearch {{
            searchCategories(query: "{}") {{
                edges {{
                    node {{
                        id
                        name
                        boxArtURL(width: 52, height: 72)
                        followersCount
                        tags(tagType: CONTENT) {{
                            id
                            localizedName
                        }}
                    }}
                }}
            }}
        }}"#,
        query_esc
    );
    let payload = serde_json::json!([{
        "operationName": "CategorySearch",
        "query": gql_query,
    }]);

    let body_str = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let resp = ureq::post(GQL_URL)
        .timeout(Duration::from_secs(10))
        .set("Client-Id", GQL_CLIENT_ID)
        .set("Content-Type", "application/json")
        .send_string(&body_str)
        .map_err(|e| e.to_string())?;

    let json_val = read_json(resp);
    let arr = json_val.as_array().ok_or("Invalid response array")?;
    let first = arr.first().ok_or("Empty response")?;

    if let Some(errors) = first.get("errors") {
        return Err(format!("GQL error: {}", errors));
    }

    let edges = first
        .pointer("/data/searchCategories/edges")
        .and_then(|v| v.as_array())
        .ok_or("No searchCategories edges in response")?;

    let mut results = Vec::new();
    for edge in edges {
        let Some(node) = edge.get("node") else { continue };
        let id = node
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let name = node
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let box_art_url = node
            .get("boxArtURL")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let followers = node.get("followersCount").and_then(|v| v.as_u64());
        let tags: Vec<String> = node
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.get("localizedName").and_then(|n| n.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        if !id.is_empty() && !name.is_empty() {
            results.push(StreamCategory {
                id,
                name,
                box_art_url,
                followers,
                tags,
            });
        }
    }
    Ok(results)
}

pub fn fetch_category_by_id_gql(id: &str) -> Result<StreamCategory, String> {
    let id_esc = id.replace('\\', "\\\\").replace('"', "\\\"");
    let gql_query = format!(
        r#"query GetGame {{
            game(id: "{}") {{
                id
                name
                boxArtURL(width: 52, height: 72)
                followersCount
                tags(tagType: CONTENT) {{
                    id
                    localizedName
                }}
            }}
        }}"#,
        id_esc
    );
    let payload = serde_json::json!([{
        "operationName": "GetGame",
        "query": gql_query,
    }]);

    let body_str = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let resp = ureq::post(GQL_URL)
        .timeout(Duration::from_secs(10))
        .set("Client-Id", GQL_CLIENT_ID)
        .set("Content-Type", "application/json")
        .send_string(&body_str)
        .map_err(|e| e.to_string())?;

    let json_val = read_json(resp);
    let arr = json_val.as_array().ok_or("Invalid response array")?;
    let first = arr.first().ok_or("Empty response")?;

    let game = first
        .pointer("/data/game")
        .ok_or("Game not found in GQL")?;
    let id = game
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name = game
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let box_art_url = game
        .get("boxArtURL")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let followers = game.get("followersCount").and_then(|v| v.as_u64());
    let tags: Vec<String> = game
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.get("localizedName").and_then(|n| n.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    Ok(StreamCategory {
        id,
        name,
        box_art_url,
        followers,
        tags,
    })
}

fn call_get_channels(token: &StoredToken) -> Result<ureq::Response, ureq::Error> {
    ureq::get(CHANNELS_URL)
        .query("broadcaster_id", &token.user_id)
        .set("Client-Id", token.effective_client_id())
        .set("Authorization", &format!("Bearer {}", token.access_token))
        .timeout(Duration::from_secs(12))
        .call()
}

fn call_patch_channels(token: &StoredToken, body_str: &str) -> Result<ureq::Response, ureq::Error> {
    ureq::patch(CHANNELS_URL)
        .query("broadcaster_id", &token.user_id)
        .set("Client-Id", token.effective_client_id())
        .set("Authorization", &format!("Bearer {}", token.access_token))
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(15))
        .send_string(body_str)
}

pub fn fetch_channel_info(token: &StoredToken) -> Result<StreamInfo, String> {
    let mut active_token = match crate::twitch::auth::ensure_valid(token) {
        Ok(Some(refreshed)) => refreshed,
        Ok(None) => token.clone(),
        Err(crate::twitch::auth::AuthError::Invalid(e)) => {
            return Err(format!("Twitch login expired ({e}). Please log in again."));
        }
        Err(crate::twitch::auth::AuthError::Network(_)) => token.clone(),
    };
    if active_token.user_id.is_empty() {
        return Err("User ID is missing from login token".into());
    }
    let resp = match call_get_channels(&active_token) {
        Ok(resp) => resp,
        Err(ureq::Error::Status(401, _)) => {
            if let Ok(refreshed) = crate::twitch::auth::force_refresh(&active_token) {
                active_token = refreshed;
                call_get_channels(&active_token)
                    .map_err(|e| format!("Failed to fetch channel info: {e}"))?
            } else {
                return Err("Twitch login expired or unauthorized (401)".into());
            }
        }
        Err(e) => return Err(format!("Failed to fetch channel info: {e}")),
    };

    let body = read_json(resp);
    let data = body
        .get("data")
        .and_then(|d| d.as_array())
        .and_then(|arr| arr.first())
        .ok_or("Empty channel data from Twitch")?;

    let title = data
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let game_id = data
        .get("game_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let game_name = data
        .get("game_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let category = if !game_id.is_empty() {
        match fetch_category_by_id_gql(&game_id) {
            Ok(cat) => Some(cat),
            Err(_) => Some(StreamCategory {
                id: game_id.clone(),
                name: game_name,
                box_art_url: format!(
                    "https://static-cdn.jtvnw.net/ttv-boxart/{}-52x72.jpg",
                    game_id
                ),
                followers: None,
                tags: Vec::new(),
            }),
        }
    } else {
        None
    };

    Ok(StreamInfo { title, category })
}

pub fn update_channel_info(
    token: &StoredToken,
    title: &str,
    category: Option<&StreamCategory>,
) -> Result<StreamInfo, String> {
    let mut active_token = match crate::twitch::auth::ensure_valid(token) {
        Ok(Some(refreshed)) => refreshed,
        Ok(None) => token.clone(),
        Err(crate::twitch::auth::AuthError::Invalid(e)) => {
            return Err(format!("Twitch login expired ({e}). Please log in again."));
        }
        Err(crate::twitch::auth::AuthError::Network(_)) => token.clone(),
    };
    if active_token.user_id.is_empty() {
        return Err("User ID is missing from login token".into());
    }
    let game_id = category.map(|c| c.id.as_str()).unwrap_or("");
    let payload = serde_json::json!({
        "title": title,
        "game_id": game_id,
    });
    let body_str = serde_json::to_string(&payload).map_err(|e| e.to_string())?;

    let resp_res = match call_patch_channels(&active_token, &body_str) {
        Err(ureq::Error::Status(401, resp)) => {
            let msg = resp.into_string().unwrap_or_default();
            if msg.contains("channel:manage:broadcast")
                || msg.contains("user:edit:broadcast")
                || msg.contains("Missing scope")
            {
                return Err("Missing permission: your login lacks channel:manage:broadcast. Please Log Out and Log In again under 'Twitch Account' to grant access.".into());
            }
            if let Ok(refreshed) = crate::twitch::auth::force_refresh(&active_token) {
                active_token = refreshed;
                call_patch_channels(&active_token, &body_str)
            } else {
                return Err(format!("Twitch unauthorized (401): {msg}"));
            }
        }
        other => other,
    };

    match resp_res {
        Ok(_) => Ok(StreamInfo {
            title: title.to_string(),
            category: category.cloned(),
        }),
        Err(ureq::Error::Status(401, resp)) => {
            let msg = resp.into_string().unwrap_or_default();
            if msg.contains("channel:manage:broadcast")
                || msg.contains("user:edit:broadcast")
                || msg.contains("Missing scope")
            {
                Err("Missing permission: your login lacks channel:manage:broadcast. Please Log Out and Log In again under 'Twitch Account' to grant access.".into())
            } else {
                Err(format!("Twitch unauthorized (401): {msg}"))
            }
        }
        Err(ureq::Error::Status(code, resp)) => {
            let msg = resp.into_string().unwrap_or_default();
            Err(format!("Twitch API error {code}: {msg}"))
        }
        Err(e) => Err(format!("Network error: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_category_model() {
        let cat = StreamCategory {
            id: "1471096082".into(),
            name: "Nijuuei".into(),
            box_art_url: "https://static-cdn.jtvnw.net/ttv-boxart/1471096082-52x72.jpg".into(),
            followers: Some(1),
            tags: vec!["Adventure Game".into(), "Visual Novel".into()],
        };
        assert_eq!(cat.name, "Nijuuei");
        assert_eq!(cat.followers, Some(1));
        assert_eq!(cat.tags.len(), 2);
    }

    #[test]
    fn test_stream_info_has_changes() {
        let mut state = StreamInfoState::default();
        state.original_title = "Original Title".into();
        state.title_input = "Original Title".into();
        assert!(!state.has_changes());

        state.title_input = "New Title".into();
        assert!(state.has_changes());
    }
}
