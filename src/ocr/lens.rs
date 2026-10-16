use super::models::*;
use prost::Message;
use rand::RngCore;
use std::io::Read;
use std::time::Duration;

const LENS_ENDPOINT: &str = "https://lensfrontend-pa.googleapis.com/v1/crupload";
const LENS_API_KEY: &str = "AIzaSyDr2UxVnv_U85AbhhY8XSHSIavUW0DC-sY";
const CHROME_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

pub fn create_lens_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(15))
        .timeout_write(Duration::from_secs(15))
        .max_idle_connections(5)
        .max_idle_connections_per_host(2)
        .build()
}

pub fn ping_warmup(agent: &ureq::Agent) {
    match agent
        .request("HEAD", LENS_ENDPOINT)
        .set("User-Agent", CHROME_USER_AGENT)
        .call()
    {
        Ok(resp) => {
            let _ = resp.into_string();
        }
        Err(ureq::Error::Status(_, resp)) => {
            let _ = resp.into_string();
        }
        Err(_) => {}
    }
}

pub fn encode_jpeg_for_ocr(
    rgb: &[u8],
    width: u32,
    height: u32,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    use image::ImageEncoder;
    out.clear();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut *out, 80);
    encoder
        .write_image(rgb, width, height, image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("Failed to encode JPEG for OCR: {e}"))
}

pub fn execute_lens_ocr_with_agent(
    agent: &ureq::Agent,
    image_bytes: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<Vec<ParsedLine>, String> {
    if image_bytes.is_empty() || width == 0 || height == 0 {
        return Err("Invalid image data provided for OCR".to_string());
    }

    let mut rng = rand::thread_rng();
    let mut analytics_id = [0u8; 16];
    rng.fill_bytes(&mut analytics_id);

    let request = LensOverlayServerRequest {
        objects_request: Some(LensOverlayObjectsRequest {
            request_context: Some(LensOverlayRequestContext {
                request_id: Some(LensOverlayRequestId {
                    uuid: rng.next_u64(),
                    sequence_id: 0,
                    image_sequence_id: 0,
                    analytics_id: analytics_id.to_vec(),
                }),
                client_context: Some(LensOverlayClientContext {
                    platform: 3, // PLATFORM_WEB
                    surface: 4,  // SURFACE_CHROMIUM
                    locale_context: Some(LocaleContext {
                        language: "ja".to_string(),
                        region: "JP".to_string(),
                        time_zone: "Asia/Tokyo".to_string(),
                    }),
                    app_id: String::new(),
                    client_filters: Some(AppliedFilters {
                        filter: vec![AppliedFilter {
                            filter_type: 7, // AUTO_FILTER
                        }],
                    }),
                }),
            }),
            image_data: Some(ImageData {
                payload: Some(ImagePayload {
                    image_bytes,
                }),
                image_metadata: Some(ImageMetadata {
                    width: width as i32,
                    height: height as i32,
                }),
            }),
        }),
    };

    let mut payload = Vec::new();
    request
        .encode(&mut payload)
        .map_err(|e| format!("Failed to encode Lens protobuf request: {}", e))?;

    let response = agent
        .post(LENS_ENDPOINT)
        .set("Content-Type", "application/x-protobuf")
        .set("X-Goog-Api-Key", LENS_API_KEY)
        .set("Sec-Fetch-Mode", "no-cors")
        .set("Sec-Fetch-Dest", "empty")
        .set("User-Agent", CHROME_USER_AGENT)
        .timeout(Duration::from_secs(20))
        .send_bytes(&payload)
        .map_err(|e| format!("Google Lens request failed: {}", e))?;

    if response.status() != 200 {
        return Err(format!(
            "Google Lens returned non-200 HTTP status: {}",
            response.status()
        ));
    }

    let mut resp_bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut resp_bytes)
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    if resp_bytes.is_empty() {
        return Ok(Vec::new());
    }

    let parsed = LensOverlayServerResponse::decode(&resp_bytes[..])
        .map_err(|e| format!("Failed to decode Lens protobuf response: {}", e))?;

    let mut parsed_lines = Vec::new();
    if let Some(objects_resp) = parsed.objects_response {
        if let Some(text_obj) = objects_resp.text {
            if let Some(layout) = text_obj.text_layout {
                for (p_idx, paragraph) in layout.paragraphs.iter().enumerate() {
                    for line in &paragraph.lines {
                        // Try to get bounding box from line geometry
                        let mut resolved_box = line
                            .geometry
                            .as_ref()
                            .and_then(|g| g.bounding_box.as_ref())
                            .map(|b| (b.center_x, b.center_y, b.width, b.height, b.rotation_z));

                        // If line has no bounding box, compute union from words
                        if resolved_box.is_none() {
                            let mut min_x = f32::MAX;
                            let mut min_y = f32::MAX;
                            let mut max_x = f32::MIN;
                            let mut max_y = f32::MIN;
                            let mut found_word_box = false;
                            let mut avg_rotation = 0.0f32;
                            let mut rot_count = 0;

                            for word in &line.words {
                                if let Some(geom) = &word.geometry {
                                    if let Some(bbox) = &geom.bounding_box {
                                        found_word_box = true;
                                        let w_min_x = bbox.center_x - bbox.width / 2.0;
                                        let w_max_x = bbox.center_x + bbox.width / 2.0;
                                        let w_min_y = bbox.center_y - bbox.height / 2.0;
                                        let w_max_y = bbox.center_y + bbox.height / 2.0;

                                        min_x = min_x.min(w_min_x);
                                        min_y = min_y.min(w_min_y);
                                        max_x = max_x.max(w_max_x);
                                        max_y = max_y.max(w_max_y);
                                        avg_rotation += bbox.rotation_z;
                                        rot_count += 1;
                                    }
                                }
                            }

                            if found_word_box && max_x > min_x && max_y > min_y {
                                let w = max_x - min_x;
                                let h = max_y - min_y;
                                let rot = if rot_count > 0 {
                                    avg_rotation / rot_count as f32
                                } else {
                                    0.0
                                };
                                resolved_box = Some((min_x + w / 2.0, min_y + h / 2.0, w, h, rot));
                            }
                        }

                        // If still no box, fall back to paragraph geometry
                        if resolved_box.is_none() {
                            resolved_box = paragraph
                                .geometry
                                .as_ref()
                                .and_then(|g| g.bounding_box.as_ref())
                                .map(|b| (b.center_x, b.center_y, b.width, b.height, b.rotation_z));
                        }

                        if let Some((cx, cy, raw_w, raw_h, rot)) = resolved_box {
                            let p_dir = WritingDirection::from_i32(paragraph.writing_direction);
                            let r = rot.abs();
                            let is_rotated_vert = r > 0.785 && r < 2.356;
                            let (w, h) = if is_rotated_vert && raw_w > raw_h {
                                (raw_h, raw_w)
                            } else {
                                (raw_w, raw_h)
                            };

                            let raw_char_count: usize =
                                line.words.iter().map(|w| w.plain_text.chars().count()).sum();
                            let writing_direction = if p_dir.is_vertical() {
                                WritingDirection::TopToBottom
                            } else if is_rotated_vert || (h > 1.8 * w && raw_char_count >= 2) {
                                WritingDirection::TopToBottom
                            } else {
                                p_dir
                            };

                            let is_vertical = writing_direction.is_vertical();
                            let cx_clamped = cx.clamp(0.0, 1.0);
                            let cy_clamped = cy.clamp(0.0, 1.0);
                            let w_clamped = w.clamp(0.001, 1.0);
                            let h_clamped = h.clamp(0.001, 1.0);

                            let char_em = estimate_line_char_em(
                                &line.words,
                                w_clamped,
                                h_clamped,
                                width,
                                height,
                                is_vertical,
                                is_rotated_vert,
                            );

                            let (line_min, line_max) = if is_vertical {
                                (
                                    (cy_clamped - h_clamped * 0.5).clamp(0.0, 1.0),
                                    (cy_clamped + h_clamped * 0.5).clamp(0.0, 1.0),
                                )
                            } else {
                                (
                                    (cx_clamped - w_clamped * 0.5).clamp(0.0, 1.0),
                                    (cx_clamped + w_clamped * 0.5).clamp(0.0, 1.0),
                                )
                            };

                            let (line_text, char_spans) = build_line_text_and_spans(
                                &line.words,
                                line_min,
                                line_max,
                                char_em,
                                is_vertical,
                                is_rotated_vert,
                            );

                            if line_text.is_empty() {
                                continue;
                            }

                            parsed_lines.push(ParsedLine {
                                text: line_text,
                                center_x: cx_clamped,
                                center_y: cy_clamped,
                                width: w_clamped,
                                height: h_clamped,
                                paragraph_idx: p_idx,
                                writing_direction,
                                char_spans,
                            });
                        }
                    }
                }
            }
        }
    }

    Ok(parsed_lines)
}

