//! Teleprompter view: the large script over the slide, plus auto-scroll controls.
//!
//! The pill sits in the lower-right corner of the notes pane's body, under the A−/A+ header the
//! notes pane owns. It carries the auto-scroll toggle and the speed stepper; while auto-scroll
//! runs the section moves [`RecordView::notes_scroll`] itself, so the notes pane shows the new
//! offset on the next frame.

use egui::{Align2, Color32, CornerRadius, Rect, Role, Sense, Stroke, StrokeKind, Ui, WidgetInfo, pos2, vec2};

use super::super::{Layout, RecordView};
use crate::theme;

/// Auto-scroll state for the teleprompter's script pane.
#[derive(Clone, Copy, Debug)]
pub(super) struct State {
    /// The script moves without a wheel or drag.
    pub auto: bool,
    /// Scroll speed in points per second.
    pub speed: f32,
}

impl Default for State {
    fn default() -> Self {
        State { auto: false, speed: 22.0 }
    }
}

/// Slowest and fastest speed the pill offers, in points per second.
const SPEED_MIN: f32 = 6.0;
const SPEED_MAX: f32 = 90.0;
/// One press of − or +.
const SPEED_STEP: f32 = 4.0;
/// The fastest auto-scroll [`advance`] honours; anything above is hostile input.
const SPEED_CEIL: f32 = 240.0;
/// The furthest the script offset may travel.
const OFFSET_MAX: f32 = 1_000_000.0;
/// The pill's footprint in the notes body.
const PILL_SIZE: egui::Vec2 = vec2(168.0, 24.0);
/// Gap between the pill and the notes pane's inner corner.
const PILL_MARGIN: egui::Vec2 = vec2(12.0, 8.0);

/// Keep a speed inside the range the pill can set.
fn clamp_speed(speed: f32) -> f32 {
    if speed.is_finite() { speed.clamp(SPEED_MIN, SPEED_MAX) } else { State::default().speed }
}

/// Move the script `speed * dt` points forward, for auto-scroll.
///
/// Non-finite speeds and frame times are ignored so a stalled or hostile clock cannot poison the
/// offset; a non-finite offset restarts from the top instead of staying unusable. The speed is
/// capped at [`SPEED_CEIL`] and the result stays inside `0..=`[`OFFSET_MAX`].
pub(super) fn advance(offset: &mut f32, speed: f32, dt: f32) {
    if !speed.is_finite() || !dt.is_finite() {
        return;
    }
    if !offset.is_finite() {
        *offset = 0.0;
        return;
    }
    let speed = speed.clamp(0.0, SPEED_CEIL);
    *offset = (*offset + speed * dt).clamp(0.0, OFFSET_MAX);
}

