//! Ink and laser drawing for the Record view.
//!
//! Strokes are kept in slide fractions so they stay on the slide at any window size. While a
//! recording runs they are also moved into the deck's per-slide recording in slide points, which
//! is what the `.pptx` timing writer reads back. This module has no `Show` dependency: the shell
//! feeds it the live strokes and recording buffers.

use egui::{Color32, Painter, Pos2, Rect, Stroke, pos2};

/// Pen stroke width on screen, in points (the recorded deck keeps the slide-point equivalent).
pub(crate) const PEN_WIDTH: f32 = 3.0;
/// A highlighter is wider and translucent, like a real marker.
pub(crate) const HIGHLIGHTER_WIDTH: f32 = 14.0;
pub(crate) const HIGHLIGHTER_ALPHA: u8 = 110;
/// A laser is a soft dot with a bright centre.
pub(crate) const LASER_GLOW: f32 = 16.0;
pub(crate) const LASER_DOT: f32 = 5.5;
/// Most points kept for one recorded stroke or one slide's laser trace.
pub(crate) const MAX_POINTS: usize = 4096;

/// Whether a screen point falls inside the fitted slide rectangle. Degenerate rectangles contain
/// nothing, so a collapsed layout never paints.
pub(crate) fn on_slide(slide: Rect, p: Pos2) -> bool {
    slide.width() > 0.0 && slide.height() > 0.0 && p.x.is_finite() && p.y.is_finite() && slide.contains(p)
}

/// Slide fractions to screen points inside the fitted slide rectangle.
pub(crate) fn to_screen(slide: Rect, p: Pos2) -> Pos2 {
    pos2(slide.left() + p.x * slide.width(), slide.top() + p.y * slide.height())
}

/// Screen points back to slide fractions, or `None` outside the slide.
pub(crate) fn to_slide(slide: Rect, p: Pos2) -> Option<Pos2> {
    if !on_slide(slide, p) {
        return None;
    }
    Some(pos2(((p.x - slide.left()) / slide.width()).clamp(0.0, 1.0), ((p.y - slide.top()) / slide.height()).clamp(0.0, 1.0)))
}

/// Width and colour for a live stroke: the pen is thin and opaque, the highlighter wide and
/// translucent. The recorded deck always keeps the pen equivalent.
pub(crate) fn ink_style(color: Color32, highlighter: bool) -> (f32, Color32) {
    if highlighter {
        (HIGHLIGHTER_WIDTH, Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), HIGHLIGHTER_ALPHA))
    } else {
        (PEN_WIDTH, color)
    }
}

