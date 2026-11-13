use super::models::DictPopupState;
use super::render::render_term_entry;
use super::theme;
use eframe::egui::{self, Stroke};

/// Line-width unit of the retro window chrome at popup scale 1.0.
const POPUP_CHROME_UNIT: f32 = 2.0;
/// Height of the footer row (`━━ 1 / 3 ━━ ▼`) below the entry list.
const POPUP_FOOTER_HEIGHT: f32 = 30.0;
/// After the cursor leaves the popup or the highlighted word, hovering a different word
/// only replaces the popup once this much time has passed.
pub const POPUP_SWITCH_DELAY: std::time::Duration = std::time::Duration::from_millis(300);
/// Hover dwell delay before a recursive child popup (`depth > 0`) opens when hovering a word
/// inside an already-open popup. Layer 0 (root OCR popup) opens with no delay.
pub const RECURSIVE_POPUP_HOVER_DELAY: std::time::Duration = std::time::Duration::from_millis(500);
/// Maximum number of stacked dictionary popups (root layer 0 + recursive child layers).
pub const MAX_POPUP_DEPTH: usize = 10;
/// Grace period before an unhovered child popup layer automatically closes.
const CHILD_POPUP_GRACE_PERIOD: std::time::Duration = std::time::Duration::from_millis(450);

fn popup_hover_dwell_key(depth: usize) -> egui::Id {
    egui::Id::new(("jitendex_dict_popup_hover_dwell", depth))
}

fn popup_layer_id(popup_under_crt: bool, depth: usize) -> egui::LayerId {
    match (popup_under_crt, depth) {
        (true, 0) => egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("jitendex_dict_popup_crt"),
        ),
        (false, 0) => egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("jitendex_dict_popup"),
        ),
        (true, d) => egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new(("jitendex_dict_popup_crt", d)),
        ),
        (false, d) => egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new(("jitendex_dict_popup", d)),
        ),
    }
}

fn popup_session_key(depth: usize) -> egui::Id {
    if depth == 0 {
        egui::Id::new("jitendex_dict_popup_session")
    } else {
        egui::Id::new(("jitendex_dict_popup_session", depth))
    }
}

fn popup_offset_key(depth: usize) -> egui::Id {
    if depth == 0 {
        egui::Id::new("jitendex_dict_popup_scroll_offset")
    } else {
        egui::Id::new(("jitendex_dict_popup_scroll_offset", depth))
    }
}

fn popup_scroll_id_source(depth: usize) -> egui::Id {
    if depth == 0 {
        egui::Id::new("jitendex_dict_popup_scroll")
    } else {
        egui::Id::new(("jitendex_dict_popup_scroll", depth))
    }
}

fn paint_word_highlight(
    painter: &egui::Painter,
    rects: impl Iterator<Item = egui::Rect>,
    inv_scale: f32,
) {
    // Dashed gold box, like a VN text cursor selection.
    for rect in rects {
        let r = rect.expand(3.0 * inv_scale);
        painter.rect_filled(r, 0.0, theme::WORD_HIGHLIGHT_FILL);
        let corners = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top(),
        ];
        painter.extend(egui::Shape::dashed_line(
            &corners,
            Stroke::new(2.0 * inv_scale, theme::WORD_HIGHLIGHT_STROKE),
            5.0 * inv_scale,
            3.0 * inv_scale,
        ));
    }
}

fn update_hover_and_expire_children(
    popup: &mut DictPopupState,
    hover_pos: Option<egui::Pos2>,
    now: std::time::Instant,
    depth: usize,
) -> bool {
    let child_active = if let Some(child) = popup.child.as_mut() {
        let active = update_hover_and_expire_children(child, hover_pos, now, depth + 1);
        if !active && child.last_hover_time.elapsed() >= CHILD_POPUP_GRACE_PERIOD {
            popup.child = None;
            false
        } else {
            active
        }
    } else {
        false
    };

    let on_word = hover_pos.is_some_and(|ptr| pointer_on_word(popup, ptr));
    let on_word_expanded = hover_pos.is_some_and(|ptr| popup.word_rect.expand(6.0).contains(ptr));
    let on_box = depth == 0 && hover_pos.is_some_and(|ptr| popup.box_rect.contains(ptr));

    if popup.is_popup_hovered || on_word || on_word_expanded || on_box || child_active {
        popup.last_hover_time = now;
    }
    if popup.is_popup_hovered || on_word || child_active {
        popup.last_word_hover_time = now;
    }

    popup.is_popup_hovered || on_word || on_word_expanded || child_active
}

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

/// User action triggered from an entry inside the popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupAction {
    Mine(usize),
    MineAndClip(usize),
    DeleteCustomName(i64),
}

/// Renders the word highlight and interactive popup window on top of the video feed.
///
/// `mine_status` returns the mining-bank state of an entry (shown as a `+` button).
/// `is_mined` returns true if the entry is already in the mined word bank (marked with an icon).
/// `can_clip` enables drawing the `++` button for Twitch clip creation when the stream is live.
/// Returns the action clicked by the user, if any.
pub fn draw_dict_popup(
    ui: &mut egui::Ui,
    popup_state: &mut Option<DictPopupState>,
    video_rect: egui::Rect,
    show_highlight: bool,
    popup_under_crt: bool,
    scale: f32,
    mine_status: impl Fn(&super::models::TermEntry) -> crate::bank::MineStatus,
    is_mined: impl Fn(&super::models::TermEntry) -> bool,
    can_clip: bool,
) -> (Option<PopupAction>, Vec<egui::ClippedPrimitive>) {
    draw_dict_popup_with_lookup(
        ui,
        popup_state,
        video_rect,
        show_highlight,
        popup_under_crt,
        scale,
        mine_status,
        is_mined,
        can_clip,
        None::<fn(&str, usize) -> Option<(usize, usize, Vec<super::models::TermEntry>)>>,
    )
}

