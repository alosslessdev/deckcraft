//! Pointer gestures for the live Cameo bubble.
//!
//! Dragging the circle's body moves it; dragging the handle on its rim resizes it. The bubble
//! painting lives in [`super::cameo`] and the slide rectangle is owned by the canvas, so this
//! module only decides which gesture a press starts and applies it to the bubble's clamped
//! geometry. Clamping itself stays in [`Cameo::clamp`], which the caller runs every frame.

use egui::{CursorIcon, Pos2, Rect, Sense, Ui};

use super::cameo::Cameo;

/// Screen-pixel reach of the bubble's grab area around the circle. Matches the handle the bubble
/// paints, so the cursor changes exactly where a drag is picked up.
const HANDLE: f32 = 9.0;

/// Id of the bubble's interaction region, unique inside the canvas.
const ID: &str = "cameo_bubble";

/// What a bubble drag is changing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Grab {
    /// Drag on the circle body: the centre follows the pointer.
    Move,
    /// Drag on the rim handle: the radius follows the pointer's distance from the centre.
    Resize,
}

/// Turn a pointer drag over the bubble into a clamped geometry change.
///
/// `grab` remembers which gesture the drag started with, so a drag that wanders off the handle
/// keeps resizing until the button is released. Returns true while the pointer is over the bubble
/// or dragging it, which the caller uses to keep a click from reaching the show behind it.
pub(super) fn interact(cameo: &mut Cameo, ui: &Ui, slide: Rect, grab: &mut Option<Grab>) -> bool {
    let resp = ui.interact(cameo.rect(slide).expand(HANDLE), ui.id().with(ID), Sense::click_and_drag());
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(if resp.dragged() { CursorIcon::Grabbing } else { CursorIcon::Grab });
    }
    if resp.drag_started() {
        *grab = Some(grab_for(cameo, ui, slide));
    }
    if resp.dragged()
        && let Some(kind) = *grab
    {
        match kind {
            Grab::Move => cameo.drag_by(slide, resp.drag_delta()),
            Grab::Resize => {
                if let Some(pointer) = resp.interact_pointer_pos() {
                    cameo.resize_to(slide, pointer);
                }
            }
        }
    }
    if resp.drag_stopped() || !resp.dragged() {
        *grab = None;
    }
    resp.hovered() || resp.dragged()
}

/// Which gesture the current drag started with, read from the press origin so a few pixels of
/// movement past the drag threshold cannot turn a handle grab into a body drag or vice versa.
fn grab_for(cameo: &Cameo, ui: &Ui, slide: Rect) -> Grab {
    let pointer = ui.ctx().input(|i| i.pointer.press_origin().or_else(|| i.pointer.interact_pos()));
    pointer.and_then(|p| grab_kind(cameo, slide, p)).unwrap_or(Grab::Move)
}

