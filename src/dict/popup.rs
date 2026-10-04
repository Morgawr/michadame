use super::models::DictPopupState;
use super::render::render_term_entry;
use eframe::egui::{self, Color32, Rounding, Stroke};

const POPUP_BG: Color32 = Color32::from_rgb(22, 24, 30);
const POPUP_STROKE: Color32 = Color32::from_rgb(51, 65, 85);
const WORD_HIGHLIGHT_FILL: Color32 = Color32::from_rgba_premultiplied(56, 189, 248, 45);
const WORD_HIGHLIGHT_STROKE: Color32 = Color32::from_rgb(56, 189, 248);
/// After the cursor leaves the popup or the highlighted word, hovering a different word
/// only replaces the popup once this much time has passed.
pub const POPUP_SWITCH_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

/// True if `pos` is over the highlighted word (including parts wrapped onto other lines).
fn pointer_on_word(popup: &DictPopupState, pos: egui::Pos2) -> bool {
    std::iter::once(&popup.word_rect)
        .chain(popup.extra_word_rects.iter())
        .any(|r| r.expand(2.0).contains(pos))
}

/// Decides whether `new` (the word under the cursor at `pos`) may replace the `current`
/// popup. Returns `None` if it may, or the remaining wait otherwise: the cursor is still on
/// the current word, or left it / the popup less than `delay` ago.
pub fn popup_switch_wait(
    current: &DictPopupState,
    new: &DictPopupState,
    pos: egui::Pos2,
    delay: std::time::Duration,
) -> Option<std::time::Duration> {
    let same_word = current.source_text == new.source_text && current.char_range == new.char_range;
    if same_word {
        return None;
    }
    if pointer_on_word(current, pos) {
        return Some(delay);
    }
    let elapsed = current.last_word_hover_time.elapsed();
    (elapsed < delay).then(|| delay - elapsed)
}

/// Hash identifying what a popup shows (which lookup, which entries). Changes whenever the
/// popup switches to a different word, so per-popup UI state like scrolling can be reset.
fn popup_identity(popup: &DictPopupState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    popup.source_text.hash(&mut h);
    popup.char_range.hash(&mut h);
    for e in &popup.entries {
        (&e.term, &e.reading, e.sequence).hash(&mut h);
    }
    h.finish()
}

