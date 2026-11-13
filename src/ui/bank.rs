//! Standalone "Mining Bank" window (toggled with B): lists mined words, most recent first.

use crate::app::AppState;
use crate::bank::{tags, BankEntryMeta, TagEdit};
#[cfg(test)]
use crate::dict::render::build_headword_reading_job;
use crate::dict::render::{
    dict_font, render_entry_header, render_glossaries, render_headword_and_reading, with_dict_scale,
};
use crate::dict::theme;
use crate::ui::tag_input::tag_input;
use eframe::egui::{self, text::LayoutJob, Color32, RichText, TextFormat};

// "Faded Indigo" retro palette, shared with the dictionary popup (see `dict::theme`).
const COLOR_TERM: Color32 = theme::HEAD;
const COLOR_READING: Color32 = theme::GOLD;
const COLOR_SENTENCE: Color32 = theme::TEXT;
const COLOR_HIGHLIGHT: Color32 = theme::GOLD;
const COLOR_HIGHLIGHT_BG: Color32 = theme::MINED_BG;
const COLOR_MUTED: Color32 = theme::MUTED;
const COLOR_DEFINITION: Color32 = theme::TEXT;
const COLOR_TAG: Color32 = theme::TAG_TEXT;
const COLOR_TAG_BG: Color32 = theme::TAG_BG;
const COLOR_TAG_STROKE: Color32 = theme::TAG_BORDER;
const COLOR_TAG_HOVER_BG: Color32 = theme::DEINFLECT_BG;
const COLOR_TAG_HOVER_STROKE: Color32 = theme::DEINFLECT_BORDER;

/// Scale of the dictionary entry relative to the (large, over-video) popup rendering.
const DICT_SCALE: f32 = 0.55;
/// Display size of list thumbnails (images are fit inside, preserving aspect ratio).
const THUMB_SIZE: egui::Vec2 = egui::vec2(240.0, 135.0);
/// Maximum number of thumbnails decoded per frame, to keep scrolling smooth.
const THUMB_LOADS_PER_FRAME: usize = 6;
/// Designed size (points) of the mining bank window.
const BANK_WINDOW_SIZE: egui::Vec2 = egui::vec2(900.0, 900.0);
/// Extra vertical padding (points) rendered above and below the scroll viewport to prevent pop-in.
const VIRTUAL_OVERSCAN: f32 = 400.0;
/// Default unmeasured card height in compact mode (collapsed).
const DEFAULT_COMPACT_CARD_HEIGHT: f32 = 76.0;
/// Default unmeasured card height in full / expanded mode.
const DEFAULT_FULL_CARD_HEIGHT: f32 = 210.0;

pub fn draw_bank_window(state: &mut AppState, ctx: &egui::Context) {
    if !state.bank.window_open || !state.ui.scale.settled_once {
        return;
    }
    // Designed size, shrunk to fit smaller screens (the window stays freely resizable).
    let size = crate::ui::scaling::fit_window(
        BANK_WINDOW_SIZE,
        crate::ui::scaling::monitor_size(ctx, &state.ui.scale),
    )
    .size;
    ctx.show_viewport_immediate(
        crate::ui::scaling::viewport_id("bank_window", ctx, size),
        egui::ViewportBuilder::default()
            .with_title("Michadame Mining Bank")
            .with_inner_size(size)
            .with_min_inner_size(crate::ui::scaling::MIN_FALLBACK_WINDOW_SIZE.min(size)),
        |ctx, _class| {
            state.ui.any_pointer_down |= ctx.input(|i| i.pointer.any_down());
            draw_contents(state, ctx);
            if ctx.input(|i| i.viewport().close_requested()) {
                state.bank.window_open = false;
                state.bank.enlarged = None;
            }
        },
    );
}

