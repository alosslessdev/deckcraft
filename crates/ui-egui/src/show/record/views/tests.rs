//! The Views dropdown, camera preview and per-view layout: every entry switches the shell and
//! leaves the pane/state combination PowerPoint's recording studio shows.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use deckcraft_engine::Session;
use deckcraft_model::Presentation;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use super::*;
use crate::show::Show;
use crate::{Services, SlideApp};

use super::super::{Layout, RecordView};

struct ViewHarness {
    app: SlideApp,
    show: Show,
    doc: Arc<Presentation>,
    rec: RecordView,
    layout: Layout,
}

fn deck() -> (Session, Arc<Presentation>) {
    let mut session = Session::with_new();
    for _ in 0..2 {
        session.execute("slide.new", &json!({})).unwrap();
    }
    let doc = session.active().unwrap().doc.clone();
    (session, doc)
}

fn view_harness(view: ViewKind) -> Harness<'static, ViewHarness> {
    let (session, doc) = deck();
    let app = SlideApp::new(session, Services::default());
    let show = Show::new(&doc, 0, false, false);
    let mut rec = RecordView::default();
    select(&mut rec, view);
    let full = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1400.0, 900.0));
    let layout = super::super::layout(full, &doc, &rec);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, s: &mut ViewHarness| {
                let ViewHarness { app, show, doc, rec, layout } = s;
                let _ = super::ui(app, ui, show, doc, rec, layout);
            },
            ViewHarness { app, show, doc, rec, layout },
        );
    h.run_steps(3);
    h
}

#[test]
fn menu_order_matches_powerpoint() {
    assert_eq!(ViewKind::ALL.map(ViewKind::label), ["Teleprompter", "Presenter View", "Slide View"]);
    assert_eq!(ViewKind::default(), ViewKind::Teleprompter);
    assert!(!ViewKind::Slide.shows_notes());
    assert_eq!(ViewKind::ALL.map(ViewKind::id), ["teleprompter", "presenter", "slide"]);
}

#[test]
fn each_view_reserves_its_panes() {
    let (_session, doc) = deck();
    let mut rec = RecordView::default();
    let full = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1400.0, 900.0));

    select(&mut rec, ViewKind::Teleprompter);
    let tele = super::super::layout(full, &doc, &rec);
    assert!(tele.notes.height() > 100.0, "teleprompter keeps a large script");
    assert!(tele.next.is_none(), "no next strip in teleprompter");
    assert!(tele.thumbs.is_none(), "no thumbnail rail in teleprompter");

    select(&mut rec, ViewKind::Presenter);
    let presenter = super::super::layout(full, &doc, &rec);
    assert!(presenter.notes.height() > 40.0, "presenter keeps the compact script");
    assert!(presenter.next.is_some_and(|r| r.height() > 30.0), "next-slide strip");
    assert!(presenter.thumbs.is_none(), "no thumbnail rail in presenter");

    select(&mut rec, ViewKind::Slide);
    let slide = super::super::layout(full, &doc, &rec);
    assert_eq!(slide.notes.height(), 0.0, "slide view hides the script");
    assert!(slide.next.is_none(), "no next strip in slide view");
    assert!(slide.thumbs.is_some_and(|r| r.width() > 40.0), "slide view has the thumbnail rail");
    assert!(slide.slide.width() > 0.0 && slide.slide.height() > 0.0, "slide still fits the remaining canvas");
}

#[test]
fn selecting_a_view_leaves_the_right_pane_state() {
    let mut rec = RecordView::default();
    rec.teleprompter.auto = true;
    select(&mut rec, ViewKind::Presenter);
    assert_eq!(rec.view, ViewKind::Presenter);
    assert!(rec.notes_open);
    assert!(!rec.teleprompter.auto, "leaving teleprompter stops auto-scroll");

    rec.teleprompter.auto = true;
    select(&mut rec, ViewKind::Teleprompter);
    assert!(rec.notes_open);
    assert!(rec.teleprompter.auto, "returning does not invent a stopped state");

    select(&mut rec, ViewKind::Slide);
    assert!(!rec.notes_open, "slide view is slide only");
    assert!(!rec.teleprompter.auto);
}

#[test]
fn the_dropdown_switches_every_view_with_the_mouse() {
    let mut h = view_harness(ViewKind::Teleprompter);
    h.get_by_label("Views").click();
    h.run_steps(2);
    h.get_by_label("Presenter View").click();
    h.run_steps(2);
    assert_eq!(h.state().rec.view, ViewKind::Presenter);
    assert!(h.state().rec.notes_open);

    h.get_by_label("Views").click();
    h.run_steps(2);
    h.get_by_label("Slide View").click();
    h.run_steps(2);
    assert_eq!(h.state().rec.view, ViewKind::Slide);
    assert!(!h.state().rec.notes_open);

    h.get_by_label("Views").click();
    h.run_steps(2);
    h.get_by_label("Teleprompter").click();
    h.run_steps(2);
    assert_eq!(h.state().rec.view, ViewKind::Teleprompter);
    assert!(h.state().rec.notes_open);
}

#[test]
fn the_camera_icon_toggles_the_on_slide_preview() {
    let mut h = view_harness(ViewKind::Teleprompter);
    assert!(!h.state().rec.camera_preview && h.state().rec.camera_bubble);
    h.get_by_label("Camera preview").click();
    h.run_steps(2);
    assert!(h.state().rec.camera_preview && h.state().rec.camera_bubble, "first click shows both");
    h.get_by_label("Camera preview").click();
    h.run_steps(2);
    assert!(!h.state().rec.camera_preview && !h.state().rec.camera_bubble, "second click hides both");
}

#[test]
fn presenter_view_shows_the_next_slide_and_slide_view_shows_thumbnails() {
    let mut h = view_harness(ViewKind::Presenter);
    h.get_by_label("Next slide 2").rect();
    assert!(h.state().layout.next.is_some());

    let mut h = view_harness(ViewKind::Slide);
    h.get_by_label("Slide 2 thumbnail").rect();
    h.get_by_label("Slide 2 thumbnail").click();
    h.run_steps(3);
    assert_eq!(h.state().show.state.slide, 1, "thumbnail navigates the recording");
}
