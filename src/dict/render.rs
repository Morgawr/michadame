use super::models::{GlossaryEntry, TermEntry};
use eframe::egui::{self, text::LayoutJob, Color32, FontId, RichText, TextFormat};

pub const FONT_DICT_FAMILY: &str = "GothicCJK";

thread_local! {
    /// Scale applied to all dictionary fonts. The popup renders at 1.0 (it is drawn large
    /// over the video); other views (e.g. the mining bank) render the same content smaller.
    static DICT_SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

/// Current dictionary font scale.
#[inline]
pub fn dict_scale() -> f32 {
    DICT_SCALE.with(|s| s.get())
}

/// Runs `f` with all dictionary rendering scaled by `scale`.
pub fn with_dict_scale<R>(scale: f32, f: impl FnOnce() -> R) -> R {
    let previous = DICT_SCALE.with(|s| s.replace(scale));
    let result = f();
    DICT_SCALE.with(|s| s.set(previous));
    result
}

#[inline]
pub fn dict_font(size: f32) -> FontId {
    FontId::new(size * dict_scale(), egui::FontFamily::Name(FONT_DICT_FAMILY.into()))
}

/// Dark-theme palette constants
const COLOR_HEADWORD: Color32 = Color32::from_rgb(248, 250, 252);
const COLOR_READING: Color32 = Color32::from_rgb(56, 189, 248); // sky-400
const COLOR_DEINFLECT_BG: Color32 = Color32::from_rgb(59, 45, 84);
const COLOR_DEINFLECT_TEXT: Color32 = Color32::from_rgb(216, 180, 254);
const COLOR_TAG_BG: Color32 = Color32::from_rgb(30, 41, 59);
const COLOR_TAG_BORDER: Color32 = Color32::from_rgb(51, 65, 85);
const COLOR_TAG_TEXT: Color32 = Color32::from_rgb(148, 163, 184);
const COLOR_DEFINITION: Color32 = Color32::from_rgb(226, 232, 240);
const COLOR_EXAMPLE_JA: Color32 = Color32::from_rgb(241, 245, 249);
const COLOR_EXAMPLE_EN: Color32 = Color32::from_rgb(203, 213, 225);
const COLOR_EXAMPLE_BG: Color32 = Color32::from_rgba_premultiplied(30, 41, 59, 70);
const COLOR_RUBY: Color32 = Color32::from_rgb(125, 211, 252);
const COLOR_MUTED: Color32 = Color32::from_rgb(100, 116, 139);
const COLOR_FREQ_BG: Color32 = Color32::from_rgb(19, 78, 74);
const COLOR_FREQ_BORDER: Color32 = Color32::from_rgb(20, 184, 166);
const COLOR_FREQ_TEXT: Color32 = Color32::from_rgb(94, 234, 212);
const COLOR_MINED_BG: Color32 = Color32::from_rgb(67, 26, 7);
const COLOR_MINED_BORDER: Color32 = Color32::from_rgb(249, 115, 22);
const COLOR_MINED_TEXT: Color32 = Color32::from_rgb(254, 215, 170);

/// Circled numbers for sense indices ①..⑳
const CIRCLED_NUMBERS: &[&str] = &[
    "①", "②", "③", "④", "⑤", "⑥", "⑦", "⑧", "⑨", "⑩",
    "⑪", "⑫", "⑬", "⑭", "⑮", "⑯", "⑰", "⑱", "⑲", "⑳",
];

/// Renders a single dictionary entry inside the popup scroll area.
/// Renders a dictionary entry. If `mine_status` is given, a mining button (`+`) is drawn at
/// the right of the header row. Returns true if the mining button was clicked.
pub fn render_term_entry(
    ui: &mut egui::Ui,
    entry: &TermEntry,
    index: usize,
    mine_status: Option<crate::bank::MineStatus>,
    is_mined: bool,
) -> bool {
    if index > 0 {
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);
    }

    let mut mine_clicked = false;

    // 1. Entry Header
    ui.horizontal(|ui| {
        // Reserve room on the right for the mining button.
        let button_size = 56.0;
        let header_width = if mine_status.is_some() {
            (ui.available_width() - button_size - 12.0).max(100.0)
        } else {
            ui.available_width()
        };
        ui.allocate_ui_with_layout(
            egui::vec2(header_width, 0.0),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| render_entry_header(ui, entry, is_mined),
        );
        if let Some(status) = mine_status {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                mine_clicked = render_mine_button(ui, status, button_size);
            });
        }
    });

    ui.add_space(6.0);
    render_glossaries(ui, entry);
    mine_clicked
}

/// Round `+` / `…` / `✔` button used to add an entry to the mining bank.
fn render_mine_button(ui: &mut egui::Ui, status: crate::bank::MineStatus, size: f32) -> bool {
    use crate::bank::MineStatus;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let hovered = response.hovered() && status == MineStatus::Available;
    let (fill, stroke, glyph, glyph_color) = match status {
        MineStatus::Available => (
            if hovered { Color32::from_rgb(14, 116, 144) } else { COLOR_TAG_BG },
            COLOR_READING,
            "+",
            COLOR_HEADWORD,
        ),
        MineStatus::Pending => (COLOR_TAG_BG, COLOR_TAG_BORDER, "…", COLOR_TAG_TEXT),
        MineStatus::Mined => (COLOR_FREQ_BG, COLOR_FREQ_BORDER, "✔", COLOR_FREQ_TEXT),
    };
    ui.painter()
        .circle(rect.center(), size / 2.0 - 1.0, fill, egui::Stroke::new(1.5, stroke));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(size * 0.6),
        glyph_color,
    );
    let response = match status {
        MineStatus::Available => {
            if hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            response.on_hover_text("Add to mining bank (B)")
        }
        MineStatus::Pending => response.on_hover_text("Saving…"),
        MineStatus::Mined => response.on_hover_text("Already in the mining bank"),
    };
    status == MineStatus::Available && response.clicked()
}

/// Combines a headword and its bracketed kana reading into a single `LayoutJob`,
/// ensuring typographic baseline alignment across differing font sizes.
pub fn build_headword_reading_job(
    term: &str,
    reading: &str,
    term_size: f32,
    reading_size: f32,
    term_color: Color32,
    reading_color: Color32,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        term,
        0.0,
        TextFormat {
            font_id: dict_font(term_size),
            color: term_color,
            ..Default::default()
        },
    );
    if !reading.is_empty() && reading != term {
        let space = (term_size * 0.16).max(5.0) * dict_scale();
        job.append(
            &format!("【{}】", reading),
            space,
            TextFormat {
                font_id: dict_font(reading_size),
                color: reading_color,
                ..Default::default()
            },
        );
    }
    job
}

