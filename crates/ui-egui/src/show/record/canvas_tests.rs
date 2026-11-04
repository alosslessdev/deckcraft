//! The Record view canvas driven with the mouse: the Cameo bubble's place and gestures, its
//! interaction with show navigation, and the ink/laser layers that are captured into the deck.
//!
//! The repo has no image baselines and a wgpu snapshot would differ per platform, so these use
//! kittest's live widget tree and the show's own state as the snapshot of what was drawn.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use egui::{Color32, Event, Modifiers, PointerButton, Pos2, Rect, vec2};
use egui_kittest::Harness;

use crate::SlideApp;

use super::{InkTool, Layout, RecordStatus, RecordView, layout};

/// A two-slide blank deck, in the Record view on slide 0, recording.
fn record_harness() -> Harness<'static, SlideApp> {
    let mut session = deckcraft_engine::Session::with_new();
    session.execute("slide.new", &serde_json::json!({"layout": "blank"})).unwrap();
    session.execute("slide.go", &serde_json::json!({"index": 0})).unwrap();
    let app = SlideApp::new(session, crate::Services::default());
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_step_dt(0.05).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(2);
    h.state_mut().start_show(0, false);
    {
        let show = h.state_mut().show.as_mut().expect("a show started");
        show.record = true;
        show.record_view.status = RecordStatus::Recording;
    }
    h.run_steps(2);
    h
}

fn shell_layout(h: &Harness<'_, SlideApp>) -> Layout {
    let rec = &h.state().show.as_ref().expect("a show").record_view;
    let doc = h.state().session.active().expect("a document").doc.clone();
    layout(h.ctx().content_rect(), &doc, rec)
}

fn rec(h: &Harness<'_, SlideApp>) -> &RecordView {
    &h.state().show.as_ref().expect("a show").record_view
}

fn rec_mut(h: &mut Harness<'_, SlideApp>) -> &mut RecordView {
    &mut h.state_mut().show.as_mut().expect("a show").record_view
}

fn slide(h: &Harness<'_, SlideApp>) -> usize {
    h.state().show.as_ref().expect("a show").state.slide
}

fn click_at(h: &mut Harness<'_, SlideApp>, p: Pos2) {
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
}

