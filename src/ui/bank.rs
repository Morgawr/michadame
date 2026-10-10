//! Standalone "Mining Bank" window (toggled with B): lists mined words, most recent first.

use crate::app::AppState;
use crate::bank::{tags, BankEntryMeta, TagEdit};
use crate::dict::render::{
    build_headword_reading_job, dict_font, render_entry_header, render_glossaries, with_dict_scale,
};
use crate::ui::tag_input::tag_input;
use eframe::egui::{self, text::LayoutJob, Color32, RichText, TextFormat};

const ROW_BG: Color32 = Color32::from_rgb(24, 27, 34);
const ROW_STROKE: Color32 = Color32::from_rgb(51, 65, 85);
const COLOR_TERM: Color32 = Color32::from_rgb(248, 250, 252);
const COLOR_READING: Color32 = Color32::from_rgb(56, 189, 248);
const COLOR_SENTENCE: Color32 = Color32::from_rgb(226, 232, 240);
const COLOR_HIGHLIGHT: Color32 = Color32::from_rgb(253, 224, 71);
const COLOR_HIGHLIGHT_BG: Color32 = Color32::from_rgba_premultiplied(66, 56, 10, 160);
const COLOR_MUTED: Color32 = Color32::from_rgb(100, 116, 139);
const COLOR_DEFINITION: Color32 = Color32::from_rgb(203, 213, 225);
const COLOR_TAG: Color32 = Color32::from_rgb(165, 175, 195);
const COLOR_TAG_BG: Color32 = Color32::from_rgba_premultiplied(45, 52, 68, 160);
const COLOR_TAG_STROKE: Color32 = Color32::from_rgba_premultiplied(75, 85, 105, 120);

/// Scale of the dictionary entry relative to the (large, over-video) popup rendering.
const DICT_SCALE: f32 = 0.55;
/// Display size of list thumbnails (images are fit inside, preserving aspect ratio).
const THUMB_SIZE: egui::Vec2 = egui::vec2(240.0, 135.0);
/// Maximum number of thumbnails decoded per frame, to keep scrolling smooth.
const THUMB_LOADS_PER_FRAME: usize = 6;

pub fn draw_bank_window(state: &mut AppState, ctx: &egui::Context) {
    if !state.bank.window_open {
        return;
    }
    ctx.show_viewport_immediate(
        egui::ViewportId::from_hash_of("bank_window"),
        egui::ViewportBuilder::default()
            .with_title("Michadame Mining Bank")
            .with_inner_size([900.0, 900.0]),
        |ctx, class| {
            assert!(
                class == egui::ViewportClass::Immediate,
                "This egui backend doesn't support multiple viewports"
            );
            draw_contents(state, ctx);
            if ctx.input(|i| i.viewport().close_requested()) {
                state.bank.window_open = false;
                state.bank.enlarged = None;
            }
        },
    );
}