/// Renders the headword, reading, frequency, deinflection and tag badges of an entry.
pub fn render_entry_header(ui: &mut egui::Ui, entry: &TermEntry, is_mined: bool) {
    let scale = dict_scale();

    // Tier 1: Primary lexical info (Headword + Kana reading + Mined badge)
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0 * scale, 4.0 * scale);

        // Headword + Kana reading combined in a single LayoutJob for baseline alignment
        let job = build_headword_reading_job(
            &entry.term,
            &entry.reading,
            48.0,
            34.0,
            COLOR_HEADWORD,
            COLOR_READING,
        );
        ui.label(job);

        // Mined badge / icon if already in the mining bank
        if is_mined {
            render_pill_badge(
                ui,
                "✔ Mined",
                COLOR_MINED_BG,
                COLOR_MINED_TEXT,
                COLOR_MINED_BORDER,
            )
            .on_hover_text("Already in the mining bank");
        }
    });

    // Tier 2: Subordinate metadata row (frequency, deinflection trail, part of speech / definition tags)
    let has_freq = entry.frequency.is_some();
    let has_deinflect = !entry.inflection_reasons.is_empty();
    let tags_to_show: Vec<&str> = entry
        .definition_tags
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .filter(|t| !t.is_empty() && *t != "★" && *t != "form" && *t != "P")
        .collect();
    let has_tags = !tags_to_show.is_empty();

    if has_freq || has_deinflect || has_tags {
        ui.add_space(3.0 * scale);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0 * scale, 4.0 * scale);

            // Frequency rank badge
            if let Some(freq) = &entry.frequency {
                render_pill_badge(
                    ui,
                    &freq.display_text(),
                    COLOR_FREQ_BG,
                    COLOR_FREQ_TEXT,
                    COLOR_FREQ_BORDER,
                );
            }

            // Deinflection trail badge
            if has_deinflect {
                let trail = entry.inflection_reasons.join(" ← ");
                render_pill_badge(
                    ui,
                    &format!("← {trail}"),
                    COLOR_DEINFLECT_BG,
                    COLOR_DEINFLECT_TEXT,
                    Color32::from_rgba_premultiplied(147, 51, 234, 80),
                );
            }

            // Part-of-speech & definition tags (excluding noise tags like ★ and form)
            for tag in tags_to_show {
                render_pill_badge(
                    ui,
                    tag,
                    COLOR_TAG_BG,
                    COLOR_TAG_TEXT,
                    COLOR_TAG_BORDER,
                );
            }
        });
    }
}

/// Renders all glossaries / senses of an entry.
pub fn render_glossaries(ui: &mut egui::Ui, entry: &TermEntry) {
    for (g_idx, item) in entry.glossary.iter().enumerate() {
        match item {
            GlossaryEntry::Text(text) => {
                ui.horizontal_wrapped(|ui| {
                    if entry.glossary.len() > 1 {
                        ui.label(
                            RichText::new(format!("{}.", g_idx + 1))
                                .font(dict_font(34.0))
                                .strong()
                                .color(COLOR_READING),
                        );
                    }
                    render_plain_html_text(ui, text);
                });
                ui.add_space(4.0);
            }
            GlossaryEntry::Structured(val) => {
                if let Some((term, reasons)) = as_deinflection_glossary(val) {
                    // Yomitan "deinflection" glossary item `[term, [reasons...]]`, e.g.
                    // Jitendex redirects: ["熱い", ["redirected from あっつい"]].
                    let text = if reasons.is_empty() {
                        term.to_string()
                    } else {
                        format!("{term}  ({})", reasons.join(", "))
                    };
                    ui.label(RichText::new(text).font(dict_font(28.0)).color(COLOR_MUTED));
                    ui.add_space(4.0);
                } else {
                    render_structured_entry(ui, val);
                }
            }
        }
    }
}

/// Recognizes Yomitan's deinflection glossary item: `[uninflected_term, [reason, ...]]`.
fn as_deinflection_glossary(val: &serde_json::Value) -> Option<(&str, Vec<&str>)> {
    let arr = val.as_array()?;
    let [term, reasons] = arr.as_slice() else {
        return None;
    };
    let term = term.as_str()?;
    let reasons = reasons
        .as_array()?
        .iter()
        .map(|r| r.as_str())
        .collect::<Option<Vec<_>>>()?;
    Some((term, reasons))
}

/// Renders a small stylish pill badge (e.g. for tags or deinflections).
pub fn render_pill_badge(
    ui: &mut egui::Ui,
    text: &str,
    bg: Color32,
    fg: Color32,
    border: Color32,
) -> egui::Response {
    let scale = dict_scale();
    let padding = egui::vec2(10.0, 4.0) * scale;
    let font_id = dict_font(23.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font_id, fg);

    let desired_size = galley.size() + padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

    ui.painter().rect(
        rect,
        5.0 * scale,
        bg,
        egui::Stroke::new(1.0, border),
    );

    let text_pos = rect.min + padding;
    ui.painter().galley(text_pos, galley, fg);
    response
}

/// Top-level dispatcher for Yomitan / Jitendex structured content.
pub fn render_structured_entry(ui: &mut egui::Ui, val: &serde_json::Value) {
    if let serde_json::Value::Array(arr) = val {
        for item in arr {
            render_structured_entry(ui, item);
        }
        return;
    }

    let Some(obj) = val.as_object() else {
        if let Some(s) = val.as_str() {
            render_plain_html_text(ui, s);
        }
        return;
    };

    let content_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if content_type == "structured-content" {
        if let Some(content) = obj.get("content") {
            render_structured_entry(ui, content);
        }
        return;
    }

    let data_content = obj
        .get("data")
        .and_then(|d| d.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");

    match data_content {
        "sense-groups" => {
            if let Some(content) = obj.get("content") {
                render_sense_groups(ui, content);
            }
        }
        "sense-group" => {
            render_single_sense_group(ui, val);
        }
        "forms" => {
            render_forms(ui, val);
        }
        "attribution" => {
            render_attribution(ui, val);
        }
        _ => {
            render_generic_node(ui, val, 0);
        }
    }
}

/// Renders a group of senses and forms.
fn render_sense_groups(ui: &mut egui::Ui, content: &serde_json::Value) {
    match content {
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(obj) = item.as_object() {
                    let dc = obj
                        .get("data")
                        .and_then(|d| d.get("content"))
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    match dc {
                        "sense-group" => render_single_sense_group(ui, item),
                        "forms" => render_forms(ui, item),
                        _ => render_generic_node(ui, item, 0),
                    }
                } else {
                    render_generic_node(ui, item, 0);
                }
            }
        }
        serde_json::Value::Object(_) => {
            render_single_sense_group(ui, content);
        }
        _ => {}
    }
}

