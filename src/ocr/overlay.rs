use crate::app::AppState;
use eframe::egui;
use std::time::Instant;

/// Returns the mining status and headword of the top-most dictionary entry (`entries[0]`)
/// in the currently open OCR dictionary popup, or `None` if no popup is open.
pub fn top_popup_entry_mine_info(state: &AppState) -> Option<(crate::bank::MineStatus, String)> {
    let popup = state.dict.popup.as_ref()?;
    let first = popup.entries.first()?;
    let (sentence, _) =
        crate::bank::sentence::extract_sentence(&popup.source_text, popup.char_range);
    let status = state.bank.status(&first.term, &first.reading, &sentence);
    Some((status, first.term.clone()))
}

/// Triggers mining (`+`) or mining with a 20s Twitch clip (`++`) for the entry at `idx`
/// in the currently open dictionary popup. Returns `true` if a mine request was queued.
pub fn execute_popup_mine_action(
    state: &mut AppState,
    ctx: &egui::Context,
    idx: usize,
    with_clip: bool,
) -> bool {
    let tag = state.bank.mining_tag();
    let entry_data = state.dict.popup.as_ref().and_then(|p| {
        p.entries.get(idx).map(|e| {
            (
                crate::bank::MineRequest::from_lookup(e, &p.source_text, p.char_range, tag.clone()),
                e.term.clone(),
                e.reading.clone(),
            )
        })
    });
    let Some((request, term, reading)) = entry_data else {
        return false;
    };

    let request_id = request.request_id;
    if !state.bank.request_mine(request) {
        state.error("Another word is still being mined, try again.");
        ctx.request_repaint();
        return false;
    }

    if with_clip && state.steam_deck.is_none() {
        let clip_title = crate::twitch::clips::format_clip_title(&term, &reading, tag.as_deref());
        if let Err(e) = state.twitch.request_clip(clip_title, Some(request_id), ctx) {
            state.error(format!("Could not create Twitch clip: {e}"));
        } else {
            state.info(format!(
                "Creating Twitch clip for '{term}' (buffering {}s)...",
                crate::twitch::clips::PRE_CLIP_BUFFER.as_secs()
            ));
        }
    }

    ctx.request_repaint();
    true
}