/// The gesture a press at `pointer` starts, or `None` when the press is outside the grab area.
/// The handle wins over the body because it sits inside the expanded body rectangle.
fn grab_kind(cameo: &Cameo, slide: Rect, pointer: Pos2) -> Option<Grab> {
    if cameo.handle_rect(slide).contains(pointer) {
        Some(Grab::Resize)
    } else if cameo.rect(slide).expand(HANDLE).contains(pointer) {
        Some(Grab::Move)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::super::cameo::{MAX_RADIUS, MIN_RADIUS};
    use super::*;
    use egui::{pos2, vec2};

    fn slide() -> Rect {
        Rect::from_min_size(pos2(100.0, 50.0), vec2(1600.0, 900.0))
    }

    /// A bubble parked in the middle of the slide, where resizes have room to grow without the
    /// edge clamp moving the centre. `Cameo` resolves its default anchor until it is dragged, so
    /// the move goes through `drag_by` instead of setting the fraction directly.
    fn centred() -> Cameo {
        let mut cameo = Cameo::default();
        let delta = slide().center() - cameo.rect(slide()).center();
        cameo.drag_by(slide(), delta);
        cameo
    }

    #[test]
    fn default_bubble_is_bottom_right_and_round() {
        let cameo = Cameo::default();
        let rect = cameo.rect(slide());
        assert!((rect.width() - rect.height()).abs() < 0.01, "the bubble is circular");
        assert!(rect.right() < slide().right() && rect.bottom() < slide().bottom());
        assert!(rect.center().x > slide().center().x && rect.center().y > slide().center().y);
    }

    #[test]
    fn a_drag_started_on_the_body_moves_but_a_drag_started_on_the_handle_resizes() {
        let mut cameo = Cameo::default();
        assert_eq!(grab_kind(&cameo, slide(), cameo.rect(slide()).center()), Some(Grab::Move));
        assert_eq!(grab_kind(&cameo, slide(), cameo.handle_rect(slide()).center()), Some(Grab::Resize));

        let before = cameo.rect(slide());
        let before_center = before.center();
        let before_width = before.width();
        cameo.drag_by(slide(), vec2(-150.0, -90.0));
        let moved = cameo.rect(slide());
        assert!((moved.width() - before_width).abs() < 0.01, "moving keeps the radius");
        assert!(moved.center().x < before_center.x && moved.center().y < before_center.y, "a body drag moves the bubble");

        let center = moved.center();
        cameo.resize_to(slide(), center + vec2(150.0, 150.0));
        let resized = cameo.rect(slide());
        assert!(resized.width() > moved.width(), "a handle drag outward grows the bubble");
        assert!((resized.center() - center).length() < 0.01, "resizing keeps the centre");
    }

    #[test]
    fn dragging_far_beyond_every_edge_keeps_the_circle_inside_the_slide() {
        let mut cameo = Cameo::default();
        for delta in [
            vec2(-5000.0, -5000.0),
            vec2(5000.0, -5000.0),
            vec2(-5000.0, 5000.0),
            vec2(5000.0, 5000.0),
            vec2(-5000.0, 0.0),
            vec2(5000.0, 0.0),
            vec2(0.0, -5000.0),
            vec2(0.0, 5000.0),
        ] {
            cameo.drag_by(slide(), delta);
            let rect = cameo.rect(slide());
            assert!(slide().expand(0.01).contains_rect(rect), "{rect:?} must stay inside {:?}", slide());
        }
    }

    #[test]
    fn resizing_to_a_tiny_or_huge_pointer_distance_respects_the_limits() {
        let mut cameo = centred();
        let center = cameo.rect(slide()).center();

        cameo.resize_to(slide(), center + vec2(0.5, 0.5));
        assert!((cameo.radius - MIN_RADIUS).abs() < 1.0e-5, "tiny drags clamp to MIN_RADIUS");
        assert!(slide().expand(0.01).contains_rect(cameo.rect(slide())));

        cameo.resize_to(slide(), center + vec2(5000.0, 5000.0));
        assert!((cameo.radius - MAX_RADIUS).abs() < 1.0e-5, "huge drags clamp to MAX_RADIUS");
        let doubled = 2.0 * MAX_RADIUS * slide().width().min(slide().height());
        assert!(cameo.rect(slide()).width() <= doubled + 0.01);
        assert!(slide().expand(0.01).contains_rect(cameo.rect(slide())));
    }

    #[test]
    fn the_resize_radius_follows_the_pointer_distance_from_the_centre() {
        let mut cameo = centred();
        let center = cameo.rect(slide()).center();
        let pointer = center + vec2(180.0, 0.0);
        cameo.resize_to(slide(), pointer);
        let rect = cameo.rect(slide());
        assert!((rect.center() - center).length() < 0.01, "the centre does not move");
        assert!((rect.width() / 2.0 - (pointer - rect.center()).length()).abs() < 0.5, "the handle lands under the pointer");
        assert!((rect.width() / 2.0 - 180.0).abs() < 0.5);
    }

    #[test]
    fn grasping_the_handle_does_not_change_the_radius() {
        let mut cameo = centred();
        let before = cameo.radius;
        let handle = cameo.handle_rect(slide()).center();
        cameo.resize_to(slide(), handle);
        assert!((cameo.radius - before).abs() < 1.0e-5, "the handle is on the rim, so grabbing it keeps the radius");
    }

    #[test]
    fn hit_testing_outside_the_bubble_is_false() {
        let cameo = Cameo::default();
        let outside = cameo.rect(slide()).left_top() - vec2(2.0 * HANDLE, 2.0 * HANDLE);
        assert_eq!(grab_kind(&cameo, slide(), outside), None);
        assert!(!cameo.hits(slide(), outside));
        assert_eq!(grab_kind(&cameo, slide(), cameo.rect(slide()).center()), Some(Grab::Move));
        assert_eq!(grab_kind(&cameo, slide(), cameo.handle_rect(slide()).center()), Some(Grab::Resize));
    }
}