/// Renders a single sense group (POS tags followed by numbered senses).
fn render_single_sense_group(ui: &mut egui::Ui, group: &serde_json::Value) {
    // 1. Part-of-speech & misc pills
    let mut tags = Vec::new();
    collect_tags(group, &mut tags);
    if !tags.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for tag in &tags {
                render_pill_badge(ui, tag, COLOR_TAG_BG, COLOR_TAG_TEXT, COLOR_TAG_BORDER);
            }
        });
        ui.add_space(4.0);
    }

    // 2. Find and render all numbered senses
    let mut senses = Vec::new();
    collect_nodes_by_data_content(group, "sense", &mut senses);

    for (s_idx, sense_val) in senses.iter().enumerate() {
        render_sense(ui, sense_val, s_idx);
    }
}

/// Renders an individual numbered dictionary sense.
fn render_sense(ui: &mut egui::Ui, sense_val: &serde_json::Value, index: usize) {
    let sense = sense_val.as_object();

    // A. Sense number (e.g. ①, ②, ③)
    let list_style = sense
        .and_then(|s| s.get("style"))
        .and_then(|s| s.get("listStyleType"))
        .and_then(|t| t.as_str())
        .map(|s| s.trim_matches('"'))
        .unwrap_or("");

    let sense_num = if !list_style.is_empty() && list_style != "none" {
        list_style.to_string()
    } else if index < CIRCLED_NUMBERS.len() {
        CIRCLED_NUMBERS[index].to_string()
    } else {
        format!("{}.", index + 1)
    };

    // B. Definition synonyms from "glossary" ul/li
    let mut defs = Vec::new();
    if let Some(glossary_node) = find_node_by_data_content(sense_val, "glossary") {
        collect_definitions(glossary_node, &mut defs);
    }

    let def_str = if !defs.is_empty() {
        defs.join("; ")
    } else {
        extract_plain_text_excluding_extras(sense_val)
    };

    // Render definition line
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(&sense_num)
                .font(dict_font(36.0))
                .strong()
                .color(COLOR_READING),
        );
        ui.label(
            RichText::new(&def_str)
                .font(dict_font(34.0))
                .color(COLOR_DEFINITION),
        );
    });

    // C. Sense notes / info-gloss
    let mut notes = Vec::new();
    collect_nodes_by_data_content(sense_val, "sense-note", &mut notes);
    collect_nodes_by_data_content(sense_val, "info-gloss", &mut notes);

    for note in notes {
        let label = find_node_by_data_content(note, "sense-note-label")
            .or_else(|| find_node_by_data_content(note, "info-gloss-label"))
            .map(|n| extract_plain_text_from_node(Some(n)))
            .unwrap_or_else(|| "Note".to_string());

        let content = find_node_by_data_content(note, "sense-note-content")
            .or_else(|| find_node_by_data_content(note, "info-gloss-content"))
            .map(|n| extract_plain_text_from_node(Some(n)))
            .unwrap_or_default();

        let note_text = if !content.is_empty() {
            format!("{label}: {content}")
        } else {
            extract_plain_text_from_node(Some(note))
        };

        if !note_text.is_empty() {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(32.0);
                ui.label(RichText::new("ℹ").font(dict_font(28.0)).color(COLOR_MUTED));
                ui.label(
                    RichText::new(note_text)
                        .font(dict_font(30.0))
                        .color(COLOR_TAG_TEXT),
                );
            });
        }
    }

    // D. Example sentences
    let mut examples = Vec::new();
    collect_nodes_by_data_content(sense_val, "example-sentence", &mut examples);

    for ex in examples {
        let ja_node = find_node_by_data_content(ex, "example-sentence-a");
        let en_text = find_node_by_data_content(ex, "example-sentence-b")
            .map(extract_en_sentence_text)
            .unwrap_or_default();

        ui.add_space(4.0);
        let accent_color = Color32::from_rgb(56, 189, 248); // sky-400 accent
        let frame_resp = egui::Frame::none()
            .fill(COLOR_EXAMPLE_BG)
            .rounding(egui::Rounding { nw: 4.0, sw: 4.0, ne: 4.0, se: 4.0 })
            .stroke(egui::Stroke::NONE)
            .inner_margin(egui::Margin {
                left: 14.0,
                right: 10.0,
                top: 6.0,
                bottom: 6.0,
            })
            .show(ui, |ui| {
                // Japanese sentence with vertical ruby furigana
                if let Some(ja) = ja_node {
                    render_ruby_sentence(ui, ja);
                }
                // English translation below
                if !en_text.is_empty() {
                    ui.add_space(3.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(en_text)
                                .font(dict_font(27.0))
                                .italics()
                                .color(COLOR_EXAMPLE_EN),
                        );
                    });
                }
            });
        let r = frame_resp.response.rect;
        ui.painter().vline(r.min.x + 2.0, r.y_range(), egui::Stroke::new(2.5, accent_color));
    }

    // E. Cross references ("See also")
    let mut xrefs = Vec::new();
    collect_nodes_by_data_content(sense_val, "xref", &mut xrefs);

    for xref in xrefs {
        let target_node = find_node_by_data_content(xref, "xref-content");
        let gloss_text = find_node_by_data_content(xref, "xref-glossary")
            .map(|n| extract_plain_text_from_node(Some(n)))
            .unwrap_or_default();

        ui.add_space(3.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_space(20.0);
            ui.label(RichText::new("🔗").font(dict_font(28.0)).color(COLOR_MUTED));
            if let Some(target) = target_node {
                render_inline_content(ui, target);
            }
            if !gloss_text.is_empty() {
                ui.label(
                    RichText::new(format!("— {gloss_text}"))
                        .font(dict_font(30.0))
                        .color(COLOR_TAG_TEXT),
                );
            }
        });
    }

    ui.add_space(6.0);
}

/// Renders variant forms table/row.
fn render_forms(ui: &mut egui::Ui, forms_val: &serde_json::Value) {
    let mut form_words = Vec::new();
    collect_definitions(forms_val, &mut form_words);

    if !form_words.is_empty() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Forms: ")
                    .font(dict_font(30.0))
                    .strong()
                    .color(COLOR_MUTED),
            );
            ui.label(
                RichText::new(form_words.join(", "))
                    .font(dict_font(32.0))
                    .color(COLOR_HEADWORD),
            );
        });
    }
}

/// Renders attribution footer.
fn render_attribution(ui: &mut egui::Ui, attr_val: &serde_json::Value) {
    let text = extract_plain_text_from_node(Some(attr_val));
    if !text.is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new(text)
                .font(dict_font(24.0))
                .color(COLOR_MUTED),
        );
    }
}