/// Renders the word highlight and interactive popup window on top of the video feed.
///
/// `mine_status` returns the mining-bank state of an entry (shown as a `+` button).
/// `is_mined` returns true if the entry is already in the mined word bank (marked with an icon).
/// Returns the index of the entry whose mining button was clicked, if any.
pub fn draw_dict_popup(
    ui: &mut egui::Ui,
    popup_state: &mut Option<DictPopupState>,
    video_rect: egui::Rect,
    show_highlight: bool,
    mine_status: impl Fn(&super::models::TermEntry) -> crate::bank::MineStatus,
    is_mined: impl Fn(&super::models::TermEntry) -> bool,
) -> Option<usize> {
    let Some(popup) = popup_state else {
        return None;
    };

    if popup.entries.is_empty() {
        return None;
    }

    let mut mine_clicked: Option<usize> = None;

    // 1. Draw glowing highlight over the recognized word boundary (if highlight is not hidden)
    if show_highlight {
        for rect in std::iter::once(&popup.word_rect).chain(popup.extra_word_rects.iter()) {
            ui.painter().rect(
                rect.expand(2.0),
                3.0,
                WORD_HIGHLIGHT_FILL,
                Stroke::new(1.5, WORD_HIGHLIGHT_STROKE),
            );
        }
    }

    // 2. Compute popup window dimensions & smart placement (scaled ~1.7x)
    let desired_popup_width = 980.0f32;
    let desired_popup_height = 760.0f32;

    let placement = calculate_popup_placement(
        popup.word_rect,
        desired_popup_width,
        desired_popup_height,
        video_rect,
    );

    let mut is_pointer_in_popup = false;

    // 3. Render floating popup in Foreground layer
    let area_response = egui::Area::new(egui::Id::new("jitendex_dict_popup"))
        .fixed_pos(placement.pos)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            let frame = egui::Frame::none()
                .fill(POPUP_BG)
                .stroke(Stroke::new(1.5, POPUP_STROKE))
                .rounding(Rounding::same(12.0))
                .shadow(egui::epaint::Shadow {
                    offset: egui::vec2(0.0, 6.0),
                    blur: 16.0,
                    spread: 3.0,
                    color: Color32::from_black_alpha(180),
                })
                .inner_margin(egui::Margin::symmetric(24.0, 18.0));

            let response = frame.show(ui, |ui| {
                ui.set_width(placement.width);
                ui.set_max_height(placement.height);
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 5.0);

                let scroll_max_h = (placement.height - 36.0).max(40.0);

                // While the popup is open, mouse-wheel scrolling anywhere goes to the popup.
                // When the pointer is over the popup, egui's ScrollArea handles it natively;
                // otherwise take the scroll delta (so nothing underneath scrolls) and apply
                // it to the popup's scroll offset ourselves. egui clamps the offset.
                let offset_key = egui::Id::new("jitendex_dict_popup_scroll_offset");
                let pointer_over_popup = match (ui.input(|i| i.pointer.hover_pos()), popup.popup_rect) {
                    (Some(ptr), Some(rect)) => rect.contains(ptr),
                    _ => false,
                };
                let external_scroll = if pointer_over_popup {
                    0.0
                } else {
                    ui.ctx().input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta.y))
                };

                // Start at the top whenever the popup shows a different lookup, or it was not
                // drawn on the previous frame (i.e. it was closed and has been re-opened).
                let session_key = egui::Id::new("jitendex_dict_popup_session");
                let identity = popup_identity(popup);
                let frame_nr = ui.ctx().frame_nr();
                let last: Option<(u64, u64)> = ui.ctx().data(|d| d.get_temp(session_key));
                let is_new_session = match last {
                    Some((last_identity, last_frame)) => {
                        last_identity != identity || last_frame + 1 < frame_nr
                    }
                    None => true,
                };
                ui.ctx().data_mut(|d| d.insert_temp(session_key, (identity, frame_nr)));

                let mut scroll_area = egui::ScrollArea::vertical()
                    .id_source("jitendex_dict_popup_scroll")
                    .auto_shrink([false, false])
                    .max_height(scroll_max_h);
                if is_new_session {
                    scroll_area = scroll_area.vertical_scroll_offset(0.0);
                } else if external_scroll != 0.0 {
                    let current: f32 =
                        ui.ctx().data(|d| d.get_temp(offset_key)).unwrap_or(0.0);
                    scroll_area = scroll_area.vertical_scroll_offset((current - external_scroll).max(0.0));
                }
                let scroll_output = scroll_area.show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 5.0);
                    for (idx, entry) in popup.entries.iter().enumerate() {
                        if render_term_entry(ui, entry, idx, Some(mine_status(entry)), is_mined(entry)) {
                            mine_clicked = Some(idx);
                        }
                    }
                });
                let offset = scroll_output.state.offset.y;
                ui.ctx().data_mut(|d| d.insert_temp(offset_key, offset));
                if external_scroll != 0.0 {
                    // Keep animating smooth scrolling even if the pointer is idle.
                    ui.ctx().request_repaint();
                }
            });

            if let Some(ptr) = ui.input(|i| i.pointer.hover_pos()) {
                if response.response.rect.contains(ptr) {
                    is_pointer_in_popup = true;
                }
            }
        });

    // 4. Update whether the user is interacting with the popup window
    popup.popup_rect = Some(area_response.response.rect);
    popup.is_popup_hovered = is_pointer_in_popup;
    if let Some(ptr) = ui.input(|i| i.pointer.hover_pos()) {
        let now = std::time::Instant::now();
        if is_pointer_in_popup
            || popup.word_rect.expand(6.0).contains(ptr)
            || popup.box_rect.contains(ptr)
        {
            popup.last_hover_time = now;
        }
        if is_pointer_in_popup || pointer_on_word(popup, ptr) {
            popup.last_word_hover_time = now;
        }
    }
    mine_clicked
}

/// Placement result containing the top-left position and dimensions for the popup.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopupPlacement {
    pub pos: egui::Pos2,
    pub width: f32,
    pub height: f32,
}

/// Calculates smart placement for the popup window, keeping it strictly inside video_rect
/// and anchoring directly to the specific line (word_rect) the user is hovering on.
/// Prioritizes rendering at the top of the hovered line unless there is no space.
/// In multiline text, anchors directly to the hovered line rather than the entire OCR box
/// to prevent gaps.
pub fn calculate_popup_placement(
    word_rect: egui::Rect,
    desired_w: f32,
    desired_h: f32,
    video_rect: egui::Rect,
) -> PopupPlacement {
    let margin = 10.0;

    // Available vertical headroom above and below word_rect (the specific hovered line)
    let space_above = (word_rect.min.y - margin - (video_rect.min.y + margin)).max(0.0);
    let space_below = ((video_rect.max.y - margin) - (word_rect.max.y + margin)).max(0.0);

    // Rule:
    // 1. Prioritize rendering at the top if full desired_h fits above.
    // 2. Otherwise, if full desired_h fits below, render at the bottom.
    // 3. Otherwise (neither fits full desired_h), render where there is more space
    //    (preferring above if space_above >= space_below and space_above >= 160.0).
    let (place_above, max_avail_h) = if space_above >= desired_h {
        (true, space_above)
    } else if space_below >= desired_h {
        (false, space_below)
    } else if space_above >= space_below && space_above >= 160.0 {
        (true, space_above)
    } else {
        (false, space_below)
    };

    let actual_h = desired_h.min(max_avail_h).max(60.0);

    let y = if place_above {
        // Place strictly above word_rect (the hovered line)
        word_rect.min.y - margin - actual_h
    } else {
        // Place strictly below word_rect (the hovered line)
        word_rect.max.y + margin
    };

    // Horizontal placement: anchor on the matched word string with an inset
    // so the popup's inner content aligns naturally with the word
    let max_avail_w = (video_rect.width() - 2.0 * margin).max(200.0);
    let actual_w = desired_w.min(max_avail_w);

    let mut x = word_rect.min.x - 16.0;
    x = x.clamp(
        video_rect.min.x + margin,
        (video_rect.max.x - actual_w - margin).max(video_rect.min.x + margin),
    );

    PopupPlacement {
        pos: egui::pos2(x, y),
        width: actual_w,
        height: actual_h,
    }
}