/// Checks if a character is a fullwidth CJK ideograph or kana (excluding punctuation/symbols).
fn is_cjk_ideograph_or_kana(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A1}'..='\u{30FA}' | '\u{30FD}'..='\u{30FF}' // Katakana (excluding ・ and ー)
        | '\u{31F0}'..='\u{31FF}' // Katakana Phonetic Extensions
        | '\u{3400}'..='\u{4DBF}' // CJK Unified Ideographs Extension A
        | '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs
    )
}

/// Returns the natural relative width/span weight of a character when subdividing a multi-character
/// word bounding box into individual character spans.
pub fn char_natural_weight(c: char) -> f32 {
    match c {
        '、' | '。' | '，' | '．' | '「' | '」' | '『' | '』' | '（' | '）'
        | '【' | '】' | '〈' | '〉' | '《' | '》' | '〔' | '〕' | '［' | '］'
        | '｛' | '｝' | '・' | '“' | '”' | '‘' | '’' => 0.4,
        '.' | ',' | ':' | ';' | '\'' | '"' | '!' | '|' | '/' | '\\'
        | '(' | ')' | '[' | ']' | '{' | '}' => 0.35,
        '\u{FF61}'..='\u{FF9F}' => 0.5,
        c if c.is_ascii() => 0.55,
        _ => 1.0,
    }
}

/// Subdivides `[min_pos, max_pos]` across `text.chars()` weighted by [`char_natural_weight`].
pub fn subdivide_span_weighted(text: &str, min_pos: f32, max_pos: f32) -> Vec<(f32, f32)> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let span = (max_pos - min_pos).max(0.0);
    let weights: Vec<f32> = chars.iter().map(|&c| char_natural_weight(c)).collect();
    let total_w: f32 = weights.iter().sum::<f32>().max(1e-6);
    let mut out = Vec::with_capacity(chars.len());
    let mut cur = min_pos;
    for (i, w) in weights.iter().enumerate() {
        let next = if i + 1 == chars.len() {
            max_pos.max(cur)
        } else {
            cur + span * (w / total_w)
        };
        out.push((cur, next));
        cur = next;
    }
    out
}

fn is_dot_leader_char(c: char) -> bool {
    matches!(c, '…' | '‥' | '⋮' | '︙' | '･' | '・' | '.' | '．' | '·')
}

/// Normalizes OCR dot-leader / ellipsis transcriptions (e.g. `"…………"`, `"………………"`, `"･･････"`,
/// `"・・・・・・"`) so that the number of `'…'` characters matches the underlying glyph grid width
/// (`word_span / char_em`).
///
/// In Japanese game fonts, a 6-dot ellipsis (`……`) occupies 2 fullwidth character cells (3 dots per
/// cell), but OCR models frequently transcribe those 6 dots as 4–6 `'…'` characters (12–18 dots)
/// or 6–7 halfwidth/fullwidth middle dots (`'･'` / `'・'`).
pub fn normalize_dot_leader_word(plain_text: &str, word_span: Option<f32>, char_em: f32) -> String {
    if plain_text.is_empty() || !plain_text.chars().any(is_dot_leader_char) {
        return plain_text.to_string();
    }

    let chars: Vec<char> = plain_text.chars().collect();
    let mut out = String::with_capacity(plain_text.len());
    let mut i = 0;

    while i < chars.len() {
        if !is_dot_leader_char(chars[i]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }

        let start = i;
        while i < chars.len() && is_dot_leader_char(chars[i]) {
            i += 1;
        }
        let run = &chars[start..i];
        let run_len = run.len();
        let has_ellipsis = run.iter().any(|&c| matches!(c, '…' | '‥' | '⋮' | '︙'));
        let has_hw_dot = run.iter().any(|&c| c == '･');

        // Estimate the physical span occupied by this dot-leader run
        let non_run_weight: f32 = chars[..start]
            .iter()
            .chain(chars[i..].iter())
            .map(|&c| char_natural_weight(c))
            .sum();
        let run_span = word_span.and_then(|ws| {
            if ws > 0.0 && char_em > 0.0 {
                Some((ws - non_run_weight * char_em).max(char_em * 0.5))
            } else {
                None
            }
        });

        if let Some(r_span) = run_span {
            let is_packed_dots = run_len >= 3 && (r_span / run_len as f32) < 0.65 * char_em;
            if has_ellipsis || (has_hw_dot && run_len >= 2) || is_packed_dots {
                let cells = (r_span / char_em).round().clamp(1.0, 16.0) as usize;
                for _ in 0..cells {
                    out.push('…');
                }
                continue;
            }
        } else if has_hw_dot && run_len >= 2 {
            let cells = (run_len as f32 / 3.0).round().clamp(1.0, 16.0) as usize;
            for _ in 0..cells {
                out.push('…');
            }
            continue;
        } else if has_ellipsis && run_len >= 4 {
            let cells = (run_len / 2).clamp(1, 16);
            for _ in 0..cells {
                out.push('…');
            }
            continue;
        }

        for &c in run {
            out.push(c);
        }
    }

    out
}

/// Estimates the normalized size of 1 fullwidth CJK character (`1 em`) along the line's reading axis.
fn estimate_line_char_em(
    words: &[TextLayoutWord],
    line_w: f32,
    line_h: f32,
    img_w: u32,
    img_h: u32,
    is_vertical: bool,
    is_rotated_vert: bool,
) -> f32 {
    let mut cjk_span_sum = 0.0f32;
    let mut cjk_char_sum = 0usize;

    for w in words {
        if w.plain_text.is_empty() || !w.plain_text.chars().all(is_cjk_ideograph_or_kana) {
            continue;
        }
        if let Some(bbox) = w.geometry.as_ref().and_then(|g| g.bounding_box.as_ref()) {
            let (bw, bh) = if is_rotated_vert && bbox.width > bbox.height {
                (bbox.height, bbox.width)
            } else {
                (bbox.width, bbox.height)
            };
            let span = if is_vertical { bh } else { bw };
            if span > 0.0 {
                cjk_span_sum += span;
                cjk_char_sum += w.plain_text.chars().count();
            }
        }
    }

    if cjk_char_sum > 0 {
        return cjk_span_sum / cjk_char_sum as f32;
    }

    let aspect = if is_vertical {
        img_w.max(1) as f32 / img_h.max(1) as f32
    } else {
        img_h.max(1) as f32 / img_w.max(1) as f32
    };
    let cross = if is_vertical { line_w } else { line_h };
    (cross * aspect * 0.95).max(0.001)
}

/// Builds the line text and per-character normalized `[start, end]` spans from Lens word boxes.
fn build_line_text_and_spans(
    words: &[TextLayoutWord],
    line_min: f32,
    line_max: f32,
    char_em: f32,
    is_vertical: bool,
    is_rotated_vert: bool,
) -> (String, Vec<(f32, f32)>) {
    struct WordPiece<'a> {
        text: String,
        sep: &'a str,
        span: Option<(f32, f32)>,
    }

    let mut pieces = Vec::with_capacity(words.len());
    let mut all_have_boxes = true;

    for w in words {
        let span = w
            .geometry
            .as_ref()
            .and_then(|g| g.bounding_box.as_ref())
            .map(|b| {
                let (bw, bh) = if is_rotated_vert && b.width > b.height {
                    (b.height, b.width)
                } else {
                    (b.width, b.height)
                };
                if is_vertical {
                    (
                        (b.center_y - bh * 0.5).clamp(0.0, 1.0),
                        (b.center_y + bh * 0.5).clamp(0.0, 1.0),
                    )
                } else {
                    (
                        (b.center_x - bw * 0.5).clamp(0.0, 1.0),
                        (b.center_x + bw * 0.5).clamp(0.0, 1.0),
                    )
                }
            });

        let span_len = span.map(|(a, b)| (b - a).max(0.0));
        let norm_text = normalize_dot_leader_word(&w.plain_text, span_len, char_em);
        if !norm_text.is_empty() && span.is_none() {
            all_have_boxes = false;
        }
        pieces.push(WordPiece {
            text: norm_text,
            sep: w.text_separator.as_deref().unwrap_or(""),
            span,
        });
    }

    if all_have_boxes && !pieces.is_empty() {
        let mut char_items: Vec<(char, (f32, f32))> = Vec::new();
        for (idx, piece) in pieces.iter().enumerate() {
            if let Some((w_min, w_max)) = piece.span {
                if !piece.text.is_empty() {
                    let sub = subdivide_span_weighted(&piece.text, w_min, w_max);
                    for (c, s) in piece.text.chars().zip(sub) {
                        char_items.push((c, s));
                    }
                }
                if !piece.sep.is_empty() {
                    let next_min = pieces[idx + 1..]
                        .iter()
                        .find_map(|p| p.span.map(|s| s.0))
                        .unwrap_or(w_max)
                        .max(w_max);
                    let sub = subdivide_span_weighted(piece.sep, w_max, next_min);
                    for (c, s) in piece.sep.chars().zip(sub) {
                        char_items.push((c, s));
                    }
                }
            }
        }

        let start_trim = char_items
            .iter()
            .position(|(c, _)| !c.is_whitespace())
            .unwrap_or(char_items.len());
        let end_trim = char_items
            .iter()
            .rposition(|(c, _)| !c.is_whitespace())
            .map(|p| p + 1)
            .unwrap_or(start_trim);

        let trimmed = &char_items[start_trim..end_trim];
        let text: String = trimmed.iter().map(|(c, _)| *c).collect();
        let spans: Vec<(f32, f32)> = trimmed.iter().map(|(_, s)| *s).collect();
        return (text, spans);
    }

    let line_text: String = pieces
        .iter()
        .map(|p| format!("{}{}", p.text, p.sep))
        .collect::<String>()
        .trim()
        .to_string();
    let spans = subdivide_span_weighted(&line_text, line_min, line_max);
    (line_text, spans)
}