pub(crate) fn draw_contents(state: &mut AppState, ctx: &egui::Context) {
    // Keyboard: Esc closes the enlarged screenshot or add-name dialog; B closes the window.
    if state.bank.enlarged.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.bank.enlarged = None;
    } else if state.bank.add_name_dialog.open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.bank.add_name_dialog.open = false;
    } else if state.steam_deck.is_none() && crate::ui::check_toggle_key(ctx, egui::Key::B) {
        state.bank.window_open = false;
        state.bank.enlarged = None;
        return;
    }

    if state.steam_deck.is_none() && crate::ui::check_toggle_key(ctx, egui::Key::L) {
        state.ui.studio_window_open = !state.ui.studio_window_open;
    }

    let panel_frame = egui::Frame::central_panel(&ctx.style()).fill(theme::BACKDROP);
    egui::TopBottomPanel::top("bank_header")
        .frame(
            egui::Frame::side_top_panel(&ctx.style())
                .fill(theme::BACKDROP)
                .stroke(egui::Stroke::new(2.0, theme::RULE)),
        )
        .show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("◆ Mining Bank").heading().color(COLOR_TERM));
                let total = state.bank.entries.len();
                let plural = |n: usize| if n == 1 { "" } else { "s" };
                let text = if state.bank.filter_tag.trim().is_empty() {
                    format!("{total} word{}", plural(total))
                } else {
                    let shown = state.bank.visible_indices_cached().len();
                    format!("{shown} of {total} word{}", plural(total))
                };
                ui.label(RichText::new(text).color(COLOR_MUTED));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let add_btn = egui::Button::new(
                        RichText::new("➕ Add Name")
                            .font(egui::FontId::proportional(13.0))
                            .color(COLOR_TERM),
                    )
                    .fill(theme::BUTTON_CLIP)
                    .stroke(egui::Stroke::new(2.0, theme::RULE))
                    .rounding(0.0);
                    if ui
                        .add(add_btn)
                        .on_hover_text("Add a custom name to the dictionary")
                        .clicked()
                    {
                        state.bank.open_add_name_dialog();
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("🏷 Filter by tag:").color(COLOR_TAG));
                let mut compact_changed = false;
                {
                    let bank = &mut state.bank;
                    let out = tag_input(
                        ui,
                        "bank_tag_filter",
                        &mut bank.filter_tag,
                        &bank.known_tags,
                        "Type a tag (partial matches included)…",
                        320.0,
                        false,
                    );
                    if out.cancelled {
                        bank.filter_tag.clear();
                    }
                    if !bank.filter_tag.is_empty()
                        && render_close_button(ui, "Clear filter").clicked()
                    {
                        bank.filter_tag.clear();
                    }
                    ui.add_space(16.0);
                    let prev_compact = bank.compact_mode;
                    ui.checkbox(&mut bank.compact_mode, "Compact mode");
                    if bank.compact_mode != prev_compact {
                        bank.expanded_entries.clear();
                        bank.card_heights.clear();
                        compact_changed = true;
                    }
                }
                if compact_changed {
                    crate::config::save_config(state);
                }
            });
            ui.add_space(6.0);
        });

    let mut enlarge: Option<i64> = None;
    let mut delete: Option<i64> = None;
    let mut copy_screenshot: Option<i64> = None;
    let mut toggle_expand: Option<i64> = None;

    egui::CentralPanel::default().frame(panel_frame).show(ctx, |ui| {
        theme::apply_widget_visuals(ui);
        if let Some(err) = &state.bank.load_error {
            ui.colored_label(
                theme::BRICK,
                format!("⚠ Could not open the mining bank database: {err}"),
            );
            return;
        }
        if state.bank.entries.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No mined words yet.").size(20.0));
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Hover a word in an OCR box and click + in the dictionary popup to add it here.",
                    )
                    .color(COLOR_MUTED),
                );
            });
            return;
        }

        let visible = state.bank.visible_indices_cached();
        if visible.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(format!("No words tagged “{}”.", state.bank.filter_tag.trim()))
                        .size(20.0),
                );
            });
            return;
        }

        let today = crate::bank::current_local_day();
        let mut loads_left = THUMB_LOADS_PER_FRAME;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let avail_w = ui.available_width().max(1.0);
                let width_bucket = (avail_w / 4.0).round() as u32;
                state.bank.prepare_card_heights(width_bucket);

                let default_item_spacing = ui.spacing().item_spacing;
                ui.spacing_mut().item_spacing.y = 0.0;

                let view_min = viewport.min.y - VIRTUAL_OVERSCAN;
                let view_max = viewport.max.y + VIRTUAL_OVERSCAN;

                let mut top_skip_height = 0.0f32;
                let mut first_render_pos = visible.len();

                for (pos, &idx) in visible.iter().enumerate() {
                    let entry = &state.bank.entries[idx];
                    let is_expanded = !state.bank.compact_mode
                        || state.bank.expanded_entries.contains(&entry.id);
                    let gap = default_item_spacing.y + if is_expanded { 8.0 } else { 4.0 };
                    let est_card_h = state.bank.card_heights.get(&entry.id).copied().unwrap_or(
                        if is_expanded {
                            DEFAULT_FULL_CARD_HEIGHT
                        } else {
                            DEFAULT_COMPACT_CARD_HEIGHT
                        },
                    );
                    let stride = est_card_h + gap;
                    if top_skip_height + stride < view_min {
                        top_skip_height += stride;
                    } else {
                        first_render_pos = pos;
                        break;
                    }
                }

                if top_skip_height > 0.0 {
                    ui.allocate_space(egui::vec2(avail_w, top_skip_height));
                }

                let mut y_cursor = top_skip_height;
                let mut trailing_start_pos = visible.len();

                for (offset, &idx) in visible[first_render_pos..].iter().enumerate() {
                    if y_cursor > view_max {
                        trailing_start_pos = first_render_pos + offset;
                        break;
                    }
                    let entry_id = state.bank.entries[idx].id;
                    let created_at = state.bank.entries[idx].created_at;
                    let is_expanded = !state.bank.compact_mode
                        || state.bank.expanded_entries.contains(&entry_id);
                    let gap = default_item_spacing.y + if is_expanded { 8.0 } else { 4.0 };
                    let date_str = state
                        .bank
                        .formatted_friendly_date(entry_id, created_at, today);

                    let (action, measured_card_h) = if is_expanded {
                        draw_row_at(
                            ui,
                            ctx,
                            state,
                            idx,
                            &date_str,
                            default_item_spacing,
                            &mut loads_left,
                        )
                    } else {
                        let pitches = state.bank.compact_pitches_at(idx, &state.dict.pitch_db);
                        let entry = &state.bank.entries[idx];
                        draw_compact_row_with_date(
                            ui,
                            entry,
                            &pitches,
                            &date_str,
                            default_item_spacing,
                        )
                    };

                    state.bank.card_heights.insert(entry_id, measured_card_h);
                    match action {
                        RowAction::None => {}
                        RowAction::Enlarge => enlarge = Some(entry_id),
                        RowAction::Delete => delete = Some(entry_id),
                        RowAction::CopyScreenshot => copy_screenshot = Some(entry_id),
                        RowAction::ToggleExpand(id) => toggle_expand = Some(id),
                        RowAction::SetFilterTag(tag) => state.bank.filter_tag = tag,
                    }
                    ui.add_space(gap);
                    y_cursor += measured_card_h + gap;
                }

                let mut bottom_skip_height = 0.0f32;
                for &idx in &visible[trailing_start_pos..] {
                    let entry = &state.bank.entries[idx];
                    let is_expanded = !state.bank.compact_mode
                        || state.bank.expanded_entries.contains(&entry.id);
                    let gap = default_item_spacing.y + if is_expanded { 8.0 } else { 4.0 };
                    let est_card_h = state.bank.card_heights.get(&entry.id).copied().unwrap_or(
                        if is_expanded {
                            DEFAULT_FULL_CARD_HEIGHT
                        } else {
                            DEFAULT_COMPACT_CARD_HEIGHT
                        },
                    );
                    bottom_skip_height += est_card_h + gap;
                }
                if bottom_skip_height > 0.0 {
                    ui.allocate_space(egui::vec2(avail_w, bottom_skip_height));
                }
            });
        if loads_left == 0 {
            // More thumbnails are waiting to be decoded.
            ctx.request_repaint();
        }
    });

    if let Some(id) = enlarge {
        state.bank.enlarge(ctx, id);
        if state.bank.enlarged.is_none() {
            state.error("Failed to load screenshot.");
        }
    }
    if let Some(id) = delete {
        state.bank.pending_delete = None;
        if let Err(e) = state.bank.delete(id) {
            state.error(format!("Failed to delete entry: {e}"));
        }
    }
    if let Some(id) = copy_screenshot {
        state.copy_bank_screenshot(id, ctx);
    }
    if let Some(id) = toggle_expand {
        state.bank.toggle_entry_expanded(id);
    }

    draw_enlarged(state, ctx);
    draw_add_name_dialog(state, ctx);
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RowAction {
    None,
    Enlarge,
    Delete,
    CopyScreenshot,
    ToggleExpand(i64),
    SetFilterTag(String),
}

