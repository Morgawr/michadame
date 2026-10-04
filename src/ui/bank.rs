//! Standalone "Mining Bank" window (toggled with B): lists mined words, most recent first.

use crate::app::AppState;
use crate::bank::BankEntryMeta;
use crate::dict::render::{dict_font, render_entry_header, render_glossaries, with_dict_scale};
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
            let n = state.bank.entries.len();
            ui.label(
                RichText::new(format!("{n} word{}", if n == 1 { "" } else { "s" }))
                    .color(COLOR_MUTED),
            );
        });
        ui.add_space(6.0);
    });

    let mut enlarge: Option<i64> = None;
    let mut delete: Option<i64> = None;

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

        let mut loads_left = THUMB_LOADS_PER_FRAME;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for idx in 0..state.bank.entries.len() {
                    let entry = state.bank.entries[idx].clone();
                    match draw_row(ui, ctx, state, &entry, &mut loads_left) {
                        RowAction::None => {}
                        RowAction::Enlarge => enlarge = Some(entry.id),
                        RowAction::Delete => delete = Some(entry.id),
                    }
                    ui.add_space(8.0);
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

    draw_enlarged(state, ctx);
}

enum RowAction {
    None,
    Enlarge,
    Delete,
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
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ZoomIn);
                    }
                    if response.on_hover_text("Click to enlarge").clicked() {
                        action = RowAction::Enlarge;
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
                                    render_entry_header(ui, de);
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
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
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
                                if ui.button("🗑").on_hover_text("Delete this entry").clicked() {
                                    state.bank.pending_delete = Some(entry.id);
                                }
                                ui.label(
                                    RichText::new(crate::bank::format_timestamp(entry.created_at))
                                        .small()
                                        .color(COLOR_MUTED),
                                );
                            }
                        });
                    });

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
