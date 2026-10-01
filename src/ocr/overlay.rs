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

    let mut copied_text: Option<String> = None;
    let mut copied_index: Option<usize> = None;

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
        let is_hovered = response.hovered();

        // Style the box according to its state
        let (stroke, fill) = if is_recently_copied {
            (
                egui::Stroke::new(2.5, egui::Color32::from_rgb(46, 204, 113)),
                egui::Color32::from_rgba_unmultiplied(46, 204, 113, 80),
            )
        } else if is_hovered {
            (
                egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 215, 0)),
                egui::Color32::from_rgba_unmultiplied(255, 215, 0, 50),
            )
        } else {
            (
                egui::Stroke::new(1.5, egui::Color32::from_rgba_unmultiplied(66, 133, 244, 210)),
                egui::Color32::from_rgba_unmultiplied(66, 133, 244, 35),
            )
        };

        ui.painter().rect(box_rect, 3.0, fill, stroke);

        if is_hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        if response.clicked() {
            copied_text = Some(ocr_box.text.clone());
            copied_index = Some(idx);
        }

        response.on_hover_ui(|ui| {
            ui.label(egui::RichText::new(&ocr_box.text).size(16.0).strong());
            ui.label(egui::RichText::new("📋 Click to copy to clipboard").italics().weak());
        });
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
}