fn draw_compact_row_with_date(
    ui: &mut egui::Ui,
    entry: &BankEntryMeta,
    pitches: &[crate::dict::PitchAccent],
    date_str: &str,
    default_item_spacing: egui::Vec2,
) -> (RowAction, f32) {
    let mut action = RowAction::None;
    let card_height = 36.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), card_height),
        egui::Sense::click(),
    );
    let is_hovered = response.hovered();
    if is_hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let row_clicked = response.on_hover_text("Click to expand entry").clicked();

    ui.painter().extend(theme::panel_shapes(
        rect,
        1.0,
        is_hovered.then_some(theme::GOLD),
    ));

    let inner_rect = rect.shrink2(egui::vec2(12.0, 5.0));
    let mut tag_clicked = false;

    ui.allocate_ui_at_rect(inner_rect, |ui| {
        ui.spacing_mut().item_spacing = default_item_spacing;
        ui.horizontal(|ui| {
            let arrow_color = if is_hovered { theme::GOLD } else { COLOR_MUTED };
            ui.label(RichText::new("▶").size(11.0).color(arrow_color));
            ui.add_space(4.0);
            render_headword_and_reading(
                ui,
                &entry.term,
                &entry.reading,
                pitches,
                19.0,
                15.0,
                COLOR_TERM,
                COLOR_READING,
            );

            // Subtle subordinate tag chip
            if let Some(tag) = &entry.tag {
                ui.add_space(6.0);
                let tag_resp = render_subtle_tag_chip(ui, tag);
                if tag_resp.clicked() {
                    tag_clicked = true;
                    action = RowAction::SetFilterTag(tag.clone());
                }
            }

            // Timestamp aligned to the right
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(date_str)
                        .font(egui::FontId::proportional(11.5))
                        .color(COLOR_MUTED),
                );
            });
        });
    });

    if row_clicked && !tag_clicked {
        action = RowAction::ToggleExpand(entry.id);
    }

    (action, rect.height())
}