/// Distance from `p` to segment `a`–`b`, in the same units as the points.
fn segment_distance(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 <= f32::EPSILON {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Whether any part of a stroke comes within `radius` of `p` (all in slide fractions), and where
/// that stroke is. A single-point stroke counts as a point.
pub(crate) fn stroke_at(strokes: &[Vec<Pos2>], p: Pos2, radius: f32) -> Option<usize> {
    strokes.iter().position(|s| match s.as_slice() {
        [] => false,
        [one] => one.distance(p) <= radius,
        pts => pts.windows(2).any(|w| w.len() == 2 && segment_distance(w[0], w[1], p) <= radius),
    })
}

/// Remove every stroke the eraser touches at `p`. Returns true when something was removed.
pub(crate) fn erase_at(strokes: &mut Vec<Vec<Pos2>>, p: Pos2, radius: f32) -> bool {
    let before = strokes.len();
    strokes.retain(|s| stroke_at(std::slice::from_ref(s), p, radius).is_none());
    strokes.len() != before
}

/// Draw one stroke in slide fractions, clipped to the slide so ink never covers the chrome around
/// it. A single-point stroke becomes a dot; non-finite points are skipped.
pub(crate) fn draw_stroke(painter: &Painter, slide: Rect, points: &[Pos2], color: Color32, width: f32) {
    if slide.width() <= 0.0 || slide.height() <= 0.0 {
        return;
    }
    let painter = painter.with_clip_rect(slide);
    let screen: Vec<Pos2> = points.iter().filter(|p| p.x.is_finite() && p.y.is_finite()).map(|p| to_screen(slide, *p)).collect();
    if screen.is_empty() {
        return;
    }
    if screen.len() == 1 {
        painter.circle_filled(screen[0], (width * 0.5).max(0.5), color);
    } else {
        painter.add(egui::Shape::line(screen, Stroke::new(width, color)));
    }
}

/// Draw every live stroke. `highlighter` widens the line and makes it translucent; the caller
/// passes the pen colour in both cases.
pub(crate) fn draw_strokes(painter: &Painter, slide: Rect, strokes: &[Vec<Pos2>], color: Color32, highlighter: bool) {
    let (width, color) = ink_style(color, highlighter);
    for s in strokes {
        draw_stroke(painter, slide, s, color, width);
    }
}

/// Draw the laser dot under the pointer, if it is over the slide.
pub(crate) fn draw_laser(painter: &Painter, slide: Rect, pointer: Option<Pos2>, color: Color32) {
    let Some(p) = pointer.filter(|p| slide.contains(*p)) else { return };
    laser_dot(painter, p, color);
}

/// Move the live strokes into the recording, in slide points (the format's unit), tagged with the
/// slide they were drawn on.
///
/// The strokes are taken by value on purpose: the live buffer is consumed even when there is
/// nothing to record, so the shell never has to clear it afterwards. Strokes are dropped when the
/// slide has no usable size, when they do not keep at least two finite points, and every stroke is
/// capped at [`MAX_POINTS`].
pub(crate) fn stash_strokes(recorded: &mut Vec<(usize, Vec<(f64, f64)>)>, slide: usize, slide_size: (f64, f64), strokes: Vec<Vec<Pos2>>) {
    let (w, h) = slide_size;
    let usable = w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0;
    if !usable {
        return;
    }
    for stroke in strokes {
        let mut pts = Vec::new();
        for q in stroke.iter().filter(|q| q.x.is_finite() && q.y.is_finite()).take(MAX_POINTS) {
            let x = q.x as f64 * w;
            let y = q.y as f64 * h;
            if x.is_finite() && y.is_finite() {
                pts.push((x, y));
            }
        }
        if pts.len() >= 2 {
            recorded.push((slide, pts));
        }
    }
}

/// Move the live laser samples into the recording, keeping empty traces out.
pub(crate) fn stash_laser(saved: &mut Vec<(usize, Vec<(f64, f64, u32)>)>, slide: usize, live: &mut Vec<(f64, f64, u32)>) {
    let pts = std::mem::take(live);
    if !pts.is_empty() {
        saved.push((slide, pts));
    }
}

/// Record one laser sample in slide points and milliseconds from the slide start. Samples closer
/// together than 40 ms that did not move at least 1.5 points are dropped; a slide trace is capped.
pub(crate) fn note_laser(live: &mut Vec<(f64, f64, u32)>, x: f64, y: f64, t_ms: u32) {
    if !x.is_finite() || !y.is_finite() || live.len() >= MAX_POINTS {
        return;
    }
    if let Some((px, py, pt)) = live.last() {
        let moved = (x - px).hypot(y - py) >= 1.5;
        if t_ms.saturating_sub(*pt) < 40 && !moved {
            return;
        }
    }
    live.push((x, y, t_ms));
}

/// A soft two-tone laser dot; `p` is in screen points.
pub(crate) fn laser_dot(painter: &Painter, p: Pos2, color: Color32) {
    painter.circle_filled(p, LASER_GLOW, Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 70));
    painter.circle_filled(p, LASER_DOT, color);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use egui::{FullOutput, LayerId, RawInput, vec2};

    fn slide() -> Rect {
        Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 450.0))
    }

    /// Run one `painter` pass in a real context so the emitted shapes can be inspected.
    fn painted(f: impl Fn(&Painter)) -> FullOutput {
        let ctx = egui::Context::default();
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1080.0))), ..Default::default() };
        ctx.run_pass(input, |ctx| f(&ctx.layer_painter(LayerId::background())))
    }

    #[test]
    fn fractions_round_trip_inside_the_slide_and_reject_outside_points() {
        let p = pos2(0.25, 0.75);
        let screen = to_screen(slide(), p);
        assert!((screen.x - 210.0).abs() < 0.01 && (screen.y - 357.5).abs() < 0.01);
        assert_eq!(to_slide(slide(), screen), Some(p));
        assert!(to_slide(slide(), pos2(0.0, 0.0)).is_none());
        assert!(to_slide(Rect::from_min_size(Pos2::ZERO, vec2(0.0, 0.0)), Pos2::ZERO).is_none());
    }

    #[test]
    fn the_clipping_predicate_rejects_points_outside_the_slide() {
        assert!(on_slide(slide(), pos2(100.0, 100.0)));
        assert!(!on_slide(slide(), pos2(0.0, 0.0)));
        assert!(!on_slide(slide(), pos2(900.0, 100.0)));
        assert!(!on_slide(slide(), pos2(f32::NAN, 100.0)));
        assert!(!on_slide(Rect::from_min_size(Pos2::ZERO, vec2(0.0, 10.0)), Pos2::ZERO));
    }

    #[test]
    fn a_stroke_is_hit_on_a_segment_and_erased_as_a_whole() {
        let mut strokes = vec![vec![pos2(0.1, 0.1), pos2(0.9, 0.1)]];
        assert_eq!(stroke_at(&strokes, pos2(0.5, 0.11), 0.02), Some(0));
        assert_eq!(stroke_at(&strokes, pos2(0.5, 0.5), 0.02), None);
        assert!(erase_at(&mut strokes, pos2(0.5, 0.11), 0.02));
        assert!(strokes.is_empty());
        assert!(!erase_at(&mut strokes, pos2(0.5, 0.5), 0.02));
    }

    #[test]
    fn touching_any_segment_removes_that_whole_stroke() {
        let mut strokes = vec![vec![pos2(0.0, 0.0), pos2(1.0, 1.0)], vec![pos2(0.0, 1.0), pos2(1.0, 0.0)], vec![pos2(0.2, 0.8), pos2(0.4, 0.8)]];
        // Near the middle of the second stroke only: the first and third survive.
        assert!(erase_at(&mut strokes, pos2(0.8, 0.21), 0.02));
        assert_eq!(strokes.len(), 2);
        assert_eq!(strokes[1], vec![pos2(0.2, 0.8), pos2(0.4, 0.8)]);
    }

    #[test]
    fn a_single_tap_counts_as_a_stroke() {
        let strokes = vec![vec![pos2(0.4, 0.4)]];
        assert_eq!(stroke_at(&strokes, pos2(0.41, 0.4), 0.02), Some(0));
        assert_eq!(stroke_at(&strokes, pos2(0.5, 0.4), 0.02), None);
    }

    #[test]
    fn the_highlighter_is_wider_and_translucent_while_the_pen_is_opaque() {
        let color = Color32::from_rgb(200, 50, 40);
        assert_eq!(ink_style(color, false), (PEN_WIDTH, color));
        let (width, hi) = ink_style(color, true);
        assert_eq!(width, HIGHLIGHTER_WIDTH);
        assert_eq!((hi.r(), hi.g(), hi.b()), (color.r(), color.g(), color.b()));
        assert_eq!(hi.a(), HIGHLIGHTER_ALPHA);
        assert!((width - PEN_WIDTH) >= 10.0);
    }

    #[test]
    fn drawing_a_one_point_stroke_paints_a_dot_inside_the_slide() {
        let out = painted(|p| draw_stroke(p, slide(), &[pos2(0.5, 0.5)], Color32::RED, PEN_WIDTH));
        assert_eq!(out.shapes.len(), 1, "one dot shape");
        assert_eq!(out.shapes[0].clip_rect, slide());
    }

    #[test]
    fn strokes_are_clipped_to_the_slide_rect() {
        let out = painted(|p| draw_stroke(p, slide(), &[pos2(0.0, 0.0), pos2(1.0, 1.0)], Color32::RED, PEN_WIDTH));
        assert!(!out.shapes.is_empty());
        assert!(out.shapes.iter().all(|s| s.clip_rect == slide()), "every shape is clipped to the slide");
    }

    #[test]
    fn strokes_outside_the_slide_paint_nothing_inside_it() {
        let out = painted(|p| draw_stroke(p, slide(), &[pos2(-0.5, -0.5)], Color32::RED, PEN_WIDTH));
        assert_eq!(out.shapes.len(), 1);
        assert_eq!(out.shapes[0].clip_rect, slide(), "still clipped to the slide");
        assert!(!out.shapes[0].shape.visual_bounding_rect().intersects(slide()), "the dot lies outside the slide");
    }

    #[test]
    fn a_pen_stroke_draws_one_line_shape() {
        let out = painted(|p| draw_strokes(p, slide(), &[vec![pos2(0.1, 0.1), pos2(0.9, 0.9)]], Color32::BLUE, false));
        assert_eq!(out.shapes.len(), 1);
        assert_eq!(out.shapes[0].clip_rect, slide());
    }

    #[test]
    fn stashing_converts_fractions_to_slide_points_and_drops_taps() {
        let mut recorded = Vec::new();
        stash_strokes(&mut recorded, 2, (960.0, 540.0), vec![vec![pos2(0.1, 0.2), pos2(0.1, 0.3)], vec![pos2(0.5, 0.5)]]);
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, 2);
        assert!((recorded[0].1[0].0 - 96.0).abs() < 1e-9);
        assert!((recorded[0].1[1].1 - 162.0).abs() < 1e-9);
    }

    #[test]
    fn stashing_consumes_the_strokes_and_drops_invalid_geometry() {
        let live = vec![vec![pos2(0.1, 0.1), pos2(0.2, 0.2)]];
        let mut recorded = Vec::new();
        // The buffer is moved in and dropped: with no usable slide there is nothing to record.
        stash_strokes(&mut recorded, 0, (0.0, 100.0), live);
        assert!(recorded.is_empty());
        let mut recorded = Vec::new();
        stash_strokes(&mut recorded, 0, (f64::NAN, 100.0), vec![vec![pos2(0.1, 0.1), pos2(0.2, 0.2)]]);
        assert!(recorded.is_empty());
    }

    #[test]
    fn non_finite_fractions_never_reach_the_recording() {
        let mut recorded = Vec::new();
        stash_strokes(&mut recorded, 0, (100.0, 100.0), vec![vec![pos2(f32::NAN, 0.1), pos2(f32::INFINITY, 0.3)]]);
        assert!(recorded.is_empty(), "non-finite fractions leave no stroke");
        stash_strokes(&mut recorded, 0, (100.0, 100.0), vec![vec![pos2(f32::NAN, 0.1), pos2(0.2, 0.2)]]);
        assert!(recorded.is_empty(), "one finite point is not a stroke");
    }

    #[test]
    fn laser_samples_throttle_stillness_but_keep_motion() {
        let mut live = Vec::new();
        note_laser(&mut live, 1.0, 1.0, 0);
        note_laser(&mut live, 1.1, 1.0, 10);
        assert_eq!(live.len(), 1, "still and under 40 ms");
        note_laser(&mut live, 5.0, 1.0, 10);
        assert_eq!(live.len(), 2, "moved");
        note_laser(&mut live, 5.0, 1.0, 100);
        assert_eq!(live.len(), 3, "40 ms later");
        let mut saved = Vec::new();
        stash_laser(&mut saved, 0, &mut live);
        assert_eq!(saved.len(), 1);
        assert!(live.is_empty());
    }

    #[test]
    fn stash_caps_points_and_ignores_invalid_geometry() {
        let pts = vec![pos2(0.0, 0.0); MAX_POINTS + 10];
        let mut recorded = Vec::new();
        stash_strokes(&mut recorded, 0, (100.0, 100.0), vec![pts]);
        assert_eq!(recorded[0].1.len(), MAX_POINTS);
        let mut none = Vec::new();
        stash_strokes(&mut none, 0, (0.0, 100.0), vec![vec![pos2(0.1, 0.1), pos2(0.2, 0.2)]]);
        assert!(none.is_empty());
    }
}