/// Generic fallback renderer for arbitrary HTML/structured nodes.
fn render_generic_node(ui: &mut egui::Ui, node: &serde_json::Value, depth: usize) {
    match node {
        serde_json::Value::String(s) => {
            render_plain_html_text(ui, s);
        }
        serde_json::Value::Array(arr) => {
            for child in arr {
                render_generic_node(ui, child, depth);
            }
        }
        serde_json::Value::Object(map) => {
            let tag = map.get("tag").and_then(|v| v.as_str()).unwrap_or("");
            match tag {
                "ul" | "ol" => {
                    if let Some(content) = map.get("content") {
                        render_generic_node(ui, content, depth + 1);
                    }
                }
                "li" => {
                    let list_symbol = map
                        .get("style")
                        .and_then(|s| s.get("listStyleType"))
                        .and_then(|t| t.as_str())
                        .map(|s| s.trim_matches('"'))
                        .unwrap_or("•");

                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(list_symbol)
                                .font(dict_font(30.0))
                                .color(COLOR_MUTED),
                        );
                        if let Some(content) = map.get("content") {
                            render_inline_content(ui, content);
                        }
                    });
                }
                "ruby" => {
                    render_ruby_inline(ui, map);
                }
                "br" => {
                    ui.add_space(2.0);
                }
                _ => {
                    let Some(content) = map.get("content") else {
                        return;
                    };
                    // Inline-only content (text, links, ruby, spans) must stay on one line,
                    // e.g. a redirect `⟶ <a><ruby>熱<rt>あつ</rt></ruby>い</a>`, instead of
                    // rendering each child as its own widget/line.
                    if !content.is_string() && is_inline_only(content) {
                        if contains_ruby(content) {
                            let mut segments = extract_ruby_segments(content);
                            normalize_glyphs(&mut segments);
                            render_ruby_flow(ui, segments, None);
                        } else {
                            ui.horizontal_wrapped(|ui| render_inline_content(ui, content));
                        }
                    } else {
                        render_generic_node(ui, content, depth);
                    }
                }
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RubySegment {
    Ruby { base: String, rt: String },
    Plain(String),
}

pub fn is_cjk_char(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x309F | // Hiragana
        0x30A0..=0x30FF | // Katakana
        0x4E00..=0x9FFF | // CJK Unified Ideographs
        0x3400..=0x4DBF | // CJK Extension A
        0x3000..=0x303F | // CJK Symbols and Punctuation
        0xFF01..=0xFF0F | // Fullwidth punctuation
        0xFF1A..=0xFF20 |
        0xFF3B..=0xFF40 |
        0xFF5B..=0xFF60
    )
}

pub fn push_plain_text_segments(s: &str, out: &mut Vec<RubySegment>) {
    let mut current_word = String::new();
    for c in s.chars() {
        if is_cjk_char(c) {
            if !current_word.is_empty() {
                out.push(RubySegment::Plain(std::mem::take(&mut current_word)));
            }
            out.push(RubySegment::Plain(c.to_string()));
        } else if c.is_whitespace() {
            if !current_word.is_empty() {
                out.push(RubySegment::Plain(std::mem::take(&mut current_word)));
            }
            out.push(RubySegment::Plain(c.to_string()));
        } else {
            current_word.push(c);
        }
    }
    if !current_word.is_empty() {
        out.push(RubySegment::Plain(current_word));
    }
}

pub fn extract_ruby_segments(node: &serde_json::Value) -> Vec<RubySegment> {
    let mut segments = Vec::new();
    collect_ruby_segments_recursive(node, &mut segments);
    segments
}

fn collect_ruby_segments_recursive(val: &serde_json::Value, out: &mut Vec<RubySegment>) {
    match val {
        serde_json::Value::String(s) => {
            push_plain_text_segments(s, out);
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                collect_ruby_segments_recursive(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            let tag = map.get("tag").and_then(|v| v.as_str()).unwrap_or("");
            if tag == "ruby" {
                let (base, rt) = extract_ruby_components(map);
                if !base.is_empty() {
                    if !rt.is_empty() {
                        out.push(RubySegment::Ruby { base, rt });
                    } else {
                        push_plain_text_segments(&base, out);
                    }
                }
            } else if let Some(content) = map.get("content") {
                collect_ruby_segments_recursive(content, out);
            }
        }
        _ => {}
    }
}

/// Calculates vertical placement for ruby furigana snugly atop base kanji.
/// Returns `(base_y_offset, total_h)`.
pub fn calculate_ruby_metrics(sample_rt: &egui::Galley, sample_base: &egui::Galley) -> (f32, f32) {
    let rt_bottom = sample_rt
        .rows
        .first()
        .and_then(|r| r.glyphs.first())
        .map(|g| g.pos.y + g.uv_rect.offset.y + g.uv_rect.size.y)
        .unwrap_or(sample_rt.size().y * 0.80);

    let (base_top, base_bottom) = sample_base
        .rows
        .first()
        .and_then(|r| r.glyphs.first())
        .map(|g| {
            let top = g.pos.y + g.uv_rect.offset.y;
            let bottom = top + g.uv_rect.size.y;
            (top, bottom)
        })
        .unwrap_or((sample_base.size().y * 0.18, sample_base.size().y * 0.84));

    // Snug 2.0px gap directly between bottom edge of furigana and top edge of base kanji
    let ruby_gap = 2.0;
    let base_y_offset = (rt_bottom + ruby_gap - base_top).max(0.0);
    // Bounding box height: bottom of base glyph + 3.0px baseline breathing room
    let total_h = (base_y_offset + base_bottom + 3.0).ceil();
    (base_y_offset, total_h)
}

/// Renders a single ruby element with furigana placed directly on top of the kanji.
pub fn render_ruby_item(ui: &mut egui::Ui, base: &str, rt: &str) {
    if base.is_empty() {
        return;
    }
    let font_base = dict_font(38.0);
    let font_rt = dict_font(20.0);
    let galley_rt = ui.painter().layout_no_wrap(rt.to_string(), font_rt.clone(), COLOR_RUBY);
    let galley_base = ui.painter().layout_no_wrap(base.to_string(), font_base.clone(), COLOR_EXAMPLE_JA);
    let w = galley_rt.size().x.max(galley_base.size().x);
    if w <= 0.0 {
        return;
    }
    let (base_y_offset, total_h) = calculate_ruby_metrics(&galley_rt, &galley_base);

    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, total_h), egui::Sense::hover());

    let base_top = galley_base
        .rows
        .first()
        .and_then(|r| r.glyphs.first())
        .map(|g| g.pos.y + g.uv_rect.offset.y)
        .unwrap_or(galley_base.size().y * 0.18);
    let rt_glyph_bottom = galley_rt
        .rows
        .first()
        .and_then(|r| r.glyphs.first())
        .map(|g| g.pos.y + g.uv_rect.offset.y + g.uv_rect.size.y)
        .unwrap_or(galley_rt.size().y * 0.80);
    let ruby_gap = 2.0;
    let target_bottom = base_y_offset + base_top - ruby_gap;
    let rt_y = (target_bottom - rt_glyph_bottom).max(0.0);

    let rt_x = rect.min.x + (w - galley_rt.size().x) / 2.0;
    ui.painter().galley(egui::pos2(rt_x, rect.min.y + rt_y), galley_rt, COLOR_RUBY);

    let base_x = rect.min.x + (w - galley_base.size().x) / 2.0;
    ui.painter().galley(egui::pos2(base_x, rect.min.y + base_y_offset), galley_base, COLOR_EXAMPLE_JA);
}

