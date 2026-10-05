//! Standalone "Mining Bank" window (toggled with B): lists mined words, most recent first.

use crate::app::AppState;
use crate::bank::{tags, BankEntryMeta, TagEdit};
use crate::dict::render::{dict_font, render_entry_header, render_glossaries, with_dict_scale};
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
const COLOR_TAG: Color32 = Color32::from_rgb(196, 181, 253);
const COLOR_TAG_BG: Color32 = Color32::from_rgb(46, 38, 74);

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
                if !bank.filter_tag.is_empty() && ui.button("✖").on_hover_text("Clear filter").clicked() {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowAction {
    None,
    Enlarge,
    Delete,
    CopyScreenshot,
    ToggleExpand(i64),
}

fn draw_compact_row(ui: &mut egui::Ui, entry: &BankEntryMeta) -> RowAction {
    let mut action = RowAction::None;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 38.0),
        egui::Sense::click(),
    );
    let is_hovered = response.hovered();
    if is_hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if response.on_hover_text("Click to expand entry").clicked() {
        action = RowAction::ToggleExpand(entry.id);
    }

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

    ui.allocate_ui_at_rect(rect.shrink2(egui::vec2(12.0, 0.0)), |ui| {
        ui.with_layout(
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let arrow_color = if is_hovered {
                    Color32::from_rgb(148, 163, 184)
                } else {
                    COLOR_MUTED
                };
                ui.label(RichText::new("▶").size(11.0).color(arrow_color));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(&entry.term)
                        .font(dict_font(20.0))
                        .strong()
                        .color(COLOR_TERM),
                );
                if !entry.reading.is_empty() && entry.reading != entry.term {
                    ui.label(
                        RichText::new(format!("【{}】", entry.reading))
                            .font(dict_font(16.0))
                            .color(COLOR_READING),
                    );
                }
                if let Some(tag) = &entry.tag {
                    ui.add_space(8.0);
                    let text = format!("🏷 {tag}");
                    let font_id = egui::FontId::proportional(12.0);
                    let galley = ui.painter().layout_no_wrap(text, font_id, COLOR_TAG);
                    let chip_padding = egui::vec2(7.0, 3.0);
                    let chip_size = galley.size() + chip_padding * 2.0;
                    let (chip_rect, _) = ui.allocate_exact_size(chip_size, egui::Sense::hover());
                    ui.painter().rect(
                        chip_rect,
                        6.0,
                        COLOR_TAG_BG,
                        egui::Stroke::new(1.0, Color32::from_rgb(88, 70, 130)),
                    );
                    ui.painter().galley(chip_rect.min + chip_padding, galley, COLOR_TAG);
                }
            },
        );
    });
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
    let mut interactive_rects: Vec<egui::Rect> = Vec::new();
    let mut delete_interacted = false;
    let mut tag_interacted = false;

    let frame_resp = egui::Frame::none()
        .fill(ROW_BG)
        .stroke(egui::Stroke::new(1.0, ROW_STROKE))
        .rounding(8.0)
        .inner_margin(egui::Margin::same(12.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                if state.bank.compact_mode {
                    ui.label(RichText::new("▼").size(11.0).color(COLOR_MUTED));
                    ui.add_space(4.0);
                }

                // Screenshot preview
                let sense = if entry.has_screenshot {
                    egui::Sense::click()
                } else {
                    egui::Sense::hover()
                };
                let (rect, response) = ui.allocate_exact_size(THUMB_SIZE, sense);
                if entry.has_screenshot {
                    interactive_rects.push(rect);
                }
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
                        let controls_width = 170.0;
                        let header_width = (ui.available_width() - controls_width).max(120.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(header_width, 0.0),
                            egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                            |ui| match &dict_entry {
                                Some(de) => with_dict_scale(DICT_SCALE, || {
                                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                                    render_entry_header(ui, de, false);
                                }),
                                None => {
                                    ui.label(
                                        RichText::new(&entry.term)
                                            .font(dict_font(28.0))
                                            .strong()
                                            .color(COLOR_TERM),
                                    );
                                    if !entry.reading.is_empty() && entry.reading != entry.term {
                                        ui.label(
                                            RichText::new(format!("【{}】", entry.reading))
                                                .font(dict_font(21.0))
                                                .color(COLOR_READING),
                                        );
                                    }
                                }
                            },
                        );
                        let controls_resp = ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            if state.bank.pending_delete == Some(entry.id) {
                                if ui.button("Cancel").clicked() {
                                    state.bank.pending_delete = None;
                                    delete_interacted = true;
                                }
                                if ui
                                    .button(RichText::new("Delete").color(Color32::from_rgb(248, 113, 113)))
                                    .clicked()
                                {
                                    action = RowAction::Delete;
                                    delete_interacted = true;
                                }
                            } else {
                                if ui.button("🗑").on_hover_text("Delete this entry").clicked() {
                                    state.bank.pending_delete = Some(entry.id);
                                    delete_interacted = true;
                                }
                                ui.label(
                                    RichText::new(crate::bank::format_timestamp(entry.created_at))
                                        .small()
                                        .color(COLOR_MUTED),
                                );
                            }
                        });
                        interactive_rects.push(controls_resp.response.rect);
                    });

                    // Tag (click to edit)
                    ui.add_space(2.0);
                    let (t_interacted, tag_rect) = draw_tag_row(ui, state, entry);
                    tag_interacted = t_interacted;
                    interactive_rects.push(tag_rect);

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

    if state.bank.compact_mode {
        let card_rect = frame_resp.response.rect;
        let mut card_resp = ui.interact(
            card_rect,
            ui.id().with(("compact_card_click", entry.id)),
            egui::Sense::click(),
        );
        let pointer_pos = ui.ctx().pointer_latest_pos();
        let over_interactive = pointer_pos.map_or(false, |p| {
            interactive_rects.iter().any(|r| r.contains(p))
        });
        if card_resp.hovered() && !over_interactive {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            card_resp = card_resp.on_hover_text("Click to collapse entry");
        }
        if card_resp.clicked() {
            let click_pos = ui.ctx().input(|i| i.pointer.interact_pos());
            let clicked_interactive = click_pos.map_or(false, |p| {
                interactive_rects.iter().any(|r| r.contains(p))
            });
            if !clicked_interactive && action == RowAction::None && !delete_interacted && !tag_interacted {
                action = RowAction::ToggleExpand(entry.id);
            }
        }
    }

    action
}

