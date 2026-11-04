//! Pins the Record view's slide-rendering contract: where [`super::layout`] puts the deck's
//! picture, and that the live shell draws and hit-tests the deck at that same rect, inside the
//! canvas pane between the notes and the bottom strip.
//!
//! Everything here is geometry. The repo carries no image baselines, and a wgpu snapshot of the
//! shell would differ per platform, so the tests assert rects and the accesskit tree instead.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use deckcraft_model::Presentation;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use crate::show::SlideApp;

use super::{EDGE, RecordStatus, RecordView, layout};

/// A fresh deck: one slide with the deck's default slide size.
fn deck() -> Arc<Presentation> {
    let session = deckcraft_engine::Session::with_new();
    session.active().expect("a new session has a document").doc.clone()
}

/// The shell pass rect captured from a running Record view: `show::ui` lays the view out from
/// the `ui.max_rect()` it is handed, kittest insets that behind its own frame margin, so the
/// tests record the very rect from inside the pass instead of guessing it.
type ShellRect = Rc<Cell<Rect>>;

/// A harness running the Record view on its first slide. The returned cell holds the rect of the
/// last full pass, which is the input of `record::layout` in `show::ui`.
fn record_harness() -> (Harness<'static, SlideApp>, ShellRect) {
    let app = SlideApp::new(deckcraft_engine::Session::with_new(), crate::Services::default());
    let shell_rect: ShellRect = Rc::new(Cell::new(Rect::NOTHING));
    let seen = shell_rect.clone();
    let mut h = Harness::builder().with_size(vec2(1920.0, 1200.0)).with_step_dt(0.05).build_ui_state(
        move |ui, app: &mut SlideApp| {
            seen.set(ui.max_rect());
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.state_mut().start_show(0, false);
    h.state_mut().show.as_mut().expect("the show started").record = true;
    h.run_steps(3);
    (h, shell_rect)
}

/// Press and release the primary button at `p`.
fn click_at(harness: &mut Harness<'static, SlideApp>, p: Pos2) {
    for pressed in [true, false] {
        harness.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        harness.step();
    }
}

/// The Record view lays the deck out with its own aspect, inside the canvas pane that sits
/// between the notes pane and the bottom strip, and inside the shell's horizontal margin.
#[test]
fn layout_fits_the_slide_between_the_notes_and_the_bottom() {
    let doc = deck();
    let rec = RecordView::default();
    let full = Rect::from_min_size(pos2(0.0, 0.0), vec2(1920.0, 1200.0));
    let l = layout(full, &doc, &rec);

    // The default view is the Teleprompter, so the notes pane is real: the assertions below are
    // about a slide that had to make room for it.
    assert!(l.notes.height() > 1.0, "the notes pane is open: {:?}", l.notes);
    assert!(l.slide.width() > 1.0 && l.slide.height() > 1.0, "the deck gets a real picture: {:?}", l.slide);

    let deck_aspect = (doc.slide_size.width / doc.slide_size.height) as f32;
    let aspect = l.slide.width() / l.slide.height();
    assert!((aspect - deck_aspect).abs() < 1e-3, "aspect {aspect} keeps the deck's {deck_aspect}");

    assert!(l.slide.min.y >= l.notes.max.y, "the slide starts below the notes: {:?} vs {:?}", l.slide, l.notes);
    assert!(l.slide.max.y <= l.bottom.min.y, "the slide ends above the bottom strip: {:?} vs {:?}", l.slide, l.bottom);
    assert!(l.slide.min.x >= full.min.x + EDGE, "left margin: {:?}", l.slide);
    assert!(l.slide.max.x <= full.max.x - EDGE, "right margin: {:?}", l.slide);
    assert!(full.contains_rect(l.slide), "the slide stays in the window");
    assert!((l.slide.center().x - full.center().x).abs() < 0.01, "the fitted deck is centred in the pane: {:?}", l.slide);
}

/// A window too small for every strip still yields a valid, empty slide rect: the shell may draw
/// no deck, but it must never panic or produce a NaN rect it would later hand to the renderer.
#[test]
fn a_tiny_window_still_yields_a_valid_slide_rect() {
    let doc = deck();
    let rec = RecordView::default();
    for size in [vec2(320.0, 200.0), vec2(1.0, 1.0), vec2(0.0, 0.0)] {
        let full = Rect::from_min_size(pos2(0.0, 0.0), size);
        let l = layout(full, &doc, &rec);
        assert!(
            l.slide.min.x.is_finite() && l.slide.min.y.is_finite() && l.slide.max.x.is_finite() && l.slide.max.y.is_finite(),
            "finite empty rect at {size:?}: {:?}",
            l.slide
        );
        assert!(l.slide.width() >= 0.0 && l.slide.height() >= 0.0, "no negative extents at {size:?}: {:?}", l.slide);
        assert_eq!((l.slide.width(), l.slide.height()), (0.0, 0.0), "no room means no picture at {size:?}: {:?}", l.slide);
    }
}

/// With the Record view open the shell draws the deck at [`layout`]'s slide rect, and its chrome
/// (top bar, notes, bottom strip) stays off that rect. The normal full-screen show toolbar is not
/// drawn in this mode; the Record top bar's Record button is.
#[test]
fn the_shell_draws_the_deck_inside_the_record_canvas() {
    let (h, shell_rect) = record_harness();
    let full = shell_rect.get();
    assert!(full.width() > 1.0 && full.height() > 1.0, "the shell ran a real pass: {full:?}");

    let state = h.state();
    let show = state.show.as_ref().expect("still showing");
    assert!(show.record, "the Record view is open");
    assert_eq!(show.record_view.status, RecordStatus::Idle, "opening the view does not start a take");

    let doc = state.session.active().expect("a deck is open").doc.clone();
    let l = layout(full, &doc, &show.record_view);
    // `record::layout` is a pure function of the pass rect, and `show::ui` uses this very rect
    // for both the renderer's slide rect (`srect`) and the Record canvas. The hit-test test below
    // pins that equality against the live shell through its click-zoom mapping.
    assert!(l.slide.width() > 1.0 && l.slide.height() > 1.0, "the shell draws a real deck: {:?}", l.slide);

    let deck_aspect = (doc.slide_size.width / doc.slide_size.height) as f32;
    assert!((l.slide.width() / l.slide.height() - deck_aspect).abs() < 1e-3, "the shell keeps the deck's aspect: {:?}", l.slide);

    // The Record button sits in the top bar that `record::ui` draws from `layout.top`, so its
    // place proves the recomputed layout is the live one.
    let record = h.query_by_label("Record").expect("the Record top bar is drawn");
    assert!(l.top.contains_rect(record.rect()), "the Record button lives in the top bar: {:?} vs {:?}", record.rect(), l.top);

    for (what, pane) in [("top bar", l.top), ("notes pane", l.notes), ("bottom strip", l.bottom)] {
        assert!(!l.slide.intersects(pane), "the {what} does not cover the deck: {:?} vs {:?}", l.slide, pane);
    }
    assert!((l.slide.center().x - full.center().x).abs() < 0.01, "the deck is centred in the pane: {:?}", l.slide);

    // Record mode replaces the full-screen show toolbar, so none of its buttons are in the tree.
    assert!(h.query_by_label("Black").is_none(), "the normal show toolbar's Black is not drawn");
    assert!(h.query_by_label("All").is_none(), "the normal show toolbar's All is not drawn");
}

/// The shell hit-tests the deck at the rect [`layout`] computes: arming click-zoom and clicking
/// the middle of that rect zooms on the deck's centre, while a click just outside its left edge
/// does not. The click-zoom path maps the pointer through the shell's own `srect`
/// (`show::ui`), so the two probes pin the rendered rect to the layout instead of assuming it.
#[test]
fn the_shell_hit_tests_the_deck_at_the_layout_slide_rect() {
    let (mut h, shell_rect) = record_harness();
    let full = shell_rect.get();
    let state = h.state();
    let doc = state.session.active().expect("a deck is open").doc.clone();
    let show = state.show.as_ref().expect("still showing");
    let l = layout(full, &doc, &show.record_view);
    let (slide, full_rect) = (l.slide, full);

    h.state_mut().show.as_mut().expect("still showing").zoom_arm = true;
    click_at(&mut h, slide.center());
    let show = h.state().show.as_ref().expect("still showing");
    let (nx, ny, _) = show.zoom_view().expect("the click landed on the deck");
    assert!((nx - 0.5).abs() < 0.01 && (ny - 0.5).abs() < 0.01, "the deck's centre maps to its middle: ({nx}, {ny})");

    // Four points left of the deck's left edge, in the pane's letterbox: outside the live rect.
    let outside = pos2(slide.min.x - 4.0, slide.center().y);
    assert!(full_rect.contains(outside), "the probe sits in the window: {outside:?}");
    h.state_mut().show.as_mut().expect("still showing").zoom_arm = true;
    click_at(&mut h, outside);
    let show = h.state().show.as_ref().expect("still showing");
    assert!(show.zoom_arm, "a click beside the deck does not zoom");
    let (nx, ny, _) = show.zoom_view().expect("the earlier zoom is kept");
    assert!((nx - 0.5).abs() < 0.01 && (ny - 0.5).abs() < 0.01, "and it still shows the deck's middle: ({nx}, {ny})");
}
