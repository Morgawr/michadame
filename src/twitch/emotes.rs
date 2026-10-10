//! Twitch emote parsing and asynchronous image/texture cache.

use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use std::collections::HashMap;
use std::io::Read;
use std::time::Duration;

const FETCH_THREADS: usize = 4;
const MAX_KNOWN_NAMES: usize = 4096;

#[derive(Clone, Debug, PartialEq)]
pub enum Fragment {
    Text(String),
    Emote { id: String, name: String },
}

/// Split a message into text/emote fragments using the IRC `emotes` tag
/// (`id:start-end,start-end/id2:start-end`, positions in Unicode code points).
pub fn split_fragments(text: &str, emotes_tag: Option<&str>) -> Vec<Fragment> {
    let chars: Vec<char> = text.chars().collect();
    let mut ranges: Vec<(usize, usize, &str)> = Vec::new();
    for entry in emotes_tag.unwrap_or_default().split('/') {
        let Some((id, positions)) = entry.split_once(':') else {
            continue;
        };
        if !is_valid_emote_id(id) {
            continue;
        }
        for pos in positions.split(',') {
            let Some((a, b)) = pos.split_once('-') else {
                continue;
            };
            if let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>()) {
                if a <= b && b < chars.len() {
                    ranges.push((a, b, id));
                }
            }
        }
    }
    ranges.sort_by_key(|r| r.0);

    let mut out = Vec::new();
    let mut cursor = 0;
    for (a, b, id) in ranges {
        if a < cursor {
            continue; // overlapping/duplicate range
        }
        if a > cursor {
            out.push(Fragment::Text(chars[cursor..a].iter().collect()));
        }
        out.push(Fragment::Emote {
            id: id.to_string(),
            name: chars[a..=b].iter().collect(),
        });
        cursor = b + 1;
    }
    if cursor < chars.len() {
        out.push(Fragment::Text(chars[cursor..].iter().collect()));
    }
    out
}

/// Best-effort emote detection for our own (locally echoed) messages, which
/// Twitch doesn't send back with emote tags: match words against known names.
pub fn split_by_known_names(text: &str, names: &HashMap<String, String>) -> Vec<Fragment> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            buf.push(' ');
        }
        match names.get(word) {
            Some(id) => {
                if !buf.is_empty() {
                    out.push(Fragment::Text(std::mem::take(&mut buf)));
                }
                out.push(Fragment::Emote {
                    id: id.clone(),
                    name: word.to_string(),
                });
            }
            None => buf.push_str(word),
        }
    }
    if !buf.is_empty() {
        out.push(Fragment::Text(buf));
    }
    out
}

fn is_valid_emote_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

enum EmoteEntry {
    Loading,
    Ready(egui::TextureHandle),
    Failed,
}

type FetchResult = (String, Result<egui::ColorImage, String>);

pub struct EmoteCache {
    entries: HashMap<String, EmoteEntry>,
    /// Emote name -> id, learned from received messages.
    pub names: HashMap<String, String>,
    job_tx: Option<Sender<String>>,
    result_tx: Sender<FetchResult>,
    result_rx: Receiver<FetchResult>,
}

impl Default for EmoteCache {
    fn default() -> Self {
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        Self {
            entries: HashMap::new(),
            names: HashMap::new(),
            job_tx: None,
            result_tx,
            result_rx,
        }
    }
}

impl EmoteCache {
    pub fn learn(&mut self, fragments: &[Fragment]) {
        for f in fragments {
            if let Fragment::Emote { id, name } = f {
                if self.names.len() >= MAX_KNOWN_NAMES && !self.names.contains_key(name) {
                    continue;
                }
                self.names.insert(name.clone(), id.clone());
            }
        }
    }

    pub fn texture(&self, id: &str) -> Option<&egui::TextureHandle> {
        match self.entries.get(id) {
            Some(EmoteEntry::Ready(t)) => Some(t),
            _ => None,
        }
    }

    /// Queue a download if this emote hasn't been requested yet.
    pub fn request(&mut self, id: &str, ctx: &egui::Context) {
        if self.entries.contains_key(id) || !is_valid_emote_id(id) {
            return;
        }
        self.entries.insert(id.to_string(), EmoteEntry::Loading);
        let tx = self.job_tx.get_or_insert_with(|| {
            let (job_tx, job_rx) = crossbeam_channel::unbounded::<String>();
            for i in 0..FETCH_THREADS {
                let job_rx = job_rx.clone();
                let result_tx = self.result_tx.clone();
                let ctx = ctx.clone();
                let _ = std::thread::Builder::new()
                    .name(format!("twitch-emotes-{i}"))
                    .spawn(move || {
                        // Exits when the cache (and its job sender) is dropped.
                        while let Ok(id) = job_rx.recv() {
                            let result = fetch_emote(&id);
                            if result_tx.send((id, result)).is_err() {
                                break;
                            }
                            ctx.request_repaint_of(egui::ViewportId::ROOT);
                        }
                    });
            }
            job_tx
        });
        let _ = tx.send(id.to_string());
    }

    /// Upload finished downloads as textures. Returns true if anything changed.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = false;
        while let Ok((id, result)) = self.result_rx.try_recv() {
            let entry = match result {
                Ok(image) => EmoteEntry::Ready(ctx.load_texture(
                    format!("twitch-emote-{id}"),
                    image,
                    egui::TextureOptions::LINEAR,
                )),
                Err(e) => {
                    tracing::warn!("Failed to load Twitch emote {id}: {e}");
                    EmoteEntry::Failed
                }
            };
            self.entries.insert(id, entry);
            changed = true;
        }
        changed
    }
}

fn fetch_emote(id: &str) -> Result<egui::ColorImage, String> {
    let url = format!("https://static-cdn.jtvnw.net/emoticons/v2/{id}/static/dark/2.0");
    let resp = ureq::get(&url)
        .timeout(Duration::from_secs(15))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Fragment {
        Fragment::Text(s.into())
    }
    fn emote(id: &str, name: &str) -> Fragment {
        Fragment::Emote {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn splits_emotes_by_code_point_ranges() {
        let frags = split_fragments("Kappa hi Kappa PogChamp", Some("25:0-4,9-13/88:15-22"));
        assert_eq!(
            frags,
            vec![
                emote("25", "Kappa"),
                text(" hi "),
                emote("25", "Kappa"),
                text(" "),
                emote("88", "PogChamp"),
            ]
        );
    }

    #[test]
    fn handles_multibyte_text_and_bad_ranges() {
        // Japanese + emoji before the emote: positions are code points, not bytes.
        let frags = split_fragments("こんにちは😀 Kappa", Some("25:7-11"));
        assert_eq!(frags, vec![text("こんにちは😀 "), emote("25", "Kappa")]);

        // Out-of-range, malformed and invalid ids are ignored.
        let frags = split_fragments("abc", Some("25:0-10/x y:0-1/26:a-b"));
        assert_eq!(frags, vec![text("abc")]);
        assert_eq!(split_fragments("plain", None), vec![text("plain")]);
    }

    #[test]
    fn local_echo_matches_known_names() {
        let mut names = HashMap::new();
        names.insert("Kappa".to_string(), "25".to_string());
        assert_eq!(
            split_by_known_names("hello Kappa world", &names),
            vec![text("hello "), emote("25", "Kappa"), text(" world")]
        );
        assert_eq!(split_by_known_names("Kappas", &names), vec![text("Kappas")]);
    }
}
