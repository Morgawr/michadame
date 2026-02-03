use crate::app::AppState;
use eframe::egui;
use eframe::egui_glow;
use crate::devices::filter_type::CrtFilter;
use crate::video;

use std::thread;
use tungstenite::{connect, Message};
use url::Url;

pub mod controls;
pub mod dialogs;

fn send_ws_command(command: serde_json::Value) {
    thread::spawn(move || {
        println!("Connecting to WebSocket at ws://127.0.0.1:9002 ...");
        match connect(Url::parse("ws://127.0.0.1:9002").unwrap()) {
            Ok((mut socket, _)) => {
                println!("Connected to WebSocket.");
                let msg = command.to_string();
                if let Err(e) = socket.send(Message::Text(msg)) {
                    println!("WebSocket write error: {}", e);
                    return;
                }
                match socket.read() {
                    Ok(msg) => println!("WebSocket received: {}", msg),
                    Err(e) => println!("WebSocket read error: {}", e),
                }
                let _ = socket.close(None);
            }
            Err(e) => println!("Failed to connect to WebSocket: {}", e),
        }
    });
}

pub fn draw_main_ui(state: &mut AppState, ctx: &egui::Context) -> bool {
    if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        println!("Spacebar pressed, sending command...");
        send_ws_command(serde_json::json!({"command": "manual_ocr"}));
    }

    let panel_frame = if state.is_fullscreen {
        egui::Frame::none()
    } else {
        egui::Frame::central_panel(&ctx.style())
    };

    egui::CentralPanel::default()
        .frame(panel_frame)
        .show(ctx, |ui| {
            let mut repaint_requested = false;
            if state.show_first_run_dialog {
                repaint_requested |= dialogs::show_first_run_dialog(state, ctx, ui);
            }

            repaint_requested |= controls::layout_top_ui(ui, state);

            repaint_requested
        })
        .inner
}

pub fn draw_video_player(state: &mut AppState, ui: &mut egui::Ui, ctx: &egui::Context) {
    if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
        println!("Spacebar pressed, sending command...");
        send_ws_command(serde_json::json!({"command": "manual_ocr"}));
    }

    if state.video_window_open {
        let response = ui.allocate_response(ui.available_size(), egui::Sense::click());
        let video_texture = state.video_texture.as_ref().unwrap();
        let video_texture_id = video_texture.id();
        let texture_size = video_texture.size_vec2();

        let filter = CrtFilter::from_u8(state.crt_filter.load(std::sync::atomic::Ordering::Relaxed));

        // All GPU filtering is handled within a single paint callback to ensure correct state.
        if state.pixelate_filter_enabled || filter == CrtFilter::Lottes {
            if let Some(renderer_arc) = &state.crt_renderer {
                let renderer_clone = renderer_arc.clone();
                let params = video::gpu_filter::ShaderParams::from_state(state);
                let pixelate = state.pixelate_filter_enabled;
                let run_lottes = filter == CrtFilter::Lottes;
                let rect = response.rect;
    
                let callback = egui::PaintCallback {
                    rect: response.rect,
                    callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                        let mut renderer = renderer_clone.lock().unwrap();
                        let output_size = (rect.width(), rect.height()); // The size of the viewport area to draw in
                        renderer.paint(painter, video_texture_id, (texture_size.x as u32, texture_size.y as u32), output_size, &params, pixelate, run_lottes)
                    })),
                };
                ui.painter().add(callback);
            }
        } else {
            // Fallback to a simple passthrough shader if no other GPU filters are active.
            let renderer_clone = state.crt_renderer.as_ref().unwrap().clone();
            let rect = response.rect;
            let callback = egui::PaintCallback { rect, callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                renderer_clone.lock().unwrap().draw_passthrough(painter.gl(), painter.texture(video_texture_id).unwrap(), (texture_size.x as u32, texture_size.y as u32), (rect.width(), rect.height()));
            }))};
            ui.painter().add(callback);
        }
        if response.double_clicked() {
            let is_fullscreen = !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(is_fullscreen));
        }
    }
}