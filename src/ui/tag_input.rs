//! Single-line text input with a dropdown of fuzzy-matched tag suggestions.
//!
//! The dropdown is shown while the field has keyboard focus. Up/Down highlight a
//! suggestion, Enter/Tab accept it, clicking picks one, and Escape cancels.

use crate::bank::tags;
use eframe::egui;

const MAX_SUGGESTIONS: usize = 8;

pub struct TagInputOutput {
    /// Editing finished: Enter was pressed, a suggestion was picked or focus moved away.
    pub committed: bool,
    /// Editing was abandoned with Escape (not reported as `committed`).
    pub cancelled: bool,
}

/// Draws the input. `known` is the list of existing tags, most recently used first.
pub fn tag_input(
    ui: &mut egui::Ui,
    id_source: impl std::hash::Hash,
    text: &mut String,
    known: &[String],
    hint: &str,
    width: f32,
    request_focus: bool,
) -> TagInputOutput {
    let id = ui.make_persistent_id(id_source);
    let selected_id = id.with("selected");
    let had_focus = ui.memory(|m| m.has_focus(id));

    // Keyboard navigation of the suggestions shown last frame. This must run before the
    // TextEdit so it doesn't also act on these keys.
    let mut selected: Option<usize> = ui.data(|d| d.get_temp(selected_id)).unwrap_or(None);
    let mut picked: Option<String> = None;
    if had_focus {
        let shown = tags::suggest(text, known, MAX_SUGGESTIONS);
        if selected.map_or(false, |s| s >= shown.len()) {
            selected = None;
        }
        ui.input_mut(|i| {
            if shown.is_empty() {
                return;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                selected = Some(selected.map_or(0, |s| (s + 1).min(shown.len() - 1)));
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                selected = selected.and_then(|s| s.checked_sub(1));
            }
            if let Some(s) = selected {
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                    picked = Some(shown[s].clone());
                }
            }
        });
    }

    let response = egui::TextEdit::singleline(text)
        .id(id)
        .hint_text(hint)
        .desired_width(width)
        .show(ui)
        .response;
    if request_focus && !had_focus {
        response.request_focus();
    }

    let lost_focus = response.lost_focus();
    let (escape, tab) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Tab)));
    let cancelled = lost_focus && escape;

    // Tab moves focus before widgets run, so accept the highlighted suggestion here.
    if picked.is_none() && lost_focus && tab {
        if let Some(s) = selected {
            picked = tags::suggest(text, known, MAX_SUGGESTIONS).get(s).cloned();
        }
    }

    // Show the dropdown while focused, and also on the frame focus is lost: pressing on a
    // suggestion takes focus away from the text field, and the pick must still register.
    if (response.has_focus() || (lost_focus && !cancelled)) && picked.is_none() {
        let suggestions = tags::suggest(text, known, MAX_SUGGESTIONS);
        if response.changed() {
            selected = None;
        }
        if !suggestions.is_empty() {
            egui::Area::new(id.with("suggestions"))
                .order(egui::Order::Foreground)
                .fixed_pos(response.rect.left_bottom() + egui::vec2(0.0, 2.0))
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(response.rect.width() - 12.0);
                        for (i, suggestion) in suggestions.iter().enumerate() {
                            let r = ui.add(
                                egui::SelectableLabel::new(selected == Some(i), suggestion.as_str())
                            );
                            // Pick on press: the text field loses focus at that moment.
                            if r.is_pointer_button_down_on() || r.clicked() {
                                picked = Some(suggestion.clone());
                            }
                        }
                    });
                });
        }
    }

    if let Some(p) = &picked {
        *text = p.clone();
        ui.memory_mut(|m| m.surrender_focus(id));
        selected = None;
    }
    if !response.has_focus() {
        selected = None;
    }
    ui.data_mut(|d| d.insert_temp(selected_id, selected));

    TagInputOutput {
        committed: picked.is_some() || (lost_focus && !cancelled),
        cancelled,
    }
}