/// Draws the interactive OCR bounding boxes and floating controls on top of the video feed.
pub fn draw_ocr_overlay(ui: &mut egui::Ui, state: &mut AppState, video_rect: egui::Rect) {
    if video_rect.width() <= 10.0 || video_rect.height() <= 10.0 || state.ocr.boxes.is_empty() {
        state.ocr.is_box_hovered = false;
        if state.ocr.boxes.is_empty() {
            state.dict.popup = None;
        }
        if let Some(ref r) = state.crt_renderer {
            r.lock().unwrap().popup_primitives.clear();
        }
        return;
    }

    // Expire copied feedback highlight after 1.5 seconds (paused while whiteboard is open)
    if state.whiteboard.open {
        if let Some(ref mut feedback_time) = state.ocr.copy_feedback_time {
            *feedback_time = Instant::now();
        }
    } else if let Some(feedback_time) = state.ocr.copy_feedback_time {
        if feedback_time.elapsed().as_millis() > 1500 {
            state.ocr.last_copied_index = None;
            state.ocr.copy_feedback_time = None;
        }
    }

    let pointer_pos = ui.input(|i| {
        i.pointer
            .hover_pos()
            .or(i.pointer.interact_pos())
            .or(i.pointer.latest_pos())
    });
    let deck_mode = state.steam_deck.is_some();
    let mut tapped_word = false;
    let whiteboard_open = state.whiteboard.open;
    let hide_for_mining_capture = state.bank.has_pending() && state.whiteboard.is_visible();
    // The dictionary popup "eats" pointer events: if the cursor is currently over the
    // popup window (using its rect from the last drawn frame), words/boxes underneath it
    // must not react to hover or clicks. This is checked against the *current* pointer
    // position so there is no one-frame lag when entering the popup.
    let pointer_in_popup = !whiteboard_open
        && match (
            pointer_pos,
            state.dict.popup.as_ref().and_then(|p| p.popup_rect),
        ) {
            (Some(pos), Some(rect)) => rect.contains(pos),
            _ => false,
        };
    let mut hovered_any_box = false;
    let mut copied_text: Option<String> = None;
    let mut copied_index: Option<usize> = None;
    let mut dismissed_box_idx: Option<usize> = None;

    // Draw detected OCR boxes
    for (idx, ocr_box) in state.ocr.boxes.iter().enumerate() {
        let min_x =
            video_rect.min.x + (ocr_box.center_x - ocr_box.width / 2.0) * video_rect.width();
        let min_y =
            video_rect.min.y + (ocr_box.center_y - ocr_box.height / 2.0) * video_rect.height();
        let max_x =
            video_rect.min.x + (ocr_box.center_x + ocr_box.width / 2.0) * video_rect.width();
        let max_y =
            video_rect.min.y + (ocr_box.center_y + ocr_box.height / 2.0) * video_rect.height();

        let box_rect = egui::Rect::from_min_max(egui::pos2(min_x, min_y), egui::pos2(max_x, max_y));

        let box_id = ui.id().with("ocr_box").with(idx);
        let ocr_sense = egui::Sense {
            click: true,
            drag: false,
            focusable: false,
        };
        let response = ui.interact(box_rect, box_id, ocr_sense);

        let is_recently_copied = state.ocr.last_copied_index == Some(idx);
        let is_hovered = (response.hovered() || (deck_mode && response.clicked()))
            && !pointer_in_popup
            && !whiteboard_open;
        let is_active_popup_box = state.dict.popup.as_ref().map_or(false, |p| {
            box_rect.expand(2.0).contains(p.word_rect.center())
        });

        let is_active_box = is_hovered || is_active_popup_box;

        if is_hovered {
            hovered_any_box = true;
            let software_mouse_active =
                crate::ui::video_player::is_software_mouse_active(state, true);
            if !software_mouse_active {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }

        // Right-click dismisses the OCR box from the UI (disabled when constant scanning is active)
        if !state.ocr.is_constant_scanning_active()
            && response.secondary_clicked()
            && !pointer_in_popup
            && !whiteboard_open
        {
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
                egui::Stroke::new(
                    1.5,
                    egui::Color32::from_rgba_unmultiplied(66, 133, 244, 210),
                ),
                egui::Color32::from_rgba_unmultiplied(66, 133, 244, 35),
            )
        };

        if !state.ocr.hide_overlay && !hide_for_mining_capture {
            ui.painter().rect(box_rect, 4.0, fill, stroke);

            // When hovered or actively displaying a dictionary popup, render the OCR text inside the darkened box!
            if is_active_box {
                let text_color = egui::Color32::from_rgb(240, 243, 246);
                let is_box_vertical = ocr_box.writing_direction.is_vertical();
                if ocr_box.lines.is_empty() {
                    paint_ocr_text_line(
                        ui.painter(),
                        &ocr_box.text,
                        &[],
                        video_rect,
                        min_x,
                        max_x,
                        min_y,
                        max_y,
                        text_color,
                        is_box_vertical,
                    );
                } else {
                    for line in &ocr_box.lines {
                        let line_min_x = video_rect.min.x
                            + (line.center_x - line.width / 2.0) * video_rect.width();
                        let line_max_x = video_rect.min.x
                            + (line.center_x + line.width / 2.0) * video_rect.width();
                        let line_min_y = video_rect.min.y
                            + (line.center_y - line.height / 2.0) * video_rect.height();
                        let line_max_y = video_rect.min.y
                            + (line.center_y + line.height / 2.0) * video_rect.height();

                        paint_ocr_text_line(
                            ui.painter(),
                            &line.text,
                            &line.char_spans,
                            video_rect,
                            line_min_x,
                            line_max_x,
                            line_min_y,
                            line_max_y,
                            text_color,
                            line.writing_direction.is_vertical(),
                        );
                    }
                }
            }
        }

        if response.clicked() && !pointer_in_popup && !whiteboard_open {
            if deck_mode {
                tapped_word = true;
            } else {
                copied_text = Some(ocr_box.text.clone());
                copied_index = Some(idx);
            }
        }

        // Fallback tooltip only if no dictionary is installed
        if state.dict.installed_metadata.is_none() && !whiteboard_open {
            let hint_label = if state.ocr.is_constant_scanning_active() {
                "📋 Click to copy"
            } else {
                "📋 Click to copy | Right-click to dismiss"
            };
            response.on_hover_ui(|ui| {
                ui.label(egui::RichText::new(&ocr_box.text).size(16.0).strong());
                ui.label(egui::RichText::new(hint_label).italics().weak());
            });
        }
    }

    state.ocr.is_box_hovered = hovered_any_box;

    // Handle right-click dismissal
    if let Some(dismiss_idx) = dismissed_box_idx {
        if dismiss_idx < state.ocr.boxes.len() {
            let removed = state.ocr.boxes.remove(dismiss_idx);
            state.ocr.raw_lines.retain(|l| {
                !removed.lines.iter().any(|bl| {
                    let exact_match = bl.text == l.text
                        && (bl.center_x - l.center_x).abs() < 1e-4
                        && (bl.center_y - l.center_y).abs() < 1e-4;
                    let min_x = bl.center_x - bl.width * 0.5;
                    let max_x = bl.center_x + bl.width * 0.5;
                    let min_y = bl.center_y - bl.height * 0.5;
                    let max_y = bl.center_y + bl.height * 0.5;
                    let trimmed = l.text.trim();
                    let contained_match = !trimmed.is_empty()
                        && l.center_x >= min_x - 1e-3
                        && l.center_x <= max_x + 1e-3
                        && l.center_y >= min_y - 1e-3
                        && l.center_y <= max_y + 1e-3
                        && bl.text.contains(trimmed);
                    exact_match || contained_match
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

    // Handle Dictionary Word Recognition and Popup with persistence grace period.
    // While the transparent whiteboard is open, any open OCR dictionary popup stays open
    // and its hover grace timer is frozen.
    if whiteboard_open {
        if let Some(ref mut popup) = state.dict.popup {
            let now = Instant::now();
            popup.last_hover_time = now;
            popup.last_word_hover_time = now;
        }
    }

    let is_popup_currently_hovered = !whiteboard_open
        && (pointer_in_popup
            || state
                .dict
                .popup
                .as_ref()
                .map(|p| p.is_popup_hovered)
                .unwrap_or(false));

    if tapped_word {
        state.steam_deck.as_mut().unwrap().pinned_dictionary = true;
        state.dict.popup = None;
    } else if deck_mode
        && ui.input(|i| i.pointer.primary_clicked())
        && !pointer_in_popup
        && !hovered_any_box
    {
        state.dict.popup = None;
        state.steam_deck.as_mut().unwrap().pinned_dictionary = false;
    }
    let pinned = state
        .steam_deck
        .as_ref()
        .is_some_and(|deck| deck.pinned_dictionary);
    let grace_period = std::time::Duration::from_millis(450);

    // If mouse is inside the popup or the whiteboard is open, keep it open!
    if (tapped_word || !is_popup_currently_hovered) && !whiteboard_open && (!pinned || tapped_word)
    {
        if let Some(pos) = pointer_pos {
            if hovered_any_box {
                let has_db = state.dict.db.lock().map(|g| g.is_some()).unwrap_or(false);
                let has_custom = !state.bank.custom_names.is_empty();
                if has_db || has_custom {
                    let db_guard = state.dict.db.lock().ok();
                    let db = db_guard.as_ref().and_then(|g| g.as_ref());
                    let freq_guard = state.dict.freq_db.lock().ok();
                    let freq_db = freq_guard.as_ref().and_then(|g| g.as_ref());
                    let pitch_guard = state.dict.pitch_db.lock().ok();
                    let pitch_db = pitch_guard.as_ref().and_then(|g| g.as_ref());
                    let current_tag = state.bank.mining_tag();
                    let lookup = crate::dict::lookup::lookup_word_at_pointer_with_pitch(
                        pos,
                        &state.ocr.boxes,
                        video_rect,
                        db,
                        freq_db,
                        pitch_db,
                        crate::dict::global_deinflector(),
                        &state.bank.custom_names,
                        current_tag.as_deref(),
                        state.bank.custom_names_generation,
                    );
                    if let Some(new_popup) = lookup {
                        // Don't jump to a neighboring word right after leaving the current
                        // word/popup: the cursor may just be passing over it on its way to
                        // the popup.
                        let wait = state.dict.popup.as_ref().and_then(|current| {
                            crate::dict::popup::popup_switch_wait(
                                current,
                                &new_popup,
                                pos,
                                crate::dict::popup::POPUP_SWITCH_DELAY,
                            )
                        });
                        match wait {
                            Some(remaining) => ui.ctx().request_repaint_after(remaining),
                            None => state.dict.popup = Some(new_popup),
                        }
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
    let target_rect = if state.dict.popup_under_crt {
        let border_crop = [
            state.video.border_crop_left,
            state.video.border_crop_right,
            state.video.border_crop_top,
            state.video.border_crop_bottom,
        ];
        let underscan = [state.video.underscan_x, state.video.underscan_y];
        let overscan = [state.video.overscan_x, state.video.overscan_y];
        let filter = crate::devices::filter_type::CrtFilter::from_u8(
            state.crt_filter.load(std::sync::atomic::Ordering::Relaxed),
        );
        let is_crt_on = filter != crate::devices::filter_type::CrtFilter::Off;
        let retro_pc_frame = is_crt_on && state.video.retro_pc_frame;
        let curvature_active = is_crt_on
            && (filter == crate::devices::filter_type::CrtFilter::Lottes
                || (filter == crate::devices::filter_type::CrtFilter::Halo
                    && state.halo.curvature));
        let ppp = ui.ctx().pixels_per_point();
        crate::dict::popup::calculate_crt_popup_safe_area(
            video_rect,
            border_crop,
            underscan,
            overscan,
            retro_pc_frame,
            curvature_active,
            ppp,
        )
    } else {
        video_rect
    };

    let (popup_action, popup_primitives) = if hide_for_mining_capture {
        (None, Vec::new())
    } else {
        crate::dict::popup::draw_dict_popup(
            ui,
            &mut state.dict.popup,
            target_rect,
            !state.ocr.hide_overlay,
            state.dict.popup_under_crt,
            if deck_mode {
                crate::steam_deck::dictionary_scale(ui.ctx())
            } else {
                1.0
            },
            |entry| bank.status(&entry.term, &entry.reading, &popup_sentence),
            |entry| bank.is_entry_mined(&entry.term, &entry.reading, entry.sequence),
            !deck_mode && state.twitch.is_stream_live(),
        )
    };

    if let Some(ref r) = state.crt_renderer {
        r.lock().unwrap().popup_primitives = popup_primitives;
    }

    // Handle user actions in popup: mining, clipping, or deleting a custom name
    if let Some(action) = popup_action {
        match action {
            crate::dict::popup::PopupAction::Mine(idx) => {
                execute_popup_mine_action(state, ui.ctx(), idx, false);
            }
            crate::dict::popup::PopupAction::MineAndClip(idx) => {
                execute_popup_mine_action(state, ui.ctx(), idx, true);
            }
            crate::dict::popup::PopupAction::DeleteCustomName(id) => {
                if let Err(e) = state.bank.delete_custom_name(id) {
                    state.error(format!("Failed to delete custom name: {e}"));
                } else {
                    state.info("Deleted custom name.");
                    if let Some(ref mut popup) = state.dict.popup {
                        popup.entries.retain(|e| e.custom_name_id != Some(id));
                        if popup.entries.is_empty() {
                            state.dict.popup = None;
                        }
                    }
                    crate::dict::lookup::invalidate_scan_cache();
                    ui.ctx().request_repaint();
                }
            }
        }
    }
}

/// Paints an individual OCR text line character-by-character to guarantee exact alignment
/// with mouse hover coordinates and the dictionary popup highlight box.
fn paint_ocr_text_line(
    painter: &egui::Painter,
    text: &str,
    char_spans: &[(f32, f32)],
    video_rect: egui::Rect,
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
    text_color: egui::Color32,
    is_vertical: bool,
) {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return;
    }

    let line_w = (max_x - min_x).max(0.0);
    let line_h = (max_y - min_y).max(0.0);
    let use_spans = char_spans.len() == chars.len();
    let effective_char_count = chars
        .iter()
        .map(|&c| crate::ocr::lens::char_natural_weight(c))
        .sum::<f32>()
        .max(1.0);
    let font_size = if is_vertical {
        (line_w * 0.85).min(line_h / effective_char_count * 1.25)
    } else {
        (line_h * 0.85).min(line_w / effective_char_count * 1.25)
    }
    .min(72.0);

    for (i, &c) in chars.iter().enumerate() {
        if c.is_whitespace() {
            continue;
        }
        let cell = if is_vertical {
            let (y0, y1) = if use_spans {
                let (s0, s1) = char_spans[i];
                (
                    video_rect.min.y + s0 * video_rect.height(),
                    video_rect.min.y + s1 * video_rect.height(),
                )
            } else {
                let height = line_h / chars.len() as f32;
                (min_y + i as f32 * height, min_y + (i + 1) as f32 * height)
            };
            egui::Rect::from_min_max(egui::pos2(min_x, y0), egui::pos2(max_x, y1))
        } else {
            let (x0, x1) = if use_spans {
                let (s0, s1) = char_spans[i];
                (
                    video_rect.min.x + s0 * video_rect.width(),
                    video_rect.min.x + s1 * video_rect.width(),
                )
            } else {
                let width = line_w / chars.len() as f32;
                (min_x + i as f32 * width, min_x + (i + 1) as f32 * width)
            };
            egui::Rect::from_min_max(egui::pos2(x0, min_y), egui::pos2(x1, max_y))
        };
        paint_ocr_glyph(painter, c, cell, font_size, text_color);
    }
}

/// Fit the actual font ink to its OCR hit cell. Logical minimum font sizes and
/// typographic advance widths can otherwise put visible ink in a neighbor's
/// selection cell, particularly at fractional/high DPI and for punctuation.
fn paint_ocr_glyph(
    painter: &egui::Painter,
    character: char,
    cell: egui::Rect,
    mut font_size: f32,
    color: egui::Color32,
) {
    if !cell.is_finite()
        || cell.width() <= 0.0
        || cell.height() <= 0.0
        || !font_size.is_finite()
        || font_size <= 0.0
    {
        return;
    }
    let mut galley;
    // Font rasterization rounds to physical pixels, so remeasure after sizing.
    for attempt in 0..3 {
        galley = painter.layout_no_wrap(
            character.to_string(),
            egui::FontId::new(font_size, egui::FontFamily::Name("GothicCJK".into())),
            color,
        );
        let ink = galley.mesh_bounds;
        if !ink.is_positive() {
            return;
        }
        let fit = (cell.width() / ink.width())
            .min(cell.height() / ink.height())
            .min(1.0);
        if fit >= 0.999 || attempt == 2 {
            let origin = cell.center() - ink.center().to_vec2();
            // Clipping also covers subpixel rounding and very narrow OCR spans.
            painter.with_clip_rect(cell).galley(origin, galley, color);
            return;
        }
        font_size *= fit * 0.98;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::{OcrBox, ParsedLine, WritingDirection};

    fn japanese_context(ppp: f32) -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(ppp);
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "test_jp".into(),
            egui::FontData::from_static(include_bytes!("../../assets/NotoSansJP-Regular.ttf")),
        );
        fonts.families.insert(
            egui::FontFamily::Name("GothicCJK".into()),
            vec!["test_jp".into()],
        );
        ctx.set_fonts(fonts);
        ctx
    }

    #[test]
    fn rasterized_ocr_ink_selects_its_character_at_fractional_dpi() {
        let characters = ['日', '本', '語'];
        let names: Vec<_> = characters
            .iter()
            .enumerate()
            .map(|(i, c)| crate::bank::database::CustomNameEntry {
                id: i as i64 + 1,
                created_at: 0,
                kanji: c.to_string(),
                furigana: String::new(),
                source_tag: None,
                notes: None,
            })
            .collect();
        for ppp in [1.0, 13.0 / 12.0, 1.25, 1.5, 13.0 / 6.0, 2.5, 3.0] {
            for vertical in [false, true] {
                for precise_spans in [false, true] {
                    let ctx = japanese_context(ppp);
                    let video = egui::Rect::from_min_size(
                        egui::pos2(17.0, 9.0),
                        egui::vec2(1280.0 / ppp, 800.0 / ppp),
                    );
                    let line_rect = egui::Rect::from_min_size(
                        video.min + egui::vec2(100.0 / ppp, 100.0 / ppp),
                        if vertical {
                            egui::vec2(16.0 / ppp, 48.0 / ppp)
                        } else {
                            egui::vec2(48.0 / ppp, 16.0 / ppp)
                        },
                    );
                    let spans: Vec<_> = if precise_spans {
                        (0..3)
                            .map(|i| {
                                (
                                    (100.0 + i as f32 * 16.0)
                                        / if vertical { 800.0 } else { 1280.0 },
                                    (116.0 + i as f32 * 16.0)
                                        / if vertical { 800.0 } else { 1280.0 },
                                )
                            })
                            .collect()
                    } else {
                        vec![]
                    };
                    let line = ParsedLine {
                        text: "日本語".into(),
                        center_x: (line_rect.center().x - video.min.x) / video.width(),
                        center_y: (line_rect.center().y - video.min.y) / video.height(),
                        width: line_rect.width() / video.width(),
                        height: line_rect.height() / video.height(),
                        writing_direction: if vertical {
                            WritingDirection::TopToBottom
                        } else {
                            WritingDirection::LeftToRight
                        },
                        char_spans: spans.clone(),
                        ..Default::default()
                    };
                    let ocr_box = OcrBox {
                        text: line.text.clone(),
                        center_x: line.center_x,
                        center_y: line.center_y,
                        width: line.width,
                        height: line.height,
                        writing_direction: line.writing_direction,
                        lines: vec![line],
                    };
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                video.max.to_vec2(),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            paint_ocr_text_line(
                                &ctx.layer_painter(egui::LayerId::background()),
                                "日本語",
                                &spans,
                                video,
                                line_rect.left(),
                                line_rect.right(),
                                line_rect.top(),
                                line_rect.bottom(),
                                egui::Color32::WHITE,
                                vertical,
                            );
                        },
                    );
                    let atlas = output
                        .textures_delta
                        .set
                        .iter()
                        .find_map(|(_, delta)| {
                            if let egui::ImageData::Font(image) = &delta.image {
                                Some(image)
                            } else {
                                None
                            }
                        })
                        .unwrap();
                    let mut samples = 0;
                    for shape in &output.shapes {
                        let egui::Shape::Text(text) = &shape.shape else {
                            continue;
                        };
                        let expected = characters
                            .iter()
                            .position(|c| c.to_string() == text.galley.job.text)
                            .unwrap();
                        let mesh = &text.galley.rows[0].visuals.mesh;
                        let first = &mesh.vertices[0];
                        let last = &mesh.vertices[3];
                        for y in first.uv.y as usize..last.uv.y as usize {
                            for x in first.uv.x as usize..last.uv.x as usize {
                                if atlas.pixels[y * atlas.size[0] + x] < 0.5 {
                                    continue;
                                }
                                let point = text.pos
                                    + first.pos.to_vec2()
                                    + egui::vec2(
                                        (x as f32 + 0.5 - first.uv.x) / (last.uv.x - first.uv.x)
                                            * (last.pos.x - first.pos.x),
                                        (y as f32 + 0.5 - first.uv.y) / (last.uv.y - first.uv.y)
                                            * (last.pos.y - first.pos.y),
                                    );
                                if !shape.clip_rect.contains(point) {
                                    continue;
                                }
                                let popup = crate::dict::lookup::lookup_word_at_pointer(
                                    point,
                                    std::slice::from_ref(&ocr_box),
                                    video,
                                    None,
                                    None,
                                    crate::dict::global_deinflector(),
                                    &names,
                                    None,
                                    1,
                                )
                                .unwrap();
                                assert_eq!(
                                    popup.char_range,
                                    (expected, expected + 1),
                                    "ppp={ppp} vertical={vertical} point={point:?}"
                                );
                                samples += 1;
                            }
                        }
                    }
                    assert!(samples > 30, "No visible glyphs at ppp={ppp}");
                }
            }
        }
    }

    #[test]
    fn punctuation_ink_fits_narrow_hit_cells() {
        let ctx = japanese_context(13.0 / 6.0);
        let cells: Vec<_> = (0..4)
            .map(|i| {
                egui::Rect::from_min_size(
                    egui::pos2(10.0 + i as f32 * 10.0, 10.0),
                    egui::vec2(3.0, 8.0),
                )
            })
            .collect();
        let output = ctx.run(Default::default(), |ctx| {
            for (c, cell) in ['「', '。', '、', '」'].into_iter().zip(&cells) {
                paint_ocr_glyph(
                    &ctx.layer_painter(egui::LayerId::background()),
                    c,
                    *cell,
                    32.0,
                    egui::Color32::WHITE,
                );
            }
        });
        let shapes: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    Some((shape, text))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(shapes.len(), cells.len());
        for ((shape, text), cell) in shapes.into_iter().zip(cells) {
            let visible = text
                .galley
                .mesh_bounds
                .translate(text.pos.to_vec2())
                .intersect(shape.clip_rect);
            assert!(visible.is_positive());
            assert!(cell.contains_rect(visible));
        }
    }

    #[test]
    fn test_box_coordinate_mapping() {
        let video_rect =
            egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(800.0, 600.0));
        let ocr_box = OcrBox {
            text: "テスト".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.2,
            height: 0.1,
            lines: Vec::new(),
            writing_direction: WritingDirection::LeftToRight,
        };

        let min_x =
            video_rect.min.x + (ocr_box.center_x - ocr_box.width / 2.0) * video_rect.width();
        let max_x =
            video_rect.min.x + (ocr_box.center_x + ocr_box.width / 2.0) * video_rect.width();
        let min_y =
            video_rect.min.y + (ocr_box.center_y - ocr_box.height / 2.0) * video_rect.height();
        let max_y =
            video_rect.min.y + (ocr_box.center_y + ocr_box.height / 2.0) * video_rect.height();

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
    fn test_vertical_char_alignment_exact_match() {
        // Vertical column spanning x: 100..130 (w: 30), y: 200..500 (h: 300)
        let text = "日本昔話";
        let chars: Vec<char> = text.chars().collect();
        let min_y = 200.0f32;
        let max_y = 500.0f32;
        let line_h = max_y - min_y;
        let char_h = line_h / chars.len() as f32; // 300 / 4 = 75

        // For word "昔話" (indices 2..4):
        let word_start = 2;
        let word_end = 4;
        let word_min_y = min_y + (word_start as f32 / chars.len() as f32) * line_h;
        let word_max_y = min_y + (word_end as f32 / chars.len() as f32) * line_h;

        // Verify char centers of 2, 3 fall strictly inside [word_min_y, word_max_y]
        for i in word_start..word_end {
            let center_y = min_y + (i as f32 + 0.5) * char_h;
            assert!(center_y > word_min_y);
            assert!(center_y < word_max_y);
        }
    }

    #[test]
    fn test_dismissal_cleans_boxes_and_raw_lines() {
        let mut ocr = crate::ocr::OcrState::default();
        let line1 = ParsedLine {
            text: "Box 1".to_string(),
            center_x: 0.2,
            center_y: 0.2,
            width: 0.1,
            height: 0.05,
            paragraph_idx: 0,
            writing_direction: WritingDirection::LeftToRight,
            ..Default::default()
        };
        let line2 = ParsedLine {
            text: "Box 2".to_string(),
            center_x: 0.5,
            center_y: 0.5,
            width: 0.1,
            height: 0.05,
            paragraph_idx: 1,
            writing_direction: WritingDirection::LeftToRight,
            ..Default::default()
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
                writing_direction: WritingDirection::LeftToRight,
            },
            OcrBox {
                text: "Box 2".to_string(),
                center_x: 0.5,
                center_y: 0.5,
                width: 0.1,
                height: 0.05,
                lines: vec![line2],
                writing_direction: WritingDirection::LeftToRight,
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
