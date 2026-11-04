//! Slide View: the slide alone, with a thumbnail rail for jumping during a recording.
//!
//! The rail lists every slide top to bottom. It is the only chrome in this view besides the
//! shared bottom strip, so it stays quiet: a flat dark background, a one pixel separator against
//! the canvas, the thumbnails themselves with a slide-number badge underneath, and the theme's
//! accent on the slide the recording is on. Clicking a thumbnail navigates through the show's own
//! `apply` path, so the recording records the timing of the jump exactly as it would for a
//! keyboard or clicker next.

use std::sync::Arc;

use deckcraft_anim::ShowAction;
use deckcraft_model::Presentation;
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::super::Layout;
use crate::SlideApp;
use crate::show::Show;
use crate::theme;

/// The rail's own background, darker than the shell so the thumbnails read as cards.
const RAIL_BG: Color32 = Color32::from_rgb(23, 23, 27);
/// Unselected thumbnail outline.
const THUMB_BORDER: Color32 = Color32::from_gray(60);
/// Placeholder fill while a thumbnail render is queued.
const THUMB_PLACEHOLDER: Color32 = Color32::from_gray(40);
/// The separator on the rail's right edge, against the canvas.
const RAIL_SEPARATOR: Color32 = Color32::from_gray(44);

/// The rail's horizontal padding (10 px each side), used to size the thumbnails.
const RAIL_PAD: f32 = 20.0;
/// The narrowest a thumbnail may be drawn; below this the rail is not worth showing.
const MIN_RAIL_W: f32 = 40.0;
/// Room under every thumbnail for the slide-number badge.
const BADGE_H: f32 = 20.0;
/// New thumbnail renders started per frame; the cache owns the rest.
const RENDER_BUDGET: u32 = 2;

/// Accessibility label for a rail thumbnail. One-based, matching the slide numbers in the rest of
/// the UI.
pub(super) fn thumb_label(i: usize) -> String {
    format!("Slide {} thumbnail", i + 1)
}

/// The outline of a rail thumbnail: the theme accent, 2 px, on the slide being recorded; a thin
/// grey otherwise.
fn thumb_border(selected: bool, accent: Color32) -> Stroke {
    if selected { Stroke::new(2.0, accent) } else { Stroke::new(1.0, THUMB_BORDER) }
}

/// The thumbnail aspect from the deck's own slide size. A hostile or unreadable size falls back to
/// 16:9 rather than producing a NaN or towering rect.
fn thumb_aspect(doc: &Presentation) -> f32 {
    let (w, h) = (doc.slide_size.width, doc.slide_size.height);
    let aspect = h / w;
    if w > 0.0 && aspect.is_finite() && aspect > 0.0 { (aspect as f32).clamp(0.05, 8.0) } else { 9.0 / 16.0 }
}

/// Draw the Slide View's thumbnail rail. Returns true when a click moved the show to another
/// slide.
pub(super) fn ui(app: &mut SlideApp, ui: &mut Ui, show: &mut Show, doc: &Arc<Presentation>, now: f64, layout: &Layout) -> bool {
    let Some(rail) = layout.thumbs else { return false };
    if rail.width() < MIN_RAIL_W || rail.height() <= 0.0 {
        return false;
    }

    // The rail sits on the canvas: paint its background and the edge that separates the two.
    let painter = ui.painter();
    painter.rect_filled(rail, CornerRadius::ZERO, RAIL_BG);
    let edge = rail.max.x - 0.5;
    painter.line_segment([pos2(edge, rail.min.y), pos2(edge, rail.max.y)], Stroke::new(1.0, RAIL_SEPARATOR));

    let t = theme::Tokens::get(ui.ctx());
    let aspect = thumb_aspect(doc);
    let width_px = (rail.width() - RAIL_PAD).max(MIN_RAIL_W) as u32;

    let content = rail.shrink2(vec2(RAIL_PAD / 2.0, 8.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(egui::Align::Min)));
    let mut budget = RENDER_BUDGET;
    let mut changed = false;

    egui::ScrollArea::vertical()
        .id_salt("record_slide_thumbs")
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 6.0);
            for i in 0..doc.slides.len() {
                let selected = i == show.state.slide;
                let tw = (width_px as f32).min(ui.available_width()).max(MIN_RAIL_W);
                let th = (tw * aspect).max(1.0);
                let row_w = ui.available_width().max(tw);
                let (row, resp) = ui.allocate_exact_size(vec2(row_w, th + BADGE_H), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, thumb_label(i)));

                let img = Rect::from_min_size(pos2(row.center().x - tw / 2.0, row.min.y), vec2(tw, th));
                let tex = app.textures.thumb(ui.ctx(), doc, i, width_px, &mut budget);
                let p = ui.painter();
                if resp.hovered() && !selected {
                    p.rect_filled(img.expand(2.0), CornerRadius::same(3), Color32::from_gray(32));
                }
                match tex {
                    Some(tex) => {
                        p.image(tex.id(), img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        // The render is queued or over this frame's budget: a neutral card, never
                        // the previous slide's picture.
                        p.rect_filled(img, CornerRadius::same(2), THUMB_PLACEHOLDER);
                    }
                }
                p.rect_stroke(img, CornerRadius::same(2), thumb_border(selected, t.accent), StrokeKind::Outside);
                p.text(
                    pos2(row.center().x, img.max.y + 4.0),
                    Align2::CENTER_TOP,
                    format!("{}", i + 1),
                    theme::font(11.0),
                    if selected { t.text } else { t.text_dim },
                );

                if resp.clicked() && !selected {
                    changed |= show.apply(ShowAction::Slide(i), doc, now);
                }
            }
        });

    changed
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn thumb_label_numbers_slides_from_one() {
        assert_eq!(thumb_label(0), "Slide 1 thumbnail");
        assert_eq!(thumb_label(6), "Slide 7 thumbnail");
        assert_eq!(thumb_label(41), "Slide 42 thumbnail");
    }

    #[test]
    fn selected_thumbnail_takes_the_accent() {
        let accent = Color32::from_rgb(0xF2, 0x6B, 0x1D);
        let selected = thumb_border(true, accent);
        assert_eq!(selected.color, accent);
        assert_eq!(selected.width, 2.0);
    }

    #[test]
    fn rest_of_the_rail_stays_grey() {
        let border = thumb_border(false, Color32::from_rgb(0xF2, 0x6B, 0x1D));
        assert_eq!(border.color, THUMB_BORDER);
        assert_eq!(border.width, 1.0);
    }

    #[test]
    fn aspect_follows_the_deck_and_falls_back_on_bad_sizes() {
        let mut doc = Presentation::default();
        assert!((thumb_aspect(&doc) - 9.0 / 16.0).abs() < 1e-4);
        doc.slide_size = deckcraft_model::Size::new(960.0, 720.0);
        assert!((thumb_aspect(&doc) - 0.75).abs() < 1e-4);
        doc.slide_size = deckcraft_model::Size::new(f64::NAN, 540.0);
        assert!((thumb_aspect(&doc) - 9.0 / 16.0).abs() < 1e-4);
    }
}
