//! The camera-preview control beside the Views dropdown: a person/camera icon with a chevron
//! that toggles the on-slide camera preview and opens its small options popover.

use egui::{Color32, CornerRadius, Rect, RichText, Sense, Stroke, Ui, WidgetInfo, pos2};

use super::super::{Layout, RecordView, devices};
use crate::widgets::chevron;

/// Width of the chevron-only target at the right of the button.
const CHEVRON_W: f32 = 9.0;

/// Margin around the glyph inside the button.
const PAD: f32 = 4.0;

/// Draw the camera-preview toggle. Returns whether a click changed the preview state.
pub(super) fn ui(ui: &mut Ui, rec: &mut RecordView, layout: &Layout) -> bool {
    let r = layout.camera_preview;
    if !r.is_positive() {
        return false;
    }
    // The right strip is a separate target so the chevron opens the options menu and the body
    // only toggles the preview, like a split button.
    let main_rect = Rect::from_min_max(r.min, pos2(r.max.x - CHEVRON_W, r.max.y));
    let chev_rect = Rect::from_min_max(pos2(r.max.x - CHEVRON_W, r.min.y), r.max);

    let main = ui.interact(main_rect, ui.id().with("record_camera_preview"), Sense::click());
    let chev = ui.interact(chev_rect, ui.id().with("record_camera_chevron"), Sense::click());

    // Both flags are the same switch: the bubble is the on-slide preview, so a half-set state
    // reads as off until the next click puts them back in step.
    let on = rec.camera_preview && rec.camera_bubble;
    let hovered = main.hovered() || chev.hovered();

    let painter = ui.painter_at(r);
    painter.rect_filled(r, CornerRadius::same(5), if hovered { Color32::from_gray(58) } else { Color32::from_gray(38) });
    if chev.hovered() {
        painter.rect_filled(chev_rect, CornerRadius::same(5), Color32::from_gray(68));
    }
    person(&painter, main_rect, on);
    chevron(&painter, chev_rect.center(), 2.8, Color32::from_gray(200));
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    main.widget_info(|| WidgetInfo::selected(egui::Role::CheckBox, true, on, "Camera preview"));
    let main = main.on_hover_text(if on { "Hide camera preview" } else { "Show camera preview" });
    chev.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, "Camera preview options"));

    let mut changed = false;
    if main.clicked() {
        changed |= set_preview(rec, !on);
    }

    // The popover repeats the switch and reports the camera without ever claiming a live image.
    let note = camera_note(rec.devices.camera.status, rec.devices.camera.selected.as_deref());
    egui::Popup::menu(&chev)
        .anchor(r)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(210.0)
        .show(|ui| {
            if ui.checkbox(&mut rec.camera_preview, "Camera preview").changed() {
                let wanted = rec.camera_preview;
                changed |= set_preview(rec, wanted);
            }
            ui.separator();
            ui.add_enabled(false, egui::Label::new(RichText::new(&note).size(11.0).color(Color32::from_gray(150))));
        });

    changed
}

/// Keep the preview window and the on-slide camera bubble on the same switch. Returns whether
/// either flag actually moved.
pub(super) fn set_preview(rec: &mut RecordView, on: bool) -> bool {
    let changed = rec.camera_preview != on || rec.camera_bubble != on;
    rec.camera_preview = on;
    rec.camera_bubble = on;
    changed
}

/// The status line under the preview switch. It states what the backend reports and names the
/// chosen device; it never implies that frames are flowing when they are not.
pub(super) fn camera_note(status: devices::DeviceStatus, selected: Option<&str>) -> String {
    let base = match status {
        devices::DeviceStatus::Unavailable => "No camera found",
        devices::DeviceStatus::Off => "Camera off",
        devices::DeviceStatus::On => "Camera on",
        devices::DeviceStatus::Denied => "Camera access denied",
        devices::DeviceStatus::Busy => "Camera in use",
    };
    match selected {
        Some(name) => format!("{base} — {name}"),
        None => base.to_owned(),
    }
}

/// A presenter silhouette: a head circle over a shoulders arc, drawn from painter primitives.
/// A small green dot marks that the preview is on.
fn person(painter: &egui::Painter, area: Rect, live: bool) {
    let ink = Color32::from_gray(205);
    let cx = area.center().x;
    let head_y = area.min.y + PAD + 5.5;
    painter.circle_filled(pos2(cx, head_y), 2.9, ink);

    // A flattened upper semicircle (screen y grows down, so the arc rises from its ends).
    const STEPS: usize = 12;
    let base = area.max.y - PAD - 4.5;
    let radius = (area.width() * 0.5 - PAD).clamp(1.0, 6.4);
    let pts: Vec<_> = (0..=STEPS)
        .map(|i| {
            let t = i as f32 / STEPS as f32;
            let a = std::f32::consts::PI + t * std::f32::consts::PI;
            pos2(cx + radius * a.cos(), base + radius * 0.9 * a.sin())
        })
        .collect();
    painter.add(egui::Shape::line(pts, Stroke::new(1.5, ink)));

    if live {
        painter.circle_filled(pos2(area.max.x - 3.5, area.min.y + PAD - 0.5), 1.9, Color32::from_rgb(120, 200, 140));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::record::devices::DeviceStatus;

    #[test]
    fn the_note_names_every_camera_state() {
        assert_eq!(camera_note(DeviceStatus::Unavailable, None), "No camera found");
        assert_eq!(camera_note(DeviceStatus::Off, None), "Camera off");
        assert_eq!(camera_note(DeviceStatus::On, None), "Camera on");
        assert_eq!(camera_note(DeviceStatus::Denied, None), "Camera access denied");
        assert_eq!(camera_note(DeviceStatus::Busy, None), "Camera in use");
    }

    #[test]
    fn the_note_appends_the_selected_device() {
        assert_eq!(camera_note(DeviceStatus::On, Some("Logitech C920")), "Camera on — Logitech C920");
        assert_eq!(camera_note(DeviceStatus::Unavailable, Some("Face Cam")), "No camera found — Face Cam");
        assert_eq!(camera_note(DeviceStatus::Busy, Some("Face Cam")), "Camera in use — Face Cam");
    }

    #[test]
    fn set_preview_moves_the_toggle_and_the_bubble_together() {
        let mut rec = RecordView::default();
        rec.camera_preview = false;
        rec.camera_bubble = true;
        assert!(set_preview(&mut rec, true));
        assert!(rec.camera_preview && rec.camera_bubble);
        assert!(!set_preview(&mut rec, true));
        assert!(set_preview(&mut rec, false));
        assert!(!rec.camera_preview && !rec.camera_bubble);
    }

    #[test]
    fn set_preview_reports_a_half_set_state_as_a_change() {
        let mut rec = RecordView::default();
        rec.camera_preview = true;
        rec.camera_bubble = false;
        assert!(set_preview(&mut rec, true));
        assert!(rec.camera_bubble);
        rec.camera_bubble = false;
        assert!(set_preview(&mut rec, true));
    }
}