/// Renders a complete Japanese example sentence with furigana stacked neatly on top of the kanji.
pub fn render_ruby_sentence(ui: &mut egui::Ui, node: &serde_json::Value) {
    render_ruby_flow(ui, extract_ruby_segments(node), Some("例  "));
}

/// Renders ruby/plain segments as one wrapped line of text, with furigana on top of the
/// kanji and all text sharing the same baseline. `badge` is an optional muted prefix.
fn render_ruby_flow(ui: &mut egui::Ui, segments: Vec<RubySegment>, badge: Option<&str>) {
    if segments.is_empty() {
        return;
    }

    let font_base = dict_font(38.0);
    let font_rt = dict_font(20.0);

    let sample_rt = ui.painter().layout_no_wrap("あ".to_string(), font_rt.clone(), COLOR_RUBY);
    let sample_base = ui.painter().layout_no_wrap("あ".to_string(), font_base.clone(), COLOR_EXAMPLE_JA);
    let (base_y_offset, total_h) = calculate_ruby_metrics(&sample_rt, &sample_base);
    let base_top = sample_base
        .rows
        .first()
        .and_then(|r| r.glyphs.first())
        .map(|g| g.pos.y + g.uv_rect.offset.y)
        .unwrap_or(sample_base.size().y * 0.18);
    let ruby_gap = 2.0;
    let target_bottom = base_y_offset + base_top - ruby_gap;

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);

        // Render the badge (e.g. example "例 ") aligned at the same base baseline
        if let Some(badge) = badge {
            let badge_galley = ui.painter().layout_no_wrap(badge.to_string(), dict_font(28.0), COLOR_MUTED);
            let badge_y_offset = if let (Some(r_base), Some(r_badge)) = (sample_base.rows.first(), badge_galley.rows.first()) {
                let base_ascent = r_base.glyphs.first().map(|g| g.ascent).unwrap_or(sample_base.size().y * 0.77);
                let badge_ascent = r_badge.glyphs.first().map(|g| g.ascent).unwrap_or(badge_galley.size().y * 0.77);
                base_y_offset + (base_ascent - badge_ascent)
            } else {
                base_y_offset + (sample_base.size().y - badge_galley.size().y) / 2.0
            };

            let (badge_rect, _) = ui.allocate_exact_size(egui::vec2(badge_galley.size().x, total_h), egui::Sense::hover());
            ui.painter().galley(
                egui::pos2(badge_rect.min.x, badge_rect.min.y + badge_y_offset),
                badge_galley,
                COLOR_MUTED,
            );
        }

        for seg in segments {
            match seg {
                RubySegment::Ruby { base, rt } => {
                    let galley_rt = ui.painter().layout_no_wrap(rt, font_rt.clone(), COLOR_RUBY);
                    let galley_base = ui.painter().layout_no_wrap(base, font_base.clone(), COLOR_EXAMPLE_JA);
                    let w = galley_rt.size().x.max(galley_base.size().x);
                    if w <= 0.0 {
                        continue;
                    }

                    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, total_h), egui::Sense::hover());

                    let rt_glyph_bottom = galley_rt
                        .rows
                        .first()
                        .and_then(|r| r.glyphs.first())
                        .map(|g| g.pos.y + g.uv_rect.offset.y + g.uv_rect.size.y)
                        .unwrap_or(galley_rt.size().y * 0.80);
                    let rt_y = (target_bottom - rt_glyph_bottom).max(0.0);

                    let rt_x = rect.min.x + (w - galley_rt.size().x) / 2.0;
                    ui.painter().galley(egui::pos2(rt_x, rect.min.y + rt_y), galley_rt, COLOR_RUBY);

                    let base_x = rect.min.x + (w - galley_base.size().x) / 2.0;
                    ui.painter().galley(egui::pos2(base_x, rect.min.y + base_y_offset), galley_base, COLOR_EXAMPLE_JA);
                }
                RubySegment::Plain(text) => {
                    let galley = ui.painter().layout_no_wrap(text, font_base.clone(), COLOR_EXAMPLE_JA);
                    let w = galley.size().x;
                    if w <= 0.0 {
                        continue;
                    }

                    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, total_h), egui::Sense::hover());

                    ui.painter().galley(egui::pos2(rect.min.x, rect.min.y + base_y_offset), galley, COLOR_EXAMPLE_JA);
                }
            }
        }
    });
}

/// Tags whose content flows inline (on the same line as surrounding text).
fn is_inline_tag(tag: &str) -> bool {
    matches!(
        tag,
        "" | "span" | "a" | "ruby" | "rt" | "rp" | "b" | "strong" | "i" | "em" | "sup" | "sub" | "small"
    )
}

/// True if `node` consists only of text and inline elements (no blocks like div/ul/li/br).
fn is_inline_only(node: &serde_json::Value) -> bool {
    match node {
        serde_json::Value::String(_) => true,
        serde_json::Value::Array(arr) => arr.iter().all(is_inline_only),
        serde_json::Value::Object(map) => {
            let tag = map.get("tag").and_then(|v| v.as_str()).unwrap_or("");
            is_inline_tag(tag) && map.get("content").map_or(true, is_inline_only)
        }
        _ => true,
    }
}

/// True if `node` contains a `<ruby>` element anywhere.
fn contains_ruby(node: &serde_json::Value) -> bool {
    match node {
        serde_json::Value::Array(arr) => arr.iter().any(contains_ruby),
        serde_json::Value::Object(map) => {
            map.get("tag").and_then(|v| v.as_str()) == Some("ruby")
                || map.get("content").is_some_and(contains_ruby)
        }
        _ => false,
    }
}

/// Glyphs used by Jitendex that the dictionary font lacks, mapped to renderable ones.
fn normalize_glyphs(segments: &mut [RubySegment]) {
    for seg in segments {
        if let RubySegment::Plain(text) = seg {
            if text.contains('⟶') {
                *text = text.replace('⟶', "→");
            }
        }
    }
}

/// Renders inline content (handling ruby, bold, plain text) using LayoutJob.
pub fn render_inline_content(ui: &mut egui::Ui, node: &serde_json::Value) {
    let mut job = LayoutJob::default();
    build_layout_job_from_node(node, &mut job, false);
    if !job.text.is_empty() {
        ui.label(job);
    }
}

