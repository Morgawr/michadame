//! Full-screen transparent drawing whiteboard layer (toggled with `H`).
//!
//! Covers the entire video viewport with a transparent drawing surface.
//! Renders above shaders, CRT borders, Niconico comments, OCR boxes, and OCR dictionary popups,
//! while staying beneath the Webcam overlay, Debug window, and Twitch chat overlay.
//! Supports freehand `Pen`, filled borderless `Square`, and `Eraser` tools with zero
//! anti-aliasing or smoothing, plus `Ctrl+Z` undo and `Ctrl+Y` redo history.

use crate::app::AppState;
use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Sense, Stroke, TextureOptions, Vec2};
use std::collections::VecDeque;

/// Default pen/eraser brush diameter in pixels.
pub const DEFAULT_BRUSH_SIZE: u32 = 4;
/// Minimum brush diameter in pixels.
pub const MIN_BRUSH_SIZE: u32 = 1;
/// Maximum brush diameter in pixels.
pub const MAX_BRUSH_SIZE: u32 = 50;
/// Maximum number of canvas snapshots retained in the `Ctrl+Z` undo buffer.
pub const MAX_UNDO_STEPS: usize = 32;

/// Active drawing tool on the whiteboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WhiteboardTool {
    #[default]
    Pen,
    Square,
    Eraser,
}

/// Basic palette colors available in the whiteboard toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WhiteboardColor {
    Green,
    Red,
    Blue,
    Yellow,
    White,
    #[default]
    Black,
}

impl WhiteboardColor {
    /// All palette colors in toolbar display order.
    pub const ALL: [WhiteboardColor; 6] = [
        WhiteboardColor::Green,
        WhiteboardColor::Red,
        WhiteboardColor::Blue,
        WhiteboardColor::Yellow,
        WhiteboardColor::White,
        WhiteboardColor::Black,
    ];

    /// Exact RGBA bytes written to the raster canvas buffer when drawing with this color.
    #[inline]
    pub const fn to_rgba(self) -> [u8; 4] {
        match self {
            WhiteboardColor::Green => [22, 163, 74, 255],
            WhiteboardColor::Red => [220, 38, 38, 255],
            WhiteboardColor::Blue => [37, 99, 235, 255],
            WhiteboardColor::Yellow => [250, 204, 21, 255],
            WhiteboardColor::White => [255, 255, 255, 255],
            WhiteboardColor::Black => [0, 0, 0, 255],
        }
    }

    /// Converts the palette color to an `egui::Color32` for UI rendering.
    #[inline]
    pub const fn to_color32(self) -> Color32 {
        let [r, g, b, a] = self.to_rgba();
        Color32::from_rgba_premultiplied(r, g, b, a)
    }

    /// Human-readable name for hover tooltips.
    #[inline]
    pub const fn label(self) -> &'static str {
        match self {
            WhiteboardColor::Green => "Green",
            WhiteboardColor::Red => "Red",
            WhiteboardColor::Blue => "Blue",
            WhiteboardColor::Yellow => "Yellow",
            WhiteboardColor::White => "White",
            WhiteboardColor::Black => "Black",
        }
    }
}

/// Compact bounding-box diff patch stored in the `Ctrl+Z` / `Ctrl+Y` history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndoPatch {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub before_rgba: Vec<u8>,
    pub after_rgba: Vec<u8>,
}

/// Result of synchronizing `canvas_rgba` with the GPU texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhiteboardTextureUpload {
    /// No upload required this frame.
    None,
    /// Initial texture allocation or full-canvas upload (`load_texture` / `set`).
    Full { width: usize, height: usize },
    /// Sub-region upload via `TextureHandle::set_partial`.
    Partial {
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    },
}

/// Runtime state of the transparent drawing whiteboard.
pub struct WhiteboardState {
    /// Whether the transparent whiteboard layer is currently active/visible (toggled with `H`).
    pub open: bool,
    /// Currently selected tool (`Pen`, `Square`, or `Eraser`).
    pub tool: WhiteboardTool,
    /// Currently selected color for `Pen` and `Square`.
    pub color: WhiteboardColor,
    /// Diameter of the pen/eraser brush in pixels (`1..=50`).
    pub brush_size: u32,
    /// Backing RGBA pixel buffer (`[0, 0, 0, 0]` = unpainted/erased, `alpha = 255` = painted).
    pub canvas_rgba: Vec<u8>,
    /// Dimensions `(width, height)` of `canvas_rgba` in pixels.
    pub canvas_size: (u32, u32),
    /// Nearest-neighbor GPU texture uploaded from `canvas_rgba`.
    pub texture: Option<egui::TextureHandle>,
    /// Whether `canvas_rgba` has been modified since the last texture upload.
    pub texture_dirty: bool,
    /// Whether the entire `canvas_rgba` must be uploaded (initial create or canvas resize).
    pub texture_full_upload_needed: bool,
    /// Inclusive pixel bounding box `[min_x, min_y, max_x, max_y]` modified since last upload.
    pub dirty_rect: Option<[usize; 4]>,
    /// Whether the user is currently holding the primary mouse button for a `Pen`/`Eraser` stroke.
    pub is_drawing: bool,
    /// Previous integer canvas pixel position during an active stroke, for Bresenham interpolation.
    pub last_draw_pos: Option<(i32, i32)>,
    /// Anchor pixel `(x, y)` when dragging a filled `Square` shape.
    pub square_start: Option<(i32, i32)>,
    /// Current drag pixel `(x, y)` when dragging a filled `Square` shape.
    pub square_current: Option<(i32, i32)>,
    /// Undo history queue (`Ctrl+Z`) storing compact bounding-box diff patches.
    pub undo_stack: VecDeque<UndoPatch>,
    /// Redo history queue (`Ctrl+Y`) storing undone compact bounding-box diff patches.
    pub redo_stack: VecDeque<UndoPatch>,
    /// Whether the back entry of `undo_stack` is still accumulating modifications for an active stroke.
    active_patch_open: bool,
    /// Height of the top toolbar from the most recent frame.
    pub last_toolbar_height: f32,
}

impl Default for WhiteboardState {
    fn default() -> Self {
        Self {
            open: false,
            tool: WhiteboardTool::Pen,
            color: WhiteboardColor::Black,
            brush_size: DEFAULT_BRUSH_SIZE,
            canvas_rgba: Vec::new(),
            canvas_size: (0, 0),
            texture: None,
            texture_dirty: true,
            texture_full_upload_needed: true,
            dirty_rect: None,
            is_drawing: false,
            last_draw_pos: None,
            square_start: None,
            square_current: None,
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            active_patch_open: false,
            last_toolbar_height: 36.0,
        }
    }
}