#[cfg(test)]
pub fn execute_lens_ocr_and_group(
    png_bytes: Vec<u8>,
    width: u32,
    height: u32,
    sticky_distance: f32,
) -> Result<Vec<OcrBox>, String> {
    let lines = execute_lens_ocr_with_agent(&ureq::agent(), png_bytes, width, height)?;
    Ok(group_lines_into_blocks(&lines, sticky_distance))
}

/// Checks whether a character belongs to CJK character sets or Japanese punctuation.
pub fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs (Kanji)
        | '\u{3400}'..='\u{4DBF}' // CJK Unified Ideographs Extension A
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation (、, 。, 「, 」, etc.)
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth and Fullwidth Forms
        | '\u{31F0}'..='\u{31FF}' // Katakana Phonetic Extensions
        | '…' | '‥' | '―' | '—' | '“' | '”' | '‘' | '’'
    )
}

/// Joins multi-line text blocks cleanly, omitting whitespace between CJK characters
/// while preserving whitespace between Latin words.
pub fn join_lines_text(lines: &[&str]) -> String {
    let mut result = String::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if result.is_empty() {
            result.push_str(trimmed);
        } else {
            let prev_last_char = result.chars().last().unwrap();
            let next_first_char = trimmed.chars().next().unwrap();
            let prev_is_cjk = is_cjk(prev_last_char);
            let next_is_cjk = is_cjk(next_first_char);

            // Add space only if neither side is CJK and previous character is not a hyphen
            if !prev_is_cjk && !next_is_cjk && prev_last_char != '-' {
                result.push(' ');
            }
            result.push_str(trimmed);
        }
    }
    result
}