fn drag(h: &mut Harness<'_, SlideApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for i in 1..=8 {
        h.hover_at(from + (to - from) * (i as f32 / 8.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(2);
}

#[test]
fn the_bubble_starts_circular_at_the_bottom_right_of_the_slide() {
    let h = record_harness();
    let l = shell_layout(&h);
    let r = rec(&h).cameo.rect(l.slide);
    assert!((r.width() - r.height()).abs() < 0.01, "circular: {r:?}");
    assert!(l.slide.contains_rect(r), "{r:?} inside {:?}", l.slide);
    assert!(r.center().x > l.slide.center().x && r.center().y > l.slide.center().y, "bottom-right");
    // Default geometry mirrors `insert.cameo`'s 2.25 in circle and 6.5 pt margin.
    let edge = (l.slide.right() - r.right()).max(l.slide.bottom() - r.bottom());
    let gap = (l.slide.right() - r.right()).min(l.slide.bottom() - r.bottom());
    assert!((edge - gap).abs() < 0.01, "equal margins");
}

#[test]
fn a_click_on_the_bubble_does_not_advance_the_show() {
    let mut h = record_harness();
    let l = shell_layout(&h);
    click_at(&mut h, rec(&h).cameo.rect(l.slide).center());
    assert_eq!(slide(&h), 0, "the bubble takes the click");
    let outside = Pos2::new(l.slide.left() + l.slide.width() * 0.2, l.slide.top() + l.slide.height() * 0.2);
    click_at(&mut h, outside);
    assert_eq!(slide(&h), 1, "a click on the slide still advances");
}

#[test]
fn dragging_the_bubble_moves_it_and_keeps_it_inside_the_slide() {
    let mut h = record_harness();
    let l = shell_layout(&h);
    let before = rec(&h).cameo.rect(l.slide);
    drag(&mut h, before.center(), Pos2::new(l.slide.left() + 4.0, l.slide.top() + 4.0));
    let after = rec(&h).cameo.rect(l.slide);
    assert!(l.slide.contains_rect(after), "{after:?} stays inside {:?}", l.slide);
    assert!(after.center().x < before.center().x && after.center().y < before.center().y, "it moved");
    assert_eq!(slide(&h), 0, "dragging the bubble is not navigation");
}

#[test]
fn dragging_the_handle_resizes_the_bubble_and_clamps_it() {
    let mut h = record_harness();
    let l = shell_layout(&h);
    let before = rec(&h).cameo.rect(l.slide);
    let handle = rec(&h).cameo.handle_rect(l.slide).center();
    drag(&mut h, handle, handle + vec2(500.0, 500.0));
    let after = rec(&h).cameo.rect(l.slide);
    assert!(after.width() > before.width(), "the corner drag grows the bubble");
    assert!(l.slide.contains_rect(after), "{after:?} stays inside {:?}", l.slide);
    // Far outside every edge: still a circle on the slide.
    drag(&mut h, rec(&h).cameo.rect(l.slide).center(), Pos2::new(-5000.0, -5000.0));
    let clamped = rec(&h).cameo.rect(l.slide);
    assert!(l.slide.contains_rect(clamped));
    assert!((clamped.width() - clamped.height()).abs() < 0.01, "still circular");
}

#[test]
fn a_fake_camera_frame_becomes_the_bubble_texture() {
    let mut h = record_harness();
    let image = egui::ColorImage::filled([8, 8], Color32::from_rgb(12, 34, 56));
    let ctx = h.ctx().clone();
    rec_mut(&mut h).set_cameo_frame(&ctx, image);
    h.run_steps(2);
    assert!(rec(&h).cameo_frame.is_some(), "the frame is kept for the canvas");
    rec_mut(&mut h).clear_cameo_frame();
    assert!(rec(&h).cameo_frame.is_none(), "a disconnected camera drops the picture");
}

#[test]
fn pen_strokes_are_drawn_and_stashed_into_the_recording_on_slide_change() {
    let mut h = record_harness();
    let l = shell_layout(&h);
    {
        let show = h.state_mut().show.as_mut().expect("a show");
        show.record_view.ink.tool = InkTool::Pen;
        show.set_pen(true);
    }
    let a = Pos2::new(l.slide.left() + l.slide.width() * 0.3, l.slide.top() + l.slide.height() * 0.4);
    let b = Pos2::new(l.slide.left() + l.slide.width() * 0.6, l.slide.top() + l.slide.height() * 0.5);
    drag(&mut h, a, b);
    assert!(!h.state().show.as_ref().unwrap().strokes.is_empty(), "a stroke was captured");
    let doc = h.state().session.active().unwrap().doc.clone();
    let now = h.ctx.input(|i| i.time);
    {
        let show = h.state_mut().show.as_mut().unwrap();
        let action = show.state.next(&doc);
        show.apply(action, &doc, now);
    }
    let show = h.state().show.as_ref().unwrap();
    assert!(show.recorded.iter().any(|(s, pts)| *s == 0 && pts.len() >= 2), "slide 0 ink is in the recording");
    assert!(show.strokes.is_empty(), "the live buffer is cleared");
}

#[test]
fn the_eraser_removes_a_whole_stroke_it_touches() {
    let mut h = record_harness();
    let l = shell_layout(&h);
    {
        let show = h.state_mut().show.as_mut().unwrap();
        show.record_view.ink.tool = InkTool::Eraser;
        show.set_pen(true);
        show.strokes = vec![vec![egui::pos2(0.2, 0.5), egui::pos2(0.8, 0.5)]];
    }
    let a = Pos2::new(l.slide.left() + l.slide.width() * 0.4, l.slide.top() + l.slide.height() * 0.5);
    let b = Pos2::new(l.slide.left() + l.slide.width() * 0.5, l.slide.top() + l.slide.height() * 0.5);
    drag(&mut h, a, b);
    assert!(h.state().show.as_ref().unwrap().strokes.is_empty(), "the touched stroke is gone");
}

#[test]
fn laser_samples_are_recorded_in_slide_points() {
    let mut h = record_harness();
    let l = shell_layout(&h);
    {
        let show = h.state_mut().show.as_mut().unwrap();
        show.set_laser(true);
        show.record_view.status = RecordStatus::Recording;
    }
    h.hover_at(l.slide.center());
    h.run_steps(3);
    let show = h.state().show.as_ref().unwrap();
    assert!(!show.laser_live.is_empty(), "a hovered laser leaves samples");
    let sample = show.laser_live[0];
    let size = h.state().session.active().unwrap().doc.slide_size;
    assert!(sample.0 >= 0.0 && sample.0 <= size.width);
    assert!(sample.1.is_finite() && sample.2 < 3_600_000);
    let slide = show.state.slide;
    {
        let show = h.state_mut().show.as_mut().unwrap();
        show.stash_laser(slide);
    }
    assert!(h.state().show.as_ref().unwrap().laser_saved.iter().any(|(s, pts)| *s == slide && !pts.is_empty()));
}

#[test]
fn a_hidden_bubble_does_not_take_clicks_or_hits() {
    let mut h = record_harness();
    let l = shell_layout(&h);
    let center = rec(&h).cameo.rect(l.slide).center();
    rec_mut(&mut h).camera_bubble = false;
    assert!(!rec(&h).cameo_contains(&l, center));
    click_at(&mut h, center);
    assert_eq!(slide(&h), 1, "with the bubble hidden the slide takes the click");
}