fn draw_contents(state: &mut AppState, ctx: &egui::Context) {
    // Keyboard: Esc closes the enlarged screenshot; B closes the window.
    if state.bank.enlarged.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.bank.enlarged = None;
    } else if !ctx.wants_keyboard_input()
        && ctx.input(|i| i.modifiers.is_none() && i.key_pressed(egui::Key::B))
    {
        state.bank.window_open = false;
        state.bank.enlarged = None;
        return;
    }

    egui::TopBottomPanel::top("bank_header").show(ctx, |ui| {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.heading("Mining Bank");
            let total = state.bank.entries.len();
            let plural = |n: usize| if n == 1 { "" } else { "s" };
            let text = if state.bank.filter_tag.trim().is_empty() {
                format!("{total} word{}", plural(total))
            } else {
                let shown = state.bank.visible_indices().len();
                format!("{shown} of {total} word{}", plural(total))
            };
            ui.label(RichText::new(text).color(COLOR_MUTED));
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
                if !bank.filter_tag.is_empty() && render_close_button(ui, "Clear filter").clicked() {
                    bank.filter_tag.clear();
                }
                ui.add_space(16.0);
                let prev_compact = bank.compact_mode;
                ui.checkbox(&mut bank.compact_mode, "Compact mode");
                if bank.compact_mode != prev_compact {
                    bank.expanded_entries.clear();
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

    egui::CentralPanel::default().show(ctx, |ui| {
        if let Some(err) = &state.bank.load_error {
            ui.colored_label(
                Color32::from_rgb(255, 100, 100),
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

        let visible = state.bank.visible_indices();
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

        let mut loads_left = THUMB_LOADS_PER_FRAME;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for idx in visible {
                    let entry = state.bank.entries[idx].clone();
                    let is_expanded = !state.bank.compact_mode
                        || state.bank.expanded_entries.contains(&entry.id);
                    let action = if is_expanded {
                        draw_row(ui, ctx, state, &entry, &mut loads_left)
                    } else {
                        draw_compact_row(ui, &entry)
                    };
                    match action {
                        RowAction::None => {}
                        RowAction::Enlarge => enlarge = Some(entry.id),
                        RowAction::Delete => delete = Some(entry.id),
                        RowAction::CopyScreenshot => copy_screenshot = Some(entry.id),
                        RowAction::ToggleExpand(id) => toggle_expand = Some(id),
                        RowAction::SetFilterTag(tag) => state.bank.filter_tag = tag,
                    }
                    ui.add_space(if is_expanded { 8.0 } else { 4.0 });
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

fn draw_compact_row(ui: &mut egui::Ui, entry: &BankEntryMeta) -> RowAction {
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

    let bg_color = if is_hovered {
        Color32::from_rgb(33, 38, 48)
    } else {
        ROW_BG
    };
    let stroke_color = if is_hovered {
        Color32::from_rgb(99, 102, 241)
    } else {
        ROW_STROKE
    };

    ui.painter().rect(
        rect,
        6.0,
        bg_color,
        egui::Stroke::new(1.0, stroke_color),
    );

    let inner_rect = rect.shrink2(egui::vec2(12.0, 5.0));
    let mut tag_clicked = false;

    ui.allocate_ui_at_rect(inner_rect, |ui| {
        ui.horizontal(|ui| {
            let arrow_color = if is_hovered {
                Color32::from_rgb(148, 163, 184)
            } else {
                COLOR_MUTED
            };
            ui.label(RichText::new("▶").size(11.0).color(arrow_color));
            ui.add_space(4.0);
            ui.label(build_headword_reading_job(
                &entry.term,
                &entry.reading,
                19.0,
                15.0,
                COLOR_TERM,
                COLOR_READING,
            ));

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
                    RichText::new(format_friendly_timestamp(entry.created_at))
                        .font(egui::FontId::proportional(11.5))
                        .color(COLOR_MUTED),
                );
            });
        });
    });

    if row_clicked && !tag_clicked {
        action = RowAction::ToggleExpand(entry.id);
    }

    action
}

fn draw_row(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    state: &mut AppState,
    entry: &BankEntryMeta,
    loads_left: &mut usize,
) -> RowAction {
    let mut action = RowAction::None;

    egui::Frame::none()
        .fill(ROW_BG)
        .stroke(egui::Stroke::new(1.0, ROW_STROKE))
        .rounding(8.0)
        .inner_margin(egui::Margin::same(12.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                if state.bank.compact_mode {
                    let arrow_size = egui::vec2(22.0, 22.0);
                    let (arrow_rect, arrow_resp) = ui.allocate_exact_size(arrow_size, egui::Sense::click());
                    let arrow_hovered = arrow_resp.hovered();
                    if arrow_hovered {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    let arrow_bg = if arrow_hovered {
                        Color32::from_rgba_premultiplied(55, 65, 85, 180)
                    } else {
                        Color32::TRANSPARENT
                    };
                    ui.painter().rect_filled(arrow_rect, 4.0, arrow_bg);
                    let arrow_color = if arrow_hovered {
                        Color32::from_rgb(226, 232, 240)
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
                    if arrow_resp.on_hover_text("Click to collapse entry").clicked() {
                        action = RowAction::ToggleExpand(entry.id);
                    }
                    ui.add_space(4.0);
                }

                // Screenshot preview
                let sense = if entry.has_screenshot {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                };
                let (rect, response) = ui.allocate_exact_size(THUMB_SIZE, sense);
                if ui.is_rect_visible(rect) {
                    let cached = state.bank.thumbnails.contains_key(&entry.id);
                    let texture = if cached || *loads_left > 0 {
                        if !cached {
                            *loads_left -= 1;
                        }
                        state.bank.thumbnail(ctx, entry)
                    } else {
                        None
                    };
                    paint_thumbnail(ui, rect, texture.as_ref(), entry.has_screenshot, cached);
                }
                if entry.has_screenshot {
                    let copying = state.bank.copying_screenshot == Some(entry.id);
                    let copy_id = ui.id().with(("copy_btn", entry.id));
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
                let dict_entry = state.bank.dict_entry(entry);
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
                                    ui.label(build_headword_reading_job(
                                        &entry.term,
                                        &entry.reading,
                                        28.0,
                                        21.0,
                                        COLOR_TERM,
                                        COLOR_READING,
                                    ));
                                }
                            },
                        );
                        let _controls_resp = ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            if state.bank.pending_delete == Some(entry.id) {
                                if ui.button("Cancel").clicked() {
                                    state.bank.pending_delete = None;
                                }
                                if ui
                                    .button(RichText::new("Delete").color(Color32::from_rgb(248, 113, 113)))
                                    .clicked()
                                {
                                    action = RowAction::Delete;
                                }
                            } else {
                                let del_size = egui::vec2(28.0, 28.0);
                                let (del_rect, del_resp) = ui.allocate_exact_size(del_size, egui::Sense::click());
                                let del_hovered = del_resp.hovered();
                                if del_hovered {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                paint_delete_button(ui.painter(), del_rect, del_hovered);
                                if del_resp.on_hover_text("Delete this entry").clicked() {
                                    state.bank.pending_delete = Some(entry.id);
                                }
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new(format_friendly_timestamp(entry.created_at))
                                        .font(egui::FontId::proportional(12.5))
                                        .color(COLOR_MUTED),
                                );
                            }
                        });
                    });

                    // Tag (click to edit)
                    ui.add_space(2.0);
                    draw_tag_row(ui, state, entry);

                    // Sentence, with the mined word highlighted
                    ui.add_space(4.0);
                    ui.label(sentence_job(entry, ui.available_width()));

                    // Dictionary entry senses, as shown in the popup (without examples)
                    ui.add_space(6.0);
                    match &dict_entry {
                        Some(de) => with_dict_scale(DICT_SCALE, || {
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 3.0);
                            render_glossaries(ui, de);
                        }),
                        None if !entry.definition_text.is_empty() => {
                            ui.label(
                                RichText::new(&entry.definition_text)
                                    .font(dict_font(17.0))
                                    .color(COLOR_DEFINITION),
                            );
                        }
                        None => {}
                    }
                });
            });
        });

    action
}