/// The auto-scroll pill: a toggle plus − / + speed buttons in the notes body's lower-right corner.
pub(super) fn ui(ui: &mut Ui, rec: &mut RecordView, layout: &Layout) -> bool {
    if rec.view != super::ViewKind::Teleprompter || layout.notes.height() < 28.0 {
        return false;
    }

    let pill = Rect::from_min_size(
        pos2(layout.notes.max.x - PILL_MARGIN.x - PILL_SIZE.x, layout.notes.max.y - PILL_MARGIN.y - PILL_SIZE.y),
        PILL_SIZE,
    );
    let inner = pill.shrink2(vec2(8.0, 2.0));
    let toggle = Rect::from_min_size(inner.min, vec2(80.0, inner.height()));
    let faster = Rect::from_min_size(pos2(inner.max.x - 20.0, inner.min.y), vec2(20.0, inner.height()));
    let slower = Rect::from_min_size(pos2(faster.min.x - 4.0 - 20.0, inner.min.y), vec2(20.0, inner.height()));
    let readout = Rect::from_min_max(pos2(toggle.max.x, inner.min.y), pos2(slower.min.x, inner.max.y));

    let auto_resp = ui.interact(toggle, ui.id().with("record_teleprompter_auto"), Sense::click()).on_hover_text("Scroll the script automatically");
    let slower_resp = ui.interact(slower, ui.id().with("record_teleprompter_slower"), Sense::click()).on_hover_text("Auto-scroll speed");
    let faster_resp = ui.interact(faster, ui.id().with("record_teleprompter_faster"), Sense::click()).on_hover_text("Auto-scroll speed");
    auto_resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Auto-scroll"));
    slower_resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Slower"));
    faster_resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Faster"));

    let mut changed = false;
    if auto_resp.clicked() {
        rec.teleprompter.auto = !rec.teleprompter.auto;
        changed = true;
    }
    let current = clamp_speed(rec.teleprompter.speed);
    let step = if faster_resp.clicked() {
        Some(SPEED_STEP)
    } else if slower_resp.clicked() {
        Some(-SPEED_STEP)
    } else {
        None
    };
    if let Some(step) = step {
        let next = clamp_speed(current + step);
        if next != current {
            rec.teleprompter.speed = next;
            changed = true;
        }
    }
    let shown = clamp_speed(rec.teleprompter.speed);

    let on = rec.teleprompter.auto;
    let accent = theme::Tokens::get(ui.ctx()).accent;
    let painter = ui.painter();
    painter.rect_filled(pill, CornerRadius::same(12), Color32::from_rgba_unmultiplied(16, 16, 20, 210));
    painter.rect_stroke(pill, CornerRadius::same(12), Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 120, 126, 90)), StrokeKind::Inside);

    let toggle_fill = if on {
        accent
    } else if auto_resp.hovered() {
        Color32::from_gray(58)
    } else {
        Color32::from_gray(40)
    };
    painter.rect_filled(toggle, CornerRadius::same(10), toggle_fill);
    let dot = pos2(toggle.min.x + 9.0, toggle.center().y);
    if on {
        painter.circle_filled(dot, 3.5, Color32::from_white_alpha(230));
    } else {
        painter.circle_stroke(dot, 3.5, Stroke::new(1.0, Color32::from_gray(170)));
    }
    painter.text(
        pos2(toggle.min.x + 18.0, toggle.center().y),
        Align2::LEFT_CENTER,
        "Auto-scroll",
        theme::font(11.0),
        if on { Color32::WHITE } else { Color32::from_gray(210) },
    );

    for (rect, label, hovered) in [(slower, "−", slower_resp.hovered()), (faster, "+", faster_resp.hovered())] {
        if hovered {
            painter.rect_filled(rect, CornerRadius::same(6), Color32::from_gray(58));
        }
        painter.text(rect.center(), Align2::CENTER_CENTER, label, theme::font(13.0), Color32::from_gray(215));
    }
    painter.text(readout.center(), Align2::CENTER_CENTER, format!("{shown:.0}"), theme::font(11.0), Color32::from_gray(170));

    if rec.teleprompter.auto {
        let dt = ui.ctx().input(|i| i.stable_dt).clamp(0.0, 0.1);
        advance(&mut rec.notes_scroll, rec.teleprompter.speed, dt);
        ui.ctx().request_repaint();
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn advance_moves_proportionally() {
        let mut offset = 0.0;
        advance(&mut offset, 20.0, 0.5);
        assert!(close(offset, 10.0), "20 pt/s for half a second moves 10 points, got {offset}");
        advance(&mut offset, 30.0, 0.1);
        assert!(close(offset, 13.0), "the next frame adds to the offset, got {offset}");
    }

    #[test]
    fn advance_ignores_non_finite_speed_and_time() {
        for speed in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut offset = 42.0;
            advance(&mut offset, speed, 0.1);
            assert!(close(offset, 42.0), "speed {speed} must not move the script, got {offset}");
        }
        for dt in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut offset = 42.0;
            advance(&mut offset, 22.0, dt);
            assert!(close(offset, 42.0), "dt {dt} must not move the script, got {offset}");
        }
    }

    #[test]
    fn advance_restarts_from_a_non_finite_offset() {
        let mut offset = f32::NAN;
        advance(&mut offset, 10.0, 0.1);
        assert!(close(offset, 0.0), "a poisoned offset restarts at the top, got {offset}");
    }

    #[test]
    fn advance_clamps_speed() {
        let mut fast = 0.0;
        advance(&mut fast, 1_000.0, 1.0);
        assert!(close(fast, SPEED_CEIL), "speed is capped at {SPEED_CEIL}, got {fast}");

        let mut backwards = 20.0;
        advance(&mut backwards, -50.0, 1.0);
        assert!(close(backwards, 20.0), "a negative speed clamps to zero, got {backwards}");
    }

    #[test]
    fn advance_clamps_the_offset() {
        let mut bottom = OFFSET_MAX - 1.0;
        advance(&mut bottom, 240.0, 1.0);
        assert!(close(bottom, OFFSET_MAX), "the offset stops at {OFFSET_MAX}, got {bottom}");

        let mut above = -5.0;
        advance(&mut above, 10.0, 0.1);
        assert!(close(above, 0.0), "the offset never goes negative, got {above}");
    }

    #[test]
    fn zero_speed_leaves_the_script_alone() {
        let mut offset = 123.0;
        advance(&mut offset, 0.0, 0.1);
        assert!(close(offset, 123.0), "zero speed does nothing, got {offset}");
    }

    #[test]
    fn the_speed_stepper_stays_in_range() {
        assert!(close(clamp_speed(SPEED_MIN - 20.0), SPEED_MIN));
        assert!(close(clamp_speed(SPEED_MAX + 20.0), SPEED_MAX));
        assert!(close(clamp_speed(f32::NAN), State::default().speed));
        assert!(close(clamp_speed(State::default().speed + SPEED_STEP), 26.0));
    }

    #[test]
    fn the_pill_has_a_speed_to_show() {
        let state = State::default();
        assert!(!state.auto, "auto-scroll starts off");
        assert!((SPEED_MIN..=SPEED_MAX).contains(&state.speed), "default speed is in range");
    }
}
