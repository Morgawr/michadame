use super::models::*;
use prost::Message;
use rand::RngCore;
use std::io::Read;
use std::time::Duration;

const LENS_ENDPOINT: &str = "https://lensfrontend-pa.googleapis.com/v1/crupload";
const LENS_API_KEY: &str = "AIzaSyDr2UxVnv_U85AbhhY8XSHSIavUW0DC-sY";
const CHROME_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

/// Sends PNG image bytes to Google Lens and returns detected OCR boxes for Japanese text.
pub fn execute_lens_ocr(
    png_bytes: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<Vec<ParsedLine>, String> {
    if png_bytes.is_empty() || width == 0 || height == 0 {
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
                    image_bytes: png_bytes,
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

    let response = ureq::post(LENS_ENDPOINT)
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
                        let line_text: String = line
                            .words
                            .iter()
                            .map(|w| {
                                format!(
                                    "{}{}",
                                    w.plain_text,
                                    w.text_separator.as_deref().unwrap_or("")
                                )
                            })
                            .collect::<String>()
                            .trim()
                            .to_string();

                        if line_text.is_empty() {
                            continue;
                        }

                        // Try to get bounding box from line geometry
                        let mut resolved_box = line
                            .geometry
                            .as_ref()
                            .and_then(|g| g.bounding_box.as_ref())
                            .map(|b| (b.center_x, b.center_y, b.width, b.height));

                        // If line has no bounding box, compute union from words
                        if resolved_box.is_none() {
                            let mut min_x = f32::MAX;
                            let mut min_y = f32::MAX;
                            let mut max_x = f32::MIN;
                            let mut max_y = f32::MIN;
                            let mut found_word_box = false;

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
                                    }
                                }
                            }

                            if found_word_box && max_x > min_x && max_y > min_y {
                                let w = max_x - min_x;
                                let h = max_y - min_y;
                                resolved_box = Some((min_x + w / 2.0, min_y + h / 2.0, w, h));
                            }
                        }

                        // If still no box, fall back to paragraph geometry
                        if resolved_box.is_none() {
                            resolved_box = paragraph
                                .geometry
                                .as_ref()
                                .and_then(|g| g.bounding_box.as_ref())
                                .map(|b| (b.center_x, b.center_y, b.width, b.height));
                        }

                        if let Some((cx, cy, w, h)) = resolved_box {
                            parsed_lines.push(ParsedLine {
                                text: line_text,
                                center_x: cx.clamp(0.0, 1.0),
                                center_y: cy.clamp(0.0, 1.0),
                                width: w.clamp(0.001, 1.0),
                                height: h.clamp(0.001, 1.0),
                                paragraph_idx: p_idx,
                            });
                        }
                    }
                }
            }
        }
    }

    Ok(parsed_lines)
}

#[cfg(test)]
pub fn execute_lens_ocr_and_group(
    png_bytes: Vec<u8>,
    width: u32,
    height: u32,
    sticky_distance: f32,
) -> Result<Vec<OcrBox>, String> {
    let lines = execute_lens_ocr(png_bytes, width, height)?;
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

/// Groups close horizontal lines into unified text blocks with union bounding boxes.
///
/// `sticky_distance` defines the maximum vertical distance between adjacent lines
/// to merge them into a single block (as a multiple of line height).
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
            })
            .collect();
    }

    let mut blocks: Vec<Vec<ParsedLine>> = Vec::new();

    for line in lines {
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

            let min_h = last.height.min(line.height);
            let min_w = last.width.min(line.width);

            let vertical_gap = line_top - last_bottom;
            let overlap_x = last_right.min(line_right) - last_left.max(line_left);

            // 1. Order check: Line must be positioned below the last line
            let is_below = line.center_y > last.center_y && line_top >= last.center_y;

            // 2. Vertical gap check: Tight distance threshold based on sticky_distance
            // Allow up to 20% bounding box overlap (negative gap) due to glyph ascender/descender padding
            let is_tight_vertical =
                vertical_gap >= -0.2 * min_h && vertical_gap <= sticky_distance * min_h;

            // 3. Horizontal overlap & alignment check:
            // Must have substantial horizontal overlap (>= 50% of the narrower line)
            // Or if slightly indented, left margins must be aligned within sticky_distance fraction of font height
            let is_tight_horizontal = overlap_x >= 0.5 * min_w
                || (overlap_x >= 0.3 * min_w
                    && (last_left - line_left).abs() <= sticky_distance * min_h);

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
            blocks[idx].push(line.clone());
        } else {
            blocks.push(vec![line.clone()]);
        }
    }

    let mut boxes = Vec::new();
    for block in blocks {
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
        });
    }

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
            },
            ParsedLine {
                text: "なにかが変わってしまったような気がしていた。".to_string(),
                center_x: 0.46,
                center_y: 0.48,
                width: 0.85,
                height: 0.21,
                paragraph_idx: 0,
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
            },
            // Distant line far below (gap = 0.35 - 0.05 = 0.30 >> 0.6 * 0.1 = 0.06)
            ParsedLine {
                text: "離れたボタン".to_string(),
                center_x: 0.5,
                center_y: 0.85,
                width: 0.2,
                height: 0.03, // also very different height
                paragraph_idx: 0,
            },
        ];

        let boxes = group_lines_into_blocks(&lines, 0.6);
        assert_eq!(boxes.len(), 2, "Distant UI text must not be merged with dialogue");
        assert_eq!(boxes[0].text, "セリフの一行目");
        assert_eq!(boxes[1].text, "離れたボタン");
    }
}