/// Draws the entry's tag chip with an integrated sleek remove button, or the inline tag editor if this entry is being edited.
fn draw_tag_row(
    ui: &mut egui::Ui,
    state: &mut AppState,
    entry: &BankEntryMeta,
) {
    let editing = state.bank.editing_tag.as_ref().map_or(false, |e| e.id == entry.id);
    ui.horizontal(|ui| {
        if editing {
            let bank = &mut state.bank;
            let Some(edit) = bank.editing_tag.as_mut() else { return };
            let request_focus = !edit.focused;
            edit.focused = true;
            ui.label(RichText::new("🏷").color(COLOR_TAG));
            let out = tag_input(
                ui,
                ("bank_tag_edit", entry.id),
                &mut edit.text,
                &bank.known_tags,
                "Tag (leave empty for none)",
                260.0,
                request_focus,
            );
            ui.label(RichText::new("Enter to save · Esc to cancel").small().color(COLOR_MUTED));
            if out.cancelled {
                state.bank.editing_tag = None;
            } else if out.committed {
                let text = edit.text.clone();
                state.bank.editing_tag = None;
                if let Err(e) = state.bank.set_tag(entry.id, &text) {
                    state.error(format!("Failed to update tag: {e}"));
                }
            }
            return;
        }

        let start_edit = |state: &mut AppState, text: String| {
            state.bank.editing_tag = Some(TagEdit { id: entry.id, text, focused: false });
        };
        match &entry.tag {
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

                let (chip_rect, mut response) = ui.allocate_exact_size(
                    egui::vec2(width, height),
                    egui::Sense::click(),
                );

                let remove_rect = egui::Rect::from_min_max(
                    egui::pos2(chip_rect.max.x - (remove_size + padding_right + 3.0), chip_rect.min.y),
                    chip_rect.max,
                );

                let is_hovered = response.hovered();
                let pointer_pos = ui.ctx().pointer_latest_pos();
                let remove_hovered = is_hovered && pointer_pos.map_or(false, |p| remove_rect.contains(p));
                let tag_hovered = is_hovered && !remove_hovered;

                if is_hovered {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }

                let (bg, border) = if tag_hovered {
                    (
                        Color32::from_rgba_premultiplied(65, 75, 98, 200),
                        Color32::from_rgb(148, 163, 184),
                    )
                } else {
                    (COLOR_TAG_BG, COLOR_TAG_STROKE)
                };

                // Draw base pill
                ui.painter().rect(
                    chip_rect,
                    5.0,
                    bg,
                    egui::Stroke::new(1.0, border),
                );

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
                    (
                        Color32::WHITE,
                        Color32::from_rgb(220, 38, 38),
                    )
                } else {
                    (
                        Color32::from_rgba_premultiplied(165, 175, 195, 200),
                        Color32::TRANSPARENT,
                    )
                };

                if x_bg != Color32::TRANSPARENT {
                    ui.painter().circle_filled(remove_center, 8.5, x_bg);
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
                        if let Err(e) = state.bank.set_tag(entry.id, "") {
                            state.error(format!("Failed to remove tag: {e}"));
                        }
                    } else {
                        start_edit(state, tag.clone());
                    }
                }
            }
            None => {
                let add_btn = egui::Button::new(
                    RichText::new("🏷 Add tag")
                        .font(egui::FontId::proportional(12.0))
                        .color(COLOR_MUTED),
                )
                .fill(Color32::from_rgba_premultiplied(35, 42, 55, 120))
                .stroke(egui::Stroke::new(1.0, Color32::from_rgba_premultiplied(70, 80, 100, 100)))
                .rounding(5.0);

                if ui.add(add_btn).on_hover_text("Add a tag to this word").clicked() {
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
        if !bank.current_tag.is_empty()
            && render_close_button(ui, "Clear the tag").clicked()
        {
            bank.current_tag.clear();
            committed = true;
        }
    });

    if !committed {
        return false;
    }
    let bank = &mut state.bank;
    bank.current_tag = tags::canonicalize_tag(&bank.current_tag, &bank.known_tags).unwrap_or_default();
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
    painter.rect_filled(rect, 6.0, Color32::from_rgb(12, 14, 18));
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
            let text = if has_screenshot && !loaded { "Loading…" } else { "No screenshot" };
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

fn paint_copy_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    hovered: bool,
    copying: bool,
) {
    let (bg, stroke_color, icon_color) = if copying {
        (
            Color32::from_rgba_premultiplied(30, 41, 59, 230),
            Color32::from_rgb(148, 163, 184),
            Color32::from_rgb(148, 163, 184),
        )
    } else if hovered {
        (
            Color32::from_rgba_premultiplied(51, 65, 85, 240),
            Color32::from_rgb(226, 232, 240),
            Color32::WHITE,
        )
    } else {
        (
            Color32::from_rgba_premultiplied(15, 23, 42, 200),
            Color32::from_rgba_premultiplied(100, 116, 139, 180),
            Color32::from_rgb(203, 213, 225),
        )
    };

    painter.rect_filled(rect, 5.0, bg);
    painter.rect_stroke(rect, 5.0, egui::Stroke::new(1.0, stroke_color));

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
        let back = egui::Rect::from_min_size(center + egui::vec2(-2.0, -7.0), egui::vec2(10.0, 12.0));
        painter.rect_stroke(back, 1.5, stroke);

        // Front sheet (bottom-left) - filled with bg to occlude overlapping back sheet
        let front = egui::Rect::from_min_size(center + egui::vec2(-7.0, -3.0), egui::vec2(10.0, 12.0));
        painter.rect_filled(front, 1.5, bg);
        painter.rect_stroke(front, 1.5, stroke);

        // Lines on front sheet to clearly represent a document
        let line_stroke = egui::Stroke::new(1.0, icon_color);
        painter.line_segment(
            [center + egui::vec2(-4.5, 0.5), center + egui::vec2(0.5, 0.5)],
            line_stroke,
        );
        painter.line_segment(
            [center + egui::vec2(-4.5, 3.5), center + egui::vec2(0.5, 3.5)],
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

fn sentence_job(entry: &BankEntryMeta, wrap_width: f32) -> LayoutJob {
    let chars: Vec<char> = entry.sentence.chars().collect();
    let start = entry.word_range.0.min(chars.len());
    let end = entry.word_range.1.clamp(start, chars.len());
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
    for (range, format) in [(0..start, &normal), (start..end, &highlight), (end..chars.len(), &normal)] {
        if !range.is_empty() {
            let text: String = chars[range].iter().collect();
            job.append(&text, 0.0, format.clone());
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
        (
            Color32::from_rgba_premultiplied(70, 80, 105, 200),
            Color32::from_rgb(148, 163, 184),
        )
    } else {
        (COLOR_TAG_BG, COLOR_TAG_STROKE)
    };
    ui.painter().rect(
        chip_rect,
        4.0,
        bg,
        egui::Stroke::new(1.0, border),
    );
    ui.painter().galley(chip_rect.min + padding, galley, if hovered { COLOR_TERM } else { COLOR_TAG });
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
        (
            Color32::from_rgb(220, 38, 38),
            Color32::WHITE,
            Color32::from_rgb(239, 68, 68),
        )
    } else {
        (
            Color32::from_rgba_premultiplied(45, 52, 68, 140),
            Color32::from_rgb(148, 163, 184),
            Color32::from_rgba_premultiplied(75, 85, 105, 120),
        )
    };
    ui.painter().rect_filled(rect, 4.0, bg);
    ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(1.0, border_color));
    let center = rect.center();
    let arm = 3.5;
    let stroke = egui::Stroke::new(1.4, stroke_color);
    ui.painter().line_segment([center + egui::vec2(-arm, -arm), center + egui::vec2(arm, arm)], stroke);
    ui.painter().line_segment([center + egui::vec2(-arm, arm), center + egui::vec2(arm, -arm)], stroke);
    resp.on_hover_text(hover_text)
}

fn format_friendly_timestamp(ms: i64) -> String {
    let secs = (ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm).is_null() } {
        return String::new();
    }
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    let mut now_tm: libc::tm = unsafe { std::mem::zeroed() };
    let is_today = unsafe { !libc::localtime_r(&now_secs, &mut now_tm).is_null() }
        && now_tm.tm_year == tm.tm_year
        && now_tm.tm_yday == tm.tm_yday;
    let is_yesterday = unsafe { !libc::localtime_r(&now_secs, &mut now_tm).is_null() }
        && now_tm.tm_year == tm.tm_year
        && now_tm.tm_yday == tm.tm_yday + 1;

    let hour = tm.tm_hour;
    let min = tm.tm_min;
    if is_today {
        format!("Today {:02}:{:02}", hour, min)
    } else if is_yesterday {
        format!("Yesterday {:02}:{:02}", hour, min)
    } else {
        crate::bank::format_timestamp(ms)
    }
}

