use crate::app::AppState;
use eframe::egui;
use std::time::Instant;

/// Draws the interactive OCR bounding boxes and floating controls on top of the video feed.
pub fn draw_ocr_overlay(ui: &mut egui::Ui, state: &mut AppState, video_rect: egui::Rect) {
    if video_rect.width() <= 10.0 || video_rect.height() <= 10.0 || state.ocr.boxes.is_empty() {
        return;
    }

    // Expire copied feedback highlight after 1.5 seconds
    if let Some(feedback_time) = state.ocr.copy_feedback_time {
        if feedback_time.elapsed().as_millis() > 1500 {
            state.ocr.last_copied_index = None;
            state.ocr.copy_feedback_time = None;
        }
    }

    let pointer_pos = ui.input(|i| i.pointer.hover_pos());
    // The dictionary popup "eats" pointer events: if the cursor is currently over the
    // popup window (using its rect from the last drawn frame), words/boxes underneath it
    // must not react to hover or clicks. This is checked against the *current* pointer
    // position so there is no one-frame lag when entering the popup.
    let pointer_in_popup = match (pointer_pos, state.dict.popup.as_ref().and_then(|p| p.popup_rect)) {
        (Some(pos), Some(rect)) => rect.contains(pos),
        _ => false,
    };
    let mut hovered_any_box = false;
    let mut copied_text: Option<String> = None;
    let mut copied_index: Option<usize> = None;
    let mut dismissed_box_idx: Option<usize> = None;

    // Draw detected OCR boxes
    for (idx, ocr_box) in state.ocr.boxes.iter().enumerate() {
        let min_x = video_rect.min.x + (ocr_box.center_x - ocr_box.width / 2.0) * video_rect.width();
        let min_y = video_rect.min.y + (ocr_box.center_y - ocr_box.height / 2.0) * video_rect.height();
        let max_x = video_rect.min.x + (ocr_box.center_x + ocr_box.width / 2.0) * video_rect.width();
        let max_y = video_rect.min.y + (ocr_box.center_y + ocr_box.height / 2.0) * video_rect.height();

        let box_rect = egui::Rect::from_min_max(
            egui::pos2(min_x, min_y),
            egui::pos2(max_x, max_y),
        );

        let box_id = ui.id().with("ocr_box").with(idx);
        let response = ui.interact(box_rect, box_id, egui::Sense::click());

        let is_recently_copied = state.ocr.last_copied_index == Some(idx);
        let is_hovered = response.hovered() && !pointer_in_popup;
        let is_active_popup_box = state
            .dict
            .popup
            .as_ref()
            .map_or(false, |p| box_rect.expand(2.0).contains(p.word_rect.center()));

        let is_active_box = is_hovered || is_active_popup_box;

        if is_hovered {
            hovered_any_box = true;
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // Right-click dismisses the OCR box from the UI
        if response.secondary_clicked() && !pointer_in_popup {
            dismissed_box_idx = Some(idx);
        }

        // Style the box according to its state
        let (stroke, fill) = if is_recently_copied {
            (
                egui::Stroke::new(2.5, egui::Color32::from_rgb(46, 204, 113)),
                egui::Color32::from_rgba_unmultiplied(46, 204, 113, 80),
            )
        } else if is_active_box {
            // Darken the OCR box to hide the underlying game image
            (
                egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 215, 0)),
                egui::Color32::from_rgba_unmultiplied(14, 16, 22, 250),
            )
        } else {
            (
                egui::Stroke::new(1.5, egui::Color32::from_rgba_unmultiplied(66, 133, 244, 210)),
                egui::Color32::from_rgba_unmultiplied(66, 133, 244, 35),
            )
        };

        if !state.ocr.hide_overlay {
            ui.painter().rect(box_rect, 4.0, fill, stroke);

            // When hovered or actively displaying a dictionary popup, render the OCR text inside the darkened box!
            if is_active_box {
                let text_color = egui::Color32::from_rgb(240, 243, 246);
                if ocr_box.lines.is_empty() {
                    paint_ocr_text_line(
                        ui.painter(),
                        &ocr_box.text,
                        min_x,
                        max_x,
                        min_y,
                        max_y,
                        text_color,
                    );
                } else {
                    for line in &ocr_box.lines {
                        let line_min_x =
                            video_rect.min.x + (line.center_x - line.width / 2.0) * video_rect.width();
                        let line_max_x =
                            video_rect.min.x + (line.center_x + line.width / 2.0) * video_rect.width();
                        let line_min_y =
                            video_rect.min.y + (line.center_y - line.height / 2.0) * video_rect.height();
                        let line_max_y =
                            video_rect.min.y + (line.center_y + line.height / 2.0) * video_rect.height();

                        paint_ocr_text_line(
                            ui.painter(),
                            &line.text,
                            line_min_x,
                            line_max_x,
                            line_min_y,
                            line_max_y,
                            text_color,
                        );
                    }
                }
            }
        }

        if response.clicked() && !pointer_in_popup {
            copied_text = Some(ocr_box.text.clone());
            copied_index = Some(idx);
        }

        // Fallback tooltip only if no dictionary is installed
        if state.dict.installed_metadata.is_none() {
            response.on_hover_ui(|ui| {
                ui.label(egui::RichText::new(&ocr_box.text).size(16.0).strong());
                ui.label(egui::RichText::new("📋 Click to copy | Right-click to dismiss").italics().weak());
            });
        }
    }

    // Handle right-click dismissal
    if let Some(dismiss_idx) = dismissed_box_idx {
        if dismiss_idx < state.ocr.boxes.len() {
            let removed = state.ocr.boxes.remove(dismiss_idx);
            state.ocr.raw_lines.retain(|l| {
                !removed.lines.iter().any(|bl| {
                    bl.text == l.text
                        && (bl.center_x - l.center_x).abs() < 1e-4
                        && (bl.center_y - l.center_y).abs() < 1e-4
                })
            });
            if state.ocr.last_copied_index == Some(dismiss_idx) {
                state.ocr.last_copied_index = None;
            } else if let Some(last_idx) = state.ocr.last_copied_index {
                if last_idx > dismiss_idx {
                    state.ocr.last_copied_index = Some(last_idx - 1);
                }
            }
            state.dict.popup = None;
            if state.ocr.boxes.is_empty() {
                state.ocr.last_scan_time = None;
            }
        }
    }

    // Execute copy if a box was clicked
    if let (Some(text), Some(idx)) = (copied_text, copied_index) {
        match arboard::Clipboard::new() {
            Ok(mut clipboard) => {
                if let Err(e) = clipboard.set_text(&text) {
                    state.error(format!("Failed to copy to clipboard: {e}"));
                } else {
                    state.info(format!("Copied: {}", text));
                    state.ocr.last_copied_index = Some(idx);
                    state.ocr.copy_feedback_time = Some(Instant::now());
                }
            }
            Err(e) => {
                state.error(format!("Clipboard initialization failed: {e}"));
            }
        }
    }

    // If a box was just dismissed, do not run dictionary lookup this frame
    if dismissed_box_idx.is_some() {
        return;
    }

    // Handle Dictionary Word Recognition and Popup with persistence grace period
    let is_popup_currently_hovered = pointer_in_popup
        || state
            .dict
            .popup
            .as_ref()
            .map(|p| p.is_popup_hovered)
            .unwrap_or(false);

    let grace_period = std::time::Duration::from_millis(450);

    // If mouse is inside the popup, keep it open!
    if !is_popup_currently_hovered {
        if let Some(pos) = pointer_pos {
            if hovered_any_box {
                let has_db = state.dict.db.lock().map(|g| g.is_some()).unwrap_or(false);
                if has_db {
                    let db_guard = state.dict.db.lock().unwrap();
                    let db = db_guard.as_ref().unwrap();
                    let freq_guard = state.dict.freq_db.lock().ok();
                    let freq_db = freq_guard.as_ref().and_then(|g| g.as_ref());
                    let lookup = crate::dict::lookup::lookup_word_at_pointer(
                        pos,
                        &state.ocr.boxes,
                        video_rect,
                        db,
                        freq_db,
                        crate::dict::global_deinflector(),
                    );
                    if lookup.is_some() {
                        state.dict.popup = lookup;
                    } else if let Some(ref popup) = state.dict.popup {
                        if popup.last_hover_time.elapsed() < grace_period {
                            let remaining =
                                grace_period.saturating_sub(popup.last_hover_time.elapsed());
                            ui.ctx().request_repaint_after(remaining);
                        } else {
                            state.dict.popup = None;
                        }
                    }
                }
            } else if let Some(ref popup) = state.dict.popup {
                if popup.last_hover_time.elapsed() < grace_period {
                    let remaining = grace_period.saturating_sub(popup.last_hover_time.elapsed());
                    ui.ctx().request_repaint_after(remaining);
                } else {
                    state.dict.popup = None;
                }
            } else {
                state.dict.popup = None;
            }
        } else if let Some(ref popup) = state.dict.popup {
            if popup.last_hover_time.elapsed() < grace_period {
                let remaining = grace_period.saturating_sub(popup.last_hover_time.elapsed());
                ui.ctx().request_repaint_after(remaining);
            } else {
                state.dict.popup = None;
            }
        } else {
            state.dict.popup = None;
        }
    }

    // Draw the dictionary popup window and word highlight on top of the video feed
    let popup_sentence = state
        .dict
        .popup
        .as_ref()
        .map(|p| crate::bank::sentence::extract_sentence(&p.source_text, p.char_range).0)
        .unwrap_or_default();
    let bank = &state.bank;
    let mine_clicked = crate::dict::popup::draw_dict_popup(
        ui,
        &mut state.dict.popup,
        video_rect,
        !state.ocr.hide_overlay,
        |entry| bank.status(&entry.term, &entry.reading, &popup_sentence),
    );

    // Queue a mining request; the screenshot is grabbed by the video paint callback at the
    // end of this frame, before any overlay is drawn.
    if let Some(idx) = mine_clicked {
        let tag = state.bank.mining_tag();
        let request = state.dict.popup.as_ref().and_then(|p| {
            p.entries.get(idx).map(|e| {
                crate::bank::MineRequest::from_lookup(e, &p.source_text, p.char_range, tag)
            })
        });
        if let Some(request) = request {
            if !state.bank.request_mine(request) {
                state.error("Another word is still being mined, try again.");
            }
            ui.ctx().request_repaint();
        }
    }
}