fn draw_row_at(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    state: &mut AppState,
    idx: usize,
    date_str: &str,
    default_item_spacing: egui::Vec2,
    loads_left: &mut usize,
) -> (RowAction, f32) {
    let mut action = RowAction::None;
    let entry_id = state.bank.entries[idx].id;
    let has_screenshot = state.bank.entries[idx].has_screenshot;

    // Retro panel chrome is painted behind the row once its size is known.
    let chrome_idx = ui.painter().add(egui::Shape::Noop);
    let row = egui::Frame::none()
        .inner_margin(egui::Margin::same(12.0))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = default_item_spacing;
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                if state.bank.compact_mode {
                    let arrow_size = egui::vec2(22.0, 22.0);
                    let (arrow_rect, arrow_resp) =
                        ui.allocate_exact_size(arrow_size, egui::Sense::click());
                    let arrow_hovered = arrow_resp.hovered();
                    if arrow_hovered {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    let arrow_bg = if arrow_hovered {
                        COLOR_TAG_HOVER_BG
                    } else {
                        Color32::TRANSPARENT
                    };
                    ui.painter().rect_filled(arrow_rect, 0.0, arrow_bg);
                    let arrow_color = if arrow_hovered {
                        theme::GOLD
                    } else {
                        COLOR_MUTED
                    };
                    ui.painter().text(
                        arrow_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "▼",
                        egui::FontId::proportional(11.0),
                        arrow_color,
                    );
                    if arrow_resp
                        .on_hover_text("Click to collapse entry")
                        .clicked()
                    {
                        action = RowAction::ToggleExpand(entry_id);
                    }
                    ui.add_space(4.0);
                }

                // Screenshot preview
                let sense = if has_screenshot {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                };
                let (rect, response) = ui.allocate_exact_size(THUMB_SIZE, sense);
                if ui.is_rect_visible(rect) {
                    let cached = state.bank.thumbnails.contains_key(&entry_id);
                    let texture = if cached || *loads_left > 0 {
                        if !cached {
                            *loads_left -= 1;
                        }
                        state.bank.thumbnail_by_id(ctx, entry_id, has_screenshot)
                    } else {
                        None
                    };
                    paint_thumbnail(ui, rect, texture.as_ref(), has_screenshot, cached);
                }
                if has_screenshot {
                    let copying = state.bank.copying_screenshot == Some(entry_id);
                    let copy_id = ui.id().with(("copy_btn", entry_id));
                    let btn_rect = egui::Rect::from_min_size(
                        rect.min + egui::vec2(6.0, 6.0),
                        egui::vec2(28.0, 28.0),
                    );
                    let copy_resp = ui.interact(
                        btn_rect,
                        copy_id,
                        if copying {
                            egui::Sense::hover()
                        } else {
                            egui::Sense::click()
                        },
                    );
                    paint_copy_overlay(ui.painter(), btn_rect, copy_resp.hovered(), copying);

                    if copy_resp.clicked() {
                        action = RowAction::CopyScreenshot;
                    } else if response.clicked() {
                        action = RowAction::Enlarge;
                    }

                    if copy_resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        copy_resp.on_hover_text(if copying {
                            "Copying screenshot to clipboard…"
                        } else {
                            "Copy screenshot to clipboard"
                        });
                    } else if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ZoomIn);
                        response.on_hover_text("Click to enlarge");
                    }
                }

                ui.add_space(8.0);
                let (dict_entry, fallback_pitches) = state
                    .bank
                    .dict_entry_and_pitches_at(idx, &state.dict.pitch_db);
                ui.vertical(|ui| {
                    // Entry header (word, reading, frequency, tags), date and delete button
                    ui.horizontal(|ui| {
                        let controls_width = 220.0;
                        let header_width = (ui.available_width() - controls_width).max(120.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(header_width, 0.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| match &dict_entry {
                                Some(de) => with_dict_scale(DICT_SCALE, || {
                                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                                    render_entry_header(ui, de, false);
                                }),
                                None => {
                                    let entry = &state.bank.entries[idx];
                                    render_headword_and_reading(
                                        ui,
                                        &entry.term,
                                        &entry.reading,
                                        &fallback_pitches,
                                        28.0,
                                        21.0,
                                        COLOR_TERM,
                                        COLOR_READING,
                                    );
                                }
                            },
                        );
                        let _controls_resp =
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                                if state.bank.pending_delete == Some(entry_id) {
                                    let cancel_btn = egui::Button::new(
                                        RichText::new("Cancel").color(COLOR_TERM),
                                    )
                                    .fill(COLOR_TAG_BG)
                                    .stroke(egui::Stroke::new(1.5, COLOR_TAG_STROKE));
                                    if ui.add(cancel_btn).clicked() {
                                        state.bank.pending_delete = None;
                                    }
                                    let delete_btn = egui::Button::new(
                                        RichText::new("Delete").color(theme::DANGER_TEXT),
                                    )
                                    .fill(theme::BUTTON_DANGER_HOVER)
                                    .stroke(egui::Stroke::new(1.5, theme::BRICK));
                                    if ui.add(delete_btn).clicked() {
                                        action = RowAction::Delete;
                                    }
                                } else {
                                    let del_size = egui::vec2(28.0, 28.0);
                                    let (del_rect, del_resp) =
                                        ui.allocate_exact_size(del_size, egui::Sense::click());
                                    let del_hovered = del_resp.hovered();
                                    if del_hovered {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    paint_delete_button(ui.painter(), del_rect, del_hovered);
                                    if del_resp.on_hover_text("Delete this entry").clicked() {
                                        state.bank.pending_delete = Some(entry_id);
                                    }
                                    ui.add_space(8.0);
                                    if state.steam_deck.is_none() {
                                        if let Some(clip_url) =
                                            state.bank.entries[idx].twitch_clip_url.as_deref()
                                        {
                                            let clip_btn = ui.link(
                                                RichText::new("🎬 Clip")
                                                    .color(theme::DEINFLECT_TEXT)
                                                    .font(egui::FontId::proportional(12.5)),
                                            );
                                            if clip_btn
                                                .on_hover_text(format!(
                                                    "Open Twitch clip:\n{clip_url}"
                                                ))
                                                .clicked()
                                            {
                                                ctx.output_mut(|o| {
                                                    o.open_url = Some(
                                                        egui::output::OpenUrl::same_tab(clip_url),
                                                    )
                                                });
                                            }
                                            ui.add_space(8.0);
                                        }
                                    }
                                    ui.label(
                                        RichText::new(date_str)
                                            .font(egui::FontId::proportional(12.5))
                                            .color(COLOR_MUTED),
                                    );
                                }
                            });
                    });

                    // Tag (click to edit)
                    ui.add_space(2.0);
                    draw_tag_row_at(ui, state, Some(idx), entry_id, None);

                    // Sentence, with the mined word highlighted
                    ui.add_space(4.0);
                    let job = sentence_job(&state.bank.entries[idx], ui.available_width());
                    ui.label(job);

                    // Dictionary entry senses, as shown in the popup (without examples)
                    ui.add_space(6.0);
                    match &dict_entry {
                        Some(de) => with_dict_scale(DICT_SCALE, || {
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 3.0);
                            render_glossaries(ui, de);
                        }),
                        None => {
                            let def_text = &state.bank.entries[idx].definition_text;
                            if !def_text.is_empty() {
                                ui.label(
                                    RichText::new(def_text)
                                        .font(dict_font(17.0))
                                        .color(COLOR_DEFINITION),
                                );
                            }
                        }
                    }
                });
            });
        });
    ui.painter().set(
        chrome_idx,
        egui::Shape::Vec(theme::panel_shapes(row.response.rect, 1.0, None)),
    );

    (action, row.response.rect.height())
}

/// Draws the entry's tag chip with an integrated sleek remove button, or the inline tag editor if this entry is being edited.
#[cfg(test)]
fn draw_tag_row(ui: &mut egui::Ui, state: &mut AppState, entry: &BankEntryMeta) {
    draw_tag_row_at(ui, state, None, entry.id, entry.tag.as_deref());
}