/// Calculates smart placement for the popup window, keeping it strictly inside video_rect.
/// Retained for backwards compatibility in tests and external calls.
pub fn calculate_clamped_popup_pos(
    word_rect: egui::Rect,
    popup_w: f32,
    popup_h: f32,
    video_rect: egui::Rect,
) -> egui::Pos2 {
    calculate_popup_placement(word_rect, popup_w, popup_h, video_rect).pos
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn popup_at(x: f32, char_range: (usize, usize), last_word_hover_time: Instant) -> DictPopupState {
        let word_rect = egui::Rect::from_min_size(egui::pos2(x, 100.0), egui::vec2(40.0, 30.0));
        DictPopupState {
            matched_term: "語".into(),
            source_text: "今日は暑いね".into(),
            char_range,
            word_rect,
            extra_word_rects: vec![],
            box_rect: egui::Rect::from_min_size(egui::pos2(0.0, 100.0), egui::vec2(400.0, 30.0)),
            entries: vec![],
            is_popup_hovered: false,
            popup_rect: None,
            last_hover_time: last_word_hover_time,
            last_word_hover_time,
        }
    }

    #[test]
    fn switching_to_another_word_waits_after_leaving_current_word() {
        let delay = Duration::from_millis(300);
        let on_next_word = egui::pos2(70.0, 110.0);
        let next = popup_at(60.0, (3, 5), Instant::now());

        // Just left the current word: the neighbor must wait out the remaining delay.
        let current = popup_at(0.0, (0, 2), Instant::now());
        let wait = popup_switch_wait(&current, &next, on_next_word, delay).unwrap();
        assert!(wait > Duration::from_millis(250) && wait <= delay);

        // Left long enough ago: switch immediately.
        let stale = Instant::now() - Duration::from_millis(400);
        let current = popup_at(0.0, (0, 2), stale);
        assert_eq!(popup_switch_wait(&current, &next, on_next_word, delay), None);

        // Still on the current word (e.g. overlapping lookup): never switch.
        assert!(popup_switch_wait(&current, &next, egui::pos2(10.0, 110.0), delay).is_some());

        // Same word is always accepted.
        let same = popup_at(0.0, (0, 2), Instant::now());
        let current = popup_at(0.0, (0, 2), Instant::now());
        assert_eq!(popup_switch_wait(&current, &same, egui::pos2(10.0, 110.0), delay), None);
    }

    #[test]
    fn test_calculate_clamped_popup_pos_stays_in_video_rect() {
        let video_rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(800.0, 600.0));
        let word_rect = egui::Rect::from_min_size(egui::pos2(800.0, 650.0), egui::vec2(50.0, 30.0));

        let pos = calculate_clamped_popup_pos(word_rect, 400.0, 300.0, video_rect);

        assert!(pos.x >= video_rect.min.x);
        assert!(pos.x + 400.0 <= video_rect.max.x + 1.0);
        assert!(pos.y >= video_rect.min.y);
        assert!(pos.y + 300.0 <= video_rect.max.y + 1.0);
        // Word is at 650 -> popup should be above the word
        assert!(pos.y < word_rect.min.y);

        // Word at the very top (110) -> no room above, should render below word
        let top_word = egui::Rect::from_min_size(egui::pos2(200.0, 110.0), egui::vec2(50.0, 30.0));
        let pos_top = calculate_clamped_popup_pos(top_word, 400.0, 300.0, video_rect);
        assert!(pos_top.y > top_word.max.y);
    }

    #[test]
    fn test_calculate_popup_placement_follows_hovered_line() {
        let video_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0));
        // Multiline text: line 1 at y: 700..740, line 2 at y: 750..790
        // Hovering on word on line 2:
        let line2_word = egui::Rect::from_min_max(egui::pos2(650.0, 750.0), egui::pos2(750.0, 790.0));

        let placement_above = calculate_popup_placement(line2_word, 980.0, 760.0, video_rect);
        // Popup bottom must be 10px above line 2 (750 - 10 = 740), NOT above line 1 (700)!
        assert!((placement_above.pos.y + placement_above.height - 740.0).abs() < 1.0);
        assert!(placement_above.pos.y >= video_rect.min.y);
        // Horizontal placement anchored to line2_word (650 - 16 = 634)
        assert_eq!(placement_above.pos.x, 634.0);

        // Near top: line 1 at y: 80..120
        let line1_word = egui::Rect::from_min_max(egui::pos2(800.0, 80.0), egui::pos2(880.0, 120.0));
        let placement_below = calculate_popup_placement(line1_word, 980.0, 760.0, video_rect);
        // Popup top must be 10px below line 1 (120 + 10 = 130)
        assert!((placement_below.pos.y - 130.0).abs() < 1.0);
        assert_eq!(placement_below.pos.x, 784.0);
    }
}
