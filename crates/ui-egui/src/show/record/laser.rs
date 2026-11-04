//! The recording laser pointer: the live dot over the slide and the trace kept with a slide's
//! recording.
//!
//! The shell paints the dot while the presenter points and, during a recording, stores a throttled
//! trace of it in slide points so the deck's timing writer can replay it. Sample times are
//! milliseconds from the moment the slide appeared; callers convert the shell clock, clamp it to
//! 3,600,000 ms (one hour) and drop non-finite values before calling [`note`], because the
//! recording format stores the time as a `u32` and a stale or broken clock must not wrap it.

use egui::{Color32, Painter, Pos2, Rect, pos2};

/// Soft glow radius around the laser point, in screen points.
pub(super) const GLOW: f32 = 16.0;
/// Solid core radius, in screen points.
pub(super) const DOT: f32 = 5.5;
/// The glow is translucent so the slide stays readable underneath.
const GLOW_ALPHA: u8 = 70;
/// Most samples kept for one slide's trace.
pub(super) const MAX_POINTS: usize = 4096;
/// A sample is only throttled while it is both this close to the previous one (slide points)…
const STILL_POINTS: f64 = 1.5;
/// …and this soon after it (milliseconds).
const STILL_MS: u32 = 40;

/// A soft two-tone laser dot; `p` is in screen points.
pub(super) fn dot(painter: &Painter, p: Pos2, color: Color32) {
    let glow = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), GLOW_ALPHA);
    painter.circle_filled(p, GLOW, glow);
    painter.circle_filled(p, DOT, color);
}

/// Draw the laser dot under the pointer, and only while the pointer is over the slide. The layer
/// is clipped to `slide` so the glow never covers the surrounding chrome.
pub(super) fn draw(painter: &Painter, slide: Rect, pointer: Option<Pos2>, color: Color32) {
    let Some(p) = pointer.filter(|p| screen_to_slide(slide, *p).is_some()) else { return };
    dot(&painter.with_clip_rect(slide), p, color);
}

/// Screen points back to slide fractions, or `None` for a point outside the slide or a slide
/// rectangle with no area.
pub(super) fn screen_to_slide(slide: Rect, p: Pos2) -> Option<Pos2> {
    if !(slide.width() > 0.0 && slide.height() > 0.0) || !slide.contains(p) {
        return None;
    }
    Some(pos2(((p.x - slide.left()) / slide.width()).clamp(0.0, 1.0), ((p.y - slide.top()) / slide.height()).clamp(0.0, 1.0)))
}

/// Record one laser sample in slide points and milliseconds from the slide start. A sample that
/// both stayed within 1.5 points of the previous one and arrived less than 40 ms after it adds
/// nothing; a slide trace is capped at [`MAX_POINTS`].
///
/// The caller converts and bounds the time: non-finite input is dropped there and the value is
/// clamped to 3,600,000 ms, the format's one-hour cap.
pub(super) fn note(live: &mut Vec<(f64, f64, u32)>, x: f64, y: f64, t_ms: u32) {
    if !x.is_finite() || !y.is_finite() || live.len() >= MAX_POINTS {
        return;
    }
    if let Some((px, py, pt)) = live.last() {
        let moved = (x - px).hypot(y - py) >= STILL_POINTS;
        if !moved && t_ms.saturating_sub(*pt) < STILL_MS {
            return;
        }
    }
    live.push((x, y, t_ms));
}