fn draw_tag_row_at(
    ui: &mut egui::Ui,
    state: &mut AppState,
    entry_idx: Option<usize>,
    entry_id: i64,
    fallback_tag: Option<&str>,
) {
    let editing = state
        .bank
        .editing_tag
        .as_ref()
        .map_or(false, |e| e.id == entry_id);
    ui.horizontal(|ui| {
        if editing {
            let bank = &mut state.bank;
            let Some(edit) = bank.editing_tag.as_mut() else {
                return;
            };
            let request_focus = !edit.focused;
            edit.focused = true;
            ui.label(RichText::new("🏷").color(COLOR_TAG));
            let out = tag_input(
                ui,
                ("bank_tag_edit", entry_id),
                &mut edit.text,
                &bank.known_tags,
                "Tag (leave empty for none)",
                260.0,
                request_focus,
            );
            ui.label(
                RichText::new("Enter to save · Esc to cancel")
                    .small()
                    .color(COLOR_MUTED),
            );
            if out.cancelled {
                state.bank.editing_tag = None;
            } else if out.committed {
                let text = edit.text.clone();
                state.bank.editing_tag = None;
                if let Err(e) = state.bank.set_tag(entry_id, &text) {
                    state.error(format!("Failed to update tag: {e}"));
                }
            }
            return;
        }

        let start_edit = |state: &mut AppState, text: String| {
            state.bank.editing_tag = Some(TagEdit {
                id: entry_id,
                text,
                focused: false,
            });
        };
        let current_tag = entry_idx
            .and_then(|i| state.bank.entries.get(i))
            .and_then(|e| e.tag.as_deref())
            .or(fallback_tag);
        match current_tag {
            Some(tag) => {
                let font_id = egui::FontId::proportional(12.0);
                let text = format!("🏷 {tag}");
                let galley = ui.painter().layout_no_wrap(text, font_id, COLOR_TAG);

                let padding_left = 8.0;
                let remove_size = 18.0;
                let padding_right = 5.0;
                let gap = 6.0;
                let height = 24.0;
                let width = padding_left + galley.size().x + gap + remove_size + padding_right;

                let (chip_rect, mut response) =
                    ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());

                let remove_rect = egui::Rect::from_min_max(
                    egui::pos2(
                        chip_rect.max.x - (remove_size + padding_right + 3.0),
                        chip_rect.min.y,
                    ),
                    chip_rect.max,
                );

                let is_hovered = response.hovered();
                let pointer_pos = ui.ctx().pointer_latest_pos();
                let remove_hovered =
                    is_hovered && pointer_pos.map_or(false, |p| remove_rect.contains(p));
                let tag_hovered = is_hovered && !remove_hovered;

                if is_hovered {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }

                let (bg, border) = if tag_hovered {
                    (COLOR_TAG_HOVER_BG, COLOR_TAG_HOVER_STROKE)
                } else {
                    (COLOR_TAG_BG, COLOR_TAG_STROKE)
                };

                // Draw base chip
                ui.painter()
                    .rect(chip_rect, 0.0, bg, egui::Stroke::new(2.0, border));

                // Draw tag text
                let text_pos = egui::pos2(
                    chip_rect.min.x + padding_left,
                    chip_rect.center().y - galley.size().y / 2.0,
                );
                ui.painter().galley(
                    text_pos,
                    galley,
                    if tag_hovered { COLOR_TERM } else { COLOR_TAG },
                );

                // Draw integrated 'X' remove button
                let remove_center = egui::pos2(
                    chip_rect.max.x - padding_right - remove_size / 2.0,
                    chip_rect.center().y,
                );

                let (x_stroke_color, x_bg) = if remove_hovered {
                    (theme::DANGER_TEXT, theme::BUTTON_DANGER_HOVER)
                } else {
                    (COLOR_TAG, Color32::TRANSPARENT)
                };

                if x_bg != Color32::TRANSPARENT {
                    ui.painter().rect_filled(
                        egui::Rect::from_center_size(remove_center, egui::vec2(16.0, 16.0)),
                        0.0,
                        x_bg,
                    );
                }

                let arm = 3.5;
                let stroke = egui::Stroke::new(1.4, x_stroke_color);
                ui.painter().line_segment(
                    [
                        remove_center + egui::vec2(-arm, -arm),
                        remove_center + egui::vec2(arm, arm),
                    ],
                    stroke,
                );
                ui.painter().line_segment(
                    [
                        remove_center + egui::vec2(-arm, arm),
                        remove_center + egui::vec2(arm, -arm),
                    ],
                    stroke,
                );

                // Tooltip
                if remove_hovered {
                    response = response.on_hover_text("Remove tag");
                } else if tag_hovered {
                    response = response.on_hover_text("Click to edit tag");
                }

                // Click handling
                if response.clicked() {
                    let click_pos = ui.ctx().input(|i| i.pointer.interact_pos());
                    let clicked_remove = click_pos.map_or(false, |p| remove_rect.contains(p));
                    if clicked_remove {
                        if let Err(e) = state.bank.set_tag(entry_id, "") {
                            state.error(format!("Failed to remove tag: {e}"));
                        }
                    } else {
                        let tag_owned = tag.to_string();
                        start_edit(state, tag_owned);
                    }
                }
            }
            None => {
                let add_btn = egui::Button::new(
                    RichText::new("🏷 Add tag")
                        .font(egui::FontId::proportional(12.0))
                        .color(COLOR_MUTED),
                )
                .fill(COLOR_TAG_BG)
                .stroke(egui::Stroke::new(2.0, COLOR_TAG_STROKE))
                .rounding(0.0);

                if ui
                    .add(add_btn)
                    .on_hover_text("Add a tag to this word")
                    .clicked()
                {
                    start_edit(state, String::new());
                }
            }
        }
    });
}

/// Settings-panel row for the tag applied to newly mined words (shown above "Appearance").
/// Returns true if the setting changed.
pub fn draw_tag_setting(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    let mut committed = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Mining Tag:")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(150, 150, 150)),
        );
        let bank = &mut state.bank;
        let out = tag_input(
            ui,
            "settings_mining_tag",
            &mut bank.current_tag,
            &bank.known_tags,
            "e.g. Final Fantasy 7",
            140.0,
            false,
        );
        if out.cancelled {
            bank.current_tag = bank.saved_current_tag.clone();
        }
        committed = out.committed;
        if !bank.current_tag.is_empty() && render_close_button(ui, "Clear the tag").clicked() {
            bank.current_tag.clear();
            committed = true;
        }
    });

    if !committed {
        return false;
    }
    commit_tag_setting(state)
}