/// Paints an individual OCR text line character-by-character to guarantee exact alignment
/// with mouse hover coordinates and the dictionary popup highlight box.
fn paint_ocr_text_line(
    painter: &egui::Painter,
    text: &str,
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
    text_color: egui::Color32,
) {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return;
    }

    let line_w = (max_x - min_x).max(1.0);
    let line_h = (max_y - min_y).max(1.0);
    let char_w = line_w / chars.len() as f32;
    // Scale font size to fit within line height and character width
    let font_size = (line_h * 0.85).min(char_w * 1.25).clamp(10.0, 72.0);
    let font_id = egui::FontId::new(
        font_size,
        egui::FontFamily::Name("GothicCJK".into()),
    );
    let center_y = (min_y + max_y) / 2.0;

    for (i, &c) in chars.iter().enumerate() {
        if c.is_whitespace() {
            continue;
        }
        let center_x = min_x + (i as f32 + 0.5) * char_w;
        painter.text(
            egui::pos2(center_x, center_y),
            egui::Align2::CENTER_CENTER,
            c.to_string(),
            font_id.clone(),
            text_color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::OcrBox;

    #[test]
    fn test_box_coordinate_mapping() {
        let video_rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(800.0, 600.0));
        let ocr_box = OcrBox {
            text: "テスト".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.2,
            height: 0.1,
            lines: Vec::new(),
        };

        let min_x = video_rect.min.x + (ocr_box.center_x - ocr_box.width / 2.0) * video_rect.width();
        let max_x = video_rect.min.x + (ocr_box.center_x + ocr_box.width / 2.0) * video_rect.width();
        let min_y = video_rect.min.y + (ocr_box.center_y - ocr_box.height / 2.0) * video_rect.height();
        let max_y = video_rect.min.y + (ocr_box.center_y + ocr_box.height / 2.0) * video_rect.height();

        // 100 + (0.5 - 0.1) * 800 = 100 + 320 = 420
        assert_eq!(min_x, 420.0);
        // 100 + (0.5 + 0.1) * 800 = 100 + 480 = 580
        assert_eq!(max_x, 580.0);
        // 100 + (0.5 - 0.05) * 600 = 100 + 270 = 370
        assert_eq!(min_y, 370.0);
        // 100 + (0.5 + 0.05) * 600 = 100 + 330 = 430
        assert_eq!(max_y, 430.0);
    }

    #[test]
    fn test_char_alignment_exact_match_with_lookup() {
        // Line spanning x: 200..400 (w: 200), y: 100..130 (h: 30)
        let text = "林檎を食べた";
        let chars: Vec<char> = text.chars().collect();
        let min_x = 200.0f32;
        let max_x = 400.0f32;
        let line_w = max_x - min_x;
        let char_w = line_w / chars.len() as f32; // 200 / 6 = 33.3333

        // For word "食べる" (indices 3..6):
        let word_start = 3;
        let word_end = 6;
        let word_min_x = min_x + (word_start as f32 / chars.len() as f32) * line_w;
        let word_max_x = min_x + (word_end as f32 / chars.len() as f32) * line_w;

        // Verify char centers of 3, 4, 5 fall strictly inside [word_min_x, word_max_x]
        for i in word_start..word_end {
            let center_x = min_x + (i as f32 + 0.5) * char_w;
            assert!(center_x > word_min_x);
            assert!(center_x < word_max_x);
        }

        // Verify char 2 ('を') center is before word_min_x
        let prev_center_x = min_x + (2.0 + 0.5) * char_w;
        assert!(prev_center_x < word_min_x);
    }

    #[test]
    fn test_dismissal_cleans_boxes_and_raw_lines() {
        let mut ocr = crate::ocr::OcrState::default();
        let line1 = crate::ocr::ParsedLine {
            text: "Box 1".to_string(),
            center_x: 0.2,
            center_y: 0.2,
            width: 0.1,
            height: 0.05,
            paragraph_idx: 0,
        };
        let line2 = crate::ocr::ParsedLine {
            text: "Box 2".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.1,
            height: 0.05,
            paragraph_idx: 1,
        };

        ocr.raw_lines = vec![line1.clone(), line2.clone()];
        ocr.boxes = vec![
            OcrBox {
                text: "Box 1".to_string(),
                center_x: 0.2,
                center_y: 0.2,
                width: 0.1,
                height: 0.05,
                lines: vec![line1],
            },
            OcrBox {
                text: "Box 2".to_string(),
                center_x: 0.5,
                center_y: 0.5,
                width: 0.1,
                height: 0.05,
                lines: vec![line2],
            },
        ];

        // Simulate dismissing index 0
        let removed = ocr.boxes.remove(0);
        ocr.raw_lines.retain(|l| {
            !removed.lines.iter().any(|bl| {
                bl.text == l.text
                    && (bl.center_x - l.center_x).abs() < 1e-4
                    && (bl.center_y - l.center_y).abs() < 1e-4
            })
        });

        assert_eq!(ocr.boxes.len(), 1);
        assert_eq!(ocr.boxes[0].text, "Box 2");
        assert_eq!(ocr.raw_lines.len(), 1);
        assert_eq!(ocr.raw_lines[0].text, "Box 2");
    }

    #[test]
    fn test_hide_overlay_flag_defaults_to_false() {
        let mut state = crate::app::AppState::default();
        assert!(!state.ocr.hide_overlay);
        state.ocr.hide_overlay = true;
        assert!(state.ocr.hide_overlay);
    }
}