/// Draws the entry's tag chip, or the inline tag editor if this entry is being edited.
fn draw_tag_row(
    ui: &mut egui::Ui,
    state: &mut AppState,
    entry: &BankEntryMeta,
) -> (bool, egui::Rect) {
    let mut interacted = false;
    let editing = state.bank.editing_tag.as_ref().map_or(false, |e| e.id == entry.id);
    let resp = ui.horizontal(|ui| {
        if editing {
            interacted = true;
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
                let chip = egui::Button::new(RichText::new(format!("🏷 {tag}")).color(COLOR_TAG))
                    .fill(COLOR_TAG_BG)
                    .rounding(10.0);
                if ui.add(chip).on_hover_text("Click to edit the tag").clicked() {
                    start_edit(state, tag.clone());
                    interacted = true;
                }
                if ui.small_button("✖").on_hover_text("Remove the tag").clicked() {
                    if let Err(e) = state.bank.set_tag(entry.id, "") {
                        state.error(format!("Failed to remove tag: {e}"));
                    }
                    interacted = true;
                }
            }
            None => {
                let add = egui::Button::new(RichText::new("🏷 Add tag").small().color(COLOR_MUTED))
                    .frame(false);
                if ui.add(add).on_hover_text("Add a tag to this word").clicked() {
                    start_edit(state, String::new());
                    interacted = true;
                }
            }
        }
    });
    (interacted, resp.response.rect)
}

/// Settings-panel row for the tag applied to newly mined words (shown above "Appearance").
/// Returns true if the setting changed.
pub fn draw_tag_setting(ui: &mut egui::Ui, state: &mut AppState) -> bool {
    ui.separator();
    let mut committed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Mining Tag:").strong());
        let bank = &mut state.bank;
        let out = tag_input(
            ui,
            "settings_mining_tag",
            &mut bank.current_tag,
            &bank.known_tags,
            "e.g. Final Fantasy 7",
            260.0,
            false,
        );
        if out.cancelled {
            bank.current_tag = bank.saved_current_tag.clone();
        }
        committed = out.committed;
        if !bank.current_tag.is_empty()
            && ui.small_button("✖").on_hover_text("Clear the tag").clicked()
        {
            bank.current_tag.clear();
            committed = true;
        }
    });
    ui.label(
        RichText::new("Newly mined words are tagged with this (e.g. the game being played). Leave empty for no tag.")
            .weak(),
    );

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
