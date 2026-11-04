//! Presenter View in the Record window: the next slide (or the end-of-show card) in the strip
//! under the compact notes pane.
//!
//! The strip is chrome only: the shell owns the slide, the notes and the navigation, so this pane
//! never changes state and always reports `false`. It asks the texture cache for one thumbnail per
//! frame; a miss paints a dark placeholder and the shell's repaint loop tries again.

use std::sync::Arc;

use deckcraft_model::Presentation;
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::super::{Layout, RecordView};
use crate::SlideApp;
use crate::show::Show;
use crate::theme;

/// The panel's fill, matching the shell's dark chrome.
const PANEL: Color32 = Color32::from_rgb(26, 26, 30);
/// Hairline around the panel.
const BORDER: Color32 = Color32::from_gray(58);
/// Primary label colour.
const INK: Color32 = Color32::from_gray(220);
/// Secondary, de-emphasised label colour.
const DIM: Color32 = Color32::from_gray(150);
/// A thumbnail slot with nothing rendered yet.
const EMPTY: Color32 = Color32::from_gray(40);
/// Inner padding between the panel edge and its content.
const PAD: f32 = 8.0;

/// The strip's accessible label: the next slide's one-based number, or the end of the show.
pub(super) fn next_label(next: Option<usize>) -> String {
    match next {
        Some(n) => format!("Next slide {}", n + 1),
        None => "End of show".to_owned(),
    }
}

pub(super) fn ui(
    app: &mut SlideApp,
    ui: &mut Ui,
    show: &Show,
    doc: &Arc<Presentation>,
    _rec: &mut RecordView,
    layout: &Layout,
) -> bool {
    let Some(strip) = layout.next else { return false };
    if strip.height() < 20.0 || strip.width() < 1.0 {
        return false;
    }

    let next = show.state.peek_next(doc);
    let label = next_label(next);
    let resp = ui.interact(strip, ui.id().with("record_next_slide"), Sense::hover());
    let inner = strip.shrink(PAD);
    let painter = ui.painter().with_clip_rect(strip);
    painter.rect(strip, CornerRadius::same(6), PANEL, Stroke::new(1.0, BORDER), StrokeKind::Inside);

    match next {
        Some(n) => {
            // A one-slide-height thumbnail keeps the preview legible in a 56–120 px strip.
            let w = (inner.height() as f64 * doc.slide_size.width / doc.slide_size.height.max(1.0)).clamp(64.0, 320.0) as u32;
            let mut budget = 1;
            let tex = app.textures.thumb(ui.ctx(), doc, n, w, &mut budget);
            let aspect = (doc.slide_size.width.max(1.0) / doc.slide_size.height.max(1.0)) as f32;
            let h = (w as f32 / aspect.max(0.01)).min(inner.height());
            let iw = (h * aspect).min(inner.width()).max(1.0);
            let image_rect = Rect::from_min_size(inner.min, vec2(iw, h.clamp(1.0, inner.height())));
            match tex {
                Some(tex) => painter.image(
                    tex.id(),
                    image_rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                ),
                None => {
                    painter.rect(image_rect, CornerRadius::same(3), EMPTY, Stroke::new(1.0, BORDER), StrokeKind::Inside);
                }
            }
            let text_x = image_rect.max.x + 12.0;
            painter.text(
                pos2(text_x, inner.center().y - 8.0),
                Align2::LEFT_CENTER,
                &label,
                theme::font(12.0),
                INK,
            );
            painter.text(
                pos2(text_x, inner.center().y + 9.0),
                Align2::LEFT_CENTER,
                format!("You are on slide {}", show.state.slide + 1),
                theme::font(10.0),
                DIM,
            );
        }
        None => {
            painter.text(strip.center(), Align2::CENTER_CENTER, &label, theme::font(12.0), INK);
        }
    }

    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &label));
    false
}

#[cfg(test)]
mod tests {
    use super::next_label;

    #[test]
    fn the_label_counts_from_one() {
        assert_eq!(next_label(Some(0)), "Next slide 1");
        assert_eq!(next_label(Some(4)), "Next slide 5");
    }

    #[test]
    fn the_last_slide_reads_as_the_end_of_the_show() {
        assert_eq!(next_label(None), "End of show");
    }
}
