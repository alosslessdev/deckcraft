//! The Record view's slide canvas: live ink, the laser dot, and the movable Cameo bubble.
//!
//! The slide picture itself is drawn by the show's renderer into [`Layout::slide`]; this module
//! paints what sits on top of it while recording. Strokes and laser samples are captured into the
//! show's recording by [`Show::stash_ink`]/[`Show::stash_laser`] when the slide changes or the
//! recording stops, so the deck's timing writer can read them back.

use std::sync::Arc;

use deckcraft_model::Presentation;
use egui::{Color32, Pos2, Rect, Stroke, Ui};

use super::super::Show;
use super::bottom::INK_COLORS;
use super::cameo_input;
use super::ink;
use super::laser;
use super::{InkTool, Layout, RecordStatus, RecordView};

/// Screen radius of the eraser's tip.
const ERASER_PX: f32 = 14.0;

/// Draw the ink, laser and Cameo layers over the slide. Called after the slide texture and after
/// the shell chrome, but every layer is clipped to [`Layout::slide`] so chrome is never covered.
pub(super) fn ui(ui: &mut Ui, show: &mut Show, rec: &mut RecordView, doc: &Arc<Presentation>, layout: &Layout, now: f64) {
    let slide = layout.slide;
    if slide.width() < 2.0 || slide.height() < 2.0 {
        return;
    }
    let pointer = ui.ctx().input(|i| i.pointer.interact_pos());
    let over_slide = pointer.filter(|p| slide.contains(*p));
    erase(ui, show, rec, slide, over_slide);
    draw_ink(ui, show, rec, doc, slide);
    draw_laser(ui, show, rec, doc, slide, over_slide, now);
    bubble(ui, rec, slide);
}

/// While the eraser is selected, dragging over the slide removes whole strokes it touches.
fn erase(ui: &Ui, show: &mut Show, rec: &RecordView, slide: Rect, pointer: Option<Pos2>) {
    if rec.ink.tool != InkTool::Eraser || !show.pen {
        return;
    }
    let Some(p) = pointer.filter(|p| !rec.cameo.hits(slide, *p)) else { return };
    let down = ui.ctx().input(|i| i.pointer.primary_down());
    if !down {
        return;
    }
    let Some(norm) = ink::to_slide(slide, p) else { return };
    let radius = (ERASER_PX / slide.width().min(slide.height()).max(1.0)).clamp(0.002, 0.25);
    if ink::erase_at(&mut show.strokes, norm, radius) {
        ui.ctx().request_repaint();
    }
}

/// The selected pen colour, falling back to the deck's pen colour when the index is stale.
fn ink_color(rec: &RecordView, doc: &Presentation) -> Color32 {
    INK_COLORS.get(rec.ink.color).copied().unwrap_or_else(|| crate::theme::to_color32(doc.show.pen_color))
}

fn draw_ink(ui: &Ui, show: &Show, rec: &RecordView, doc: &Presentation, slide: Rect) {
    if show.strokes.is_empty() {
        return;
    }
    let highlighter = rec.ink.tool == InkTool::Highlighter;
    ink::draw_strokes(ui.painter(), slide, &show.strokes, ink_color(rec, doc), highlighter);
}

/// Draw the laser dot and, while recording, keep a throttled trace of it in slide points.
fn draw_laser(ui: &Ui, show: &mut Show, rec: &RecordView, doc: &Presentation, slide: Rect, pointer: Option<Pos2>, now: f64) {
    if !show.laser {
        return;
    }
    let color = crate::theme::to_color32(doc.show.laser_color);
    laser::draw(ui.painter(), slide, pointer, color);
    if rec.status != RecordStatus::Recording {
        return;
    }
    let Some(p) = pointer else { return };
    let Some(norm) = laser::screen_to_slide(slide, p) else { return };
    let ms = (now - show.slide_start).max(0.0) * 1000.0;
    if ms.is_finite() {
        laser::note(&mut show.laser_live, norm.x as f64 * doc.slide_size.width, norm.y as f64 * doc.slide_size.height, ms.min(3_600_000.0) as u32);
        ui.ctx().request_repaint();
    }
}

/// The live Cameo bubble: circular, bottom-right by default, draggable and resizable, clamped to
/// the slide. Without a camera frame it is an honest camera-off disc, never invented video.
fn bubble(ui: &mut Ui, rec: &mut RecordView, slide: Rect) {
    if !rec.camera_bubble {
        return;
    }
    cameo_input::interact(&mut rec.cameo, ui, slide, &mut rec.cameo_grab);
    rec.cameo.clamp(slide);
    let rect = rec.cameo.rect(slide);
    let ring = if ui.ctx().input(|i| i.pointer.any_down()) { Color32::from_gray(230) } else { Color32::from_white_alpha(190) };
    super::cameo::paint(ui.painter(), rect, rec.cameo_frame.as_ref(), ring, "Camera off");
    if rect.width() >= 24.0 {
        // A small resize handle at the circle's bottom-right, matching the corner drafters in the
        // rest of the app.
        let h = rec.cameo.handle_rect(slide).center();
        ui.painter().circle_filled(h, 5.0, Color32::from_white_alpha(220));
        ui.painter().circle_stroke(h, 5.0, Stroke::new(1.0, Color32::from_black_alpha(160)));
    }
}

/// Whether `pos` is over the Cameo bubble or its handle, so a click there must not advance the
/// show or start an ink stroke.
pub(super) fn bubble_hit(rec: &RecordView, layout: &Layout, pos: Pos2) -> bool {
    rec.camera_bubble && rec.cameo.hits(layout.slide, pos)
}

/// Reusable geometry for a camera backend: the bubble's circle in screen points.
pub(super) fn bubble_rect(rec: &RecordView, layout: &Layout) -> Rect {
    rec.cameo.rect(layout.slide)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use egui::{Rect, pos2, vec2};

    fn slide() -> Rect {
        Rect::from_min_size(pos2(64.0, 120.0), vec2(1600.0, 900.0))
    }

    #[test]
    fn bubble_hit_is_false_when_the_bubble_is_hidden() {
        let mut rec = RecordView::default();
        rec.camera_bubble = false;
        let layout = Layout {
            full: slide(),
            top: Rect::NOTHING,
            notes: Rect::NOTHING,
            slide: slide(),
            bottom: Rect::NOTHING,
            camera: Rect::NOTHING,
            views: Rect::NOTHING,
            camera_preview: Rect::NOTHING,
            next: None,
        };
        assert!(!bubble_hit(&rec, &layout, bubble_rect(&rec, &layout).center()));
    }
}