impl WhiteboardState {
    /// Toggles the transparent whiteboard layer on or off.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.cancel_active_interaction();
        }
    }

    /// Cancels any in-progress stroke or square drag, sealing any already-drawn stroke patch.
    pub fn cancel_active_interaction(&mut self) {
        self.is_drawing = false;
        self.last_draw_pos = None;
        self.square_start = None;
        self.square_current = None;
        self.seal_active_patch();
    }

    /// Returns true if the whiteboard layer is currently active on screen.
    #[inline]
    pub fn is_visible(&self) -> bool {
        self.open
    }

    /// Returns true if the user is actively dragging a stroke or square on the whiteboard.
    #[inline]
    pub fn is_interacting(&self) -> bool {
        self.is_drawing || self.square_start.is_some()
    }

    /// Returns true if `pos` is inside the whiteboard when the whiteboard is open.
    #[inline]
    pub fn contains_pointer(&self, pos: Option<Pos2>, screen_rect: Rect) -> bool {
        self.open && pos.is_some_and(|p| screen_rect.contains(p))
    }

    /// Expands `dirty_rect` to cover inclusive `[min_x, min_y, max_x, max_y]` and marks texture dirty.
    #[inline]
    fn mark_dirty_rect(&mut self, min_x: usize, min_y: usize, max_x: usize, max_y: usize) {
        let (cw, ch) = (self.canvas_size.0 as usize, self.canvas_size.1 as usize);
        if cw == 0 || ch == 0 || min_x > max_x || min_y > max_y || min_x >= cw || min_y >= ch {
            return;
        }
        let max_x = max_x.min(cw - 1);
        let max_y = max_y.min(ch - 1);
        self.dirty_rect = Some(match self.dirty_rect {
            Some([ox0, oy0, ox1, oy1]) => [
                ox0.min(min_x),
                oy0.min(min_y),
                ox1.max(max_x),
                oy1.max(max_y),
            ],
            None => [min_x, min_y, max_x, max_y],
        });
        self.texture_dirty = true;
    }

    /// Copies a `(x, y, w, h)` sub-rectangle out of `self.canvas_rgba`.
    fn extract_rect_rgba(&self, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
        let (cw, ch) = (self.canvas_size.0 as usize, self.canvas_size.1 as usize);
        let (x, y, w, h) = (x as usize, y as usize, w as usize, h as usize);
        if cw == 0 || ch == 0 || w == 0 || h == 0 || x >= cw || y >= ch {
            return Vec::new();
        }
        let copy_w = w.min(cw - x);
        let copy_h = h.min(ch - y);
        let mut out = vec![0u8; w * h * 4];
        let src_row_bytes = cw * 4;
        let dst_row_bytes = w * 4;
        let copy_bytes = copy_w * 4;
        for row in 0..copy_h {
            let src_start = (y + row) * src_row_bytes + x * 4;
            let dst_start = row * dst_row_bytes;
            out[dst_start..dst_start + copy_bytes]
                .copy_from_slice(&self.canvas_rgba[src_start..src_start + copy_bytes]);
        }
        out
    }

    /// Writes `patch_rgba` (`w * h * 4` bytes) back into `self.canvas_rgba` at `(x, y)` and marks dirty.
    fn apply_rect_rgba(&mut self, x: u32, y: u32, w: u32, h: u32, patch_rgba: &[u8]) {
        let (cw, ch) = (self.canvas_size.0 as usize, self.canvas_size.1 as usize);
        let (x, y, w, h) = (x as usize, y as usize, w as usize, h as usize);
        if cw == 0
            || ch == 0
            || w == 0
            || h == 0
            || x >= cw
            || y >= ch
            || patch_rgba.len() < w * h * 4
        {
            return;
        }
        let copy_w = w.min(cw - x);
        let copy_h = h.min(ch - y);
        let dst_row_bytes = cw * 4;
        let src_row_bytes = w * 4;
        let copy_bytes = copy_w * 4;
        for row in 0..copy_h {
            let src_start = row * src_row_bytes;
            let dst_start = (y + row) * dst_row_bytes + x * 4;
            self.canvas_rgba[dst_start..dst_start + copy_bytes]
                .copy_from_slice(&patch_rgba[src_start..src_start + copy_bytes]);
        }
        self.mark_dirty_rect(x, y, x + copy_w - 1, y + copy_h - 1);
    }

    /// Ensures the active open `UndoPatch` covers inclusive `[min_x..=max_x, min_y..=max_y]`
    /// BEFORE pixels inside that region are mutated.
    fn prepare_undo_region(&mut self, min_x: u32, min_y: u32, max_x: u32, max_y: u32) {
        if !self.active_patch_open {
            return;
        }
        let (cw, ch) = self.canvas_size;
        if cw == 0 || ch == 0 || min_x > max_x || min_y > max_y || min_x >= cw || min_y >= ch {
            return;
        }
        let rx0 = min_x;
        let ry0 = min_y;
        let rx1 = max_x.min(cw - 1);
        let ry1 = max_y.min(ch - 1);

        let Some((old_x, old_y, old_w, old_h)) =
            self.undo_stack.back().map(|p| (p.x, p.y, p.w, p.h))
        else {
            self.active_patch_open = false;
            return;
        };

        if old_w == 0 || old_h == 0 {
            let w = rx1 - rx0 + 1;
            let h = ry1 - ry0 + 1;
            let before = self.extract_rect_rgba(rx0, ry0, w, h);
            if let Some(patch) = self.undo_stack.back_mut() {
                patch.x = rx0;
                patch.y = ry0;
                patch.w = w;
                patch.h = h;
                patch.before_rgba = before;
            }
            return;
        }

        let old_x1 = old_x + old_w - 1;
        let old_y1 = old_y + old_h - 1;
        if rx0 >= old_x && ry0 >= old_y && rx1 <= old_x1 && ry1 <= old_y1 {
            return;
        }

        let ux0 = old_x.min(rx0);
        let uy0 = old_y.min(ry0);
        let ux1 = old_x1.max(rx1);
        let uy1 = old_y1.max(ry1);
        let uw = ux1 - ux0 + 1;
        let uh = uy1 - uy0 + 1;

        // Every pixel outside `[old_x..=old_x1, old_y..=old_y1]` is untouched since stroke start,
        // so extract the union from `canvas_rgba` first and restore the prior sub-rect on top.
        let mut new_before = self.extract_rect_rgba(ux0, uy0, uw, uh);
        if let Some(patch) = self.undo_stack.back_mut() {
            let dst_row_bytes = (uw as usize) * 4;
            let src_row_bytes = (old_w as usize) * 4;
            let off_x = (old_x - ux0) as usize;
            let off_y = (old_y - uy0) as usize;
            for row in 0..old_h as usize {
                let src_start = row * src_row_bytes;
                let dst_start = (off_y + row) * dst_row_bytes + off_x * 4;
                new_before[dst_start..dst_start + src_row_bytes]
                    .copy_from_slice(&patch.before_rgba[src_start..src_start + src_row_bytes]);
            }
            patch.x = ux0;
            patch.y = uy0;
            patch.w = uw;
            patch.h = uh;
            patch.before_rgba = new_before;
        }
    }

    /// Finalizes `after_rgba` for the currently open `UndoPatch` on the back of `undo_stack`.
    pub fn seal_active_patch(&mut self) {
        if !self.active_patch_open {
            return;
        }
        self.active_patch_open = false;
        let Some((x, y, w, h)) = self.undo_stack.back().map(|p| (p.x, p.y, p.w, p.h)) else {
            return;
        };
        if w == 0 || h == 0 {
            self.undo_stack.pop_back();
            return;
        }
        let after = self.extract_rect_rgba(x, y, w, h);
        if let Some(patch) = self.undo_stack.back_mut() {
            patch.after_rgba = after;
        }
    }

    /// Begins recording a compact `UndoPatch` on the `Ctrl+Z` undo stack and clears `redo_stack`.
    pub fn save_undo_snapshot(&mut self) {
        if self.canvas_rgba.is_empty() || self.canvas_size.0 == 0 || self.canvas_size.1 == 0 {
            return;
        }
        self.seal_active_patch();
        if self.undo_stack.len() >= MAX_UNDO_STEPS {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(UndoPatch {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            before_rgba: Vec::new(),
            after_rgba: Vec::new(),
        });
        self.active_patch_open = true;
        self.redo_stack.clear();
    }

    /// Undoes the most recent drawing operation (`Ctrl+Z`), moving the patch to `redo_stack`.
    /// Returns `true` if an undo patch was restored.
    pub fn undo(&mut self) -> bool {
        self.cancel_active_interaction();
        let Some(patch) = self.undo_stack.pop_back() else {
            return false;
        };
        self.apply_rect_rgba(patch.x, patch.y, patch.w, patch.h, &patch.before_rgba);
        if self.redo_stack.len() >= MAX_UNDO_STEPS {
            self.redo_stack.pop_front();
        }
        self.redo_stack.push_back(patch);
        true
    }

    /// Repeats/redoes the most recently undone drawing operation (`Ctrl+Y`).
    /// Returns `true` if a redo patch was restored.
    pub fn redo(&mut self) -> bool {
        self.cancel_active_interaction();
        let Some(patch) = self.redo_stack.pop_back() else {
            return false;
        };
        self.apply_rect_rgba(patch.x, patch.y, patch.w, patch.h, &patch.after_rgba);
        if self.undo_stack.len() >= MAX_UNDO_STEPS {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(patch);
        true
    }

    /// Resizes a raw RGBA buffer from `(old_w, old_h)` to `(new_w, new_h)`, preserving top-left pixels.
    fn resize_rgba_buffer(buf: &[u8], old_w: u32, old_h: u32, new_w: u32, new_h: u32) -> Vec<u8> {
        let mut new_rgba = vec![0u8; (new_w * new_h * 4) as usize];
        if old_w > 0 && old_h > 0 && buf.len() == (old_w * old_h * 4) as usize {
            let copy_row_bytes = (old_w.min(new_w) * 4) as usize;
            let old_row_bytes = (old_w * 4) as usize;
            let new_row_bytes = (new_w * 4) as usize;
            for y in 0..old_h.min(new_h) as usize {
                let src_start = y * old_row_bytes;
                let dst_start = y * new_row_bytes;
                new_rgba[dst_start..dst_start + copy_row_bytes]
                    .copy_from_slice(&buf[src_start..src_start + copy_row_bytes]);
            }
        }
        new_rgba
    }

    /// Ensures the backing RGBA pixel buffer is at least `(min_w, min_h)`, ignoring 0-sized
    /// transient resize frames and preserving existing pixels and compact undo/redo patches.
    pub fn ensure_canvas_size(&mut self, min_w: u32, min_h: u32) {
        if min_w == 0 || min_h == 0 {
            return;
        }
        if self.canvas_size.0 >= min_w
            && self.canvas_size.1 >= min_h
            && self.canvas_rgba.len() == (self.canvas_size.0 * self.canvas_size.1 * 4) as usize
        {
            return;
        }

        if !self.is_drawing {
            self.seal_active_patch();
        }

        let (old_w, old_h) = self.canvas_size;
        let new_w = old_w.max(min_w);
        let new_h = old_h.max(min_h);

        self.canvas_rgba = Self::resize_rgba_buffer(&self.canvas_rgba, old_w, old_h, new_w, new_h);
        self.canvas_size = (new_w, new_h);
        self.texture_full_upload_needed = true;
        self.dirty_rect = None;
        self.texture_dirty = true;
    }

    /// Clears all drawn pixels on the canvas back to transparent (`[0, 0, 0, 0]`), recording
    /// a compact bounding-box undo patch only for the painted region.
    pub fn clear_canvas(&mut self) {
        let (cw, ch) = (self.canvas_size.0 as usize, self.canvas_size.1 as usize);
        if cw == 0 || ch == 0 || self.canvas_rgba.len() != cw * ch * 4 {
            self.seal_active_patch();
            return;
        }

        let mut min_x = cw;
        let mut min_y = ch;
        let mut max_x = 0usize;
        let mut max_y = 0usize;
        let row_bytes = cw * 4;
        for y in 0..ch {
            let row = &self.canvas_rgba[y * row_bytes..(y + 1) * row_bytes];
            if !row.iter().any(|&b| b != 0) {
                continue;
            }
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            for (x, px) in row.chunks_exact(4).enumerate() {
                if px != [0, 0, 0, 0] {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
        }

        if min_x > max_x || min_y > max_y {
            self.seal_active_patch();
            return;
        }

        self.save_undo_snapshot();
        self.prepare_undo_region(min_x as u32, min_y as u32, max_x as u32, max_y as u32);
        let clear_span = (max_x - min_x + 1) * 4;
        for y in min_y..=max_y {
            let start = y * row_bytes + min_x * 4;
            self.canvas_rgba[start..start + clear_span].fill(0);
        }
        self.seal_active_patch();
        self.mark_dirty_rect(min_x, min_y, max_x, max_y);
    }

    /// Stamps a hard, un-aliased brush footprint centered at `(cx, cy)` using the current tool.
    pub fn stamp_brush(&mut self, cx: i32, cy: i32) {
        let (w, h) = (self.canvas_size.0 as i32, self.canvas_size.1 as i32);
        if w <= 0 || h <= 0 {
            return;
        }

        let rgba = match self.tool {
            WhiteboardTool::Pen | WhiteboardTool::Square => self.color.to_rgba(),
            WhiteboardTool::Eraser => [0, 0, 0, 0],
        };

        let size = self.brush_size.clamp(MIN_BRUSH_SIZE, MAX_BRUSH_SIZE) as i32;
        if size <= 1 {
            if cx >= 0 && cx < w && cy >= 0 && cy < h {
                self.prepare_undo_region(cx as u32, cy as u32, cx as u32, cy as u32);
                let idx = ((cy * w + cx) * 4) as usize;
                self.canvas_rgba[idx..idx + 4].copy_from_slice(&rgba);
                self.mark_dirty_rect(cx as usize, cy as usize, cx as usize, cy as usize);
            }
            return;
        }

        // Use integer-scaled diameter math (2 * dx + offset) so both even and odd brush sizes
        // produce a crisp, symmetric pixel circle with zero anti-aliasing or soft edges.
        let r_twice = size;
        let r_sq_four = r_twice * r_twice;
        let min_offset = -(size / 2);
        let max_offset = min_offset + size - 1;
        let center_shift = if size % 2 == 0 { 1 } else { 0 };

        let bx0 = (cx + min_offset).max(0);
        let by0 = (cy + min_offset).max(0);
        let bx1 = (cx + max_offset).min(w - 1);
        let by1 = (cy + max_offset).min(h - 1);
        if bx0 > bx1 || by0 > by1 {
            return;
        }

        self.prepare_undo_region(bx0 as u32, by0 as u32, bx1 as u32, by1 as u32);

        for dy in min_offset..=max_offset {
            let py = cy + dy;
            if py < 0 || py >= h {
                continue;
            }
            let dy_twice = 2 * dy + center_shift;
            for dx in min_offset..=max_offset {
                let px = cx + dx;
                if px < 0 || px >= w {
                    continue;
                }
                let dx_twice = 2 * dx + center_shift;
                if dx_twice * dx_twice + dy_twice * dy_twice <= r_sq_four {
                    let idx = ((py * w + px) * 4) as usize;
                    self.canvas_rgba[idx..idx + 4].copy_from_slice(&rgba);
                }
            }
        }
        self.mark_dirty_rect(bx0 as usize, by0 as usize, bx1 as usize, by1 as usize);
    }

    /// Rasterizes a continuous, un-smoothed integer Bresenham line from `from` to `to`.
    pub fn draw_stroke_line(&mut self, from: (i32, i32), to: (i32, i32)) {
        let (w, h) = (self.canvas_size.0 as i32, self.canvas_size.1 as i32);
        if w <= 0 || h <= 0 {
            return;
        }

        let (mut x0, mut y0) = from;
        let (x1, y1) = to;

        // Pre-reserve the full line segment bounds once so per-step `stamp_brush` calls never reallocate.
        if self.active_patch_open {
            let size = self.brush_size.clamp(MIN_BRUSH_SIZE, MAX_BRUSH_SIZE) as i32;
            let min_offset = -(size / 2);
            let max_offset = min_offset + size - 1;
            let sx0 = (x0.min(x1) + min_offset).max(0);
            let sy0 = (y0.min(y1) + min_offset).max(0);
            let sx1 = (x0.max(x1) + max_offset).min(w - 1);
            let sy1 = (y0.max(y1) + max_offset).min(h - 1);
            if sx0 <= sx1 && sy0 <= sy1 {
                self.prepare_undo_region(sx0 as u32, sy0 as u32, sx1 as u32, sy1 as u32);
            }
        }

        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;

        loop {
            self.stamp_brush(x0, y0);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                if x0 == x1 {
                    break;
                }
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                if y0 == y1 {
                    break;
                }
                err += dx;
                y0 += sy;
            }
        }
    }

    /// Fills a solid axis-aligned rectangle between `from` and `to` (inclusive) with the active
    /// `WhiteboardColor` and no border.
    pub fn fill_rect(&mut self, from: (i32, i32), to: (i32, i32)) {
        let (w, h) = (self.canvas_size.0 as i32, self.canvas_size.1 as i32);
        if w <= 0 || h <= 0 {
            return;
        }

        let x0 = from.0.min(to.0).clamp(0, w - 1);
        let x1 = from.0.max(to.0).clamp(0, w - 1);
        let y0 = from.1.min(to.1).clamp(0, h - 1);
        let y1 = from.1.max(to.1).clamp(0, h - 1);

        self.prepare_undo_region(x0 as u32, y0 as u32, x1 as u32, y1 as u32);

        let rgba = self.color.to_rgba();
        for y in y0..=y1 {
            let row_offset = (y * w) as usize;
            for x in x0..=x1 {
                let idx = (row_offset + x as usize) * 4;
                self.canvas_rgba[idx..idx + 4].copy_from_slice(&rgba);
            }
        }
        self.mark_dirty_rect(x0 as usize, y0 as usize, x1 as usize, y1 as usize);
    }

    /// Apply a pointer sample at `(px, py)` in canvas coordinates, connecting from `last_draw_pos`
    /// if a stroke is already active.
    pub fn apply_pointer_sample(&mut self, px: i32, py: i32) {
        if let Some(prev) = self.last_draw_pos {
            self.draw_stroke_line(prev, (px, py));
        } else {
            self.stamp_brush(px, py);
        }
        self.last_draw_pos = Some((px, py));
    }

    /// Synchronizes modified pixels in `canvas_rgba` to the nearest-neighbor GPU texture,
    /// using `TextureHandle::set_partial` whenever only a sub-rectangle changed.
    pub fn sync_texture(&mut self, ctx: &egui::Context) -> WhiteboardTextureUpload {
        let (cw, ch) = (self.canvas_size.0 as usize, self.canvas_size.1 as usize);
        if cw == 0 || ch == 0 || self.canvas_rgba.len() != cw * ch * 4 {
            return WhiteboardTextureUpload::None;
        }

        let needs_full = self.texture_full_upload_needed
            || self
                .texture
                .as_ref()
                .map_or(true, |tex| tex.size() != [cw, ch]);

        if needs_full {
            let image = egui::ColorImage::from_rgba_unmultiplied([cw, ch], &self.canvas_rgba);
            if let Some(tex) = &mut self.texture {
                tex.set(image, TextureOptions::NEAREST);
            } else {
                self.texture = Some(ctx.load_texture(
                    "whiteboard_canvas_texture",
                    image,
                    TextureOptions::NEAREST,
                ));
            }
            self.texture_dirty = false;
            self.texture_full_upload_needed = false;
            self.dirty_rect = None;
            return WhiteboardTextureUpload::Full {
                width: cw,
                height: ch,
            };
        }

        if !self.texture_dirty {
            return WhiteboardTextureUpload::None;
        }

        if let Some([min_x, min_y, max_x, max_y]) = self.dirty_rect.take() {
            let max_x = max_x.min(cw - 1);
            let max_y = max_y.min(ch - 1);
            if min_x <= max_x && min_y <= max_y {
                let sub_w = max_x - min_x + 1;
                let sub_h = max_y - min_y + 1;
                let row_stride = cw * 4;
                let sub_image = if min_x == 0 && sub_w == cw {
                    let slice = &self.canvas_rgba[min_y * row_stride..(max_y + 1) * row_stride];
                    egui::ColorImage::from_rgba_unmultiplied([sub_w, sub_h], slice)
                } else {
                    let sub_row_bytes = sub_w * 4;
                    let mut sub_rgba = vec![0u8; sub_w * sub_h * 4];
                    for row in 0..sub_h {
                        let src_start = (min_y + row) * row_stride + min_x * 4;
                        let dst_start = row * sub_row_bytes;
                        sub_rgba[dst_start..dst_start + sub_row_bytes].copy_from_slice(
                            &self.canvas_rgba[src_start..src_start + sub_row_bytes],
                        );
                    }
                    egui::ColorImage::from_rgba_unmultiplied([sub_w, sub_h], &sub_rgba)
                };
                if let Some(tex) = &mut self.texture {
                    tex.set_partial([min_x, min_y], sub_image, TextureOptions::NEAREST);
                }
                self.texture_dirty = false;
                return WhiteboardTextureUpload::Partial {
                    x: min_x,
                    y: min_y,
                    width: sub_w,
                    height: sub_h,
                };
            }
        }

        // Fallback if `texture_dirty` was set externally without `dirty_rect`.
        let image = egui::ColorImage::from_rgba_unmultiplied([cw, ch], &self.canvas_rgba);
        if let Some(tex) = &mut self.texture {
            tex.set(image, TextureOptions::NEAREST);
        }
        self.texture_dirty = false;
        WhiteboardTextureUpload::Full {
            width: cw,
            height: ch,
        }
    }
}

/// Returns true if `pos` is currently occluded by a UI layer above the whiteboard
/// (Webcam overlay, Twitch chat overlay, Debug window, Video controls, or system modal dialogs).
fn is_pointer_occluded_by_higher_ui(
    state: &AppState,
    ctx: &egui::Context,
    pos: Pos2,
    whiteboard_layer_id: egui::LayerId,
) -> bool {
    if state.ui.show_quit_dialog
        || state.ui.show_stop_stream_dialog
        || state.ui.show_first_run_dialog
        || (state.config_load_error.is_some() && !state.ui.dismissed_config_error)
    {
        return true;
    }

    if state.twitch.overlay_contains(pos) {
        return true;
    }

    if state.webcam.config.enabled
        && (state.webcam.is_dragging
            || state.webcam.resizing_corner.is_some()
            || state
                .webcam
                .last_outer_rect
                .is_some_and(|r| r.contains(pos)))
    {
        return true;
    }

    if let Some(top_layer) = ctx.layer_id_at(pos) {
        if top_layer != whiteboard_layer_id && top_layer.order > whiteboard_layer_id.order {
            return true;
        }
    }

    false
}

/// Renders the full-screen transparent whiteboard overlay onto `egui::Order::PanelResizeLine`.
pub fn draw_whiteboard(state: &mut AppState, ctx: &egui::Context, screen_rect: Rect) {
    if !state.whiteboard.open || screen_rect.width() <= 1.0 || screen_rect.height() <= 1.0 {
        return;
    }

    // Handle Ctrl+Z (Undo) and Ctrl+Y (Redo) when the whiteboard is open and not typing in a text field
    if !ctx.wants_keyboard_input() {
        let undo_pressed = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z));
        if undo_pressed && state.whiteboard.undo() {
            ctx.request_repaint();
        }
        let redo_pressed = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y));
        if redo_pressed && state.whiteboard.redo() {
            ctx.request_repaint();
        }
    }

    // Ensure backing pixel buffer covers the full screen dimensions.
    let req_w = (screen_rect.width().ceil() as u32).max(1);
    let req_h = (screen_rect.height().ceil() as u32).max(1);
    state.whiteboard.ensure_canvas_size(req_w, req_h);

    let layer_id = egui::LayerId::new(
        egui::Order::PanelResizeLine,
        egui::Id::new("whiteboard_overlay"),
    );

    let pointer_pos = ctx.input(|i| i.pointer.latest_pos());
    let primary_pressed = ctx.input(|i| i.pointer.primary_pressed());
    let primary_down = ctx.input(|i| i.pointer.primary_down());
    let pointer_occluded =
        pointer_pos.is_some_and(|p| is_pointer_occluded_by_higher_ui(state, ctx, p, layer_id));

    let panel_rect = screen_rect;
    let is_mining_capture = state.bank.has_pending();

    egui::Area::new(layer_id.id)
        .order(layer_id.order)
        .fixed_pos(panel_rect.min)
        .constrain(false)
        .movable(false)
        .show(ctx, |ui| {
            ui.set_clip_rect(panel_rect);
            ui.set_min_size(panel_rect.size());
            ui.set_max_size(panel_rect.size());

            // --- Top Toolbar (hidden on the single frame when a mining screenshot is captured) ---
            let toolbar_rect = if is_mining_capture {
                Rect::from_min_size(
                    panel_rect.min,
                    Vec2::new(panel_rect.width(), state.whiteboard.last_toolbar_height),
                )
            } else {
                let toolbar_response = egui::Frame::none()
                    .fill(Color32::from_rgb(24, 24, 24))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(56, 56, 56)))
                    .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                    .rounding(Rounding::ZERO)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);

                            // Tool selector: Pen / Square / Eraser
                            for (tool_variant, label, tip) in [
                                (WhiteboardTool::Pen, "Pen", "Freehand pixel pen"),
                                (
                                    WhiteboardTool::Square,
                                    "Square",
                                    "Drag to draw a solid filled rectangle (no border)",
                                ),
                                (WhiteboardTool::Eraser, "Eraser", "Erase pixels back to transparent"),
                            ] {
                                let active = state.whiteboard.tool == tool_variant;
                                let btn = egui::Button::new(
                                    egui::RichText::new(label)
                                        .monospace()
                                        .size(11.0)
                                        .strong()
                                        .color(if active {
                                            Color32::WHITE
                                        } else {
                                            Color32::from_rgb(190, 190, 190)
                                        }),
                                )
                                .fill(if active {
                                    Color32::from_rgb(21, 60, 102)
                                } else {
                                    Color32::from_rgb(44, 44, 44)
                                })
                                .stroke(Stroke::new(
                                    1.0,
                                    if active {
                                        Color32::from_rgb(66, 133, 244)
                                    } else {
                                        Color32::from_rgb(72, 72, 72)
                                    },
                                ))
                                .rounding(Rounding::ZERO);
                                if ui.add(btn).on_hover_text(tip).clicked() {
                                    state.whiteboard.tool = tool_variant;
                                }
                            }

                            ui.separator();

                            // Color palette: Green, Red, Blue, Yellow, White, Black
                            for palette_color in WhiteboardColor::ALL {
                                let is_selected = state.whiteboard.color == palette_color
                                    && state.whiteboard.tool != WhiteboardTool::Eraser;
                                let (swatch_rect, swatch_resp) =
                                    ui.allocate_exact_size(Vec2::new(18.0, 18.0), Sense::click());
                                if swatch_resp.clicked() {
                                    state.whiteboard.color = palette_color;
                                    if state.whiteboard.tool == WhiteboardTool::Eraser {
                                        state.whiteboard.tool = WhiteboardTool::Pen;
                                    }
                                }
                                swatch_resp.on_hover_text(palette_color.label());

                                let painter = ui.painter();
                                painter.rect_filled(
                                    swatch_rect,
                                    Rounding::ZERO,
                                    palette_color.to_color32(),
                                );
                                let border_stroke = if is_selected {
                                    Stroke::new(2.0, Color32::from_rgb(80, 180, 255))
                                } else if swatch_rect
                                    .contains(pointer_pos.unwrap_or(Pos2::new(-999.0, -999.0)))
                                {
                                    Stroke::new(1.5, Color32::WHITE)
                                } else {
                                    Stroke::new(1.0, Color32::from_rgb(90, 90, 90))
                                };
                                painter.rect_stroke(swatch_rect, Rounding::ZERO, border_stroke);
                            }

                            ui.separator();

                            // Size slider for Pen / Eraser
                            ui.label(
                                egui::RichText::new("Size:")
                                    .monospace()
                                    .size(11.0)
                                    .color(Color32::from_rgb(190, 190, 190)),
                            );
                            ui.scope(|ui| {
                                ui.spacing_mut().slider_width = 80.0;
                                ui.add(
                                    egui::Slider::new(
                                        &mut state.whiteboard.brush_size,
                                        MIN_BRUSH_SIZE..=MAX_BRUSH_SIZE,
                                    )
                                    .suffix("px"),
                                );
                            });

                            ui.separator();

                            // Undo (Ctrl+Z) & Redo (Ctrl+Y) buttons
                            let can_undo = !state.whiteboard.undo_stack.is_empty();
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("Undo")
                                    .monospace()
                                    .size(11.0)
                                    .color(if can_undo {
                                        Color32::from_rgb(208, 208, 208)
                                    } else {
                                        Color32::from_rgb(100, 100, 100)
                                    }),
                            )
                            .rounding(Rounding::ZERO);
                            if ui
                                .add_enabled(can_undo, undo_btn)
                                .on_hover_text("Undo (Ctrl+Z)")
                                .clicked()
                            {
                                state.whiteboard.undo();
                            }

                            let can_redo = !state.whiteboard.redo_stack.is_empty();
                            let redo_btn = egui::Button::new(
                                egui::RichText::new("Redo")
                                    .monospace()
                                    .size(11.0)
                                    .color(if can_redo {
                                        Color32::from_rgb(208, 208, 208)
                                    } else {
                                        Color32::from_rgb(100, 100, 100)
                                    }),
                            )
                            .rounding(Rounding::ZERO);
                            if ui
                                .add_enabled(can_redo, redo_btn)
                                .on_hover_text("Redo (Ctrl+Y)")
                                .clicked()
                            {
                                state.whiteboard.redo();
                            }

                            // Clear canvas button
                            let clear_btn = egui::Button::new(
                                egui::RichText::new("Clear")
                                    .monospace()
                                    .size(11.0)
                                    .color(Color32::from_rgb(230, 200, 200)),
                            )
                            .fill(Color32::from_rgb(56, 28, 28))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(96, 48, 48)))
                            .rounding(Rounding::ZERO);
                            if ui
                                .add(clear_btn)
                                .on_hover_text("Clear all drawings from the whiteboard")
                                .clicked()
                            {
                                state.whiteboard.clear_canvas();
                            }

                            ui.separator();

                            // + and ++ mining buttons for the top-most entry in the open dictionary popup
                            let popup_mine_info = crate::ocr::overlay::top_popup_entry_mine_info(state);
                            let can_mine = matches!(
                                popup_mine_info,
                                Some((crate::bank::MineStatus::Available, _))
                            );
                            let is_already_mined = matches!(
                                popup_mine_info,
                                Some((crate::bank::MineStatus::Mined, _))
                            );

                            let (mine_hover, clip_hover) = match &popup_mine_info {
                                Some((crate::bank::MineStatus::Available, term)) => (
                                    format!("Add '{term}' to mining bank (+)"),
                                    format!(
                                        "Add '{term}' to mining bank and create 20s Twitch clip (++)"
                                    ),
                                ),
                                Some((crate::bank::MineStatus::Pending, _)) => (
                                    "Saving…".to_string(),
                                    "Saving…".to_string(),
                                ),
                                Some((crate::bank::MineStatus::Mined, term)) => (
                                    format!("'{term}' is already in the mining bank"),
                                    format!("'{term}' is already in the mining bank"),
                                ),
                                None => (
                                    "No OCR dictionary popup is currently open".to_string(),
                                    "No OCR dictionary popup is currently open".to_string(),
                                ),
                            };

                            let mine_btn = egui::Button::new(
                                egui::RichText::new("+")
                                    .monospace()
                                    .size(11.0)
                                    .strong()
                                    .color(if can_mine {
                                        Color32::from_rgb(248, 250, 252)
                                    } else if is_already_mined {
                                        Color32::from_rgb(94, 234, 212)
                                    } else {
                                        Color32::from_rgb(100, 100, 100)
                                    }),
                            )
                            .fill(if can_mine {
                                Color32::from_rgb(14, 116, 144)
                            } else if is_already_mined {
                                Color32::from_rgb(19, 78, 74)
                            } else {
                                Color32::from_rgb(36, 36, 36)
                            })
                            .stroke(Stroke::new(
                                1.0,
                                if can_mine {
                                    Color32::from_rgb(56, 189, 248)
                                } else if is_already_mined {
                                    Color32::from_rgb(20, 184, 166)
                                } else {
                                    Color32::from_rgb(64, 64, 64)
                                },
                            ))
                            .rounding(Rounding::ZERO);

                            if ui
                                .add_enabled(can_mine, mine_btn)
                                .on_hover_text(&mine_hover)
                                .on_disabled_hover_text(&mine_hover)
                                .clicked()
                            {
                                crate::ocr::overlay::execute_popup_mine_action(
                                    state, ctx, 0, false,
                                );
                            }

                            if state.steam_deck.is_none() {
                            let clip_btn = egui::Button::new(
                                egui::RichText::new("++")
                                    .monospace()
                                    .size(11.0)
                                    .strong()
                                    .color(if can_mine {
                                        Color32::from_rgb(248, 250, 252)
                                    } else if is_already_mined {
                                        Color32::from_rgb(94, 234, 212)
                                    } else {
                                        Color32::from_rgb(100, 100, 100)
                                    }),
                            )
                            .fill(if can_mine {
                                Color32::from_rgb(107, 33, 168)
                            } else if is_already_mined {
                                Color32::from_rgb(19, 78, 74)
                            } else {
                                Color32::from_rgb(36, 36, 36)
                            })
                            .stroke(Stroke::new(
                                1.0,
                                if can_mine {
                                    Color32::from_rgb(168, 85, 247)
                                } else if is_already_mined {
                                    Color32::from_rgb(20, 184, 166)
                                } else {
                                    Color32::from_rgb(64, 64, 64)
                                },
                            ))
                            .rounding(Rounding::ZERO);

                            if ui
                                .add_enabled(can_mine, clip_btn)
                                .on_hover_text(&clip_hover)
                                .on_disabled_hover_text(&clip_hover)
                                .clicked()
                            {
                                crate::ocr::overlay::execute_popup_mine_action(
                                    state, ctx, 0, true,
                                );
                            }
                            }
                        });
                    });

                let rect = toolbar_response.response.rect;
                state.whiteboard.last_toolbar_height = rect.height();
                rect
            };

            let canvas_rect = Rect::from_min_max(
                Pos2::new(panel_rect.min.x, toolbar_rect.max.y),
                panel_rect.max,
            );

            // Reserve interaction over the canvas on Order::PanelResizeLine so clicks/double-clicks
            // do not pass through to CentralPanel (Order::Background) underneath.
            if !pointer_occluded && canvas_rect.is_positive() {
                ui.interact(
                    canvas_rect,
                    ui.id().with("whiteboard_canvas_surface"),
                    Sense::click_and_drag(),
                );
            }

            // --- Drawing, Square Shape, & Erasing Input Handling ---
            let can_draw_in_canvas = state.whiteboard.open && !pointer_occluded;

            match state.whiteboard.tool {
                WhiteboardTool::Pen | WhiteboardTool::Eraser => {
                    state.whiteboard.square_start = None;
                    state.whiteboard.square_current = None;

                    if primary_pressed && can_draw_in_canvas {
                        if let Some(pos) = pointer_pos {
                            if canvas_rect.contains(pos) && !toolbar_rect.contains(pos) {
                                state.whiteboard.save_undo_snapshot();
                                state.whiteboard.is_drawing = true;
                                state.whiteboard.last_draw_pos = None;
                            }
                        }
                    }

                    if state.whiteboard.is_drawing {
                        if primary_down && !pointer_occluded {
                            let mut sample_positions: Vec<Pos2> = ctx.input(|i| {
                                i.events
                                    .iter()
                                    .filter_map(|ev| match ev {
                                        egui::Event::PointerMoved(p) => Some(*p),
                                        egui::Event::PointerButton {
                                            pos,
                                            button: egui::PointerButton::Primary,
                                            pressed: true,
                                            ..
                                        } => Some(*pos),
                                        _ => None,
                                    })
                                    .collect()
                            });
                            if sample_positions.is_empty() {
                                if let Some(pos) = pointer_pos {
                                    sample_positions.push(pos);
                                }
                            }

                            for pos in sample_positions {
                                let px = (pos.x - panel_rect.min.x).floor() as i32;
                                let py = (pos.y - panel_rect.min.y).floor() as i32;
                                state.whiteboard.apply_pointer_sample(px, py);
                            }
                        } else {
                            state.whiteboard.is_drawing = false;
                            state.whiteboard.last_draw_pos = None;
                            state.whiteboard.seal_active_patch();
                        }
                    }
                }
                WhiteboardTool::Square => {
                    state.whiteboard.is_drawing = false;
                    state.whiteboard.last_draw_pos = None;
                    state.whiteboard.seal_active_patch();

                    if primary_pressed && can_draw_in_canvas {
                        if let Some(pos) = pointer_pos {
                            if canvas_rect.contains(pos) && !toolbar_rect.contains(pos) {
                                let px = (pos.x - panel_rect.min.x).floor() as i32;
                                let py = (pos.y - panel_rect.min.y).floor() as i32;
                                state.whiteboard.square_start = Some((px, py));
                                state.whiteboard.square_current = Some((px, py));
                            }
                        }
                    }

                    if state.whiteboard.square_start.is_some() {
                        if primary_down && !pointer_occluded {
                            if let Some(pos) = pointer_pos {
                                let px = (pos.x - panel_rect.min.x).floor() as i32;
                                let py = (pos.y - panel_rect.min.y)
                                    .max(canvas_rect.min.y - panel_rect.min.y)
                                    .floor() as i32;
                                state.whiteboard.square_current = Some((px, py));
                            }
                        } else if let (Some(start), Some(end)) = (
                            state.whiteboard.square_start.take(),
                            state.whiteboard.square_current.take(),
                        ) {
                            state.whiteboard.save_undo_snapshot();
                            state.whiteboard.fill_rect(start, end);
                            state.whiteboard.seal_active_patch();
                        }
                    }
                }
            }

            // Upload modified dirty-rect (or full canvas on create/resize) to nearest-neighbor GPU texture
            let _ = state.whiteboard.sync_texture(ctx);

            // --- Paint Transparent Canvas Texture and Previews ---
            let painter = ui.painter().with_clip_rect(panel_rect);

            // Draw nearest-neighbor pixel canvas anchored to panel_rect.min
            if let Some(tex) = &state.whiteboard.texture {
                let (cw, ch) = state.whiteboard.canvas_size;
                if cw > 0 && ch > 0 && canvas_rect.width() > 0.0 && canvas_rect.height() > 0.0 {
                    let y0 = (canvas_rect.min.y - panel_rect.min.y).max(0.0);
                    let y1 = (canvas_rect.max.y - panel_rect.min.y).max(y0);
                    let uv = Rect::from_min_max(
                        Pos2::new(0.0, y0 / ch as f32),
                        Pos2::new(canvas_rect.width() / cw as f32, y1 / ch as f32),
                    );
                    painter
                        .with_clip_rect(canvas_rect)
                        .image(tex.id(), canvas_rect, uv, Color32::WHITE);
                }
            }

            // Live filled rectangle preview (no border) while dragging in Square mode
            if let (Some(start), Some(curr)) =
                (state.whiteboard.square_start, state.whiteboard.square_current)
            {
                let x_min = panel_rect.min.x + start.0.min(curr.0) as f32;
                let x_max = panel_rect.min.x + (start.0.max(curr.0) + 1) as f32;
                let y_min = panel_rect.min.y + start.1.min(curr.1) as f32;
                let y_max = panel_rect.min.y + (start.1.max(curr.1) + 1) as f32;
                let preview_rect =
                    Rect::from_min_max(Pos2::new(x_min, y_min), Pos2::new(x_max, y_max));
                painter.with_clip_rect(canvas_rect).rect_filled(
                    preview_rect,
                    Rounding::ZERO,
                    state.whiteboard.color.to_color32(),
                );
            }

            // Draw brush size outline preview under cursor when hovering canvas in Pen/Eraser mode
            if !is_mining_capture
                && can_draw_in_canvas
                && state.whiteboard.tool != WhiteboardTool::Square
            {
                if let Some(pos) = pointer_pos {
                    if canvas_rect.contains(pos) && !toolbar_rect.contains(pos) {
                        let radius = (state.whiteboard.brush_size as f32 * 0.5).max(1.0);
                        painter.with_clip_rect(canvas_rect).circle_stroke(
                            pos,
                            radius,
                            Stroke::new(1.0, Color32::from_rgba_unmultiplied(200, 200, 200, 180)),
                        );
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whiteboard_defaults_and_toggle() {
        let mut wb = WhiteboardState::default();
        assert!(!wb.open);
        assert!(!wb.is_visible());
        assert_eq!(wb.tool, WhiteboardTool::Pen);
        assert_eq!(wb.color, WhiteboardColor::Black);
        assert_eq!(wb.brush_size, DEFAULT_BRUSH_SIZE);
        assert!(wb.undo_stack.is_empty());
        assert!(wb.redo_stack.is_empty());

        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0));
        assert!(!wb.contains_pointer(Some(Pos2::new(500.0, 400.0)), screen));

        wb.toggle();
        assert!(wb.open);
        assert!(wb.is_visible());
        assert!(wb.contains_pointer(Some(Pos2::new(500.0, 400.0)), screen));

        wb.toggle();
        assert!(!wb.open);
        assert!(!wb.is_visible());
    }

    #[test]
    fn test_unaliased_drawing_erasing_and_square_fill() {
        let mut wb = WhiteboardState::default();
        wb.ensure_canvas_size(100, 100);
        wb.brush_size = 5;

        // 1. Draw with Red pen
        wb.tool = WhiteboardTool::Pen;
        wb.color = WhiteboardColor::Red;
        wb.draw_stroke_line((10, 10), (20, 10));

        let mut painted_count = 0;
        for px in wb.canvas_rgba.chunks_exact(4) {
            if px[3] != 0 {
                assert_eq!(px, &WhiteboardColor::Red.to_rgba());
                painted_count += 1;
            } else {
                assert_eq!(px, &[0, 0, 0, 0]);
            }
        }
        assert!(painted_count > 0);

        // 2. Draw filled Square (no border) with Yellow
        wb.clear_canvas();
        wb.tool = WhiteboardTool::Square;
        wb.color = WhiteboardColor::Yellow;
        wb.fill_rect((30, 40), (35, 44)); // 6x5 = 30 pixels

        let mut yellow_count = 0;
        for y in 0..100 {
            for x in 0..100 {
                let idx = ((y * 100 + x) * 4) as usize;
                let px = &wb.canvas_rgba[idx..idx + 4];
                if (30..=35).contains(&x) && (40..=44).contains(&y) {
                    assert_eq!(px, &WhiteboardColor::Yellow.to_rgba());
                    yellow_count += 1;
                } else {
                    assert_eq!(px, &[0, 0, 0, 0]);
                }
            }
        }
        assert_eq!(yellow_count, 30);

        // 3. Erase inside the square -> resets pixels back to transparent [0, 0, 0, 0]
        wb.tool = WhiteboardTool::Eraser;
        wb.brush_size = 1;
        wb.stamp_brush(32, 42);
        let erased_idx = ((42 * 100 + 32) * 4) as usize;
        assert_eq!(&wb.canvas_rgba[erased_idx..erased_idx + 4], &[0, 0, 0, 0]);
    }

    #[test]
    fn test_undo_and_redo_buffer() {
        let mut wb = WhiteboardState::default();
        wb.ensure_canvas_size(32, 32);
        wb.brush_size = 1;
        wb.tool = WhiteboardTool::Pen;

        // Operation 1: Paint pixel (5, 5) Green
        wb.save_undo_snapshot();
        wb.color = WhiteboardColor::Green;
        wb.stamp_brush(5, 5);
        let idx1 = ((5 * 32 + 5) * 4) as usize;
        assert_eq!(
            &wb.canvas_rgba[idx1..idx1 + 4],
            &WhiteboardColor::Green.to_rgba()
        );

        // Operation 2: Fill square (10, 10)..=(12, 12) Blue
        wb.save_undo_snapshot();
        wb.color = WhiteboardColor::Blue;
        wb.fill_rect((10, 10), (12, 12));
        let idx2 = ((11 * 32 + 11) * 4) as usize;
        assert_eq!(
            &wb.canvas_rgba[idx2..idx2 + 4],
            &WhiteboardColor::Blue.to_rgba()
        );

        // Ctrl+Z (undo #1) -> removes Blue square, keeps Green pixel
        assert!(wb.undo());
        assert_eq!(&wb.canvas_rgba[idx2..idx2 + 4], &[0, 0, 0, 0]);
        assert_eq!(
            &wb.canvas_rgba[idx1..idx1 + 4],
            &WhiteboardColor::Green.to_rgba()
        );

        // Ctrl+Z (undo #2) -> removes Green pixel, canvas is completely empty
        assert!(wb.undo());
        assert_eq!(&wb.canvas_rgba[idx1..idx1 + 4], &[0, 0, 0, 0]);
        assert!(!wb.undo()); // nothing left to undo

        // Ctrl+R (redo #1) -> restores Green pixel
        assert!(wb.redo());
        assert_eq!(
            &wb.canvas_rgba[idx1..idx1 + 4],
            &WhiteboardColor::Green.to_rgba()
        );
        assert_eq!(&wb.canvas_rgba[idx2..idx2 + 4], &[0, 0, 0, 0]);

        // Ctrl+R (redo #2) -> restores Blue square
        assert!(wb.redo());
        assert_eq!(
            &wb.canvas_rgba[idx2..idx2 + 4],
            &WhiteboardColor::Blue.to_rgba()
        );
        assert!(!wb.redo()); // nothing left to redo
    }

    #[test]
    fn test_canvas_resize_preserves_existing_drawing_and_undo_stack() {
        let mut wb = WhiteboardState::default();
        wb.ensure_canvas_size(64, 64);
        wb.brush_size = 1;
        wb.tool = WhiteboardTool::Pen;
        wb.color = WhiteboardColor::Green;
        wb.save_undo_snapshot();
        wb.stamp_brush(12, 34);

        // Transient 0x0 resize frames must be ignored without wiping pixels or history
        wb.ensure_canvas_size(0, 0);
        assert_eq!(wb.canvas_size, (64, 64));

        // Expand canvas to 128x128
        wb.ensure_canvas_size(128, 128);
        assert_eq!(wb.canvas_size, (128, 128));
        assert!(wb.texture_full_upload_needed);
        let idx = ((34 * 128 + 12) * 4) as usize;
        assert_eq!(
            &wb.canvas_rgba[idx..idx + 4],
            &WhiteboardColor::Green.to_rgba()
        );
        assert_eq!(wb.undo_stack.len(), 1);
        assert_eq!(
            (
                wb.undo_stack[0].x,
                wb.undo_stack[0].y,
                wb.undo_stack[0].w,
                wb.undo_stack[0].h,
            ),
            (12, 34, 1, 1)
        );
        assert!(wb.undo());
        assert_eq!(&wb.canvas_rgba[idx..idx + 4], &[0, 0, 0, 0]);
        assert!(wb.redo());
        assert_eq!(
            &wb.canvas_rgba[idx..idx + 4],
            &WhiteboardColor::Green.to_rgba()
        );
    }

    #[test]
    fn test_whiteboard_dirty_rect_and_patch_undo_redo_roundtrip() {
        let ctx = egui::Context::default();
        let mut wb = WhiteboardState::default();

        // 0-sized transient frame before first allocation must not allocate a 1x1 buffer
        wb.ensure_canvas_size(0, 0);
        assert_eq!(wb.canvas_size, (0, 0));
        assert!(wb.canvas_rgba.is_empty());

        wb.ensure_canvas_size(120, 90);
        let full_bytes = 120 * 90 * 4;
        let state_0 = wb.canvas_rgba.clone();

        // Initial texture sync uploads the full canvas; an immediate second sync is a no-op.
        assert_eq!(
            wb.sync_texture(&ctx),
            WhiteboardTextureUpload::Full {
                width: 120,
                height: 90
            }
        );
        assert_eq!(wb.sync_texture(&ctx), WhiteboardTextureUpload::None);

        // 1. Pen multi-segment stroke
        wb.tool = WhiteboardTool::Pen;
        wb.color = WhiteboardColor::Red;
        wb.brush_size = 5;
        wb.save_undo_snapshot();
        wb.apply_pointer_sample(20, 25);
        wb.apply_pointer_sample(45, 35);
        wb.apply_pointer_sample(50, 20);
        wb.last_draw_pos = None;
        wb.seal_active_patch();
        let state_1_pen = wb.canvas_rgba.clone();
        assert_ne!(state_1_pen, state_0);
        assert_eq!(wb.undo_stack.len(), 1);
        assert!(wb.undo_stack[0].before_rgba.len() < full_bytes / 4);
        assert_eq!(
            wb.sync_texture(&ctx),
            WhiteboardTextureUpload::Partial {
                x: 18,
                y: 18,
                width: 35,
                height: 20,
            }
        );

        // 2. Filled Square overlapping part of the Pen stroke
        wb.tool = WhiteboardTool::Square;
        wb.color = WhiteboardColor::Blue;
        wb.save_undo_snapshot();
        wb.fill_rect((30, 22), (60, 48));
        wb.seal_active_patch();
        let state_2_square = wb.canvas_rgba.clone();
        assert_ne!(state_2_square, state_1_pen);
        assert_eq!(
            wb.sync_texture(&ctx),
            WhiteboardTextureUpload::Partial {
                x: 30,
                y: 22,
                width: 31,
                height: 27,
            }
        );

        // 3. Eraser stroke cutting across both the Pen stroke and Square
        wb.tool = WhiteboardTool::Eraser;
        wb.brush_size = 4;
        wb.save_undo_snapshot();
        wb.draw_stroke_line((18, 26), (62, 40));
        wb.seal_active_patch();
        let state_3_eraser = wb.canvas_rgba.clone();
        assert_ne!(state_3_eraser, state_2_square);
        assert!(matches!(
            wb.sync_texture(&ctx),
            WhiteboardTextureUpload::Partial { .. }
        ));

        // 4. Clear canvas -> captures only tight non-zero bounding box, zeroes canvas
        wb.clear_canvas();
        let state_4_clear = wb.canvas_rgba.clone();
        assert_eq!(state_4_clear, state_0);
        assert_eq!(wb.undo_stack.len(), 4);
        assert!(wb.undo_stack[3].before_rgba.len() < full_bytes / 2);
        assert!(matches!(
            wb.sync_texture(&ctx),
            WhiteboardTextureUpload::Partial { .. }
        ));

        // Undo all 4 operations in reverse and assert exact pixel-for-pixel equivalence
        assert!(wb.undo());
        assert_eq!(wb.canvas_rgba, state_3_eraser);
        assert!(matches!(
            wb.sync_texture(&ctx),
            WhiteboardTextureUpload::Partial { .. }
        ));

        assert!(wb.undo());
        assert_eq!(wb.canvas_rgba, state_2_square);
        assert!(matches!(
            wb.sync_texture(&ctx),
            WhiteboardTextureUpload::Partial { .. }
        ));

        assert!(wb.undo());
        assert_eq!(wb.canvas_rgba, state_1_pen);
        assert!(matches!(
            wb.sync_texture(&ctx),
            WhiteboardTextureUpload::Partial { .. }
        ));

        assert!(wb.undo());
        assert_eq!(wb.canvas_rgba, state_0);
        assert!(matches!(
            wb.sync_texture(&ctx),
            WhiteboardTextureUpload::Partial { .. }
        ));
        assert!(!wb.undo());

        // Redo all 4 operations forward and assert exact pixel-for-pixel equivalence
        assert!(wb.redo());
        assert_eq!(wb.canvas_rgba, state_1_pen);
        assert!(matches!(
            wb.sync_texture(&ctx),
            WhiteboardTextureUpload::Partial { .. }
        ));

        assert!(wb.redo());
        assert_eq!(wb.canvas_rgba, state_2_square);
        assert!(matches!(
            wb.sync_texture(&ctx),
            WhiteboardTextureUpload::Partial { .. }
        ));

        assert!(wb.redo());
        assert_eq!(wb.canvas_rgba, state_3_eraser);
        assert!(matches!(
            wb.sync_texture(&ctx),
            WhiteboardTextureUpload::Partial { .. }
        ));

        assert!(wb.redo());
        assert_eq!(wb.canvas_rgba, state_4_clear);
        assert!(matches!(
            wb.sync_texture(&ctx),
            WhiteboardTextureUpload::Partial { .. }
        ));
        assert!(!wb.redo());

        // Verify MAX_UNDO_STEPS VecDeque eviction bound
        wb.tool = WhiteboardTool::Pen;
        wb.color = WhiteboardColor::Yellow;
        wb.brush_size = 1;
        for x in 0..(MAX_UNDO_STEPS + 8) as i32 {
            wb.save_undo_snapshot();
            wb.stamp_brush(x, 10);
            wb.seal_active_patch();
        }
        assert_eq!(wb.undo_stack.len(), MAX_UNDO_STEPS);
    }

    #[test]
    fn test_draw_whiteboard_renders_and_draws_on_panel_resize_line() {
        let mut state = AppState::default();
        state.whiteboard.open = true;
        state.whiteboard.color = WhiteboardColor::Blue;
        state.whiteboard.brush_size = 3;

        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 600.0));

        let mut raw_input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let draw_pos = Pos2::new(120.0, 200.0);
        raw_input.events.push(egui::Event::PointerMoved(draw_pos));
        raw_input.events.push(egui::Event::PointerButton {
            pos: draw_pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });

        let _ = ctx.run(raw_input, |ctx| {
            draw_whiteboard(&mut state, ctx, screen);
        });

        assert!(state.whiteboard.is_drawing);
        assert_eq!(state.whiteboard.undo_stack.len(), 1);
        let idx = ((200 * 1000 + 120) * 4) as usize;
        assert_eq!(
            &state.whiteboard.canvas_rgba[idx..idx + 4],
            &WhiteboardColor::Blue.to_rgba()
        );
    }

    #[test]
    fn test_whiteboard_toolbar_mines_top_popup_entry() {
        let mut state = AppState::default();
        state.whiteboard.open = true;

        // When no popup is open, top_popup_entry_mine_info returns None
        assert!(crate::ocr::overlay::top_popup_entry_mine_info(&state).is_none());

        let now = std::time::Instant::now();
        let entry1 = crate::dict::models::TermEntry {
            term: "魔法".to_string(),
            reading: "まほう".to_string(),
            definition_tags: None,
            rules: String::new(),
            score: 100.0,
            glossary: Vec::new(),
            sequence: 1,
            term_tags: None,
            inflection_reasons: Vec::new(),
            frequency: None,
            pitches: Vec::new(),
            custom_name_id: None,
            custom_tag: None,
        };
        let entry2 = crate::dict::models::TermEntry {
            term: "魔".to_string(),
            reading: "ま".to_string(),
            ..entry1.clone()
        };

        state.dict.popup = Some(crate::dict::models::DictPopupState {
            matched_term: "魔法".to_string(),
            source_text: "これは魔法の力だ。".to_string(),
            char_range: (3, 5),
            word_rect: Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(40.0, 20.0)),
            extra_word_rects: Vec::new(),
            box_rect: Rect::from_min_size(Pos2::new(80.0, 90.0), Vec2::new(200.0, 40.0)),
            entries: vec![entry1, entry2],
            last_hover_time: now,
            last_word_hover_time: now,
            popup_rect: None,
            is_popup_hovered: false,
            child: None,
        });

        let info = crate::ocr::overlay::top_popup_entry_mine_info(&state);
        assert_eq!(
            info,
            Some((crate::bank::MineStatus::Available, "魔法".to_string()))
        );

        let ctx = egui::Context::default();
        assert!(crate::ocr::overlay::execute_popup_mine_action(
            &mut state, &ctx, 0, false
        ));
        assert!(state.bank.has_pending());
        assert_eq!(
            crate::ocr::overlay::top_popup_entry_mine_info(&state),
            Some((crate::bank::MineStatus::Pending, "魔法".to_string()))
        );
        // Popup stays open in state
        assert!(state.dict.popup.is_some());
    }
}