/// Appends nodes to an egui `LayoutJob`.
fn build_layout_job_from_node(node: &serde_json::Value, job: &mut LayoutJob, is_bold: bool) {
    match node {
        serde_json::Value::String(s) => {
            let color = if is_bold { COLOR_HEADWORD } else { COLOR_DEFINITION };
            append_to_layout_job(
                job,
                s,
                TextFormat {
                    font_id: dict_font(34.0),
                    color,
                    ..Default::default()
                },
            );
        }
        serde_json::Value::Array(arr) => {
            for child in arr {
                build_layout_job_from_node(child, job, is_bold);
            }
        }
        serde_json::Value::Object(map) => {
            let tag = map.get("tag").and_then(|v| v.as_str()).unwrap_or("");
            match tag {
                "b" | "strong" => {
                    if let Some(content) = map.get("content") {
                        build_layout_job_from_node(content, job, true);
                    }
                }
                "ruby" => {
                    let (base, rt) = extract_ruby_components(map);
                    append_to_layout_job(
                        job,
                        &base,
                        TextFormat {
                            font_id: dict_font(36.0),
                            color: COLOR_EXAMPLE_JA,
                            ..Default::default()
                        },
                    );
                    if !rt.is_empty() {
                        append_to_layout_job(
                            job,
                            &format!("({})", rt),
                            TextFormat {
                                font_id: dict_font(24.0),
                                color: COLOR_RUBY,
                                ..Default::default()
                            },
                        );
                    }
                }
                _ => {
                    if let Some(content) = map.get("content") {
                        build_layout_job_from_node(content, job, is_bold);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Appends text to a `LayoutJob`, inserting a space when needed to prevent mashed words.
fn append_to_layout_job(job: &mut LayoutJob, text: &str, format: TextFormat) {
    if text.is_empty() {
        return;
    }
    if let Some(last_char) = job.text.chars().last() {
        let first_char = text.chars().next().unwrap();
        if !job.text.ends_with(' ')
            && !job.text.ends_with('\n')
            && !text.starts_with(' ')
            && should_insert_space(last_char, first_char)
        {
            job.append(" ", 0.0, format.clone());
        }
    }
    job.append(text, 0.0, format);
}

/// Determines whether an automatic space is needed between two adjacent text fragments.
fn should_insert_space(last: char, first: char) -> bool {
    let last_is_ascii = last.is_ascii_alphanumeric() || last == ']' || last == ')' || last == '.' || last == ',';
    let first_is_ascii = first.is_ascii_alphanumeric() || first == '(' || first == '[';
    last_is_ascii && first_is_ascii
}

/// Renders ruby text with furigana on top.
fn render_ruby_inline(ui: &mut egui::Ui, map: &serde_json::Map<String, serde_json::Value>) {
    let (base, rt) = extract_ruby_components(map);
    render_ruby_item(ui, &base, &rt);
}

pub fn extract_ruby_components(map: &serde_json::Map<String, serde_json::Value>) -> (String, String) {
    let mut base = String::new();
    let mut rt = String::new();

    if let Some(content) = map.get("content") {
        if let Some(arr) = content.as_array() {
            for item in arr {
                match item {
                    serde_json::Value::String(s) => base.push_str(s),
                    serde_json::Value::Object(obj) => {
                        let tag = obj.get("tag").and_then(|v| v.as_str()).unwrap_or("");
                        if tag == "rt" {
                            rt.push_str(&extract_plain_text_from_node(obj.get("content")));
                        } else {
                            base.push_str(&extract_plain_text_from_node(Some(item)));
                        }
                    }
                    _ => {}
                }
            }
        } else if let Some(s) = content.as_str() {
            base.push_str(s);
        }
    }

    (base, rt)
}

/// Recursively collects all definition texts from `<li>` elements.
fn collect_definitions(node: &serde_json::Value, out: &mut Vec<String>) {
    match node {
        serde_json::Value::Object(map) => {
            let tag = map.get("tag").and_then(|v| v.as_str()).unwrap_or("");
            if tag == "li" {
                let text = extract_plain_text_from_node(map.get("content"));
                if !text.is_empty() {
                    out.push(text);
                }
            } else if let Some(content) = map.get("content") {
                collect_definitions(content, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                collect_definitions(item, out);
            }
        }
        _ => {}
    }
}

/// Extracts English sentence text from `example-sentence-b`, omitting attribution footnotes.
fn extract_en_sentence_text(node: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    collect_en_text_recursive(node, &mut parts);
    parts.join(" ").trim().to_string()
}

fn collect_en_text_recursive(val: &serde_json::Value, out: &mut Vec<String>) {
    match val {
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                collect_en_text_recursive(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            let dc = map
                .get("data")
                .and_then(|d| d.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("");
            if dc == "attribution-footnote" {
                return;
            }
            if let Some(content) = map.get("content") {
                collect_en_text_recursive(content, out);
            }
        }
        _ => {}
    }
}

/// Extracts plain text from a sense, skipping extra-info blocks.
fn extract_plain_text_excluding_extras(val: &serde_json::Value) -> String {
    let mut out = String::new();
    collect_text_skip_extras(val, &mut out);
    out.trim().to_string()
}

fn collect_text_skip_extras(val: &serde_json::Value, out: &mut String) {
    match val {
        serde_json::Value::String(s) => {
            if !out.is_empty() && !out.ends_with(' ') && !s.starts_with(' ') {
                out.push(' ');
            }
            out.push_str(s);
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                collect_text_skip_extras(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            let dc = map
                .get("data")
                .and_then(|d| d.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("");
            if matches!(dc, "extra-info" | "example-sentence" | "xref" | "sense-note" | "info-gloss") {
                return;
            }
            if let Some(content) = map.get("content") {
                collect_text_skip_extras(content, out);
            }
        }
        _ => {}
    }
}

/// Collects all tags (POS, field, dialect, misc) in a node.
fn collect_tags(node: &serde_json::Value, out: &mut Vec<String>) {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if let serde_json::Value::Object(map) = current {
            let dc = map
                .get("data")
                .and_then(|d| d.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("");
            if matches!(dc, "part-of-speech-info" | "field-info" | "misc-info" | "dialect-info") {
                let tag_text = extract_plain_text_from_node(map.get("content"));
                if !tag_text.is_empty() && !out.contains(&tag_text) {
                    out.push(tag_text);
                }
            } else if let Some(content) = map.get("content") {
                match content {
                    serde_json::Value::Array(arr) => {
                        for c in arr.iter().rev() {
                            stack.push(c);
                        }
                    }
                    serde_json::Value::Object(_) => stack.push(content),
                    _ => {}
                }
            }
        }
    }
}

/// Recursively finds the first node where `data.content == target`.
fn find_node_by_data_content<'a>(val: &'a serde_json::Value, target: &str) -> Option<&'a serde_json::Value> {
    match val {
        serde_json::Value::Object(map) => {
            let dc = map
                .get("data")
                .and_then(|d| d.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("");
            if dc == target {
                return Some(val);
            }
            if let Some(content) = map.get("content") {
                if let Some(found) = find_node_by_data_content(content, target) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(found) = find_node_by_data_content(item, target) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

/// Recursively collects all nodes where `data.content == target`.
fn collect_nodes_by_data_content<'a>(
    val: &'a serde_json::Value,
    target: &str,
    out: &mut Vec<&'a serde_json::Value>,
) {
    match val {
        serde_json::Value::Object(map) => {
            let dc = map
                .get("data")
                .and_then(|d| d.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("");
            if dc == target {
                out.push(val);
                return;
            }
            if let Some(content) = map.get("content") {
                collect_nodes_by_data_content(content, target, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                collect_nodes_by_data_content(item, target, out);
            }
        }
        _ => {}
    }
}

/// Extracts all recursive plain text from a JSON node.
pub fn extract_plain_text_from_node(node: Option<&serde_json::Value>) -> String {
    let mut result = String::new();
    if let Some(val) = node {
        collect_text_recursive(val, &mut result);
    }
    result.trim().to_string()
}

fn collect_text_recursive(val: &serde_json::Value, out: &mut String) {
    match val {
        serde_json::Value::String(s) => {
            if !out.is_empty() && !out.ends_with(' ') && !s.starts_with(' ') {
                out.push(' ');
            }
            out.push_str(s);
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                collect_text_recursive(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(content) = map.get("content") {
                collect_text_recursive(content, out);
            }
        }
        _ => {}
    }
}

/// Renders plain HTML text snippets with support for simple `<b>`, `<i>`, `<br>` tags.
fn render_plain_html_text(ui: &mut egui::Ui, text: &str) {
    let mut remaining = text;
    let mut is_bold = false;
    let mut is_italic = false;

    let mut job = LayoutJob::default();

    while !remaining.is_empty() {
        if let Some(tag_start) = remaining.find('<') {
            if tag_start > 0 {
                let piece = &remaining[..tag_start];
                append_styled_text(&mut job, piece, is_bold, is_italic);
            }

            if let Some(tag_end) = remaining[tag_start..].find('>') {
                let tag = &remaining[tag_start + 1..tag_start + tag_end];
                match tag {
                    "b" | "strong" => is_bold = true,
                    "/b" | "/strong" => is_bold = false,
                    "i" | "em" => is_italic = true,
                    "/i" | "/em" => is_italic = false,
                    "br" | "br/" | "br /" => {
                        job.append("\n", 0.0, TextFormat::default());
                    }
                    _ => {}
                }
                remaining = &remaining[tag_start + tag_end + 1..];
            } else {
                append_styled_text(&mut job, &remaining[tag_start..], is_bold, is_italic);
                break;
            }
        } else {
            append_styled_text(&mut job, remaining, is_bold, is_italic);
            break;
        }
    }

    if !job.text.is_empty() {
        ui.label(job);
    }
}

fn append_styled_text(job: &mut LayoutJob, text: &str, bold: bool, italic: bool) {
    let color = if bold { COLOR_HEADWORD } else { COLOR_DEFINITION };
    let font_id = if bold {
        dict_font(36.0)
    } else {
        dict_font(34.0)
    };
    append_to_layout_job(
        job,
        text,
        TextFormat {
            font_id,
            color,
            italics: italic,
            ..Default::default()
        },
    );
}

/// Strips HTML tags from a plain glossary string.
fn strip_html_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Builds a plain-text summary of all senses of an entry, one sense per line, numbered
/// like the popup ("① to eat; ...") when there is more than one sense.
pub fn glossary_plain_text(entry: &TermEntry) -> String {
    let mut senses: Vec<String> = Vec::new();
    for item in &entry.glossary {
        match item {
            GlossaryEntry::Text(text) => {
                let text = strip_html_tags(text);
                if !text.is_empty() {
                    senses.push(text);
                }
            }
            GlossaryEntry::Structured(val) => {
                if let Some((term, reasons)) = as_deinflection_glossary(val) {
                    senses.push(if reasons.is_empty() {
                        term.to_string()
                    } else {
                        format!("{term} ({})", reasons.join(", "))
                    });
                    continue;
                }
                let mut sense_nodes = Vec::new();
                collect_nodes_by_data_content(val, "sense", &mut sense_nodes);
                if sense_nodes.is_empty() {
                    let text = extract_plain_text_excluding_extras(val);
                    if !text.is_empty() {
                        senses.push(text);
                    }
                }
                for sense in sense_nodes {
                    let mut defs = Vec::new();
                    if let Some(glossary_node) = find_node_by_data_content(sense, "glossary") {
                        collect_definitions(glossary_node, &mut defs);
                    }
                    let text = if defs.is_empty() {
                        extract_plain_text_excluding_extras(sense)
                    } else {
                        defs.join("; ")
                    };
                    if !text.is_empty() {
                        senses.push(text);
                    }
                }
            }
        }
    }
    if senses.len() <= 1 {
        return senses.pop().unwrap_or_default();
    }
    senses
        .iter()
        .enumerate()
        .map(|(i, s)| match CIRCLED_NUMBERS.get(i) {
            Some(n) => format!("{n} {s}"),
            None => format!("{}. {s}", i + 1),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redirect_glossary_is_single_inline_run() {
        let redirect = serde_json::json!({
            "tag": "div",
            "data": { "content": "redirect-glossary" },
            "content": ["⟶", {
                "tag": "a",
                "href": "?query=熱い",
                "content": [{ "tag": "ruby", "content": ["熱", { "tag": "rt", "content": "あつ" }] }, "い"]
            }]
        });
        let content = &redirect["content"];
        assert!(is_inline_only(content));
        assert!(contains_ruby(content));
        let mut segs = extract_ruby_segments(content);
        normalize_glyphs(&mut segs);
        assert_eq!(
            segs,
            vec![
                RubySegment::Plain("→".into()),
                RubySegment::Ruby { base: "熱".into(), rt: "あつ".into() },
                RubySegment::Plain("い".into()),
            ]
        );

        // Block content is not flattened
        let block = serde_json::json!([{ "tag": "ul", "content": [{ "tag": "li", "content": "x" }] }]);
        assert!(!is_inline_only(&block));
    }

    #[test]
    fn test_as_deinflection_glossary() {
        let v = serde_json::json!(["熱い", ["redirected from あっつい"]]);
        assert_eq!(as_deinflection_glossary(&v), Some(("熱い", vec!["redirected from あっつい"])));
        assert!(as_deinflection_glossary(&serde_json::json!(["a", "b"])).is_none());
        assert!(as_deinflection_glossary(&serde_json::json!({"type": "structured-content"})).is_none());
    }

    #[test]
    fn test_extract_ruby_components() {
        let json: serde_json::Value = serde_json::json!({
            "tag": "ruby",
            "content": [
                "僕",
                {
                    "tag": "rt",
                    "content": "ぼく"
                }
            ]
        });

        let (base, rt) = extract_ruby_components(json.as_object().unwrap());
        assert_eq!(base, "僕");
        assert_eq!(rt, "ぼく");
    }

    #[test]
    fn test_extract_plain_text_from_node() {
        let json: serde_json::Value = serde_json::json!({
            "tag": "div",
            "content": [
                {
                    "tag": "span",
                    "content": "Hello"
                },
                {
                    "tag": "span",
                    "content": "World"
                }
            ]
        });

        let text = extract_plain_text_from_node(Some(&json));
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn test_collect_definitions_joining() {
        let json: serde_json::Value = serde_json::json!({
            "tag": "ul",
            "data": { "content": "glossary" },
            "content": [
                { "tag": "li", "content": "to change" },
                { "tag": "li", "content": "to be transformed" },
                { "tag": "li", "content": "to be altered" },
                { "tag": "li", "content": "to vary" }
            ]
        });

        let mut defs = Vec::new();
        collect_definitions(&json, &mut defs);
        assert_eq!(defs, vec!["to change", "to be transformed", "to be altered", "to vary"]);
        assert_eq!(defs.join("; "), "to change; to be transformed; to be altered; to vary");
    }

    #[test]
    fn test_extract_en_sentence_text_strips_footnote() {
        let json: serde_json::Value = serde_json::json!({
            "tag": "div",
            "data": { "content": "example-sentence-b" },
            "content": [
                { "tag": "span", "content": "My e-mail address has been changed." },
                { "tag": "span", "data": { "content": "attribution-footnote" }, "content": "[1]" }
            ]
        });

        let text = extract_en_sentence_text(&json);
        assert_eq!(text, "My e-mail address has been changed.");
    }

    #[test]
    fn test_should_insert_space() {
        assert!(should_insert_space('e', 'u')); // "Note" + "usu"
        assert!(should_insert_space(']', 'S')); // "[3]" + "See"
        assert!(should_insert_space(')', 'a')); // "(n)" + "apple"
        assert!(should_insert_space('.', 'I')); // "inc." + "It"
        assert!(!should_insert_space('る', '变')); // Japanese characters
    }

    #[test]
    fn test_collect_tags() {
        let json: serde_json::Value = serde_json::json!({
            "tag": "div",
            "content": [
                {
                    "tag": "span",
                    "data": { "content": "part-of-speech-info" },
                    "content": "v5r"
                },
                {
                    "tag": "span",
                    "data": { "content": "misc-info" },
                    "content": "uk"
                }
            ]
        });

        let mut tags = Vec::new();
        collect_tags(&json, &mut tags);
        assert_eq!(tags, vec!["v5r", "uk"]);
    }

    #[test]
    fn test_extract_ruby_segments_for_example_sentence() {
        let json: serde_json::Value = serde_json::json!({
            "content": [
                {
                    "tag": "ruby",
                    "content": [
                        "私",
                        { "tag": "rt", "content": "わたし" }
                    ]
                },
                "は、",
                {
                    "tag": "span",
                    "content": [
                        {
                            "tag": "ruby",
                            "content": [
                                "ＣＤ",
                                { "tag": "rt", "content": "シーディー" }
                            ]
                        }
                    ]
                },
                "を得た"
            ]
        });

        let segments = extract_ruby_segments(&json);
        assert_eq!(
            segments,
            vec![
                RubySegment::Ruby { base: "私".to_string(), rt: "わたし".to_string() },
                RubySegment::Plain("は".to_string()),
                RubySegment::Plain("、".to_string()),
                RubySegment::Ruby { base: "ＣＤ".to_string(), rt: "シーディー".to_string() },
                RubySegment::Plain("を".to_string()),
                RubySegment::Plain("得".to_string()),
                RubySegment::Plain("た".to_string()),
            ]
        );
    }

    #[test]
    fn test_is_cjk_char() {
        assert!(is_cjk_char('あ'));
        assert!(is_cjk_char('ア'));
        assert!(is_cjk_char('漢'));
        assert!(is_cjk_char('、'));
        assert!(!is_cjk_char('a'));
        assert!(!is_cjk_char('1'));
    }

    #[test]
    fn test_calculate_ruby_metrics_produces_tight_gap() {
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
        let font_base = dict_font(38.0);
        let font_rt = dict_font(20.0);
        let galley_base = ctx.fonts(|f| f.layout_no_wrap("私".to_string(), font_base, Color32::WHITE));
        let galley_rt = ctx.fonts(|f| f.layout_no_wrap("わたし".to_string(), font_rt, Color32::WHITE));

        let (base_y_offset, total_h) = calculate_ruby_metrics(&galley_rt, &galley_base);

        // base_y_offset should tuck the base galley upwards into the ruby descent zone (~14-16px)
        assert!(base_y_offset > 10.0 && base_y_offset < 20.0, "base_y_offset: {base_y_offset}");
        // total_h should be significantly more compact than the naive sum (rt_h + base_h ≈ 84px)
        assert!(total_h < 72.0, "total_h: {total_h}");

        // Verify visual gap between bottom of ruby glyph and top of base glyph
        let g_rt = &galley_rt.rows[0].glyphs[0];
        let rt_glyph_bottom = g_rt.pos.y + g_rt.uv_rect.offset.y + g_rt.uv_rect.size.y;
        let g_base = &galley_base.rows[0].glyphs[0];
        let base_glyph_top = base_y_offset + g_base.pos.y + g_base.uv_rect.offset.y;
        let gap = base_glyph_top - rt_glyph_bottom;
        assert!((gap - 2.0).abs() < 0.1, "Calculated visual gap was {gap} instead of 2.0");
    }

    #[test]
    fn test_header_layout_job_baseline_alignment() {
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

        let job = build_headword_reading_job(
            "暗い",
            "くらい",
            48.0,
            34.0,
            COLOR_HEADWORD,
            COLOR_READING,
        );

        let galley = ctx.fonts(|f| f.layout_job(job));
        assert_eq!(galley.rows.len(), 1);
        let baseline_y = galley.rows[0].glyphs[0].pos.y;
        for g in &galley.rows[0].glyphs {
            assert!(
                (g.pos.y - baseline_y).abs() < f32::EPSILON,
                "Glyph '{}' pos.y ({}) did not match headword baseline ({})",
                g.chr,
                g.pos.y,
                baseline_y
            );
        }
    }
}