/// Move the live trace into the recording, tagged with the slide it was drawn on. Always empties
/// `live`; an empty trace is dropped so a slide the laser never visited gets no entry.
pub(super) fn stash(saved: &mut Vec<(usize, Vec<(f64, f64, u32)>)>, slide: usize, live: &mut Vec<(f64, f64, u32)>) {
    let pts = std::mem::take(live);
    if !pts.is_empty() {
        saved.push((slide, pts));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use egui::{Id, LayerId, Order, RawInput, Shape, epaint::CircleShape, vec2};

    fn slide() -> Rect {
        Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 450.0))
    }

    /// Paint one frame and return the circles the laser layer emitted, each with its clip rect.
    fn painted(pointer: Option<Pos2>) -> Vec<(CircleShape, Rect)> {
        let ctx = egui::Context::default();
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0))), ..Default::default() };
        let output = ctx.run_pass(input, |ctx| {
            let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("laser-test")));
            draw(&painter, slide(), pointer, Color32::RED);
        });
        output
            .shapes
            .into_iter()
            .filter_map(|clipped| {
                let clip = clipped.clip_rect;
                match clipped.shape {
                    Shape::Circle(circle) => Some((circle, clip)),
                    _ => None,
                }
            })
            .collect()
    }

    #[test]
    fn the_laser_is_painted_only_over_the_slide_and_clipped_to_it() {
        assert!(painted(None).is_empty(), "no pointer, no dot");
        assert!(painted(Some(pos2(0.0, 0.0))).is_empty(), "outside the slide, no dot");

        let at = pos2(210.0, 357.5);
        let circles = painted(Some(at));
        assert_eq!(circles.len(), 2, "a glow and a core");
        let (glow, core) = (&circles[0].0, &circles[1].0);
        assert_eq!(glow.center, at);
        assert!((glow.radius - GLOW).abs() < f32::EPSILON);
        assert_eq!(glow.fill, Color32::from_rgba_unmultiplied(255, 0, 0, GLOW_ALPHA));
        assert_eq!(core.center, at);
        assert!((core.radius - DOT).abs() < f32::EPSILON);
        assert_eq!(core.fill, Color32::RED);
        for (_, clip) in &circles {
            assert_eq!(*clip, slide(), "the dot never paints over the chrome");
        }
    }

    #[test]
    fn screen_points_map_to_slide_fractions_and_reject_outside_or_empty_geometry() {
        let p = pos2(0.25, 0.75);
        let screen = pos2(slide().left() + p.x * slide().width(), slide().top() + p.y * slide().height());
        assert_eq!(screen_to_slide(slide(), screen), Some(p));
        assert!(screen_to_slide(slide(), pos2(0.0, 0.0)).is_none(), "left and above the slide");
        assert!(screen_to_slide(slide(), pos2(f32::NAN, 30.0)).is_none(), "a broken pointer");
        let no_width = Rect::from_min_size(pos2(10.0, 20.0), vec2(0.0, 450.0));
        assert!(screen_to_slide(no_width, pos2(10.0, 20.0)).is_none(), "a collapsing slide");
        let flipped = Rect::from_min_size(pos2(10.0, 20.0), vec2(-5.0, 450.0));
        assert!(screen_to_slide(flipped, pos2(8.0, 30.0)).is_none(), "an inverted slide");
    }

    #[test]
    fn samples_throttle_stillness_but_keep_motion_or_elapsed_time() {
        let mut live = Vec::new();
        note(&mut live, 1.0, 1.0, 0);
        note(&mut live, 1.1, 1.0, 10);
        assert_eq!(live.len(), 1, "still and under 40 ms");
        note(&mut live, 2.5, 1.0, 10);
        assert_eq!(live.len(), 2, "moved 1.5 points inside the window");
        note(&mut live, 2.6, 1.0, 20);
        assert_eq!(live.len(), 2, "still again");
        note(&mut live, 2.6, 1.0, 50);
        assert_eq!(live.len(), 3, "still, but 40 ms have passed");
        note(&mut live, 2.6, 1.0, 89);
        assert_eq!(live.len(), 3, "39 ms after the kept sample");
    }

    #[test]
    fn samples_drop_invalid_input_and_cap_the_trace() {
        let mut live = Vec::new();
        note(&mut live, f64::NAN, 0.0, 0);
        note(&mut live, 0.0, f64::INFINITY, 0);
        note(&mut live, f64::NEG_INFINITY, f64::NAN, 0);
        assert!(live.is_empty(), "non-finite points never enter a trace");

        for i in 0..MAX_POINTS {
            note(&mut live, i as f64 * 2.0, 0.0, i as u32 * STILL_MS);
        }
        assert_eq!(live.len(), MAX_POINTS);
        note(&mut live, MAX_POINTS as f64 * 2.0, 0.0, MAX_POINTS as u32 * STILL_MS);
        assert_eq!(live.len(), MAX_POINTS, "a full trace takes no more samples");
    }

    #[test]
    fn stashing_empties_the_live_trace_and_tags_the_slide() {
        let mut saved = Vec::new();
        let mut live = vec![(10.0, 20.0, 0), (14.0, 22.0, 45)];
        stash(&mut saved, 3, &mut live);
        assert!(live.is_empty(), "the live trace is always moved out");
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].0, 3);
        assert_eq!(saved[0].1, [(10.0, 20.0, 0), (14.0, 22.0, 45)]);

        stash(&mut saved, 4, &mut live);
        assert_eq!(saved.len(), 1, "an empty trace is not recorded");
        assert!(live.is_empty());
    }

    #[test]
    fn the_callers_recording_time_is_finite_and_clamped_to_the_format_cap() {
        // `canvas.rs` turns the shell clock into slide-relative milliseconds with this shape: a
        // broken or stale clock must neither produce a NaN nor exceed the format's `u32` range.
        let ms = |elapsed: f64| elapsed.is_finite().then(|| (elapsed.max(0.0) * 1000.0).min(3_600_000.0) as u32);
        assert_eq!(ms(1.25), Some(1_250));
        assert_eq!(ms(-3.0), Some(0), "time never runs backwards");
        assert_eq!(ms(7_200.0), Some(3_600_000));
        assert_eq!(ms(f64::NAN), None);
        assert_eq!(ms(f64::INFINITY), None);
    }
}