fn paint_delete_button(painter: &egui::Painter, rect: egui::Rect, hovered: bool) {
    let (bg, stroke_color, icon_color) = if hovered {
        (
            Color32::from_rgba_premultiplied(65, 25, 25, 220),
            Color32::from_rgb(239, 68, 68),
            Color32::from_rgb(248, 113, 113),
        )
    } else {
        (
            Color32::from_rgba_premultiplied(30, 35, 45, 180),
            Color32::from_rgba_premultiplied(80, 90, 110, 160),
            Color32::from_rgb(148, 163, 184),
        )
    };

    painter.rect_filled(rect, 5.0, bg);
    painter.rect_stroke(rect, 5.0, egui::Stroke::new(1.0, stroke_color));

    let center = rect.center();
    let stroke = egui::Stroke::new(1.4, icon_color);

    // Lid top handle
    let handle = egui::Rect::from_min_max(center + egui::vec2(-2.5, -7.0), center + egui::vec2(2.5, -5.5));
    painter.rect_stroke(handle, 0.5, stroke);

    // Lid horizontal bar
    painter.line_segment(
        [center + egui::vec2(-6.5, -5.0), center + egui::vec2(6.5, -5.0)],
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
        [center + egui::vec2(-1.5, -1.5), center + egui::vec2(-1.5, 4.0)],
        inner_stroke,
    );
    painter.line_segment(
        [center + egui::vec2(1.5, -1.5), center + egui::vec2(1.5, 4.0)],
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

        let job = build_headword_reading_job("暗い", "くらい", 19.0, 15.0, COLOR_TERM, COLOR_READING);
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
        };

        egui::CentralPanel::default().show(&ctx, |ui| {
            draw_tag_row(ui, &mut state, &entry);
        });
        assert!(state.bank.editing_tag.is_none());
    }
}
