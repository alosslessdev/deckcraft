//! The Record view's Views dropdown and camera preview.
//!
//! PowerPoint's recording studio switches between three layouts from one dropdown in the
//! lower-right corner: Teleprompter (big script, slide below), Presenter View (slide, compact
//! script and a next-slide strip) and Slide View (the slide alone). The person/camera control
//! beside it toggles the on-slide camera preview.
//!
//! The menu owns the state; each layout's extra chrome lives in its own module so the shell can
//! keep this file to the shared switch and camera toggle.

mod camera;
mod presenter;
mod slide;
mod teleprompter;

use std::sync::Arc;

use deckcraft_model::Presentation;
use egui::{Color32, CornerRadius, Sense, Ui, pos2};

use super::{Layout, RecordView};
use crate::SlideApp;
use crate::show::Show;
use crate::theme;
use crate::widgets::check_item;

/// How the Record view splits the screen between the script and the slide.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ViewKind {
    /// A large script with the slide below it.
    #[default]
    Teleprompter,
    /// The slide with a compact script and a next-slide strip.
    Presenter,
    /// The slide alone, with a thumbnail rail for navigation.
    Slide,
}

impl ViewKind {
    /// Menu order, matching PowerPoint's dropdown.
    pub(super) const ALL: [ViewKind; 3] = [ViewKind::Teleprompter, ViewKind::Presenter, ViewKind::Slide];

    pub(super) fn label(self) -> &'static str {
        match self {
            ViewKind::Teleprompter => "Teleprompter",
            ViewKind::Presenter => "Presenter View",
            ViewKind::Slide => "Slide View",
        }
    }

    /// Stable id for the control channel and tests.
    pub(super) fn id(self) -> &'static str {
        match self {
            ViewKind::Teleprompter => "teleprompter",
            ViewKind::Presenter => "presenter",
            ViewKind::Slide => "slide",
        }
    }

    /// The notes pane is visible in Teleprompter and Presenter View; Slide View is slide only.
    pub(super) fn shows_notes(self) -> bool {
        self != ViewKind::Slide
    }
}

pub(super) struct Out {
    pub changed: bool,
}

/// Draw the camera control and the Views dropdown. The shell calls this last so the menu and the
/// camera popover paint above the canvas.
pub(super) fn ui(
    app: &mut SlideApp,
    ui: &mut Ui,
    show: &mut Show,
    doc: &Arc<Presentation>,
    rec: &mut RecordView,
    layout: &Layout,
) -> Out {
    let now = ui.ctx().input(|i| i.time);
    let mut changed = camera::ui(ui, rec, layout);
    changed |= match rec.view {
        ViewKind::Teleprompter => teleprompter::ui(ui, rec, layout),
        ViewKind::Presenter => presenter::ui(app, ui, show, doc, rec, layout),
        ViewKind::Slide => slide::ui(app, ui, show, doc, now, layout),
    };
    changed |= menu(ui, rec, layout);
    Out { changed }
}

/// Select `view` and put the panes into the state that view starts in.
pub(super) fn select(rec: &mut RecordView, view: ViewKind) {
    rec.view = view;
    rec.notes_open = view.shows_notes();
    if view != ViewKind::Teleprompter {
        rec.teleprompter.auto = false;
    }
}

fn menu(ui: &mut Ui, rec: &mut RecordView, layout: &Layout) -> bool {
    let r = layout.views;
    let resp = ui.interact(r, ui.id().with("record_views"), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(r, CornerRadius::same(5), if resp.hovered() { Color32::from_gray(58) } else { Color32::from_gray(38) });
    painter.text(
        pos2(r.min.x + 10.0, r.center().y),
        egui::Align2::LEFT_CENTER,
        format!("Views: {}", rec.view.label()),
        theme::font(12.0),
        Color32::from_gray(220),
    );
    painter.text(
        pos2(r.max.x - 10.0, r.center().y),
        egui::Align2::RIGHT_CENTER,
        "▾",
        theme::font(12.0),
        Color32::from_gray(200),
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Views"));

    let mut changed = false;
    egui::Popup::menu(&resp).width(190.0).show(|ui| {
        for view in ViewKind::ALL {
            if check_item(ui, view.label(), true, rec.view == view).clicked() {
                select(rec, view);
                changed = true;
                ui.close();
            }
        }
        ui.separator();
        if ui.checkbox(&mut rec.notes_open, "Notes pane").changed() {
            changed = true;
        }
        let on = rec.camera_bubble && rec.camera_preview;
        if ui.checkbox(&mut rec.camera_bubble, "Camera bubble").changed() {
            let wanted = rec.camera_bubble;
            camera::set_preview(rec, wanted);
            changed = wanted != on;
        }
    });
    changed
}

#[cfg(test)]
mod tests;