/// Merges horizontally adjacent `ParsedLine` fragments on the same row into unified lines,
/// inserting appropriate whitespace to preserve character offsets and layout.
pub fn merge_row_lines(lines: &[ParsedLine], sticky_distance: f32) -> Vec<ParsedLine> {
    if lines.is_empty() || sticky_distance <= 0.0 {
        return lines.to_vec();
    }

    // Sort lines primarily by center_y, secondarily by center_x
    let mut sorted = lines.to_vec();
    sorted.sort_by(|a, b| {
        a.center_y
            .partial_cmp(&b.center_y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.center_x
                    .partial_cmp(&b.center_x)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let mut merged: Vec<ParsedLine> = Vec::new();

    for line in sorted {
        let line_top = line.center_y - line.height / 2.0;
        let line_bottom = line.center_y + line.height / 2.0;
        let line_left = line.center_x - line.width / 2.0;

        let mut matched_idx = None;
        for (i, prev) in merged.iter().enumerate().rev() {
            let prev_top = prev.center_y - prev.height / 2.0;
            let prev_bottom = prev.center_y + prev.height / 2.0;
            let prev_left = prev.center_x - prev.width / 2.0;

            let min_h = prev.height.min(line.height);
            let height_ratio = line.height / prev.height;
            let is_similar_height = (0.65..=1.55).contains(&height_ratio);

            // Vertical overlap: must be on the same horizontal row
            let overlap_y = prev_bottom.min(line_bottom) - prev_top.max(line_top);
            let is_same_row = overlap_y >= 0.5 * min_h
                || (prev.center_y - line.center_y).abs() <= 0.35 * min_h;

            if !is_similar_height || !is_same_row {
                continue;
            }

            // Horizontal relationship: determine left vs right fragment
            let (left_box, right_box, is_prev_left) = if prev_left <= line_left {
                (prev, &line, true)
            } else {
                (&line, prev, false)
            };

            let left_r = left_box.center_x + left_box.width / 2.0;
            let right_l = right_box.center_x - right_box.width / 2.0;

            // Check that right_box doesn't start way before left_box ends (no massive overlap)
            if right_l < left_r - 0.25 * min_h {
                continue;
            }

            let char_cnt_l = left_box.text.chars().count().max(1);
            let char_cnt_r = right_box.text.chars().count().max(1);
            let avg_char_w = (left_box.width + right_box.width) / (char_cnt_l + char_cnt_r) as f32;

            let gap = right_l - left_r;
            // Max allowed gap on the same row: up to ~15 character widths or 0.6 of screen
            let max_gap = ((sticky_distance * 15.0).max(8.0) * avg_char_w).min(0.6);

            if gap <= max_gap {
                matched_idx = Some((i, is_prev_left, gap, avg_char_w));
                break;
            }
        }

        if let Some((idx, is_prev_left, gap, avg_char_w)) = matched_idx {
            let prev = &merged[idx];
            let (left_box, right_box) = if is_prev_left {
                (prev, &line)
            } else {
                (&line, prev)
            };

            let space_count = if gap >= 0.75 * avg_char_w {
                (gap / avg_char_w).round().max(1.0) as usize
            } else {
                let left_last = left_box.text.chars().last();
                let right_first = right_box.text.chars().next();
                let need_space = match (left_last, right_first) {
                    (Some(c1), Some(c2)) => !is_cjk(c1) && !is_cjk(c2) && c1 != '-',
                    _ => false,
                };
                if need_space { 1 } else { 0 }
            };

            let l_trimmed = left_box.text.trim_end();
            let r_trimmed = right_box.text.trim_start();
            let new_text = format!("{}{}{}", l_trimmed, " ".repeat(space_count), r_trimmed);

            let l_min_x = left_box.center_x - left_box.width / 2.0;
            let r_max_x = right_box.center_x + right_box.width / 2.0;
            let new_min_x = l_min_x;
            let new_max_x = r_max_x;
            let new_width = (new_max_x - new_min_x).clamp(0.001, 1.0);
            let new_center_x = (new_min_x + new_max_x) / 2.0;

            let top_a = left_box.center_y - left_box.height / 2.0;
            let top_b = right_box.center_y - right_box.height / 2.0;
            let bot_a = left_box.center_y + left_box.height / 2.0;
            let bot_b = right_box.center_y + right_box.height / 2.0;
            let new_min_y = top_a.min(top_b);
            let new_max_y = bot_a.max(bot_b);
            let new_height = (new_max_y - new_min_y).clamp(0.001, 1.0);
            let new_center_y = (new_min_y + new_max_y) / 2.0;

            let new_p_idx = left_box.paragraph_idx.min(right_box.paragraph_idx);

            let new_dir = left_box.writing_direction;
            let new_char_spans = merge_line_char_spans(
                left_box,
                right_box,
                l_trimmed,
                r_trimmed,
                space_count,
                false,
            );

            merged[idx] = ParsedLine {
                text: new_text,
                center_x: new_center_x,
                center_y: new_center_y,
                width: new_width,
                height: new_height,
                paragraph_idx: new_p_idx,
                writing_direction: new_dir,
                char_spans: new_char_spans,
            };
        } else {
            merged.push(line);
        }
    }

    merged
}

fn line_effective_spans(line: &ParsedLine, is_vertical: bool) -> Vec<(f32, f32)> {
    let char_count = line.text.chars().count();
    if line.char_spans.len() == char_count {
        return line.char_spans.clone();
    }
    let (min_pos, max_pos) = if is_vertical {
        (
            (line.center_y - line.height * 0.5).clamp(0.0, 1.0),
            (line.center_y + line.height * 0.5).clamp(0.0, 1.0),
        )
    } else {
        (
            (line.center_x - line.width * 0.5).clamp(0.0, 1.0),
            (line.center_x + line.width * 0.5).clamp(0.0, 1.0),
        )
    };
    if char_count == 0 {
        return Vec::new();
    }
    let step = (max_pos - min_pos).max(0.0) / char_count as f32;
    (0..char_count)
        .map(|i| (min_pos + i as f32 * step, min_pos + (i + 1) as f32 * step))
        .collect()
}

fn merge_line_char_spans(
    first_box: &ParsedLine,
    second_box: &ParsedLine,
    first_trimmed: &str,
    second_trimmed: &str,
    space_count: usize,
    is_vertical: bool,
) -> Vec<(f32, f32)> {
    let first_spans = line_effective_spans(first_box, is_vertical);
    let second_spans = line_effective_spans(second_box, is_vertical);

    let first_keep = first_trimmed.chars().count().min(first_spans.len());
    let second_total = second_box.text.chars().count();
    let second_keep = second_trimmed.chars().count().min(second_spans.len());
    let second_skip = second_total.saturating_sub(second_keep).min(second_spans.len());

    let mut out = Vec::with_capacity(first_keep + space_count + second_keep);
    out.extend_from_slice(&first_spans[..first_keep]);

    if space_count > 0 {
        let gap_start = out.last().map(|s| s.1).unwrap_or_else(|| {
            if is_vertical {
                first_box.center_y + first_box.height * 0.5
            } else {
                first_box.center_x + first_box.width * 0.5
            }
        });
        let gap_end = second_spans
            .get(second_skip)
            .map(|s| s.0)
            .unwrap_or_else(|| {
                if is_vertical {
                    second_box.center_y - second_box.height * 0.5
                } else {
                    second_box.center_x - second_box.width * 0.5
                }
            })
            .max(gap_start);
        let step = (gap_end - gap_start) / space_count as f32;
        for i in 0..space_count {
            out.push((gap_start + i as f32 * step, gap_start + (i + 1) as f32 * step));
        }
    }

    out.extend_from_slice(&second_spans[second_skip..]);
    out
}

/// Merges vertically adjacent `ParsedLine` fragments on the same vertical column into unified lines,
/// inserting appropriate whitespace to preserve character offsets and layout.
pub fn merge_column_lines(lines: &[ParsedLine], sticky_distance: f32) -> Vec<ParsedLine> {
    if lines.is_empty() || sticky_distance <= 0.0 {
        return lines.to_vec();
    }

    // Sort lines primarily by center_x descending (Japanese Right-to-Left columns),
    // secondarily by center_y (Top-to-Bottom)
    let mut sorted = lines.to_vec();
    sorted.sort_by(|a, b| {
        b.center_x
            .partial_cmp(&a.center_x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.center_y
                    .partial_cmp(&b.center_y)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let mut merged: Vec<ParsedLine> = Vec::new();

    for line in sorted {
        let line_top = line.center_y - line.height / 2.0;
        let line_left = line.center_x - line.width / 2.0;
        let line_right = line.center_x + line.width / 2.0;

        let mut matched_idx = None;
        for (i, prev) in merged.iter().enumerate().rev() {
            let prev_top = prev.center_y - prev.height / 2.0;
            let prev_left = prev.center_x - prev.width / 2.0;
            let prev_right = prev.center_x + prev.width / 2.0;

            let min_w = prev.width.min(line.width);
            let width_ratio = line.width / prev.width;
            let is_similar_width = (0.65..=1.55).contains(&width_ratio);

            // Horizontal overlap: must be on the same vertical column
            let overlap_x = prev_right.min(line_right) - prev_left.max(line_left);
            let is_same_column = overlap_x >= 0.5 * min_w
                || (prev.center_x - line.center_x).abs() <= 0.35 * min_w;

            if !is_similar_width || !is_same_column {
                continue;
            }

            // Vertical relationship: determine top vs bottom fragment
            let (top_box, bot_box, is_prev_top) = if prev_top <= line_top {
                (prev, &line, true)
            } else {
                (&line, prev, false)
            };

            let top_b = top_box.center_y + top_box.height / 2.0;
            let bot_t = bot_box.center_y - bot_box.height / 2.0;

            // Check that bot_box doesn't start way before top_box ends (no massive overlap)
            if bot_t < top_b - 0.25 * min_w {
                continue;
            }

            let char_cnt_t = top_box.text.chars().count().max(1);
            let char_cnt_b = bot_box.text.chars().count().max(1);
            let avg_char_h = (top_box.height + bot_box.height) / (char_cnt_t + char_cnt_b) as f32;

            let gap = bot_t - top_b;
            // Max allowed vertical gap on the same column: up to ~15 character heights or 0.6 of screen
            let max_gap = ((sticky_distance * 15.0).max(8.0) * avg_char_h).min(0.6);

            if gap <= max_gap {
                matched_idx = Some((i, is_prev_top, gap, avg_char_h));
                break;
            }
        }

        if let Some((idx, is_prev_top, gap, avg_char_h)) = matched_idx {
            let prev = &merged[idx];
            let (top_box, bot_box) = if is_prev_top {
                (prev, &line)
            } else {
                (&line, prev)
            };

            let space_count = if gap >= 0.75 * avg_char_h {
                (gap / avg_char_h).round().max(1.0) as usize
            } else {
                let top_last = top_box.text.chars().last();
                let bot_first = bot_box.text.chars().next();
                let need_space = match (top_last, bot_first) {
                    (Some(c1), Some(c2)) => !is_cjk(c1) && !is_cjk(c2) && c1 != '-',
                    _ => false,
                };
                if need_space { 1 } else { 0 }
            };

            let t_trimmed = top_box.text.trim_end();
            let b_trimmed = bot_box.text.trim_start();
            let new_text = format!("{}{}{}", t_trimmed, " ".repeat(space_count), b_trimmed);

            let t_min_y = top_box.center_y - top_box.height / 2.0;
            let b_max_y = bot_box.center_y + bot_box.height / 2.0;
            let new_min_y = t_min_y;
            let new_max_y = b_max_y;
            let new_height = (new_max_y - new_min_y).clamp(0.001, 1.0);
            let new_center_y = (new_min_y + new_max_y) / 2.0;

            let left_a = top_box.center_x - top_box.width / 2.0;
            let left_b = bot_box.center_x - bot_box.width / 2.0;
            let right_a = top_box.center_x + top_box.width / 2.0;
            let right_b = bot_box.center_x + bot_box.width / 2.0;
            let new_min_x = left_a.min(left_b);
            let new_max_x = right_a.max(right_b);
            let new_width = (new_max_x - new_min_x).clamp(0.001, 1.0);
            let new_center_x = (new_min_x + new_max_x) / 2.0;

            let new_p_idx = top_box.paragraph_idx.min(bot_box.paragraph_idx);
            let new_char_spans = merge_line_char_spans(
                top_box,
                bot_box,
                t_trimmed,
                b_trimmed,
                space_count,
                true,
            );

            merged[idx] = ParsedLine {
                text: new_text,
                center_x: new_center_x,
                center_y: new_center_y,
                width: new_width,
                height: new_height,
                paragraph_idx: new_p_idx,
                writing_direction: WritingDirection::TopToBottom,
                char_spans: new_char_spans,
            };
        } else {
            merged.push(line);
        }
    }

    merged
}

/// Helper to merge blocks that overlap in 2D space.
fn merge_overlapping_blocks(
    mut blocks: Vec<Vec<ParsedLine>>,
    sticky_distance: f32,
    is_vertical: bool,
) -> Vec<Vec<ParsedLine>> {
    let mut merged_any = true;
    while merged_any {
        merged_any = false;
        let mut merge_pair = None;

        'outer: for i in 0..blocks.len() {
            let min_x_i = blocks[i].iter().map(|l| l.center_x - l.width / 2.0).fold(f32::MAX, f32::min);
            let max_x_i = blocks[i].iter().map(|l| l.center_x + l.width / 2.0).fold(f32::MIN, f32::max);
            let min_y_i = blocks[i].iter().map(|l| l.center_y - l.height / 2.0).fold(f32::MAX, f32::min);
            let max_y_i = blocks[i].iter().map(|l| l.center_y + l.height / 2.0).fold(f32::MIN, f32::max);

            for j in (i + 1)..blocks.len() {
                let min_x_j = blocks[j].iter().map(|l| l.center_x - l.width / 2.0).fold(f32::MAX, f32::min);
                let max_x_j = blocks[j].iter().map(|l| l.center_x + l.width / 2.0).fold(f32::MIN, f32::max);
                let min_y_j = blocks[j].iter().map(|l| l.center_y - l.height / 2.0).fold(f32::MAX, f32::min);
                let max_y_j = blocks[j].iter().map(|l| l.center_y + l.height / 2.0).fold(f32::MIN, f32::max);

                let overlap_x = max_x_i.min(max_x_j) - min_x_i.max(min_x_j);
                let overlap_y = max_y_i.min(max_y_j) - min_y_i.max(min_y_j);

                let min_w = (max_x_i - min_x_i).min(max_x_j - min_x_j);
                let min_h = (max_y_i - min_y_i).min(max_y_j - min_y_j);

                // Check 2D bounding box intersection
                if is_vertical {
                    if overlap_y > 0.05 * min_h && overlap_x > 0.1 * min_w {
                        merge_pair = Some((i, j));
                        break 'outer;
                    }
                } else {
                    if overlap_x > 0.05 * min_w && overlap_y > 0.1 * min_h {
                        merge_pair = Some((i, j));
                        break 'outer;
                    }
                }
            }
        }

        if let Some((i, j)) = merge_pair {
            let removed = blocks.remove(j);
            let mut combined = blocks[i].clone();
            combined.extend(removed);
            if is_vertical {
                blocks[i] = merge_column_lines(&combined, sticky_distance);
                // Sort columns right to left
                blocks[i].sort_by(|a, b| b.center_x.partial_cmp(&a.center_x).unwrap_or(std::cmp::Ordering::Equal));
            } else {
                blocks[i] = merge_row_lines(&combined, sticky_distance);
                // Sort lines top to bottom
                blocks[i].sort_by(|a, b| a.center_y.partial_cmp(&b.center_y).unwrap_or(std::cmp::Ordering::Equal));
            }
            merged_any = true;
        }
    }

    blocks
}

/// Groups horizontal lines into unified text blocks with union bounding boxes.
fn group_horizontal_lines_into_blocks(lines: &[ParsedLine], sticky_distance: f32) -> Vec<OcrBox> {
    if lines.is_empty() {
        return Vec::new();
    }

    // 1. First merge horizontal lines on the same row with whitespace preservation
    let row_lines = merge_row_lines(lines, sticky_distance);

    let mut blocks: Vec<Vec<ParsedLine>> = Vec::new();

    for line in row_lines {
        let line_top = line.center_y - line.height / 2.0;
        let line_left = line.center_x - line.width / 2.0;
        let line_right = line.center_x + line.width / 2.0;

        let mut target_block_idx = None;
        // Check previously formed blocks in reverse order
        for (idx, block) in blocks.iter().enumerate().rev() {
            let last = block.last().unwrap();
            let last_bottom = last.center_y + last.height / 2.0;
            let last_left = last.center_x - last.width / 2.0;
            let last_right = last.center_x + last.width / 2.0;

            let block_left = block
                .iter()
                .map(|l| l.center_x - l.width / 2.0)
                .fold(f32::MAX, f32::min);
            let block_right = block
                .iter()
                .map(|l| l.center_x + l.width / 2.0)
                .fold(f32::MIN, f32::max);
            let block_w = (block_right - block_left).max(0.001);

            let min_h = last.height.min(line.height);
            let min_w = last.width.min(line.width).min(block_w);

            let vertical_gap = line_top - last_bottom;

            // Horizontal overlap against both the immediately previous line and the overall block
            let overlap_x_last = last_right.min(line_right) - last_left.max(line_left);
            let overlap_x_block = block_right.min(line_right) - block_left.max(line_left);
            let overlap_x = overlap_x_last.max(overlap_x_block);

            // 1. Order check: Line must be positioned below the last line
            let is_below = line.center_y > last.center_y && line_top >= last.center_y;

            // 2. Vertical gap check: Tight distance threshold based on sticky_distance
            // Allow up to 20% bounding box overlap (negative gap) due to glyph ascender/descender padding
            let is_tight_vertical =
                vertical_gap >= -0.2 * min_h && vertical_gap <= sticky_distance * min_h;

            // 3. Horizontal overlap & alignment check:
            // Substantial overlap with last line or overall block (>= 35% of the narrower line)
            // Or if slightly indented, left margins must be aligned within sticky_distance fraction of font height
            let is_tight_horizontal = overlap_x >= 0.35 * min_w
                || (overlap_x >= 0.2 * min_w
                    && ((last_left - line_left).abs() <= sticky_distance * min_h
                        || (block_left - line_left).abs() <= sticky_distance * min_h));

            // 4. Font size / height similarity check:
            // Lines in continuous text have similar font heights (within 35% of each other)
            let height_ratio = line.height / last.height;
            let is_similar_height = (0.65..=1.55).contains(&height_ratio);

            if is_below && is_tight_vertical && is_tight_horizontal && is_similar_height {
                target_block_idx = Some(idx);
                break;
            }
        }

        if let Some(idx) = target_block_idx {
            blocks[idx].push(line);
        } else {
            blocks.push(vec![line]);
        }
    }

    // 2. Merge any blocks that overlap horizontally and vertically on screen
    let merged_blocks = merge_overlapping_blocks(blocks, sticky_distance, false);

    let mut boxes = Vec::new();
    for block in merged_blocks {
        let text_slices: Vec<&str> = block.iter().map(|l| l.text.as_str()).collect();
        let full_text = join_lines_text(&text_slices);

        let min_x = block
            .iter()
            .map(|l| l.center_x - l.width / 2.0)
            .fold(f32::MAX, f32::min);
        let max_x = block
            .iter()
            .map(|l| l.center_x + l.width / 2.0)
            .fold(f32::MIN, f32::max);
        let min_y = block
            .iter()
            .map(|l| l.center_y - l.height / 2.0)
            .fold(f32::MAX, f32::min);
        let max_y = block
            .iter()
            .map(|l| l.center_y + l.height / 2.0)
            .fold(f32::MIN, f32::max);

        let width = (max_x - min_x).clamp(0.001, 1.0);
        let height = (max_y - min_y).clamp(0.001, 1.0);
        let center_x = (min_x + width / 2.0).clamp(0.0, 1.0);
        let center_y = (min_y + height / 2.0).clamp(0.0, 1.0);
        let dir = block.first().map(|l| l.writing_direction).unwrap_or(WritingDirection::LeftToRight);

        boxes.push(OcrBox {
            text: full_text,
            center_x,
            center_y,
            width,
            height,
            lines: block,
            writing_direction: dir,
        });
    }

    boxes
}

/// Groups vertical columns into unified text blocks progressing in Japanese Right-to-Left order.
fn group_vertical_lines_into_blocks(lines: &[ParsedLine], sticky_distance: f32) -> Vec<OcrBox> {
    if lines.is_empty() {
        return Vec::new();
    }

    let col_lines = merge_column_lines(lines, sticky_distance);

    let mut blocks: Vec<Vec<ParsedLine>> = Vec::new();

    for line in col_lines {
        let line_top = line.center_y - line.height / 2.0;
        let line_bottom = line.center_y + line.height / 2.0;
        let line_right = line.center_x + line.width / 2.0;

        let mut target_block_idx = None;
        for (idx, block) in blocks.iter().enumerate().rev() {
            let last = block.last().unwrap();
            let last_top = last.center_y - last.height / 2.0;
            let last_bottom = last.center_y + last.height / 2.0;
            let last_left = last.center_x - last.width / 2.0;

            let block_top = block
                .iter()
                .map(|l| l.center_y - l.height / 2.0)
                .fold(f32::MAX, f32::min);
            let block_bottom = block
                .iter()
                .map(|l| l.center_y + l.height / 2.0)
                .fold(f32::MIN, f32::max);
            let block_h = (block_bottom - block_top).max(0.001);

            let min_w = last.width.min(line.width);
            let min_h = last.height.min(line.height).min(block_h);

            // Japanese vertical reading order advances Right-to-Left:
            // next column (`line`) is to the left of the previous column (`last`).
            let horizontal_gap = last_left - line_right;

            let overlap_y_last = last_bottom.min(line_bottom) - last_top.max(line_top);
            let overlap_y_block = block_bottom.min(line_bottom) - block_top.max(line_top);
            let overlap_y = overlap_y_last.max(overlap_y_block);

            // 1. Order check: Line must be positioned to the left of the last column
            let is_left = line.center_x < last.center_x;

            // 2. Horizontal gap check: Tight distance threshold based on sticky_distance
            let is_tight_horizontal =
                horizontal_gap >= -0.2 * min_w && horizontal_gap <= sticky_distance * min_w;

            // 3. Vertical overlap & alignment check:
            // Substantial vertical overlap with last column or overall block (>= 35% of shorter column)
            // Or if slightly indented, top margins must be aligned within sticky_distance fraction of font width
            let is_tight_vertical = overlap_y >= 0.35 * min_h
                || (overlap_y >= 0.2 * min_h
                    && ((last_top - line_top).abs() <= sticky_distance * min_w
                        || (block_top - line_top).abs() <= sticky_distance * min_w));

            // 4. Font size / column width similarity check:
            let width_ratio = line.width / last.width;
            let is_similar_width = (0.65..=1.55).contains(&width_ratio);

            if is_left && is_tight_horizontal && is_tight_vertical && is_similar_width {
                target_block_idx = Some(idx);
                break;
            }
        }

        if let Some(idx) = target_block_idx {
            blocks[idx].push(line);
        } else {
            blocks.push(vec![line]);
        }
    }

    let merged_blocks = merge_overlapping_blocks(blocks, sticky_distance, true);

    let mut boxes = Vec::new();
    for block in merged_blocks {
        let text_slices: Vec<&str> = block.iter().map(|l| l.text.as_str()).collect();
        let full_text = join_lines_text(&text_slices);

        let min_x = block
            .iter()
            .map(|l| l.center_x - l.width / 2.0)
            .fold(f32::MAX, f32::min);
        let max_x = block
            .iter()
            .map(|l| l.center_x + l.width / 2.0)
            .fold(f32::MIN, f32::max);
        let min_y = block
            .iter()
            .map(|l| l.center_y - l.height / 2.0)
            .fold(f32::MAX, f32::min);
        let max_y = block
            .iter()
            .map(|l| l.center_y + l.height / 2.0)
            .fold(f32::MIN, f32::max);

        let width = (max_x - min_x).clamp(0.001, 1.0);
        let height = (max_y - min_y).clamp(0.001, 1.0);
        let center_x = (min_x + width / 2.0).clamp(0.0, 1.0);
        let center_y = (min_y + height / 2.0).clamp(0.0, 1.0);

        boxes.push(OcrBox {
            text: full_text,
            center_x,
            center_y,
            width,
            height,
            lines: block,
            writing_direction: WritingDirection::TopToBottom,
        });
    }

    boxes
}

/// Groups close text lines into unified text blocks with union bounding boxes.
///
/// Separates horizontal text (read top-to-bottom) and vertical text (read top-to-bottom within
/// columns, right-to-left across columns) to prevent improper cross-grouping.
///
/// `sticky_distance` defines the maximum distance between adjacent lines/columns
/// to merge them into a single block (as a multiple of line height / column width).
/// If `sticky_distance <= 0.0`, no lines are merged (every line becomes its own box).
pub fn group_lines_into_blocks(lines: &[ParsedLine], sticky_distance: f32) -> Vec<OcrBox> {
    if lines.is_empty() {
        return Vec::new();
    }

    if sticky_distance <= 0.0 {
        return lines
            .iter()
            .map(|l| OcrBox {
                text: l.text.clone(),
                center_x: l.center_x,
                center_y: l.center_y,
                width: l.width,
                height: l.height,
                lines: vec![l.clone()],
                writing_direction: l.writing_direction,
            })
            .collect();
    }

    let (vert_lines, horiz_lines): (Vec<ParsedLine>, Vec<ParsedLine>) =
        lines.iter().cloned().partition(|l| l.writing_direction.is_vertical());

    let mut boxes = group_horizontal_lines_into_blocks(&horiz_lines, sticky_distance);
    let vert_boxes = group_vertical_lines_into_blocks(&vert_lines, sticky_distance);
    boxes.extend(vert_boxes);

    boxes
}


/// Calculates target dimensions to downscale a captured frame so that it fits
/// within a 720p-equivalent bounding envelope (max 1280 on longer edge, max 720 on shorter edge)
/// while preserving the exact aspect ratio (16:9, 4:3, 5:4, portrait, etc.).
/// If the image is already within 720p, the original dimensions are preserved.
pub fn calculate_720p_target_dimensions(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (width, height);
    }

    let is_landscape = width >= height;
    let (max_long, max_short) = (1280.0f32, 720.0f32);
    let (bound_w, bound_h) = if is_landscape {
        (max_long, max_short)
    } else {
        (max_short, max_long)
    };

    let scale = (bound_w / width as f32)
        .min(bound_h / height as f32)
        .min(1.0);

    let target_w = ((width as f32 * scale).round() as u32).max(1);
    let target_h = ((height as f32 * scale).round() as u32).max(1);

    (target_w, target_h)
}

/// Converts OpenGL RGBA pixel buffer (bottom-left origin) to RGB8 (top-left origin),
/// scaling to `(dst_w, dst_h)` in a single pass using bilinear filtering.
///
/// Reuses the provided `out` buffer without reallocating if capacity allows.
pub fn convert_and_scale_gl_rgba_to_rgb(
    raw_rgba: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    out: &mut Vec<u8>,
) {
    out.clear();
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return;
    }
    let expected_src_bytes = (src_w as usize)
        .checked_mul(src_h as usize)
        .and_then(|px| px.checked_mul(4))
        .unwrap_or(0);
    if expected_src_bytes == 0 || raw_rgba.len() < expected_src_bytes {
        return;
    }
    let dst_len = (dst_w as usize) * (dst_h as usize) * 3;
    out.resize(dst_len, 0);

    if src_w == dst_w && src_h == dst_h {
        // Direct row-by-row flip and strip alpha
        for y in 0..dst_h {
            let src_y = src_h - 1 - y;
            let src_row = (src_y as usize) * (src_w as usize) * 4;
            let dst_row = (y as usize) * (dst_w as usize) * 3;
            let src_slice = &raw_rgba[src_row..src_row + (src_w as usize) * 4];
            let dst_slice = &mut out[dst_row..dst_row + (dst_w as usize) * 3];

            for (p_src, p_dst) in src_slice.chunks_exact(4).zip(dst_slice.chunks_exact_mut(3)) {
                p_dst[0] = p_src[0];
                p_dst[1] = p_src[1];
                p_dst[2] = p_src[2];
            }
        }
        return;
    }

    // High quality bilinear downsampling + flip + strip alpha in one pass
    let x_scale = src_w as f32 / dst_w as f32;
    let y_scale = src_h as f32 / dst_h as f32;

    // Precalculate X mapping
    let mut x_map = Vec::with_capacity(dst_w as usize);
    for dx in 0..dst_w {
        let sx = (dx as f32 + 0.5) * x_scale - 0.5;
        let sx0 = sx.floor().max(0.0) as u32;
        let sx1 = (sx0 + 1).min(src_w - 1);
        let wx = (sx - sx0 as f32).clamp(0.0, 1.0);
        let wx_q8 = (wx * 256.0).round() as u32;
        x_map.push((sx0, sx1, wx_q8));
    }

    for dy in 0..dst_h {
        let sy = (dy as f32 + 0.5) * y_scale - 0.5;
        let sy0_top = sy.floor().max(0.0) as u32;
        let sy1_top = (sy0_top + 1).min(src_h - 1);
        let wy = (sy - sy0_top as f32).clamp(0.0, 1.0);
        let wy_q8 = (wy * 256.0).round() as u32;

        // OpenGL coordinates are flipped: row 0 is at bottom
        let sy0 = src_h - 1 - sy0_top;
        let sy1 = src_h - 1 - sy1_top;

        let row0_offset = (sy0 as usize) * (src_w as usize) * 4;
        let row1_offset = (sy1 as usize) * (src_w as usize) * 4;
        let dst_row_offset = (dy as usize) * (dst_w as usize) * 3;

        for (dx, &(sx0, sx1, wx_q8)) in x_map.iter().enumerate() {
            let p00_idx = row0_offset + (sx0 * 4) as usize;
            let p10_idx = row0_offset + (sx1 * 4) as usize;
            let p01_idx = row1_offset + (sx0 * 4) as usize;
            let p11_idx = row1_offset + (sx1 * 4) as usize;

            let inv_wx = 256 - wx_q8;
            let inv_wy = 256 - wy_q8;

            let w00 = (inv_wx * inv_wy) >> 8;
            let w10 = (wx_q8 * inv_wy) >> 8;
            let w01 = (inv_wx * wy_q8) >> 8;
            let w11 = (wx_q8 * wy_q8) >> 8;

            let dst_idx = dst_row_offset + dx * 3;
            for c in 0..3 {
                let val = (raw_rgba[p00_idx + c] as u32 * w00
                    + raw_rgba[p10_idx + c] as u32 * w10
                    + raw_rgba[p01_idx + c] as u32 * w01
                    + raw_rgba[p11_idx + c] as u32 * w11)
                    >> 8;
                out[dst_idx + c] = val.min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_720p_target_dimensions() {
        // 1080p 16:9 -> 720p 16:9
        assert_eq!(calculate_720p_target_dimensions(1920, 1080), (1280, 720));
        // 1440p 16:9 -> 720p 16:9
        assert_eq!(calculate_720p_target_dimensions(2560, 1440), (1280, 720));
        // 4K 16:9 -> 720p 16:9
        assert_eq!(calculate_720p_target_dimensions(3840, 2160), (1280, 720));
        // 1080p 4:3 -> 720p 4:3
        assert_eq!(calculate_720p_target_dimensions(1440, 1080), (960, 720));
        // 1080p 5:4 -> 720p 5:4
        assert_eq!(calculate_720p_target_dimensions(1350, 1080), (900, 720));
        // Already 480p 4:3 -> untouched
        assert_eq!(calculate_720p_target_dimensions(640, 480), (640, 480));
        // Already 720p 16:9 -> untouched
        assert_eq!(calculate_720p_target_dimensions(1280, 720), (1280, 720));
        // Portrait 9:16 (1080x1920) -> 720x1280
        assert_eq!(calculate_720p_target_dimensions(1080, 1920), (720, 1280));
    }

    #[test]
    #[ignore]
    fn test_live_lens_ocr_with_logo() {
        let logo_bytes = include_bytes!("../../assets/logo.png");
        // Decode PNG header or read dimensions
        let img = image::load_from_memory(logo_bytes).expect("Failed to load logo.png");
        let (w, h) = (img.width(), img.height());

        let result = execute_lens_ocr_and_group(logo_bytes.to_vec(), w, h, 0.6);
        match result {
            Ok(boxes) => {
                assert!(!boxes.is_empty(), "Expected at least one OCR box for logo.png");
                let found_michadame = boxes.iter().any(|b| b.text.contains("見ちゃダメ"));
                assert!(
                    found_michadame,
                    "Expected OCR to recognize '見ちゃダメ', got: {:?}",
                    boxes
                );
            }
            Err(e) => {
                eprintln!("Warning: Live Google Lens test skipped or network failed: {e}");
            }
        }
    }

    #[test]
    #[ignore]
    fn test_live_lens_ocr_with_jpeg() {
        let logo_bytes = include_bytes!("../../assets/logo.png");
        let img = image::load_from_memory(logo_bytes).expect("Failed to load logo.png");
        let rgb = img.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());

        let mut jpg_bytes = Vec::new();
        encode_jpeg_for_ocr(rgb.as_raw(), w, h, &mut jpg_bytes).expect("Failed to encode JPEG");

        let agent = create_lens_agent();

        // Warm up connection
        let start1 = std::time::Instant::now();
        let _ = execute_lens_ocr_with_agent(&agent, jpg_bytes.clone(), w, h);
        let elapsed1 = start1.elapsed();
        println!("Lens OCR request 1 (cold): {:?}", elapsed1);

        let start2 = std::time::Instant::now();
        let result2 = execute_lens_ocr_with_agent(&agent, jpg_bytes.clone(), w, h);
        let elapsed2 = start2.elapsed();
        println!("Lens OCR request 2 (warm): {:?}", elapsed2);

        match result2 {
            Ok(lines) => {
                println!("Got {} lines from JPEG!", lines.len());
                for line in &lines {
                    println!("Line: {}", line.text);
                }
                assert!(!lines.is_empty(), "Expected lines from JPEG");
                let found = lines.iter().any(|l| l.text.contains("見ちゃダメ"));
                assert!(found, "Expected to recognize 見ちゃダメ in JPEG");
            }
            Err(e) => {
                panic!("JPEG OCR failed: {e}");
            }
        }
    }

    #[test]
    #[ignore]
    fn test_jpeg_encoding_and_ocr() {
        let logo_bytes = include_bytes!("../../assets/logo.png");
        let img = image::load_from_memory(logo_bytes).expect("Failed to load logo.png");
        let rgb = img.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());

        let mut jpeg_bytes = Vec::new();
        let enc_start = std::time::Instant::now();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, 85);
        encoder.encode_image(&rgb).expect("Failed to encode JPEG");
        let enc_elapsed = enc_start.elapsed();
        println!("JPEG encoding (1024x1024) took: {:?}, size: {} bytes", enc_elapsed, jpeg_bytes.len());

        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(20))
            .build();
        let start = std::time::Instant::now();
        let lines = execute_lens_ocr_with_agent(&agent, jpeg_bytes, w, h).expect("OCR failed");
        println!("Network OCR took: {:?}", start.elapsed());
        assert!(!lines.is_empty());
        let found = lines.iter().any(|l| l.text.contains("見ちゃダメ"));
        assert!(found);
    }

    #[test]
    #[ignore]
    fn test_lens_warmup() {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
            .build();

        let start = std::time::Instant::now();
        // Warmup with HEAD
        let _ = agent.request("HEAD", LENS_ENDPOINT)
            .set("User-Agent", CHROME_USER_AGENT)
            .call();
        println!("HEAD warmup took: {:?}", start.elapsed());

        let logo_bytes = include_bytes!("../../assets/logo.png");
        let img = image::load_from_memory(logo_bytes).expect("Failed to load logo.png");
        let rgb = img.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());

        for quality in [70, 80, 85, 90] {
            let mut jpeg_bytes = Vec::new();
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, quality);
            encoder.encode_image(&rgb).unwrap();
            let start = std::time::Instant::now();
            let lines = execute_lens_ocr_with_agent(&agent, jpeg_bytes.clone(), w, h).unwrap();
            let elapsed = start.elapsed();
            println!("Quality {}: size {} bytes, time {:?}, lines: {}", quality, jpeg_bytes.len(), elapsed, lines.len());
        }
    }

    #[test]
    fn test_convert_and_scale_gl_rgba_to_rgb() {
        let src_w = 1920;
        let src_h = 1080;
        let dummy_rgba = vec![128u8; (src_w * src_h * 4) as usize];
        let mut out = Vec::new();

        let start = std::time::Instant::now();
        convert_and_scale_gl_rgba_to_rgb(&dummy_rgba, src_w, src_h, 1280, 720, &mut out);
        let elapsed = start.elapsed();
        println!("Downscale 1080p -> 720p with flip and RGB conversion took: {:?}", elapsed);
        assert_eq!(out.len(), (1280 * 720 * 3) as usize);

        // Test identical size (no scale, only flip and strip alpha)
        let start_ident = std::time::Instant::now();
        convert_and_scale_gl_rgba_to_rgb(&dummy_rgba, 1280, 720, 1280, 720, &mut out);
        let elapsed_ident = start_ident.elapsed();
        println!("Direct flip + strip alpha (1280x720) took: {:?}", elapsed_ident);
        assert_eq!(out.len(), (1280 * 720 * 3) as usize);
    }

    #[test]
    fn test_join_lines_text() {
        // CJK to CJK: no space
        assert_eq!(
            join_lines_text(&[
                "あの朝、無人の駅で『彼女』を見かけたときから、",
                "なにかが変わってしまったような気がしていた。"
            ]),
            "あの朝、無人の駅で『彼女』を見かけたときから、なにかが変わってしまったような気がしていた。"
        );

        // English words: space inserted
        assert_eq!(
            join_lines_text(&["When I saw her at the station,", "something had changed."]),
            "When I saw her at the station, something had changed."
        );

        // Hyphenated English: no space
        assert_eq!(
            join_lines_text(&["multi-", "line text"]),
            "multi-line text"
        );
    }

    #[test]
    fn test_group_lines_into_blocks_simple() {
        let lines = vec![
            ParsedLine {
                text: "あの朝、無人の駅で『彼女』を見かけたときから、".to_string(),
                center_x: 0.48,
                center_y: 0.25,
                width: 0.89,
                height: 0.20,
                paragraph_idx: 0,
                writing_direction: WritingDirection::LeftToRight,
                ..Default::default()
            },
            ParsedLine {
                text: "なにかが変わってしまったような気がしていた。".to_string(),
                center_x: 0.46,
                center_y: 0.48,
                width: 0.85,
                height: 0.21,
                paragraph_idx: 0,
                writing_direction: WritingDirection::LeftToRight,
                ..Default::default()
            },
        ];

        // Default sticky distance (0.6x line height) merges tight lines
        let boxes = group_lines_into_blocks(&lines, 0.6);
        assert_eq!(boxes.len(), 1, "Expected 2 lines to be merged into 1 block");
        assert_eq!(
            boxes[0].text,
            "あの朝、無人の駅で『彼女』を見かけたときから、なにかが変わってしまったような気がしていた。"
        );
        assert!((boxes[0].center_y - 0.37).abs() < 0.05);
        assert!((boxes[0].height - 0.435).abs() < 0.05);

        // Zero sticky distance disables merging
        let separated = group_lines_into_blocks(&lines, 0.0);
        assert_eq!(separated.len(), 2, "Expected 2 separate boxes when sticky distance is 0");
    }

    #[test]
    fn test_group_lines_rejects_distant_or_mismatched_text() {
        let lines = vec![
            // Main dialogue line
            ParsedLine {
                text: "セリフの一行目".to_string(),
                center_x: 0.5,
                center_y: 0.5,
                width: 0.8,
                height: 0.1,
                paragraph_idx: 0,
                writing_direction: WritingDirection::LeftToRight,
                ..Default::default()
            },
            // Distant line far below (gap = 0.35 - 0.05 = 0.30 >> 0.6 * 0.1 = 0.06)
            ParsedLine {
                text: "離れたボタン".to_string(),
                center_x: 0.5,
                center_y: 0.85,
                width: 0.2,
                height: 0.03, // also very different height
                paragraph_idx: 0,
                writing_direction: WritingDirection::LeftToRight,
                ..Default::default()
            },
        ];

        let boxes = group_lines_into_blocks(&lines, 0.6);
        assert_eq!(boxes.len(), 2, "Distant UI text must not be merged with dialogue");
        assert_eq!(boxes[0].text, "セリフの一行目");
        assert_eq!(boxes[1].text, "離れたボタン");
    }

    #[test]
    fn test_merge_horizontal_lines_and_overlapping_blocks() {
        // Reproduces the exact OCR lines from the visual novel screenshot:
        // Speaker name on top-left, dialogue quote on top-right separated by whitespace,
        // and 2nd dialogue line below.
        let lines = vec![
            ParsedLine {
                text: "北条".to_string(),
                center_x: 0.056598295,
                center_y: 0.21901257,
                width: 0.08984375,
                height: 0.31736526,
                paragraph_idx: 0,
                writing_direction: WritingDirection::LeftToRight,
                ..Default::default()
            },
            ParsedLine {
                text: "「その魔性に、我ら柳生一門の多く".to_string(),
                center_x: 0.62397987,
                center_y: 0.21924625,
                width: 0.734375,
                height: 0.3233533,
                paragraph_idx: 1,
                writing_direction: WritingDirection::LeftToRight,
                ..Default::default()
            },
            ParsedLine {
                text: "「が殺されたわけですね」".to_string(),
                center_x: 0.2520905,
                center_y: 0.5174159,
                width: 0.4951172,
                height: 0.2994012,
                paragraph_idx: 2,
                writing_direction: WritingDirection::LeftToRight,
                ..Default::default()
            },
        ];

        let boxes = group_lines_into_blocks(&lines, 0.6);
        assert_eq!(boxes.len(), 1, "Expected all 3 lines to be merged into a single OCR box");
        assert_eq!(
            boxes[0].text,
            "北条   「その魔性に、我ら柳生一門の多く「が殺されたわけですね」"
        );
        assert_eq!(boxes[0].lines.len(), 2, "Expected 2 visual rows in the box");
        assert_eq!(boxes[0].lines[0].text, "北条   「その魔性に、我ら柳生一門の多く");
        assert_eq!(
            boxes[0].lines[0].char_spans.len(),
            boxes[0].lines[0].text.chars().count()
        );
        assert_eq!(boxes[0].lines[1].text, "「が殺されたわけですね」");
        assert_eq!(boxes[0].writing_direction, WritingDirection::LeftToRight);
    }

    #[test]
    fn test_merge_vertical_column_lines() {
        // Vertical column split into top half and bottom half
        let lines = vec![
            ParsedLine {
                text: "東京".to_string(),
                center_x: 0.8,
                center_y: 0.2,
                width: 0.05,
                height: 0.15,
                paragraph_idx: 0,
                writing_direction: WritingDirection::TopToBottom,
                ..Default::default()
            },
            ParsedLine {
                text: "特許許可局".to_string(),
                center_x: 0.8,
                center_y: 0.4,
                width: 0.05,
                height: 0.2,
                paragraph_idx: 0,
                writing_direction: WritingDirection::TopToBottom,
                ..Default::default()
            },
        ];

        let merged = merge_column_lines(&lines, 0.6);
        assert_eq!(merged.len(), 1, "Expected top and bottom column fragments to merge");
        assert_eq!(merged[0].text, "東京特許許可局");
        assert_eq!(merged[0].char_spans.len(), merged[0].text.chars().count());
        assert!(merged[0].writing_direction.is_vertical());
        assert!((merged[0].center_x - 0.8).abs() < 1e-4);
    }

    #[test]
    fn test_group_vertical_lines_into_blocks_rtl() {
        // Multi-column Japanese vertical text: read Right-to-Left (Col 1 on right, Col 2 on left)
        let col1 = ParsedLine {
            text: "あの朝、無人の駅で".to_string(),
            center_x: 0.85, // Rightmost column
            center_y: 0.4,
            width: 0.06,
            height: 0.6,
            paragraph_idx: 0,
            writing_direction: WritingDirection::TopToBottom,
            ..Default::default()
        };
        let col2 = ParsedLine {
            text: "彼女を見かけたときから".to_string(),
            center_x: 0.77, // Next column to the left (gap = 0.85 - 0.03 - (0.77 + 0.03) = 0.02)
            center_y: 0.4,
            width: 0.06,
            height: 0.6,
            paragraph_idx: 0,
            writing_direction: WritingDirection::TopToBottom,
            ..Default::default()
        };

        // Pass in arbitrary order
        let lines = vec![col2, col1];
        let boxes = group_lines_into_blocks(&lines, 0.6);
        assert_eq!(boxes.len(), 1, "Expected 2 vertical columns to merge into 1 block");
        assert_eq!(boxes[0].writing_direction, WritingDirection::TopToBottom);
        // RTL reading order: col1 text first, then col2 text
        assert_eq!(boxes[0].text, "あの朝、無人の駅で彼女を見かけたときから");
        assert_eq!(boxes[0].lines.len(), 2);
        assert_eq!(boxes[0].lines[0].text, "あの朝、無人の駅で");
        assert_eq!(boxes[0].lines[1].text, "彼女を見かけたときから");
    }

    #[test]
    fn test_horizontal_and_vertical_lines_do_not_cross_merge() {
        let horiz_dialogue = ParsedLine {
            text: "これは横書きのセリフです。".to_string(),
            center_x: 0.5,
            center_y: 0.8,
            width: 0.7,
            height: 0.08,
            paragraph_idx: 0,
            writing_direction: WritingDirection::LeftToRight,
            ..Default::default()
        };
        let vert_sign = ParsedLine {
            text: "立入禁止".to_string(),
            center_x: 0.85,
            center_y: 0.4,
            width: 0.06,
            height: 0.3,
            paragraph_idx: 1,
            writing_direction: WritingDirection::TopToBottom,
            ..Default::default()
        };

        let lines = vec![horiz_dialogue, vert_sign];
        let boxes = group_lines_into_blocks(&lines, 0.6);
        assert_eq!(boxes.len(), 2, "Horizontal dialogue and vertical sign must stay separate");

        let h_box = boxes.iter().find(|b| !b.writing_direction.is_vertical()).unwrap();
        let v_box = boxes.iter().find(|b| b.writing_direction.is_vertical()).unwrap();
        assert_eq!(h_box.text, "これは横書きのセリフです。");
        assert_eq!(v_box.text, "立入禁止");
    }

    #[test]
    fn test_normalize_dot_leader_word_matches_grid_width() {
        // 4 ellipses "…………" spanning 2 kanji cells (char_em = 0.055, span = 0.118) -> "……" (2 cells)
        assert_eq!(
            normalize_dot_leader_word("…………", Some(0.118), 0.055),
            "……"
        );
        // 6 ellipses "………………" spanning 2 kanji cells (char_em = 0.015, span = 0.032) -> "……"
        assert_eq!(
            normalize_dot_leader_word("………………", Some(0.032), 0.015),
            "……"
        );
        // 7 halfwidth middle dots "･･･････" spanning 2 kanji cells -> "……"
        assert_eq!(
            normalize_dot_leader_word("･･･････", Some(0.031), 0.015),
            "……"
        );
        // 6 fullwidth middle dots "・・・・・・" packed into 2 kanji cells -> "……"
        assert_eq!(
            normalize_dot_leader_word("・・・・・・", Some(0.110), 0.055),
            "……"
        );
        // Single katakana middle dot separator "・" is untouched
        assert_eq!(
            normalize_dot_leader_word("・", Some(0.004), 0.015),
            "・"
        );
        // Normal word is untouched
        assert_eq!(
            normalize_dot_leader_word("六盟傑", Some(0.165), 0.055),
            "六盟傑"
        );
    }
}