/// Commit edits when a settings page is hidden or its explicit save button is used.
pub(crate) fn commit_tag_setting(state: &mut AppState) -> bool {
    let bank = &mut state.bank;
    bank.current_tag =
        tags::canonicalize_tag(&bank.current_tag, &bank.known_tags).unwrap_or_default();
    if bank.current_tag == bank.saved_current_tag {
        return false;
    }
    bank.saved_current_tag = bank.current_tag.clone();
    crate::config::save_config(state);
    true
}

fn paint_thumbnail(
    ui: &egui::Ui,
    rect: egui::Rect,
    texture: Option<&egui::TextureHandle>,
    has_screenshot: bool,
    loaded: bool,
) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, theme::EDGE);
    painter.rect_stroke(
        rect.expand(1.0),
        0.0,
        egui::Stroke::new(2.0, theme::TAG_BORDER),
    );
    match texture {
        Some(tex) => {
            let image_rect = fit_rect(tex.size_vec2(), rect);
            painter.image(
                tex.id(),
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => {
            let text = if has_screenshot && !loaded {
                "Loading…"
            } else {
                "No screenshot"
            };
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(14.0),
                COLOR_MUTED,
            );
        }
    }
}

fn paint_copy_overlay(painter: &egui::Painter, rect: egui::Rect, hovered: bool, copying: bool) {
    let (bg, stroke_color, icon_color) = if copying {
        (theme::TAG_BG, theme::TAG_BORDER, theme::TAG_TEXT)
    } else if hovered {
        (theme::BUTTON_CLIP_HOVER, theme::FRAME, COLOR_TERM)
    } else {
        (theme::WINDOW, theme::RULE, theme::TEXT)
    };

    painter.rect_filled(rect, 0.0, bg);
    painter.rect_stroke(rect, 0.0, egui::Stroke::new(2.0, stroke_color));

    let center = rect.center();
    if copying {
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            "…",
            egui::FontId::proportional(16.0),
            icon_color,
        );
    } else {
        let stroke = egui::Stroke::new(1.5, icon_color);
        // Back sheet (top-right)
        let back =
            egui::Rect::from_min_size(center + egui::vec2(-2.0, -7.0), egui::vec2(10.0, 12.0));
        painter.rect_stroke(back, 1.5, stroke);

        // Front sheet (bottom-left) - filled with bg to occlude overlapping back sheet
        let front =
            egui::Rect::from_min_size(center + egui::vec2(-7.0, -3.0), egui::vec2(10.0, 12.0));
        painter.rect_filled(front, 1.5, bg);
        painter.rect_stroke(front, 1.5, stroke);

        // Lines on front sheet to clearly represent a document
        let line_stroke = egui::Stroke::new(1.0, icon_color);
        painter.line_segment(
            [
                center + egui::vec2(-4.5, 0.5),
                center + egui::vec2(0.5, 0.5),
            ],
            line_stroke,
        );
        painter.line_segment(
            [
                center + egui::vec2(-4.5, 3.5),
                center + egui::vec2(0.5, 3.5),
            ],
            line_stroke,
        );
    }
}

/// Largest rect with the aspect ratio of `size`, centered inside `bounds`.
fn fit_rect(size: egui::Vec2, bounds: egui::Rect) -> egui::Rect {
    if size.x <= 0.0 || size.y <= 0.0 {
        return bounds;
    }
    let scale = (bounds.width() / size.x).min(bounds.height() / size.y);
    egui::Rect::from_center_size(bounds.center(), size * scale)
}

fn char_index_to_byte_offset(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map_or(s.len(), |(byte_pos, _)| byte_pos)
}

fn sentence_job(entry: &BankEntryMeta, wrap_width: f32) -> LayoutJob {
    let s = entry.sentence.as_str();
    let start_byte = char_index_to_byte_offset(s, entry.word_range.0);
    let end_char = entry.word_range.1.max(entry.word_range.0);
    let end_byte = char_index_to_byte_offset(s, end_char).max(start_byte);
    let normal = TextFormat {
        font_id: dict_font(22.0),
        color: COLOR_SENTENCE,
        ..Default::default()
    };
    let highlight = TextFormat {
        font_id: dict_font(22.0),
        color: COLOR_HIGHLIGHT,
        background: COLOR_HIGHLIGHT_BG,
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    for (slice, format) in [
        (&s[..start_byte], &normal),
        (&s[start_byte..end_byte], &highlight),
        (&s[end_byte..], &normal),
    ] {
        if !slice.is_empty() {
            job.append(slice, 0.0, format.clone());
        }
    }
    job
}

fn draw_enlarged(state: &mut AppState, ctx: &egui::Context) {
    let Some((_, texture)) = state.bank.enlarged.as_ref() else {
        return;
    };
    let screen = ctx.screen_rect();
    let mut close = false;
    egui::Area::new(egui::Id::new("bank_enlarged_screenshot"))
        .fixed_pos(screen.min)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
            let painter = ui.painter();
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha(225));
            let image_bounds = egui::Rect::from_min_max(
                rect.min + egui::vec2(24.0, 24.0),
                rect.max - egui::vec2(24.0, 48.0),
            );
            let image_rect = fit_rect(texture.size_vec2(), image_bounds);
            painter.image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            painter.text(
                egui::pos2(rect.center().x, rect.max.y - 24.0),
                egui::Align2::CENTER_CENTER,
                "Click anywhere or press Esc to close",
                egui::FontId::proportional(14.0),
                COLOR_MUTED,
            );
            if response.clicked() {
                close = true;
            }
        });
    if close {
        state.bank.enlarged = None;
    }
}