/// Renders the word highlight and interactive popup window stack on top of the video feed,
/// supporting Yomitan-style recursive dictionary lookups when `lookup_fn` is provided.
///
/// Mining buttons (`+` and `++`) are only rendered on the root popup layer (`depth == 0`).
/// Deeper recursive popup layers (`depth > 0`) never display mining buttons.
pub fn draw_dict_popup_with_lookup<F>(
    ui: &mut egui::Ui,
    popup_state: &mut Option<DictPopupState>,
    video_rect: egui::Rect,
    show_highlight: bool,
    popup_under_crt: bool,
    scale: f32,
    mine_status: impl Fn(&super::models::TermEntry) -> crate::bank::MineStatus,
    is_mined: impl Fn(&super::models::TermEntry) -> bool,
    can_clip: bool,
    mut lookup_fn: Option<F>,
) -> (Option<PopupAction>, Vec<egui::ClippedPrimitive>)
where
    F: FnMut(&str, usize) -> Option<(usize, usize, Vec<super::models::TermEntry>)>,
{
    let clear_sessions = |ctx: &egui::Context, from_depth: usize| {
        ctx.data_mut(|d| {
            for depth in from_depth..MAX_POPUP_DEPTH {
                d.remove::<(u64, u64)>(popup_session_key(depth));
                d.remove::<(u64, std::time::Instant, f64)>(popup_hover_dwell_key(depth));
            }
            if from_depth == 0 {
                d.remove::<CachedPopupTessellation>(popup_tess_cache_key());
            }
        });
    };

    if popup_state.as_ref().is_none_or(|p| p.entries.is_empty()) {
        clear_sessions(ui.ctx(), 0);
        return (None, Vec::new());
    }

    let hover_pos = ui.input(|i| i.pointer.hover_pos());
    let any_popup_contains_pointer = hover_pos.is_some_and(|ptr| {
        popup_state
            .as_ref()
            .is_some_and(|p| p.contains_pointer(ptr))
    });
    let root_recently_hovered = popup_state
        .as_ref()
        .is_some_and(|p| p.last_hover_time.elapsed().as_millis() < 50);

    let mut popup_action: Option<PopupAction> = None;
    let mut rendered_layers: Vec<(egui::LayerId, egui::emath::TSTransform)> = Vec::new();
    let mut depth = 0usize;
    let mut current_opt = popup_state.as_mut();

    while let Some(popup) = current_opt {
        if depth >= MAX_POPUP_DEPTH || popup.entries.is_empty() {
            break;
        }

        // 1. Draw glowing highlight over the root recognized word boundary on the video canvas
        if depth == 0 && show_highlight {
            let rects =
                std::iter::once(popup.word_rect).chain(popup.extra_word_rects.iter().copied());
            paint_word_highlight(ui.painter(), rects, 1.0);
        }

        // 2. Compute popup window dimensions & smart placement (scaled ~1.7x)
        let desired_popup_width = 980.0f32 * scale;
        let desired_popup_height = 760.0f32 * scale;

        let placement = calculate_popup_placement(
            popup.word_rect,
            desired_popup_width,
            desired_popup_height,
            video_rect,
        );

        let mut is_pointer_in_popup = false;
        let occluded_by_descendant = hover_pos.is_some_and(|ptr| {
            popup
                .child
                .as_ref()
                .is_some_and(|c| c.contains_pointer(ptr))
        });
        let is_leaf_layer = popup.child.is_none();

        // 3. Render floating popup for this depth
        let layer_id = popup_layer_id(popup_under_crt, depth);

        // Scale the complete popup, including fonts, action buttons, spacing and
        // hit testing. The highlighted OCR word remains in video coordinates.
        let transform = if scale == 1.0 {
            egui::emath::TSTransform::IDENTITY
        } else {
            egui::emath::TSTransform::new(placement.pos.to_vec2(), scale)
        };
        ui.ctx().set_transform_layer(layer_id, transform);
        let local_width = placement.width / scale;
        let local_height = placement.height / scale;

        let session_key = popup_session_key(depth);
        let offset_key = popup_offset_key(depth);
        let scroll_id_source = popup_scroll_id_source(depth);

        let area_response = egui::Area::new(layer_id.id)
            .fixed_pos(if scale == 1.0 {
                placement.pos
            } else {
                egui::Pos2::ZERO
            })
            .order(layer_id.order)
            .show(ui.ctx(), |ui| {
                // Disable text selection inside the popup so clicking and dragging anywhere on
                // definitions/headwords always scrolls the ScrollArea (including catching kinetic
                // momentum) rather than starting a multi-label text selection.
                ui.style_mut().interaction.selectable_labels = false;
                if scale != 1.0 {
                    // Area's initial bounds are in screen coordinates; this layer
                    // lays out the full-size content in its own local coordinates.
                    ui.set_max_size(egui::vec2(local_width + 48.0, local_height + 36.0));
                    ui.set_clip_rect(transform.inverse() * video_rect);
                }
                theme::apply_widget_visuals(ui);
                // Retro window chrome (dithered fill, frame, gems) is painted behind the content
                // once its final size is known.
                let chrome_idx = ui.painter().add(egui::Shape::Noop);
                let frame = egui::Frame::none().inner_margin(egui::Margin::symmetric(24.0, 18.0));

                let response = frame.show(ui, |ui| {
                    ui.set_width(local_width);
                    ui.set_max_height(local_height);
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 5.0);

                    let scroll_max_h = (local_height - 36.0 - POPUP_FOOTER_HEIGHT).max(40.0);

                    // While the popup is open, mouse-wheel scrolling anywhere goes to the popup.
                    // When the pointer is over a popup, that layer's ScrollArea handles it natively;
                    // otherwise take the scroll delta on the deepest layer and apply it to its
                    // scroll offset ourselves. egui clamps the offset.
                    let external_scroll =
                        if is_leaf_layer && !any_popup_contains_pointer && root_recently_hovered {
                            ui.ctx()
                                .input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta.y))
                        } else {
                            0.0
                        };

                    // Start at the top (with zero kinetic momentum) whenever the popup shows a
                    // different lookup, or it was not drawn on the previous frame (i.e. it was
                    // closed and has been re-opened).
                    let identity = popup_identity(popup);
                    let frame_nr = ui.ctx().frame_nr();
                    let last: Option<(u64, u64)> = ui.ctx().data(|d| d.get_temp(session_key));
                    let is_new_session = match last {
                        Some((last_identity, last_frame)) => {
                            last_identity != identity || last_frame + 1 < frame_nr
                        }
                        None => true,
                    };
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(session_key, (identity, frame_nr)));

                    let prev_offset: f32 = ui.ctx().data(|d| d.get_temp(offset_key)).unwrap_or(0.0);
                    let scroll_id = ui.make_persistent_id(egui::Id::new(scroll_id_source));
                    let mut scroll_area = egui::ScrollArea::vertical()
                        .id_source(scroll_id_source)
                        .auto_shrink([false, false])
                        .max_height(scroll_max_h);
                    if is_new_session {
                        egui::scroll_area::State::default().store(ui.ctx(), scroll_id);
                        scroll_area = scroll_area.vertical_scroll_offset(0.0);
                    } else if external_scroll != 0.0 {
                        scroll_area = scroll_area.vertical_scroll_offset(
                            (prev_offset - external_scroll / scale).max(0.0),
                        );
                    }
                    let mut entry_tops = Vec::with_capacity(popup.entries.len());
                    let pointer_down = ui.input(|i| i.pointer.any_down());
                    let probe_local_pos = if lookup_fn.is_some()
                        && depth + 1 < MAX_POPUP_DEPTH
                        && popup.popup_rect.is_some()
                        && !occluded_by_descendant
                        && !pointer_down
                    {
                        hover_pos
                            .filter(|&ptr| popup.popup_rect.is_some_and(|r| r.contains(ptr)))
                            .map(|ptr| transform.inverse() * ptr)
                    } else {
                        None
                    };

                    let mut button_clicked_this_layer = false;
                    let scroll_output = scroll_area.show(ui, |ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 5.0);
                        let scroll_clip = ui.clip_rect();
                        let (_, text_hit) = super::render::with_popup_hover_probe(
                            probe_local_pos,
                            scroll_clip,
                            || {
                                for (idx, entry) in popup.entries.iter().enumerate() {
                                    entry_tops.push(ui.cursor().top());
                                    // Mining buttons (+ and ++) ONLY show on the first/bottomest
                                    // layer (depth == 0). Recursive popups cannot be mined.
                                    let entry_mine_status = if depth == 0 {
                                        Some(mine_status(entry))
                                    } else {
                                        None
                                    };
                                    let entry_can_clip = depth == 0 && can_clip;
                                    let action = render_term_entry(
                                        ui,
                                        entry,
                                        idx,
                                        entry_mine_status,
                                        is_mined(entry),
                                        entry_can_clip,
                                    );
                                    if !occluded_by_descendant {
                                        match action {
                                            crate::dict::render::EntryAction::Mine
                                                if depth == 0 =>
                                            {
                                                popup_action = Some(PopupAction::Mine(idx));
                                                button_clicked_this_layer = true;
                                            }
                                            crate::dict::render::EntryAction::MineAndClip
                                                if depth == 0 =>
                                            {
                                                popup_action = Some(PopupAction::MineAndClip(idx));
                                                button_clicked_this_layer = true;
                                            }
                                            crate::dict::render::EntryAction::DeleteCustomName(
                                                id,
                                            ) => {
                                                popup_action =
                                                    Some(PopupAction::DeleteCustomName(id));
                                                button_clicked_this_layer = true;
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            },
                        );

                        let dwell_key = popup_hover_dwell_key(depth);
                        let mut matched_any_word = false;
                        if let (Some(hit), Some(lookup), Some(ptr)) =
                            (text_hit, lookup_fn.as_mut(), hover_pos)
                        {
                            let text: String = hit.chars.iter().map(|pc| pc.ch).collect();
                            if let Some((start_char, end_char, entries)) =
                                lookup(&text, hit.hovered_char_idx)
                            {
                                let end_char = end_char.min(hit.chars.len());
                                if start_char < end_char && !entries.is_empty() {
                                    let matched_slice = &hit.chars[start_char..end_char];
                                    let hovered_row = hit.chars[hit.hovered_char_idx].row_idx;
                                    let mut primary_rect = egui::Rect::NOTHING;
                                    let mut row_rects: Vec<(usize, egui::Rect)> = Vec::new();
                                    for pc in matched_slice {
                                        if !pc.rect.is_positive() {
                                            continue;
                                        }
                                        if pc.row_idx == hovered_row {
                                            primary_rect = primary_rect.union(pc.rect);
                                        } else if let Some((_, r)) =
                                            row_rects.iter_mut().find(|(row, _)| *row == pc.row_idx)
                                        {
                                            *r = r.union(pc.rect);
                                        } else {
                                            row_rects.push((pc.row_idx, pc.rect));
                                        }
                                    }
                                    if !primary_rect.is_positive() {
                                        for (_, r) in &row_rects {
                                            primary_rect = primary_rect.union(*r);
                                        }
                                        row_rects.clear();
                                    }
                                    if primary_rect.is_positive() {
                                        matched_any_word = true;
                                        let word_rect = transform * primary_rect;
                                        let extra_word_rects: Vec<egui::Rect> = row_rects
                                            .into_iter()
                                            .map(|(_, r)| transform * r)
                                            .collect();
                                        let now = std::time::Instant::now();
                                        let now_egui = ui.input(|i| i.time);
                                        let candidate = DictPopupState {
                                            matched_term: entries[0].term.clone(),
                                            source_text: text,
                                            char_range: (start_char, end_char),
                                            word_rect,
                                            extra_word_rects,
                                            box_rect: word_rect,
                                            entries,
                                            is_popup_hovered: false,
                                            popup_rect: None,
                                            last_hover_time: now,
                                            last_word_hover_time: now,
                                            child: None,
                                        };
                                        let target_hash = {
                                            use std::hash::{Hash, Hasher};
                                            let mut h =
                                                std::collections::hash_map::DefaultHasher::new();
                                            candidate.source_text.hash(&mut h);
                                            candidate.char_range.hash(&mut h);
                                            (primary_rect.min.x.round() as i32).hash(&mut h);
                                            (primary_rect.min.y.round() as i32).hash(&mut h);
                                            h.finish()
                                        };
                                        let same_as_open_child =
                                            popup.child.as_ref().is_some_and(|existing| {
                                                existing.source_text == candidate.source_text
                                                    && existing.char_range == candidate.char_range
                                                    && (existing.word_rect.center()
                                                        - candidate.word_rect.center())
                                                    .length()
                                                        < 8.0
                                            });
                                        if same_as_open_child {
                                            if let Some(existing) = popup.child.as_mut() {
                                                existing.word_rect = candidate.word_rect;
                                                existing.extra_word_rects =
                                                    candidate.extra_word_rects;
                                                existing.box_rect = candidate.box_rect;
                                                existing.last_hover_time = now;
                                                existing.last_word_hover_time = now;
                                            }
                                            ui.ctx().data_mut(|d| {
                                                d.insert_temp(
                                                    dwell_key,
                                                    (target_hash, now, now_egui),
                                                );
                                            });
                                        } else {
                                            let prev_dwell: Option<(u64, std::time::Instant, f64)> =
                                                ui.ctx().data(|d| d.get_temp(dwell_key));
                                            match prev_dwell {
                                                Some((prev_hash, start_instant, start_egui))
                                                    if prev_hash == target_hash =>
                                                {
                                                    let wall_elapsed = now
                                                        .saturating_duration_since(start_instant);
                                                    let egui_elapsed =
                                                        std::time::Duration::from_secs_f64(
                                                            (now_egui - start_egui).max(0.0),
                                                        );
                                                    let elapsed = wall_elapsed.max(egui_elapsed);
                                                    if elapsed >= RECURSIVE_POPUP_HOVER_DELAY {
                                                        if let Some(existing) = popup.child.as_ref()
                                                        {
                                                            if let Some(wait) = popup_switch_wait(
                                                                existing,
                                                                &candidate,
                                                                ptr,
                                                                POPUP_SWITCH_DELAY,
                                                            ) {
                                                                ui.ctx()
                                                                    .request_repaint_after(wait);
                                                            } else {
                                                                popup.child =
                                                                    Some(Box::new(candidate));
                                                            }
                                                        } else {
                                                            popup.child = Some(Box::new(candidate));
                                                        }
                                                    } else {
                                                        ui.ctx().request_repaint_after(
                                                            RECURSIVE_POPUP_HOVER_DELAY - elapsed,
                                                        );
                                                    }
                                                }
                                                _ => {
                                                    ui.ctx().data_mut(|d| {
                                                        d.insert_temp(
                                                            dwell_key,
                                                            (target_hash, now, now_egui),
                                                        );
                                                    });
                                                    ui.ctx().request_repaint_after(
                                                        RECURSIVE_POPUP_HOVER_DELAY,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if !matched_any_word && !occluded_by_descendant {
                            ui.ctx().data_mut(|d| {
                                d.remove::<(u64, std::time::Instant, f64)>(dwell_key);
                            });
                        }

                        // Paint highlight over the word in this popup that opened `popup.child`.
                        if show_highlight {
                            if let Some(child) = popup.child.as_ref() {
                                let inv_t = transform.inverse();
                                let rects = std::iter::once(inv_t * child.word_rect)
                                    .chain(child.extra_word_rects.iter().map(|&r| inv_t * r));
                                paint_word_highlight(ui.painter(), rects, 1.0 / scale.max(0.01));
                            }
                        }
                    });
                    debug_assert_eq!(scroll_output.id, scroll_id);
                    let offset = scroll_output.state.offset.y;
                    ui.ctx().data_mut(|d| d.insert_temp(offset_key, offset));

                    // Dismiss child popups if this parent popup is scrolled or clicked directly.
                    if !is_new_session && (offset - prev_offset).abs() > 0.5 {
                        popup.child = None;
                    }

                    // Footer: which entry is at the top of the view, plus the blinking page cursor.
                    let viewport_top = scroll_output.inner_rect.top();
                    let current = entry_tops
                        .iter()
                        .rposition(|&top| top <= viewport_top + 24.0)
                        .unwrap_or(0)
                        + 1;
                    let more_below = offset + scroll_output.inner_rect.height()
                        < scroll_output.content_size.y - 1.0;
                    draw_popup_footer(ui, current, popup.entries.len(), more_below);
                    if external_scroll != 0.0 {
                        // Keep animating smooth scrolling even if the pointer is idle.
                        ui.ctx().request_repaint();
                    }
                    button_clicked_this_layer
                });

                let time = ui.input(|i| i.time);
                ui.painter().set(
                    chrome_idx,
                    egui::Shape::Vec(theme::window_shapes(
                        ui.ctx(),
                        response.response.rect,
                        POPUP_CHROME_UNIT,
                        1.0 / scale.max(0.01),
                        Some(time),
                    )),
                );
                theme::request_animation_repaint(ui.ctx(), time);

                let screen_popup_rect = transform * response.response.rect;
                if let Some(ptr) = hover_pos {
                    if screen_popup_rect.contains(ptr) {
                        is_pointer_in_popup = true;
                        if !occluded_by_descendant
                            && !response.inner
                            && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary))
                        {
                            popup.child = None;
                        }
                    }
                }
            });

        popup.popup_rect = Some(transform * area_response.response.rect);
        popup.is_popup_hovered = is_pointer_in_popup && !occluded_by_descendant;
        rendered_layers.push((layer_id, transform));

        current_opt = popup.child.as_deref_mut();
        depth += 1;
    }

    clear_sessions(ui.ctx(), depth);

    let primitives = if popup_under_crt {
        // Steal the painted shapes from all popup layers in depth order (0, 1, 2, ...) for CRT
        // tessellation and replace them in-place with `Shape::Noop` (rather than `std::mem::take`ing
        // the `PaintList` to length 0) so egui does not paint them in the foreground AND any
        // `on_end_frame` plugin (such as `LabelSelectionState::end_frame`) holding a `ShapeIdx`
        // into `layer_id` cannot panic with an out-of-bounds index.
        let mut all_clipped_shapes = Vec::new();
        for &(layer_id, transform) in &rendered_layers {
            let mut clipped_shapes = ui
                .ctx()
                .graphics_mut(|g| {
                    g.get_mut(layer_id).map(|list| {
                        let shapes = list.all_entries().cloned().collect::<Vec<_>>();
                        for idx in 0..shapes.len() {
                            list.reset_shape(egui::layers::ShapeIdx(idx));
                        }
                        shapes
                    })
                })
                .unwrap_or_default();
            for clipped in &mut clipped_shapes {
                clipped.clip_rect = transform * clipped.clip_rect;
                clipped.shape.transform(transform);
            }
            all_clipped_shapes.extend(clipped_shapes);
        }
        if all_clipped_shapes.is_empty() {
            Vec::new()
        } else {
            tessellate_popup_shapes_cached(
                ui.ctx(),
                all_clipped_shapes,
                ui.ctx().pixels_per_point(),
                video_rect,
                scale,
                show_highlight,
                can_clip,
                rendered_layers.len(),
            )
        }
    } else {
        Vec::new()
    };

    // Keep the dictionary popups above CentralPanel within Order::Background in depth order
    // (0, 1, 2, ... so deeper layers sit above shallower layers), while staying beneath
    // Order::PanelResizeLine (whiteboard) and Order::Middle (dialogs).
    for &(layer_id, _) in &rendered_layers {
        ui.ctx().move_to_top(layer_id);
    }

    if let Some(root) = popup_state.as_mut() {
        update_hover_and_expire_children(root, hover_pos, std::time::Instant::now(), 0);
    }

    (popup_action, primitives)
}

#[derive(Clone)]
struct CachedPopupTessellation {
    signature: u64,
    primitives: std::sync::Arc<[egui::ClippedPrimitive]>,
}

#[inline]
fn popup_tess_cache_key() -> egui::Id {
    egui::Id::new("dict_popup_crt_tess_cache")
}

#[cfg(test)]
thread_local! {
    static POPUP_TESS_MISS_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn hash_rect_bits(rect: egui::Rect, hasher: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    rect.min.x.to_bits().hash(hasher);
    rect.min.y.to_bits().hash(hasher);
    rect.max.x.to_bits().hash(hasher);
    rect.max.y.to_bits().hash(hasher);
}

fn hash_shape_for_tessellation(
    shape: &egui::Shape,
    hasher: &mut impl std::hash::Hasher,
    cacheable: &mut bool,
) {
    use std::hash::Hash;
    match shape {
        egui::Shape::Noop => {
            0u8.hash(hasher);
        }
        egui::Shape::Vec(shapes) => {
            1u8.hash(hasher);
            shapes.len().hash(hasher);
            for child in shapes {
                hash_shape_for_tessellation(child, hasher, cacheable);
                if !*cacheable {
                    return;
                }
            }
        }
        egui::Shape::Circle(c) => {
            2u8.hash(hasher);
            c.center.x.to_bits().hash(hasher);
            c.center.y.to_bits().hash(hasher);
            c.radius.to_bits().hash(hasher);
            c.fill.to_array().hash(hasher);
            c.stroke.width.to_bits().hash(hasher);
            c.stroke.color.to_array().hash(hasher);
        }
        egui::Shape::LineSegment { points, stroke } => {
            3u8.hash(hasher);
            points[0].x.to_bits().hash(hasher);
            points[0].y.to_bits().hash(hasher);
            points[1].x.to_bits().hash(hasher);
            points[1].y.to_bits().hash(hasher);
            stroke.width.to_bits().hash(hasher);
            stroke.color.to_array().hash(hasher);
        }
        egui::Shape::Path(p) => {
            4u8.hash(hasher);
            p.points.len().hash(hasher);
            for pt in &p.points {
                pt.x.to_bits().hash(hasher);
                pt.y.to_bits().hash(hasher);
            }
            p.closed.hash(hasher);
            p.fill.to_array().hash(hasher);
            p.stroke.width.to_bits().hash(hasher);
        }
        egui::Shape::Rect(r) => {
            5u8.hash(hasher);
            hash_rect_bits(r.rect, hasher);
            r.rounding.nw.to_bits().hash(hasher);
            r.rounding.ne.to_bits().hash(hasher);
            r.rounding.sw.to_bits().hash(hasher);
            r.rounding.se.to_bits().hash(hasher);
            r.fill.to_array().hash(hasher);
            r.stroke.width.to_bits().hash(hasher);
            r.stroke.color.to_array().hash(hasher);
            r.fill_texture_id.hash(hasher);
        }
        egui::Shape::Text(t) => {
            6u8.hash(hasher);
            t.pos.x.to_bits().hash(hasher);
            t.pos.y.to_bits().hash(hasher);
            t.fallback_color.to_array().hash(hasher);
            t.override_text_color.map(|c| c.to_array()).hash(hasher);
            t.underline.width.to_bits().hash(hasher);
            t.underline.color.to_array().hash(hasher);
            t.angle.to_bits().hash(hasher);
            t.galley.job.text.hash(hasher);
            hash_rect_bits(t.galley.mesh_bounds, hasher);
            t.galley.rows.len().hash(hasher);
            for row in &t.galley.rows {
                let verts = &row.visuals.mesh.vertices;
                verts.len().hash(hasher);
                if let Some(first) = verts.first() {
                    first.pos.x.to_bits().hash(hasher);
                    first.pos.y.to_bits().hash(hasher);
                    first.uv.x.to_bits().hash(hasher);
                    first.uv.y.to_bits().hash(hasher);
                    first.color.to_array().hash(hasher);
                }
                if let Some(last) = verts.last() {
                    last.pos.x.to_bits().hash(hasher);
                    last.pos.y.to_bits().hash(hasher);
                    last.uv.x.to_bits().hash(hasher);
                    last.uv.y.to_bits().hash(hasher);
                    last.color.to_array().hash(hasher);
                }
            }
        }
        egui::Shape::Mesh(m) => {
            7u8.hash(hasher);
            m.texture_id.hash(hasher);
            m.indices.len().hash(hasher);
            m.vertices.len().hash(hasher);
            if let Some(first) = m.vertices.first() {
                first.pos.x.to_bits().hash(hasher);
                first.pos.y.to_bits().hash(hasher);
                first.uv.x.to_bits().hash(hasher);
                first.uv.y.to_bits().hash(hasher);
                first.color.to_array().hash(hasher);
            }
            if let Some(last) = m.vertices.last() {
                last.pos.x.to_bits().hash(hasher);
                last.pos.y.to_bits().hash(hasher);
                last.uv.x.to_bits().hash(hasher);
                last.uv.y.to_bits().hash(hasher);
                last.color.to_array().hash(hasher);
            }
        }
        _ => {
            *cacheable = false;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn tessellate_popup_shapes_cached(
    ctx: &egui::Context,
    clipped_shapes: Vec<egui::epaint::ClippedShape>,
    ppp: f32,
    video_rect: egui::Rect,
    scale: f32,
    show_highlight: bool,
    can_clip: bool,
    layer_count: usize,
) -> Vec<egui::ClippedPrimitive> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    ppp.to_bits().hash(&mut hasher);
    hash_rect_bits(video_rect, &mut hasher);
    scale.to_bits().hash(&mut hasher);
    show_highlight.hash(&mut hasher);
    can_clip.hash(&mut hasher);
    layer_count.hash(&mut hasher);
    clipped_shapes.len().hash(&mut hasher);

    let mut cacheable = true;
    for cs in &clipped_shapes {
        hash_rect_bits(cs.clip_rect, &mut hasher);
        hash_shape_for_tessellation(&cs.shape, &mut hasher, &mut cacheable);
        if !cacheable {
            break;
        }
    }

    if cacheable {
        let signature = hasher.finish();
        let cache_id = popup_tess_cache_key();
        if let Some(hit) = ctx.data(|d| {
            d.get_temp::<CachedPopupTessellation>(cache_id)
                .filter(|entry| entry.signature == signature)
        }) {
            return hit.primitives.to_vec();
        }

        #[cfg(test)]
        POPUP_TESS_MISS_COUNT.with(|c| c.set(c.get() + 1));

        let primitives = ctx.tessellate(clipped_shapes, ppp);
        let shared: std::sync::Arc<[egui::ClippedPrimitive]> = primitives.clone().into();
        ctx.data_mut(|d| {
            d.insert_temp(
                cache_id,
                CachedPopupTessellation {
                    signature,
                    primitives: shared,
                },
            );
        });
        primitives
    } else {
        #[cfg(test)]
        POPUP_TESS_MISS_COUNT.with(|c| c.set(c.get() + 1));
        ctx.tessellate(clipped_shapes, ppp)
    }
}

/// Footer row of the popup: `━━━ 1 / 3 ━━━ ▼`. The page cursor blinks (gold while more
/// content is below, dim violet once the end is reached).
fn draw_popup_footer(ui: &mut egui::Ui, current: usize, total: usize, more_below: bool) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), POPUP_FOOTER_HEIGHT),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    let center = rect.center();
    let label = format!("{current} / {total}");
    let galley = painter.layout_no_wrap(label, super::render::dict_font(22.0), theme::MUTED);
    let half = galley.size().x * 0.5 + 14.0;
    let rule_len = 150.0f32.min(rect.width() * 0.3);
    let stroke = Stroke::new(3.0, theme::RULE);
    painter.hline(
        center.x - half - rule_len..=center.x - half,
        center.y,
        stroke,
    );
    painter.hline(
        center.x + half..=center.x + half + rule_len,
        center.y,
        stroke,
    );
    painter.galley(center - galley.size() * 0.5, galley, theme::MUTED);

    if theme::blink_on(ui.input(|i| i.time)) {
        theme::paint_shadow_text(
            painter,
            egui::pos2(center.x + half + rule_len + 22.0, center.y),
            egui::Align2::CENTER_CENTER,
            "▼",
            super::render::dict_font(24.0),
            if more_below { theme::GOLD } else { theme::RULE },
            2.0,
        );
    }
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
    // 3. Otherwise (neither fits full desired_h), render where there is more space.
    let (place_above, max_avail_h) = if space_above >= desired_h {
        (true, space_above)
    } else if space_below >= desired_h {
        (false, space_below)
    } else if space_above >= space_below {
        (true, space_above)
    } else {
        (false, space_below)
    };

    let actual_h = desired_h.min(max_avail_h).max(60.0);

    let y = if place_above {
        // Place strictly above word_rect (the hovered line)
        (word_rect.min.y - margin - actual_h).max(video_rect.min.y + margin)
    } else {
        // Place strictly below word_rect (the hovered line)
        (word_rect.max.y + margin)
            .min((video_rect.max.y - margin - actual_h).max(video_rect.min.y + margin))
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

/// Calculates the exact visible CRT raster surface bounds in egui screen coordinates.
/// Accounts for aspect-fitted video rect, border crops, and underscan/overscan.
/// This represents the physical CRT screen surface (raster) without safe text margin insets.
pub fn calculate_crt_raster_surface(
    video_rect: egui::Rect,
    border_crop: [f32; 4], // [left, right, top, bottom]
    underscan: [f32; 2],   // [underscan_x, underscan_y]
    overscan: [f32; 2],    // [overscan_x, overscan_y]
) -> egui::Rect {
    let w = video_rect.width();
    let h = video_rect.height();

    // 1. Account for underscan stretch & overscan offsets inside the raster
    let sx = (1.0 + underscan[0]).max(0.01);
    let sy = (1.0 + underscan[1]).max(0.01);
    let ox = overscan[0];
    let oy = overscan[1];

    let vis_u_min = (0.5 + (ox - 0.5) * sx).clamp(0.0, 1.0);
    let vis_u_max = (0.5 + (ox + 0.5) * sx).clamp(0.0, 1.0);
    let vis_v_min = (0.5 + (oy - 0.5) * sy).clamp(0.0, 1.0);
    let vis_v_max = (0.5 + (oy + 0.5) * sy).clamp(0.0, 1.0);

    // 2. Account for user border cropping ([left, right, top, bottom])
    let crop_u_min = border_crop[0].clamp(0.0, 0.45);
    let crop_u_max = (1.0 - border_crop[1].clamp(0.0, 0.45)).clamp(0.0, 1.0);
    let crop_v_min = border_crop[2].clamp(0.0, 0.45);
    let crop_v_max = (1.0 - border_crop[3].clamp(0.0, 0.45)).clamp(0.0, 1.0);

    // Combined active video content boundaries in normalized coordinates [0, 1]
    let u_start = vis_u_min.max(crop_u_min);
    let u_end = vis_u_max.min(crop_u_max);
    let v_start = vis_v_min.max(crop_v_min);
    let v_end = vis_v_max.min(crop_v_max);

    let mut min = egui::pos2(
        video_rect.min.x + w * u_start,
        video_rect.min.y + h * v_start,
    );
    let mut max = egui::pos2(video_rect.min.x + w * u_end, video_rect.min.y + h * v_end);

    // Ensure rect is valid and non-inverted
    if min.x >= max.x {
        let mid = (video_rect.min.x + video_rect.max.x) * 0.5;
        min.x = mid - 50.0;
        max.x = mid + 50.0;
    }
    if min.y >= max.y {
        let mid = (video_rect.min.y + video_rect.max.y) * 0.5;
        min.y = mid - 50.0;
        max.y = mid + 50.0;
    }

    egui::Rect::from_min_max(min, max)
}

/// Calculates the safe area boundary EXCLUSIVELY for positioning the floating dictionary popup window.
///
/// **IMPORTANT NOTICE**:
/// - This function is **ONLY** for dictionary popup window layout (ensuring definition popups
///   don't get clipped by retro PC bezel frames or glass curvature corners).
/// - **DO NOT USE THIS** for video viewports, full-screen canvas layers, or scrolling text
///   overlays (such as Niconico comments)!
/// - For the active video picture bounds, use `video_rect` directly (or `calculate_crt_raster_surface`
///   for raster edges without popup safety insets).
pub fn calculate_crt_popup_safe_area(
    video_rect: egui::Rect,
    border_crop: [f32; 4], // [left, right, top, bottom]
    underscan: [f32; 2],   // [underscan_x, underscan_y]
    overscan: [f32; 2],    // [overscan_x, overscan_y]
    retro_pc_frame: bool,
    curvature_active: bool,
    _ppp: f32,
) -> egui::Rect {
    let raster = calculate_crt_raster_surface(video_rect, border_crop, underscan, overscan);
    let mut min = raster.min;
    let mut max = raster.max;
    let w = video_rect.width();
    let h = video_rect.height();

    // 3. Account for retro PC bezel frame if active (CRT is recessed inside plastic chassis)
    if retro_pc_frame {
        // In retro PC frame mode, the bezel frame occupies roughly:
        // Top ~ 6.5%, Bottom ~ 10.0%, Left/Right ~ 5.5% of the video canvas
        min.y += h * 0.065;
        max.y -= h * 0.100;
        min.x += w * 0.055;
        max.x -= w * 0.055;
    }

    // 4. Account for CRT glass curvature safe margins (barrel distortion curves inwards)
    if curvature_active {
        // Edge curvature pulls corners and outer edges inward; inset safe margins ~2.5%
        min.x += w * 0.025;
        max.x -= w * 0.025;
        min.y += h * 0.025;
        max.y -= h * 0.025;
    }

    // Ensure rect is valid and non-inverted
    if min.x >= max.x {
        let mid = (video_rect.min.x + video_rect.max.x) * 0.5;
        min.x = mid - 50.0;
        max.x = mid + 50.0;
    }
    if min.y >= max.y {
        let mid = (video_rect.min.y + video_rect.max.y) * 0.5;
        min.y = mid - 50.0;
        max.y = mid + 50.0;
    }

    egui::Rect::from_min_max(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn deck_scaled_popup_keeps_mining_hit_target_and_crt_geometry_aligned() {
        for under_crt in [false, true] {
            let ctx = egui::Context::default();
            let mut fonts = egui::FontDefinitions::default();
            let family = fonts.families[&egui::FontFamily::Proportional].clone();
            fonts
                .families
                .insert(egui::FontFamily::Name("GothicCJK".into()), family);
            ctx.set_fonts(fonts);
            let video = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(591.0, 369.0));
            let word = egui::Rect::from_min_size(egui::pos2(200.0, 180.0), egui::vec2(40.0, 20.0));
            let mut popup = Some(DictPopupState {
                matched_term: "test".into(),
                source_text: "test".into(),
                char_range: (0, 4),
                word_rect: word,
                extra_word_rects: vec![],
                box_rect: word,
                entries: vec![super::super::models::TermEntry {
                    term: "test".into(),
                    reading: String::new(),
                    definition_tags: None,
                    rules: String::new(),
                    score: 0.0,
                    glossary: vec![super::super::models::GlossaryEntry::Text(
                        "A compact definition".into(),
                    )],
                    sequence: 1,
                    term_tags: None,
                    inflection_reasons: vec![],
                    frequency: None,
                    pitches: vec![],
                    custom_name_id: None,
                    custom_tag: None,
                }],
                is_popup_hovered: false,
                popup_rect: None,
                last_hover_time: Instant::now(),
                last_word_hover_time: Instant::now(),
                child: None,
            });
            let mut draw = |events: Vec<egui::Event>| {
                let mut result = (None, Vec::new());
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(video),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            result = draw_dict_popup(
                                ui,
                                &mut popup,
                                video,
                                true,
                                under_crt,
                                0.2,
                                |_| crate::bank::MineStatus::Available,
                                |_| false,
                                false,
                            );
                        });
                    },
                );
                (result, popup.as_ref().unwrap().popup_rect.unwrap())
            };
            let mut rect = video;
            for frame in 0..4 {
                let (result, bounds) = draw(vec![]);
                rect = bounds;
                assert_eq!(result.0, None);
                // Area uses its first frame to measure the popup invisibly.
                if frame > 0 {
                    assert_eq!(!result.1.is_empty(), under_crt);
                }
            }
            assert!(rect.width() < 220.0, "{rect:?}");
            assert!(rect.height() < 170.0, "{rect:?}");
            assert!(video.contains_rect(rect), "{rect:?}");
            let point = egui::pos2(
                rect.right() - (24.0 + 28.0) * 0.2,
                rect.top() + (18.0 + 28.0) * 0.2,
            );
            for pressed in [true, false] {
                let ((action, _), _) = draw(vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                if !pressed {
                    assert_eq!(action, Some(PopupAction::Mine(0)));
                }
            }
        }
    }

    #[test]
    fn test_calculate_crt_popup_safe_area_insets() {
        let video_rect =
            egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(1000.0, 800.0));
        let crop = [0.05, 0.05, 0.10, 0.10]; // 5% L/R (50px), 10% T/B (80px)
        let vp = calculate_crt_popup_safe_area(
            video_rect,
            crop,
            [0.0, 0.0],
            [0.0, 0.0],
            false,
            false,
            1.0,
        );
        assert_eq!(vp.min.x, 150.0);
        assert_eq!(vp.max.x, 1050.0);
        assert_eq!(vp.min.y, 180.0);
        assert_eq!(vp.max.y, 820.0);

        // With retro frame and curvature
        let vp_retro = calculate_crt_popup_safe_area(
            video_rect,
            [0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
            true,
            true,
            1.0,
        );
        assert!(vp_retro.min.x > video_rect.min.x);
        assert!(vp_retro.max.x < video_rect.max.x);
        assert!(vp_retro.min.y > video_rect.min.y);
        assert!(vp_retro.max.y < video_rect.max.y);
    }

    #[test]
    fn test_calculate_crt_raster_surface_bounds() {
        let video_rect =
            egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(1000.0, 800.0));
        let surface =
            calculate_crt_raster_surface(video_rect, [0.0, 0.0, 0.0, 0.0], [0.0, 0.0], [0.0, 0.0]);
        // Default raster surface must match the entire video rect without safe margin shrinkage
        assert_eq!(surface, video_rect);

        // Border crop
        let cropped = calculate_crt_raster_surface(
            video_rect,
            [0.05, 0.05, 0.10, 0.10],
            [0.0, 0.0],
            [0.0, 0.0],
        );
        assert_eq!(cropped.min.x, 150.0);
        assert_eq!(cropped.max.x, 1050.0);
        assert_eq!(cropped.min.y, 180.0);
        assert_eq!(cropped.max.y, 820.0);

        // Viewport with retro frame must be strictly smaller than raster surface
        let vp_retro = calculate_crt_popup_safe_area(
            video_rect,
            [0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0],
            true,
            true,
            1.0,
        );
        assert!(vp_retro.min.x > surface.min.x);
        assert!(vp_retro.max.x < surface.max.x);
        assert!(vp_retro.min.y > surface.min.y);
        assert!(vp_retro.max.y < surface.max.y);
    }

    #[test]
    fn test_calculate_crt_popup_safe_area_underscan_and_overscan() {
        let video_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1000.0));
        // Underscan -0.1 (shrunk by 10% -> 5% inset on left and right)
        let vp_under = calculate_crt_popup_safe_area(
            video_rect,
            [0.0, 0.0, 0.0, 0.0],
            [-0.1, -0.2],
            [0.0, 0.0],
            false,
            false,
            1.0,
        );
        assert!((vp_under.min.x - 50.0).abs() < 0.01);
        assert!((vp_under.max.x - 950.0).abs() < 0.01);
        assert!((vp_under.min.y - 100.0).abs() < 0.01);
        assert!((vp_under.max.y - 900.0).abs() < 0.01);

        // Overscan +0.05 X (shifted right by 50px)
        let vp_over = calculate_crt_popup_safe_area(
            video_rect,
            [0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0],
            [0.05, 0.0],
            false,
            false,
            1.0,
        );
        assert!((vp_over.min.x - 50.0).abs() < 0.01);
        assert!((vp_over.max.x - 1000.0).abs() < 0.01);
    }

    fn popup_at(
        x: f32,
        char_range: (usize, usize),
        last_word_hover_time: Instant,
    ) -> DictPopupState {
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
            child: None,
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
        assert_eq!(
            popup_switch_wait(&current, &next, on_next_word, delay),
            None
        );

        // Still on the current word (e.g. overlapping lookup): never switch.
        assert!(popup_switch_wait(&current, &next, egui::pos2(10.0, 110.0), delay).is_some());

        // Same word is always accepted.
        let same = popup_at(0.0, (0, 2), Instant::now());
        let current = popup_at(0.0, (0, 2), Instant::now());
        assert_eq!(
            popup_switch_wait(&current, &same, egui::pos2(10.0, 110.0), delay),
            None
        );
    }

    #[test]
    fn test_calculate_clamped_popup_pos_stays_in_video_rect() {
        let video_rect =
            egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(800.0, 600.0));
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
        let video_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0));
        // Multiline text: line 1 at y: 700..740, line 2 at y: 750..790
        // Hovering on word on line 2:
        let line2_word =
            egui::Rect::from_min_max(egui::pos2(650.0, 750.0), egui::pos2(750.0, 790.0));

        let placement_above = calculate_popup_placement(line2_word, 980.0, 760.0, video_rect);
        // Popup bottom must be 10px above line 2 (750 - 10 = 740), NOT above line 1 (700)!
        assert!((placement_above.pos.y + placement_above.height - 740.0).abs() < 1.0);
        assert!(placement_above.pos.y >= video_rect.min.y);
        // Horizontal placement anchored to line2_word (650 - 16 = 634)
        assert_eq!(placement_above.pos.x, 634.0);

        // Near top: line 1 at y: 80..120
        let line1_word =
            egui::Rect::from_min_max(egui::pos2(800.0, 80.0), egui::pos2(880.0, 120.0));
        let placement_below = calculate_popup_placement(line1_word, 980.0, 760.0, video_rect);
        // Popup top must be 10px below line 1 (120 + 10 = 130)
        assert!((placement_below.pos.y - 130.0).abs() < 1.0);
        assert_eq!(placement_below.pos.x, 784.0);
    }

    #[test]
    fn test_draw_dict_popup_can_clip_flag() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        let prop = fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        fonts
            .families
            .insert(egui::FontFamily::Name("GothicCJK".into()), prop);
        ctx.set_fonts(fonts);

        let video_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0));
        let mut popup_state = Some(DictPopupState {
            matched_term: "飽きる".into(),
            source_text: "飽きる".into(),
            char_range: (0, 3),
            word_rect: egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(40.0, 30.0)),
            extra_word_rects: vec![],
            box_rect: egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(100.0, 30.0)),
            entries: vec![super::super::models::TermEntry {
                term: "飽きる".into(),
                reading: "あきる".into(),
                definition_tags: None,
                rules: String::new(),
                score: 0.0,
                glossary: vec![],
                sequence: 1,
                term_tags: None,
                inflection_reasons: vec![],
                frequency: None,
                pitches: vec![],
                custom_name_id: None,
                custom_tag: None,
            }],
            is_popup_hovered: false,
            popup_rect: None,
            last_hover_time: Instant::now(),
            last_word_hover_time: Instant::now(),
            child: None,
        });

        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (action, _) = draw_dict_popup(
                    ui,
                    &mut popup_state,
                    video_rect,
                    true,
                    false,
                    1.0,
                    |_| crate::bank::MineStatus::Available,
                    |_| false,
                    false,
                );
                assert_eq!(action, None);

                let (action_clip, _) = draw_dict_popup(
                    ui,
                    &mut popup_state,
                    video_rect,
                    true,
                    false,
                    1.0,
                    |_| crate::bank::MineStatus::Available,
                    |_| false,
                    true,
                );
                assert_eq!(action_clip, None);
            });
        });
    }

    #[test]
    fn test_quit_and_stop_stream_dialogs_clickable_over_centered_dict_popup() {
        for popup_under_crt in [false, true] {
            let ctx = egui::Context::default();
            let mut fonts = egui::FontDefinitions::default();
            let prop = fonts
                .families
                .get(&egui::FontFamily::Proportional)
                .cloned()
                .unwrap_or_default();
            fonts
                .families
                .insert(egui::FontFamily::Name("GothicCJK".into()), prop);
            ctx.set_fonts(fonts);

            let screen_rect =
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0));
            let mut state = crate::app::AppState::default();
            state.ui.video_window_open = true;
            state.ui.show_quit_dialog = true;
            state.dict.popup_under_crt = popup_under_crt;

            // Place word at (600, 400) so the popup renders just below it (pos ~ (584, 440))
            // and covers the screen center (960, 540) where "Quit?" / "Stop Stream?" are anchored.
            let centered_popup = || DictPopupState {
                matched_term: "飽きる".into(),
                source_text: "飽きる".into(),
                char_range: (0, 3),
                word_rect: egui::Rect::from_min_size(
                    egui::pos2(600.0, 400.0),
                    egui::vec2(40.0, 30.0),
                ),
                extra_word_rects: vec![],
                box_rect: egui::Rect::from_min_size(
                    egui::pos2(580.0, 400.0),
                    egui::vec2(120.0, 30.0),
                ),
                entries: (0..6)
                    .map(|i| super::super::models::TermEntry {
                        term: "飽きる".into(),
                        reading: "あきる".into(),
                        definition_tags: None,
                        rules: String::new(),
                        score: 0.0,
                        glossary: vec![super::super::models::GlossaryEntry::Text(
                            "to get tired of; to lose interest in; to have enough of".into(),
                        )],
                        sequence: i + 1,
                        term_tags: None,
                        inflection_reasons: vec![],
                        frequency: None,
                        pitches: vec![],
                        custom_name_id: None,
                        custom_tag: None,
                    })
                    .collect(),
                is_popup_hovered: false,
                popup_rect: None,
                last_hover_time: Instant::now(),
                last_word_hover_time: Instant::now(),
                child: None,
            };

            // Frame 1: Dialog is shown first (before popup opens), simulating the dialog
            // already existing before the popup layer is created/re-created.
            let raw = egui::RawInput {
                screen_rect: Some(screen_rect),
                ..Default::default()
            };
            let _ = ctx.run(raw.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    crate::ui::dialogs::show_quit_dialog(&mut state, ctx, ui);
                });
            });

            // Frames 2..4: Popup is now open covering the screen center alongside the Quit dialog.
            state.dict.popup = Some(centered_popup());
            for _ in 0..2 {
                let _ = ctx.run(raw.clone(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let _ = draw_dict_popup(
                            ui,
                            &mut state.dict.popup,
                            screen_rect,
                            true,
                            popup_under_crt,
                            1.0,
                            |_| crate::bank::MineStatus::Available,
                            |_| false,
                            false,
                        );
                        crate::ui::dialogs::show_quit_dialog(&mut state, ctx, ui);
                    });
                });
            }

            let popup_rect = state.dict.popup.as_ref().unwrap().popup_rect.unwrap();
            assert!(
                popup_rect.contains(screen_rect.center()),
                "Popup must cover screen center: {popup_rect:?}"
            );

            // Verify that at the center of the screen, hit-testing resolves to the Quit? window
            // (Order::Middle), NOT the dictionary popup.
            let top_layer = ctx.layer_id_at(screen_rect.center()).unwrap();
            assert_eq!(top_layer.order, egui::Order::Middle);

            let quit_window_rect = ctx
                .memory(|m| m.area_rect(top_layer.id))
                .expect("Quit? window rect must exist");
            assert!(popup_rect.intersects(quit_window_rect));

            // Click "Yes, quit" (left button near bottom-left of the Quit? window)
            let yes_pos = egui::pos2(
                quit_window_rect.left() + 45.0,
                quit_window_rect.bottom() - 18.0,
            );
            let mut close_sent = false;
            for pressed in [true, false] {
                let out = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(screen_rect),
                        events: vec![
                            egui::Event::PointerMoved(yes_pos),
                            egui::Event::PointerButton {
                                pos: yes_pos,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let _ = draw_dict_popup(
                                ui,
                                &mut state.dict.popup,
                                screen_rect,
                                true,
                                popup_under_crt,
                                1.0,
                                |_| crate::bank::MineStatus::Available,
                                |_| false,
                                false,
                            );
                            crate::ui::dialogs::show_quit_dialog(&mut state, ctx, ui);
                        });
                    },
                );
                close_sent |= out
                    .viewport_output
                    .get(&egui::ViewportId::ROOT)
                    .is_some_and(|v| v.commands.contains(&egui::ViewportCommand::Close));
            }
            assert!(
                close_sent,
                "Clicking 'Yes, quit' over OCR popup failed (popup_under_crt={popup_under_crt})"
            );

            // Also verify "Stop Stream?" dialog "Yes, stop stream" button works over the OCR popup
            state.ui.show_quit_dialog = false;
            state.ui.show_stop_stream_dialog = true;
            for _ in 0..2 {
                let _ = ctx.run(raw.clone(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let _ = draw_dict_popup(
                            ui,
                            &mut state.dict.popup,
                            screen_rect,
                            true,
                            popup_under_crt,
                            1.0,
                            |_| crate::bank::MineStatus::Available,
                            |_| false,
                            false,
                        );
                        crate::ui::dialogs::show_stop_stream_dialog(&mut state, ctx, ui, ctx);
                    });
                });
            }
            let stop_layer = ctx.layer_id_at(screen_rect.center()).unwrap();
            assert_eq!(stop_layer.order, egui::Order::Middle);
            let stop_window_rect = ctx
                .memory(|m| m.area_rect(stop_layer.id))
                .expect("Stop Stream? window rect must exist");
            let yes_stop_pos = egui::pos2(
                stop_window_rect.left() + 60.0,
                stop_window_rect.bottom() - 18.0,
            );
            for pressed in [true, false] {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(screen_rect),
                        events: vec![
                            egui::Event::PointerMoved(yes_stop_pos),
                            egui::Event::PointerButton {
                                pos: yes_stop_pos,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let _ = draw_dict_popup(
                                ui,
                                &mut state.dict.popup,
                                screen_rect,
                                true,
                                popup_under_crt,
                                1.0,
                                |_| crate::bank::MineStatus::Available,
                                |_| false,
                                false,
                            );
                            crate::ui::dialogs::show_stop_stream_dialog(&mut state, ctx, ui, ctx);
                        });
                    },
                );
            }
            assert!(
                !state.ui.video_window_open,
                "Clicking 'Yes, stop stream' over OCR popup failed (popup_under_crt={popup_under_crt})"
            );
        }
    }

    #[test]
    fn test_dict_popup_drag_scroll_momentum_catch_and_reverse_does_not_crash() {
        for popup_under_crt in [false, true] {
            let ctx = egui::Context::default();
            let mut fonts = egui::FontDefinitions::default();
            let prop = fonts
                .families
                .get(&egui::FontFamily::Proportional)
                .cloned()
                .unwrap_or_default();
            fonts
                .families
                .insert(egui::FontFamily::Name("GothicCJK".into()), prop);
            ctx.set_fonts(fonts);

            let video_rect =
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0));
            let mut popup_state = Some(DictPopupState {
                matched_term: "飽きる".into(),
                source_text: "飽きる".into(),
                char_range: (0, 3),
                word_rect: egui::Rect::from_min_size(
                    egui::pos2(400.0, 100.0),
                    egui::vec2(40.0, 30.0),
                ),
                extra_word_rects: vec![],
                box_rect: egui::Rect::from_min_size(
                    egui::pos2(380.0, 100.0),
                    egui::vec2(120.0, 30.0),
                ),
                entries: (0..20)
                    .map(|i| super::super::models::TermEntry {
                        term: format!("飽きる{i}"),
                        reading: "あきる".into(),
                        definition_tags: Some("v1 vi".into()),
                        rules: String::new(),
                        score: 0.0,
                        glossary: vec![
                            super::super::models::GlossaryEntry::Text(
                                "to get tired of; to lose interest in; to have enough of".into(),
                            ),
                            super::super::models::GlossaryEntry::Text(
                                "to become sated; to grow weary of".into(),
                            ),
                        ],
                        sequence: i + 1,
                        term_tags: None,
                        inflection_reasons: vec![],
                        frequency: None,
                        pitches: vec![],
                        custom_name_id: None,
                        custom_tag: None,
                    })
                    .collect(),
                is_popup_hovered: false,
                popup_rect: None,
                last_hover_time: Instant::now(),
                last_word_hover_time: Instant::now(),
                child: None,
            });

            let offset_key = egui::Id::new("jitendex_dict_popup_scroll_offset");
            let mut time = 0.0f64;
            let mut run_frame = |events: Vec<egui::Event>| -> (f32, egui::Rect) {
                time += 1.0 / 60.0;
                let raw = egui::RawInput {
                    screen_rect: Some(video_rect),
                    time: Some(time),
                    predicted_dt: 1.0 / 60.0,
                    events,
                    ..Default::default()
                };
                let _ = ctx.run(raw, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let _ = draw_dict_popup(
                            ui,
                            &mut popup_state,
                            video_rect,
                            true,
                            popup_under_crt,
                            1.0,
                            |_| crate::bank::MineStatus::Available,
                            |_| false,
                            false,
                        );
                    });
                });
                (
                    ctx.data(|d| d.get_temp::<f32>(offset_key)).unwrap_or(0.0),
                    popup_state.as_ref().unwrap().popup_rect.unwrap(),
                )
            };

            // Warm up layout so ScrollArea knows content_is_too_large
            let mut popup_rect = video_rect;
            for _ in 0..3 {
                popup_rect = run_frame(vec![]).1;
            }
            let grab_x = popup_rect.left() + 120.0; // Directly over headword/definition labels
            let start_y = popup_rect.top() + 500.0;

            // 1. Click and drag upwards on the content to scroll down the list with velocity
            run_frame(vec![
                egui::Event::PointerMoved(egui::pos2(grab_x, start_y)),
                egui::Event::PointerButton {
                    pos: egui::pos2(grab_x, start_y),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            for step in 1..=6 {
                let y = start_y - step as f32 * 45.0;
                run_frame(vec![egui::Event::PointerMoved(egui::pos2(grab_x, y))]);
            }
            let end_drag1_y = start_y - 6.0 * 45.0;
            let (offset_at_release, _) = run_frame(vec![egui::Event::PointerButton {
                pos: egui::pos2(grab_x, end_drag1_y),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            assert!(
                offset_at_release > 100.0,
                "Expected drag-to-scroll to move down: {offset_at_release}"
            );

            // 2. Let inertia/momentum carry the list further down for a few frames
            let mut offset_in_momentum = offset_at_release;
            for _ in 0..4 {
                offset_in_momentum = run_frame(vec![]).0;
            }
            assert!(
                offset_in_momentum > offset_at_release,
                "Expected inertia/momentum to continue scrolling down ({offset_at_release} -> {offset_in_momentum})"
            );

            // 3. Grab the moving list again directly on a label and reverse flow back up (drag down)
            let catch_y = popup_rect.top() + 200.0;
            run_frame(vec![
                egui::Event::PointerMoved(egui::pos2(grab_x, catch_y)),
                egui::Event::PointerButton {
                    pos: egui::pos2(grab_x, catch_y),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            let mut reversed_offset = offset_in_momentum;
            for step in 1..=8 {
                let y = catch_y + step as f32 * 50.0;
                reversed_offset =
                    run_frame(vec![egui::Event::PointerMoved(egui::pos2(grab_x, y))]).0;
            }
            run_frame(vec![egui::Event::PointerButton {
                pos: egui::pos2(grab_x, catch_y + 400.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            for _ in 0..4 {
                reversed_offset = run_frame(vec![]).0;
            }

            assert!(
                reversed_offset < offset_in_momentum,
                "Reversing drag should scroll back up ({offset_in_momentum} -> {reversed_offset})"
            );
        }
    }

    #[test]
    fn test_dict_popup_momentum_does_not_transfer_to_new_popup_after_dismiss_or_switch() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        let prop = fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        fonts
            .families
            .insert(egui::FontFamily::Name("GothicCJK".into()), prop);
        ctx.set_fonts(fonts);

        let video_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0));
        let make_popup = |word: &str| DictPopupState {
            matched_term: word.into(),
            source_text: word.into(),
            char_range: (0, word.chars().count()),
            word_rect: egui::Rect::from_min_size(egui::pos2(400.0, 100.0), egui::vec2(40.0, 30.0)),
            extra_word_rects: vec![],
            box_rect: egui::Rect::from_min_size(egui::pos2(380.0, 100.0), egui::vec2(120.0, 30.0)),
            entries: (0..20)
                .map(|i| super::super::models::TermEntry {
                    term: format!("{word}{i}"),
                    reading: "あきる".into(),
                    definition_tags: Some("v1 vi".into()),
                    rules: String::new(),
                    score: 0.0,
                    glossary: vec![super::super::models::GlossaryEntry::Text(
                        "to get tired of; to lose interest in; to have enough of".into(),
                    )],
                    sequence: i + 1,
                    term_tags: None,
                    inflection_reasons: vec![],
                    frequency: None,
                    pitches: vec![],
                    custom_name_id: None,
                    custom_tag: None,
                })
                .collect(),
            is_popup_hovered: false,
            popup_rect: None,
            last_hover_time: Instant::now(),
            last_word_hover_time: Instant::now(),
            child: None,
        };

        let mut popup_state = Some(make_popup("飽きる"));
        let offset_key = egui::Id::new("jitendex_dict_popup_scroll_offset");
        let time = std::cell::Cell::new(0.0f64);

        let run_frame =
            |popup_state: &mut Option<DictPopupState>, events: Vec<egui::Event>| -> f32 {
                time.set(time.get() + 1.0 / 60.0);
                let raw = egui::RawInput {
                    screen_rect: Some(video_rect),
                    time: Some(time.get()),
                    predicted_dt: 1.0 / 60.0,
                    events,
                    ..Default::default()
                };
                let _ = ctx.run(raw, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let _ = draw_dict_popup(
                            ui,
                            popup_state,
                            video_rect,
                            true,
                            true,
                            1.0,
                            |_| crate::bank::MineStatus::Available,
                            |_| false,
                            false,
                        );
                    });
                });
                ctx.data(|d| d.get_temp::<f32>(offset_key)).unwrap_or(0.0)
            };

        for _ in 0..3 {
            run_frame(&mut popup_state, vec![]);
        }
        let popup_rect = popup_state.as_ref().unwrap().popup_rect.unwrap();
        let grab_x = popup_rect.left() + 120.0;
        let start_y = popup_rect.top() + 500.0;

        // Flick-drag popup 1 so it has strong downward kinetic scrolling momentum
        let flick = |popup_state: &mut Option<DictPopupState>| {
            run_frame(
                popup_state,
                vec![
                    egui::Event::PointerMoved(egui::pos2(grab_x, start_y)),
                    egui::Event::PointerButton {
                        pos: egui::pos2(grab_x, start_y),
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            for step in 1..=5 {
                let y = start_y - step as f32 * 50.0;
                run_frame(
                    popup_state,
                    vec![egui::Event::PointerMoved(egui::pos2(grab_x, y))],
                );
            }
            let release_y = start_y - 250.0;
            let off1 = run_frame(
                popup_state,
                vec![egui::Event::PointerButton {
                    pos: egui::pos2(grab_x, release_y),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            let off2 = run_frame(popup_state, vec![]);
            assert!(
                off2 > off1,
                "Popup should be actively scrolling with momentum ({off1} -> {off2})"
            );
        };

        flick(&mut popup_state);

        // Case 1: Dismiss popup while it still has momentum, then open a new popup (even for the same word)
        popup_state = None;
        run_frame(&mut popup_state, vec![]);
        popup_state = Some(make_popup("飽きる"));
        for frame in 0..5 {
            let offset = run_frame(&mut popup_state, vec![]);
            assert_eq!(
                offset, 0.0,
                "Reopened popup after dismissal must not inherit momentum (frame {frame})"
            );
        }

        // Case 2: Flick again, then switch directly to another word's popup without a dismissal gap
        flick(&mut popup_state);
        popup_state = Some(make_popup("食べる"));
        for frame in 0..5 {
            let offset = run_frame(&mut popup_state, vec![]);
            assert_eq!(
                offset, 0.0,
                "Switched popup must not inherit momentum from previous word (frame {frame})"
            );
        }
    }

    fn setup_japanese_test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
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

    fn simple_entry(
        term: &str,
        reading: &str,
        gloss: &str,
        seq: i64,
    ) -> super::super::models::TermEntry {
        super::super::models::TermEntry {
            term: term.into(),
            reading: reading.into(),
            definition_tags: Some("n".into()),
            rules: String::new(),
            score: 100.0,
            glossary: vec![super::super::models::GlossaryEntry::Text(gloss.into())],
            sequence: seq,
            term_tags: None,
            inflection_reasons: vec![],
            frequency: None,
            pitches: vec![],
            custom_name_id: None,
            custom_tag: None,
        }
    }

    #[test]
    fn test_recursive_popup_chain_crt_modes_and_layout_bounds() {
        for popup_under_crt in [false, true] {
            let ctx = setup_japanese_test_ctx();
            let safe_rect =
                egui::Rect::from_min_size(egui::pos2(60.0, 50.0), egui::vec2(1800.0, 960.0));
            let root_word =
                egui::Rect::from_min_size(egui::pos2(400.0, 120.0), egui::vec2(80.0, 32.0));

            let mut popup_state = Some(DictPopupState {
                matched_term: "図書館".into(),
                source_text: "図書館".into(),
                char_range: (0, 3),
                word_rect: root_word,
                extra_word_rects: vec![],
                box_rect: root_word,
                entries: vec![simple_entry(
                    "図書館",
                    "としょかん",
                    "書物を集めて公開する施設",
                    1,
                )],
                is_popup_hovered: false,
                popup_rect: None,
                last_hover_time: Instant::now(),
                last_word_hover_time: Instant::now(),
                child: None,
            });

            let lookup =
                |text: &str,
                 char_idx: usize|
                 -> Option<(usize, usize, Vec<super::super::models::TermEntry>)> {
                    let chars: Vec<char> = text.chars().collect();
                    let tail: String = chars.get(char_idx..)?.iter().collect();
                    if tail.starts_with("書物") {
                        Some((
                            char_idx,
                            char_idx + 2,
                            vec![simple_entry("書物", "しょもつ", "本や書籍のこと", 2)],
                        ))
                    } else if tail.starts_with("書籍") {
                        Some((
                            char_idx,
                            char_idx + 2,
                            vec![simple_entry("書籍", "しょせき", "印刷された冊子", 3)],
                        ))
                    } else {
                        None
                    }
                };

            let sim_time = std::cell::Cell::new(1.0f64);
            let run_frame_at =
                |popup_state: &mut Option<DictPopupState>, t: f64, events: Vec<egui::Event>| {
                    sim_time.set(t);
                    let mut out = (None, Vec::new());
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(safe_rect),
                            time: Some(t),
                            events,
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                out = draw_dict_popup_with_lookup(
                                    ui,
                                    popup_state,
                                    safe_rect,
                                    true,
                                    popup_under_crt,
                                    1.0,
                                    |_| crate::bank::MineStatus::Available,
                                    |_| false,
                                    true,
                                    Some(lookup),
                                );
                            });
                        },
                    );
                    out
                };

            // Warm up layer 0 (layer 0 has no delay)
            let mut layer0_prims = 0;
            for i in 0..3 {
                let (_, prims) = run_frame_at(&mut popup_state, 1.0 + i as f64 * 0.016, vec![]);
                layer0_prims = prims.len();
            }
            assert_eq!(popup_state.as_ref().unwrap().deepest_depth(), 0);
            let rect0 = popup_state.as_ref().unwrap().popup_rect.unwrap();
            assert!(
                safe_rect.contains_rect(rect0),
                "{rect0:?} not in {safe_rect:?}"
            );
            if popup_under_crt {
                assert!(layer0_prims > 0);
            } else {
                assert_eq!(layer0_prims, 0);
            }

            // Scan inside layer 0 to find where "書物" is rendered, verify it does NOT open
            // before RECURSIVE_POPUP_HOVER_DELAY (0.5s), and opens once 0.5s has elapsed.
            let mut opened_layer1 = false;
            let mut y = rect0.top() + 80.0;
            let mut t = 2.0f64;
            while y < rect0.bottom() - 30.0 && !opened_layer1 {
                let mut x = rect0.left() + 24.0;
                while x < rect0.left() + 300.0 {
                    t += 1.0;
                    // Initial hover at time `t`: must not open immediately
                    run_frame_at(
                        &mut popup_state,
                        t,
                        vec![egui::Event::PointerMoved(egui::pos2(x, y))],
                    );
                    assert!(
                        popup_state.as_ref().unwrap().child.is_none(),
                        "Recursive popup must wait for 0.5s hover delay before opening"
                    );
                    // At t + 0.3s (< 0.5s): still must not open
                    run_frame_at(
                        &mut popup_state,
                        t + 0.3,
                        vec![egui::Event::PointerMoved(egui::pos2(x, y))],
                    );
                    assert!(popup_state.as_ref().unwrap().child.is_none());
                    // At t + 0.55s (>= 0.5s): opens if pointer is over "書物"
                    run_frame_at(
                        &mut popup_state,
                        t + 0.55,
                        vec![egui::Event::PointerMoved(egui::pos2(x, y))],
                    );
                    if popup_state.as_ref().unwrap().child.is_some() {
                        opened_layer1 = true;
                        break;
                    }
                    x += 18.0;
                }
                y += 18.0;
            }
            assert!(
                opened_layer1,
                "Hovering '書物' in layer 0 definition for 0.5s must open recursive popup layer 1 (under_crt={popup_under_crt})"
            );

            // Settle layer 1 layout
            let mut layer1_prims = 0;
            let layer1_word_center = popup_state
                .as_ref()
                .unwrap()
                .child
                .as_ref()
                .unwrap()
                .word_rect
                .center();
            for i in 0..3 {
                let (_, prims) = run_frame_at(
                    &mut popup_state,
                    t + 0.6 + i as f64 * 0.016,
                    vec![egui::Event::PointerMoved(layer1_word_center)],
                );
                layer1_prims = prims.len();
            }
            assert_eq!(popup_state.as_ref().unwrap().deepest_depth(), 1);
            let child1 = popup_state.as_ref().unwrap().child.as_ref().unwrap();
            assert_eq!(child1.matched_term, "書物");
            let rect1 = child1.popup_rect.unwrap();
            assert!(
                safe_rect.contains_rect(rect1),
                "Layer 1 rect {rect1:?} must stay inside safe_rect {safe_rect:?}"
            );
            assert!(
                !rect1.intersects(child1.word_rect),
                "Layer 1 rect {rect1:?} must not cover its anchor word_rect {:?}",
                child1.word_rect
            );
            if popup_under_crt {
                assert!(
                    layer1_prims > layer0_prims,
                    "CRT primitives must include both layer 0 and layer 1 ({layer1_prims} <= {layer0_prims})"
                );
            }

            // Now scan inside layer 1 to find "書籍" and open layer 2 (grandchild popup) after 0.5s dwell
            let mut opened_layer2 = false;
            let mut y = rect1.top() + 80.0;
            while y < rect1.bottom() - 30.0 && !opened_layer2 {
                let mut x = rect1.left() + 24.0;
                while x < rect1.left() + 300.0 {
                    t += 1.0;
                    run_frame_at(
                        &mut popup_state,
                        t,
                        vec![egui::Event::PointerMoved(egui::pos2(x, y))],
                    );
                    run_frame_at(
                        &mut popup_state,
                        t + 0.55,
                        vec![egui::Event::PointerMoved(egui::pos2(x, y))],
                    );
                    if let Some(c2) = popup_state
                        .as_ref()
                        .unwrap()
                        .child
                        .as_ref()
                        .and_then(|c| c.child.as_ref())
                    {
                        if c2.matched_term == "書籍" {
                            opened_layer2 = true;
                            break;
                        }
                        popup_state.as_mut().unwrap().child.as_mut().unwrap().child = None;
                    }
                    x += 18.0;
                }
                y += 18.0;
            }
            assert!(
                opened_layer2,
                "Hovering '書籍' in layer 1 definition for 0.5s must open recursive popup layer 2 (under_crt={popup_under_crt})"
            );

            let layer2_word_center = popup_state
                .as_ref()
                .unwrap()
                .child
                .as_ref()
                .unwrap()
                .child
                .as_ref()
                .unwrap()
                .word_rect
                .center();
            for i in 0..3 {
                run_frame_at(
                    &mut popup_state,
                    t + 0.6 + i as f64 * 0.016,
                    vec![egui::Event::PointerMoved(layer2_word_center)],
                );
            }
            assert_eq!(popup_state.as_ref().unwrap().deepest_depth(), 2);
            let child2 = popup_state
                .as_ref()
                .unwrap()
                .child
                .as_ref()
                .unwrap()
                .child
                .as_ref()
                .unwrap();
            assert_eq!(child2.matched_term, "書籍");
            let rect2 = child2.popup_rect.unwrap();
            assert!(
                safe_rect.contains_rect(rect2),
                "Layer 2 rect {rect2:?} must stay inside safe_rect {safe_rect:?}"
            );
        }
    }

    #[test]
    fn test_recursive_same_word_with_furigana_and_hover_delay() {
        let ctx = setup_japanese_test_ctx();
        let safe_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0));
        let root_word = egui::Rect::from_min_size(egui::pos2(400.0, 120.0), egui::vec2(80.0, 32.0));

        // Root popup is for "確か", and its structured content contains an example sentence
        // with furigana `<ruby>確<rt>たし</rt></ruby>かだ`.
        let structured_sense = serde_json::json!({
            "type": "structured-content",
            "content": {
                "tag": "div",
                "data": { "content": "sense-group" },
                "content": [
                    {
                        "tag": "div",
                        "data": { "content": "sense" },
                        "content": [
                            {
                                "tag": "ul",
                                "data": { "content": "glossary" },
                                "content": [
                                    { "tag": "li", "content": "certain; sure; definite" }
                                ]
                            },
                            {
                                "tag": "div",
                                "data": { "content": "example-sentence" },
                                "content": [
                                    {
                                        "tag": "div",
                                        "data": { "content": "example-sentence-a" },
                                        "content": [
                                            {
                                                "tag": "ruby",
                                                "content": [
                                                    "確",
                                                    { "tag": "rt", "content": "たし" }
                                                ]
                                            },
                                            "かだ"
                                        ]
                                    }
                                ]
                            }
                        ]
                    }
                ]
            }
        });

        let tashika_entry = super::super::models::TermEntry {
            term: "確か".into(),
            reading: "たしか".into(),
            definition_tags: Some("adj-na adv".into()),
            rules: String::new(),
            score: 100.0,
            glossary: vec![super::super::models::GlossaryEntry::Structured(
                structured_sense,
            )],
            sequence: 1,
            term_tags: None,
            inflection_reasons: vec![],
            frequency: None,
            pitches: vec![],
            custom_name_id: None,
            custom_tag: None,
        };

        let mut popup_state = Some(DictPopupState {
            matched_term: "確か".into(),
            source_text: "確か".into(),
            char_range: (0, 2),
            word_rect: root_word,
            extra_word_rects: vec![],
            box_rect: root_word,
            entries: vec![tashika_entry.clone()],
            is_popup_hovered: false,
            popup_rect: None,
            last_hover_time: Instant::now(),
            last_word_hover_time: Instant::now(),
            child: None,
        });

        let lookup = |text: &str,
                      char_idx: usize|
         -> Option<(usize, usize, Vec<super::super::models::TermEntry>)> {
            let chars: Vec<char> = text.chars().collect();
            let tail: String = chars.get(char_idx..)?.iter().collect();
            if tail.starts_with("確か") {
                Some((char_idx, char_idx + 2, vec![tashika_entry.clone()]))
            } else {
                None
            }
        };

        let run_frame_at =
            |popup_state: &mut Option<DictPopupState>, t: f64, events: Vec<egui::Event>| {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(safe_rect),
                        time: Some(t),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let _ = draw_dict_popup_with_lookup(
                                ui,
                                popup_state,
                                safe_rect,
                                true,
                                false,
                                1.0,
                                |_| crate::bank::MineStatus::Available,
                                |_| false,
                                false,
                                Some(lookup),
                            );
                        });
                    },
                );
            };

        for i in 0..3 {
            run_frame_at(&mut popup_state, 1.0 + i as f64 * 0.016, vec![]);
        }
        let rect0 = popup_state.as_ref().unwrap().popup_rect.unwrap();

        // Scan the example sentence area (below the English definition line) for `<ruby>確<rt>たし</rt></ruby>かだ`
        let mut ruby_word_rect = None;
        let mut t = 10.0f64;
        let mut y = rect0.top() + 140.0;
        while y < rect0.bottom() - 20.0 && ruby_word_rect.is_none() {
            let mut x = rect0.left() + 40.0;
            while x < rect0.left() + 260.0 {
                t += 1.0;
                run_frame_at(
                    &mut popup_state,
                    t,
                    vec![egui::Event::PointerMoved(egui::pos2(x, y))],
                );
                run_frame_at(
                    &mut popup_state,
                    t + 0.55,
                    vec![egui::Event::PointerMoved(egui::pos2(x, y))],
                );
                if let Some(child) = popup_state.as_ref().unwrap().child.as_ref() {
                    if child.source_text == "確かだ" {
                        assert_eq!(child.matched_term, "確か");
                        assert_eq!(child.char_range, (0, 2));
                        ruby_word_rect = Some(child.word_rect);
                        break;
                    }
                    popup_state.as_mut().unwrap().child = None;
                }
                x += 12.0;
            }
            y += 12.0;
        }
        let ruby_rect = ruby_word_rect
            .expect("Hovering `<ruby>確<rt>たし</rt></ruby>か` inside `確か` must open recursive popup for `確か`");

        // Also verify hovering specifically near the TOP of `ruby_rect` (where the furigana `たし` sits)
        // opens the recursive popup for `確か` after 0.5s.
        popup_state.as_mut().unwrap().child = None;
        t += 1.0;
        run_frame_at(
            &mut popup_state,
            t,
            vec![egui::Event::PointerMoved(egui::pos2(0.0, 0.0))],
        );
        let furigana_pos = egui::pos2(ruby_rect.left() + 8.0, ruby_rect.top() + 4.0);
        t += 1.0;
        run_frame_at(
            &mut popup_state,
            t,
            vec![egui::Event::PointerMoved(furigana_pos)],
        );
        assert!(
            popup_state.as_ref().unwrap().child.is_none(),
            "Must not open before 0.5s dwell on furigana"
        );
        run_frame_at(
            &mut popup_state,
            t + 0.55,
            vec![egui::Event::PointerMoved(furigana_pos)],
        );
        let child = popup_state.as_ref().unwrap().child.as_ref().expect(
            "Hovering on the furigana `たし` of `<ruby>確<rt>たし</rt></ruby>か` must open `確か`",
        );
        assert_eq!(child.matched_term, "確か");
        assert_eq!(child.source_text, "確かだ");
        assert_eq!(child.char_range, (0, 2));
    }

    #[test]
    fn test_mining_buttons_only_on_first_layer_and_mouse_click_depth_awareness() {
        for popup_under_crt in [false, true] {
            let ctx = setup_japanese_test_ctx();
            let screen_rect =
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0));
            let root_word =
                egui::Rect::from_min_size(egui::pos2(400.0, 120.0), egui::vec2(80.0, 32.0));

            // Place child popup so it overlaps the top-right corner of parent popup (layer 0),
            // right where layer 0's `+` and `++` mining buttons live!
            let make_stack = || {
                let mut root = DictPopupState {
                    matched_term: "図書館".into(),
                    source_text: "図書館".into(),
                    char_range: (0, 3),
                    word_rect: root_word,
                    extra_word_rects: vec![],
                    box_rect: root_word,
                    entries: vec![simple_entry("図書館", "としょかん", "本を読む場所", 1)],
                    is_popup_hovered: false,
                    popup_rect: None,
                    last_hover_time: Instant::now(),
                    last_word_hover_time: Instant::now(),
                    child: None,
                };
                // Anchor child word near (650, 120) so child popup's placement overlaps root popup's
                // top-right mining buttons around x ~ 1250..1360, y ~ 160..240.
                let child_word =
                    egui::Rect::from_min_size(egui::pos2(650.0, 120.0), egui::vec2(60.0, 32.0));
                root.child = Some(Box::new(DictPopupState {
                    matched_term: "書物".into(),
                    source_text: "本を読む場所".into(),
                    char_range: (0, 1),
                    word_rect: child_word,
                    extra_word_rects: vec![],
                    box_rect: child_word,
                    entries: vec![simple_entry("書物", "しょもつ", "書籍のこと", 2)],
                    is_popup_hovered: false,
                    popup_rect: None,
                    last_hover_time: Instant::now(),
                    last_word_hover_time: Instant::now(),
                    child: None,
                }));
                root
            };

            let mut popup_state = Some(make_stack());
            let run_frame = |popup_state: &mut Option<DictPopupState>, events: Vec<egui::Event>| {
                let mut action = None;
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(screen_rect),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            action = draw_dict_popup(
                                ui,
                                popup_state,
                                screen_rect,
                                true,
                                popup_under_crt,
                                1.0,
                                |_| crate::bank::MineStatus::Available,
                                |_| false,
                                true, // can_clip = true so both + and ++ exist on layer 0
                            )
                            .0;
                        });
                    },
                );
                action
            };

            for _ in 0..3 {
                run_frame(&mut popup_state, vec![]);
            }

            let rect0 = popup_state.as_ref().unwrap().popup_rect.unwrap();
            let rect1 = popup_state
                .as_ref()
                .unwrap()
                .child
                .as_ref()
                .unwrap()
                .popup_rect
                .unwrap();
            assert!(
                rect0.intersects(rect1),
                "Expected rect0 {rect0:?} and rect1 {rect1:?} to overlap"
            );

            // 1. Verify Z-order: at the overlap region, egui's top layer is layer 1 (not layer 0).
            let overlap = rect0.intersect(rect1);
            assert!(overlap.is_positive());
            let top_layer = ctx.layer_id_at(overlap.center()).unwrap();
            assert_eq!(top_layer, popup_layer_id(popup_under_crt, 1));
            assert_eq!(
                popup_state
                    .as_ref()
                    .unwrap()
                    .topmost_layer_at(overlap.center()),
                Some(1)
            );

            // 2. Verify mining buttons (+ and ++) are NOT present on layer 1 (depth == 1):
            // Click where `+` and `++` would be in layer 1's top-right corner.
            let layer1_plus_pos =
                egui::pos2(rect1.right() - 24.0 - 28.0, rect1.top() + 18.0 + 28.0);
            let layer1_clip_pos = egui::pos2(
                rect1.right() - 24.0 - 56.0 - 8.0 - 28.0,
                rect1.top() + 18.0 + 28.0,
            );
            for pos in [layer1_plus_pos, layer1_clip_pos] {
                for pressed in [true, false] {
                    let action = run_frame(
                        &mut popup_state,
                        vec![
                            egui::Event::PointerMoved(pos),
                            egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                    );
                    assert_eq!(
                        action, None,
                        "Recursive popup (depth > 0) must NEVER show or trigger + or ++ mining buttons"
                    );
                }
            }

            // 3. Verify mouse click depth occlusion: clicking layer 0's `+` button position
            // WHILE it is covered by layer 1 must NOT trigger mining on layer 0 and must NOT
            // dismiss layer 1!
            let layer0_plus_pos =
                egui::pos2(rect0.right() - 24.0 - 28.0, rect0.top() + 18.0 + 28.0);
            assert!(
                rect1.contains(layer0_plus_pos),
                "Layer 1 {rect1:?} must cover layer 0's + button at {layer0_plus_pos:?}"
            );
            for pressed in [true, false] {
                let action = run_frame(
                    &mut popup_state,
                    vec![
                        egui::Event::PointerMoved(layer0_plus_pos),
                        egui::Event::PointerButton {
                            pos: layer0_plus_pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
                assert_eq!(
                    action, None,
                    "Clicking inside child popup over parent's + button must not click through to parent"
                );
            }
            assert!(
                popup_state.as_ref().unwrap().child.is_some(),
                "Clicking inside child popup must not dismiss child popup"
            );

            // 4. Clicking on an exposed (un-occluded) area of layer 0 dismisses child popup (layer 1).
            let exposed_layer0_pos = egui::pos2(rect0.left() + 40.0, rect0.top() + 120.0);
            assert!(rect0.contains(exposed_layer0_pos) && !rect1.contains(exposed_layer0_pos));
            for pressed in [true, false] {
                run_frame(
                    &mut popup_state,
                    vec![
                        egui::Event::PointerMoved(exposed_layer0_pos),
                        egui::Event::PointerButton {
                            pos: exposed_layer0_pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            assert!(
                popup_state.as_ref().unwrap().child.is_none(),
                "Clicking exposed parent popup area must dismiss deeper child popup"
            );

            // 5. Now that layer 0 is un-occluded, clicking layer 0's `+` and `++` buttons works!
            let mut mined = false;
            for pressed in [true, false] {
                let action = run_frame(
                    &mut popup_state,
                    vec![
                        egui::Event::PointerMoved(layer0_plus_pos),
                        egui::Event::PointerButton {
                            pos: layer0_plus_pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
                if !pressed {
                    mined = action == Some(PopupAction::Mine(0));
                }
            }
            assert!(mined, "Layer 0 + button must trigger PopupAction::Mine(0)");

            let layer0_clip_pos = egui::pos2(
                rect0.right() - 24.0 - 56.0 - 8.0 - 28.0,
                rect0.top() + 18.0 + 28.0,
            );
            let mut mined_and_clipped = false;
            for pressed in [true, false] {
                let action = run_frame(
                    &mut popup_state,
                    vec![
                        egui::Event::PointerMoved(layer0_clip_pos),
                        egui::Event::PointerButton {
                            pos: layer0_clip_pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
                if !pressed {
                    mined_and_clipped = action == Some(PopupAction::MineAndClip(0));
                }
            }
            assert!(
                mined_and_clipped,
                "Layer 0 ++ button must trigger PopupAction::MineAndClip(0)"
            );
        }
    }

    #[test]
    fn test_popup_under_crt_caches_tessellation_and_preserves_noop_paintlist() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        let family = fonts.families[&egui::FontFamily::Proportional].clone();
        fonts
            .families
            .insert(egui::FontFamily::Name("GothicCJK".into()), family);
        ctx.set_fonts(fonts);
        let video_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0));
        let word_rect =
            egui::Rect::from_min_size(egui::pos2(400.0, 800.0), egui::vec2(120.0, 40.0));
        let mut popup_state = Some(DictPopupState {
            matched_term: "食べる".into(),
            source_text: "食べる".into(),
            char_range: (0, 3),
            word_rect,
            extra_word_rects: vec![],
            box_rect: word_rect,
            entries: vec![super::super::models::TermEntry {
                term: "食べる".into(),
                reading: "たべる".into(),
                definition_tags: None,
                rules: String::new(),
                score: 0.0,
                glossary: vec![super::super::models::GlossaryEntry::Text(
                    "to eat; to consume".into(),
                )],
                sequence: 100,
                term_tags: None,
                inflection_reasons: vec![],
                frequency: None,
                pitches: vec![],
                custom_name_id: None,
                custom_tag: None,
            }],
            is_popup_hovered: false,
            popup_rect: None,
            last_hover_time: Instant::now(),
            last_word_hover_time: Instant::now(),
            child: None,
        });

        let run_crt_frame = |popup_state: &mut Option<DictPopupState>,
                             mine_status: crate::bank::MineStatus,
                             time: f64| {
            let mut prims = Vec::new();
            let layer_id = popup_layer_id(true, 0);
            let mut paint_list_len = 0usize;
            let mut all_noop = false;
            let out = ctx.run(
                egui::RawInput {
                    screen_rect: Some(video_rect),
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let (_, p) = draw_dict_popup_with_lookup::<
                            fn(
                                &str,
                                usize,
                            )
                                -> Option<(usize, usize, Vec<super::super::models::TermEntry>)>,
                        >(
                            ui,
                            popup_state,
                            video_rect,
                            true,
                            true,
                            1.0,
                            |_| mine_status,
                            |_| mine_status != crate::bank::MineStatus::Available,
                            true,
                            None,
                        );
                        prims = p;
                    });
                    ctx.graphics_mut(|g| {
                        if let Some(list) = g.get_mut(layer_id) {
                            let entries: Vec<_> = list.all_entries().collect();
                            paint_list_len = entries.len();
                            all_noop = entries
                                .iter()
                                .all(|cs| matches!(cs.shape, egui::Shape::Noop));
                        }
                    });
                },
            );
            let _ = out;
            (prims, paint_list_len, all_noop)
        };

        // Frame 0 is egui::Area's invisible sizing pass; Frame 1+ paints visible shapes.
        let _ = run_crt_frame(&mut popup_state, crate::bank::MineStatus::Available, 0.05);

        let m0 = POPUP_TESS_MISS_COUNT.with(|c| c.get());
        let (prims1, len1, noop1) =
            run_crt_frame(&mut popup_state, crate::bank::MineStatus::Available, 0.1);
        let m1 = POPUP_TESS_MISS_COUNT.with(|c| c.get());
        assert!(!prims1.is_empty());
        assert!(len1 > 0, "PaintList length must be preserved (not drained)");
        assert!(
            noop1,
            "All popup layer shapes must be overwritten with Shape::Noop"
        );
        assert_eq!(m1, m0 + 1);

        // Stationary frame within same blink/gem step: reuses cached tessellation (0 misses)
        // while still overwriting PaintList entries with Shape::Noop!
        let (prims2, len2, noop2) =
            run_crt_frame(&mut popup_state, crate::bank::MineStatus::Available, 0.2);
        let m2 = POPUP_TESS_MISS_COUNT.with(|c| c.get());
        assert_eq!(prims2.len(), prims1.len());
        assert_eq!(len2, len1);
        assert!(noop2);
        assert_eq!(
            m2, m1,
            "Unchanged popup frame must reuse cached CRT tessellation"
        );

        // Changing mining status immediately invalidates the signature and re-tessellates!
        let (prims3, _, noop3) =
            run_crt_frame(&mut popup_state, crate::bank::MineStatus::Mined, 0.2);
        let m3 = POPUP_TESS_MISS_COUNT.with(|c| c.get());
        assert!(!prims3.is_empty());
        assert!(noop3);
        assert_eq!(
            m3,
            m2 + 1,
            "Mining status visual change must invalidate cached CRT tessellation"
        );
    }
}