fn draw_add_name_dialog(state: &mut AppState, ctx: &egui::Context) {
    if !state.bank.add_name_dialog.open {
        return;
    }

    let screen_rect = ctx.screen_rect();
    // Dim the background behind the modal dialog
    egui::Area::new(egui::Id::new("bank_add_name_modal_backdrop"))
        .fixed_pos(screen_rect.min)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen_rect.size(), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 0.0, Color32::from_black_alpha(160));
        });

    let mut close = false;
    let mut save = false;

    egui::Window::new("Add Custom Name")
        .id(egui::Id::new("bank_add_name_dialog"))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .max_height(crate::ui::scaling::max_dialog_size(ctx).y)
        .vscroll(true)
        .default_width(380.0)
        .show(ctx, |ui| {
            ui.set_width(380.0);
            ui.add_space(4.0);

            if let Some(err) = &state.bank.add_name_dialog.error_msg {
                ui.colored_label(Color32::from_rgb(248, 113, 113), format!("⚠ {err}"));
                ui.add_space(4.0);
            }

            ui.label(RichText::new("Kanji / Word:").strong());
            let kanji_edit = ui.add(
                egui::TextEdit::singleline(&mut state.bank.add_name_dialog.kanji)
                    .hint_text("e.g. 田中")
                    .desired_width(f32::INFINITY),
            );
            if state.bank.add_name_dialog.focus {
                kanji_edit.request_focus();
                state.bank.add_name_dialog.focus = false;
            }

            ui.add_space(6.0);
            ui.label(RichText::new("Furigana / Reading:").strong());
            ui.add(
                egui::TextEdit::singleline(&mut state.bank.add_name_dialog.furigana)
                    .hint_text("e.g. たなか")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(6.0);
            ui.label(RichText::new("Source Tag (optional):").color(COLOR_TAG));
            ui.add(
                egui::TextEdit::singleline(&mut state.bank.add_name_dialog.source_tag)
                    .hint_text("Defaults to current mining tag; leave empty for none")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(6.0);
            ui.label(RichText::new("Notes (optional):").color(COLOR_MUTED));
            ui.add(
                egui::TextEdit::multiline(&mut state.bank.add_name_dialog.notes)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("Any extra context, character notes, etc."),
            );

            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui.button("Save Name").clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });

    if save {
        let kanji = state.bank.add_name_dialog.kanji.trim().to_string();
        let furigana = state.bank.add_name_dialog.furigana.trim().to_string();
        let source_tag = {
            let t = state.bank.add_name_dialog.source_tag.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        };
        let notes = {
            let n = state.bank.add_name_dialog.notes.trim();
            if n.is_empty() {
                None
            } else {
                Some(n.to_string())
            }
        };

        if kanji.is_empty() || furigana.is_empty() {
            state.bank.add_name_dialog.error_msg =
                Some("Kanji and Furigana / Reading are required.".to_string());
        } else {
            match state.bank.add_custom_name(
                &kanji,
                &furigana,
                source_tag.as_deref(),
                notes.as_deref(),
            ) {
                Ok(_) => {
                    crate::dict::lookup::invalidate_scan_cache();
                    state.info(format!("Added custom name: {kanji} ({furigana})"));
                    state.bank.add_name_dialog.open = false;
                }
                Err(e) => {
                    state.bank.add_name_dialog.error_msg =
                        Some(format!("Failed to save custom name: {e}"));
                }
            }
        }
    }
    if close {
        state.bank.add_name_dialog.open = false;
    }
}

fn render_subtle_tag_chip(ui: &mut egui::Ui, tag: &str) -> egui::Response {
    let font_id = egui::FontId::proportional(11.0);
    let text = format!("🏷 {tag}");
    let galley = ui.painter().layout_no_wrap(text, font_id, COLOR_TAG);
    let padding = egui::vec2(6.0, 2.0);
    let size = galley.size() + padding * 2.0;
    let (chip_rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let (bg, border) = if hovered {
        (COLOR_TAG_HOVER_BG, COLOR_TAG_HOVER_STROKE)
    } else {
        (COLOR_TAG_BG, COLOR_TAG_STROKE)
    };
    ui.painter()
        .rect(chip_rect, 0.0, bg, egui::Stroke::new(1.5, border));
    ui.painter().galley(
        chip_rect.min + padding,
        galley,
        if hovered { COLOR_TERM } else { COLOR_TAG },
    );
    response.on_hover_text("Click to filter by this tag")
}

fn render_close_button(ui: &mut egui::Ui, hover_text: &str) -> egui::Response {
    let size = egui::vec2(20.0, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let hovered = resp.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let (bg, stroke_color, border_color) = if hovered {
        (theme::BUTTON_DANGER_HOVER, theme::DANGER_TEXT, theme::BRICK)
    } else {
        (COLOR_TAG_BG, COLOR_TAG, COLOR_TAG_STROKE)
    };
    ui.painter().rect_filled(rect, 0.0, bg);
    ui.painter()
        .rect_stroke(rect, 0.0, egui::Stroke::new(1.5, border_color));
    let center = rect.center();
    let arm = 3.5;
    let stroke = egui::Stroke::new(1.4, stroke_color);
    ui.painter().line_segment(
        [
            center + egui::vec2(-arm, -arm),
            center + egui::vec2(arm, arm),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            center + egui::vec2(-arm, arm),
            center + egui::vec2(arm, -arm),
        ],
        stroke,
    );
    resp.on_hover_text(hover_text)
}

#[cfg(test)]
fn format_friendly_timestamp(ms: i64) -> String {
    crate::bank::format_friendly_timestamp_with_day(ms, crate::bank::current_local_day())
}

fn paint_delete_button(painter: &egui::Painter, rect: egui::Rect, hovered: bool) {
    let (bg, icon_color) = if hovered {
        (theme::BUTTON_DANGER_HOVER, theme::DANGER_TEXT)
    } else {
        (COLOR_TAG_BG, COLOR_TAG)
    };

    theme::paint_bevel(painter, rect, bg, false, 1.0);

    let center = rect.center();
    let stroke = egui::Stroke::new(1.4, icon_color);

    // Lid top handle
    let handle = egui::Rect::from_min_max(
        center + egui::vec2(-2.5, -7.0),
        center + egui::vec2(2.5, -5.5),
    );
    painter.rect_stroke(handle, 0.5, stroke);

    // Lid horizontal bar
    painter.line_segment(
        [
            center + egui::vec2(-6.5, -5.0),
            center + egui::vec2(6.5, -5.0),
        ],
        stroke,
    );

    // Can body (tapered slightly inward at bottom)
    let body_top_left = center + egui::vec2(-5.0, -3.5);
    let body_top_right = center + egui::vec2(5.0, -3.5);
    let body_bottom_left = center + egui::vec2(-4.0, 6.0);
    let body_bottom_right = center + egui::vec2(4.0, 6.0);

    painter.line_segment([body_top_left, body_bottom_left], stroke);
    painter.line_segment([body_bottom_left, body_bottom_right], stroke);
    painter.line_segment([body_bottom_right, body_top_right], stroke);

    // Inner vertical slots
    let inner_stroke = egui::Stroke::new(1.0, icon_color);
    painter.line_segment(
        [
            center + egui::vec2(-1.5, -1.5),
            center + egui::vec2(-1.5, 4.0),
        ],
        inner_stroke,
    );
    painter.line_segment(
        [
            center + egui::vec2(1.5, -1.5),
            center + egui::vec2(1.5, 4.0),
        ],
        inner_stroke,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_friendly_timestamp_formats_valid_string() {
        let ts = 1700000000_000i64; // arbitrary valid unix timestamp
        let formatted = format_friendly_timestamp(ts);
        assert!(!formatted.is_empty());
        assert!(formatted.contains(':'));
    }

    #[test]
    fn test_bank_headword_reading_job_baseline_alignment() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "noto_sans_jp".to_owned(),
            egui::FontData::from_static(include_bytes!("../../assets/NotoSansJP-Regular.ttf")),
        );
        fonts.families.insert(
            egui::FontFamily::Name("GothicCJK".into()),
            vec!["noto_sans_jp".to_owned()],
        );
        ctx.set_fonts(fonts);
        let _ = ctx.begin_frame(egui::RawInput::default());

        let job =
            build_headword_reading_job("暗い", "くらい", 19.0, 15.0, COLOR_TERM, COLOR_READING);
        let galley = ctx.fonts(|f| f.layout_job(job));
        assert_eq!(galley.rows.len(), 1);
        let baseline_y = galley.rows[0].glyphs[0].pos.y;
        for g in &galley.rows[0].glyphs {
            assert!(
                (g.pos.y - baseline_y).abs() < f32::EPSILON,
                "Compact row glyph '{}' pos.y ({}) did not match headword baseline ({})",
                g.chr,
                g.pos.y,
                baseline_y
            );
        }
    }

    #[test]
    fn test_draw_tag_row_renders_and_editing_initializes() {
        let ctx = egui::Context::default();
        let _ = ctx.begin_frame(egui::RawInput::default());
        let mut state = AppState::default();
        let entry = BankEntryMeta {
            id: 1,
            created_at: 1700000000_000,
            term: "除外".into(),
            reading: "じょがい".into(),
            definition_text: "exception".into(),
            definition_json: "".into(),
            sentence: "".into(),
            word_range: (0, 0),
            has_screenshot: false,
            tag: Some("インタールード".into()),
            twitch_clip_url: None,
        };

        egui::CentralPanel::default().show(&ctx, |ui| {
            draw_tag_row(ui, &mut state, &entry);
        });
        assert!(state.bank.editing_tag.is_none());
    }

    #[test]
    fn test_bank_row_virtualization_renders_only_viewport_subset() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "noto_sans_jp".to_owned(),
            egui::FontData::from_static(include_bytes!("../../assets/NotoSansJP-Regular.ttf")),
        );
        fonts.families.insert(
            egui::FontFamily::Name("GothicCJK".into()),
            vec!["noto_sans_jp".to_owned()],
        );
        ctx.set_fonts(fonts);

        let mut state = AppState::default();
        state.bank.window_open = true;
        for i in 0..200i64 {
            state.bank.entries.push(BankEntryMeta {
                id: i + 1,
                created_at: 1_700_000_000_000 + i * 1_000,
                term: format!("単語{i}"),
                reading: "たんご".into(),
                definition_text: "word".into(),
                definition_json: r#"{"term":"単語","reading":"たんご","sequence":1,"pitches":[],"glossary":["word"]}"#.into(),
                sentence: format!("これは単語{i}の例文です。"),
                word_range: (3, 5),
                has_screenshot: false,
                tag: Some(if i % 2 == 0 {
                    "Xenogears".into()
                } else {
                    "Grandia".into()
                }),
                twitch_clip_url: None,
            });
        }

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input.clone(), |ctx| {
            draw_contents(&mut state, ctx);
        });

        let measured_full = state.bank.card_heights.len();
        assert!(
            measured_full > 0 && measured_full < 30,
            "expected row virtualization to measure only viewport+overscan subset of 200 cards, got {measured_full}"
        );

        // Switching to compact mode clears old heights and virtualizes compact rows too.
        state.bank.compact_mode = true;
        let _ = ctx.run(input, |ctx| {
            draw_contents(&mut state, ctx);
        });
        let measured_compact = state.bank.card_heights.len();
        assert!(
            measured_compact > 0 && measured_compact < 60,
            "expected compact row virtualization to measure only viewport+overscan subset of 200 cards, got {measured_compact}"
        );
    }
}
