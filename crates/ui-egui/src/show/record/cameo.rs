//! The live Cameo camera bubble drawn over the recorded slide.
//!
//! PowerPoint keeps the bubble circular, places it at the bottom-right of the slide by default
//! and lets the presenter move or resize it while the show runs. The geometry here is stored in
//! slide fractions so it survives slide-size changes and stays inside the slide when dragged or
//! resized. Recording itself is separate: this module only paints the live preview and turns
//! pointer drags into clamped geometry changes.
//!
//! The preview is honest: a frame is drawn only when the camera backend actually supplied a
//! texture with pixels. Without one the bubble is a darker disc labelled "Camera off", never a
//! frozen or invented picture.

use std::f32::consts::{FRAC_1_SQRT_2, TAU};

use egui::{Color32, Mesh, Pos2, Rect, Sense, Shape, Stroke, TextureHandle, TextureId, Ui, Vec2, pos2, vec2};

/// Smallest and largest bubble radius, as a fraction of the slide's shorter side.
pub(super) const MIN_RADIUS: f32 = 0.06;
pub(super) const MAX_RADIUS: f32 = 0.45;
/// Gap between the bubble and the slide edge, as a fraction of the shorter side. Matches the
/// 6.5 pt margin of the Cameo placeholder `insert.cameo` adds for a 7.5 in-high slide.
const EDGE: f32 = 0.012;
/// Default radius: half of PowerPoint's 2.25 in Cameo, as a fraction of the shorter side.
const DEFAULT_RADIUS: f32 = 0.15;
/// Handle radius in screen pixels (the grab target around a point on the circle).
const HANDLE: f32 = 9.0;
/// Segments of the circular texture crop.
const SEGMENTS: usize = 48;
/// Ring stroke width and shadow drop in screen pixels.
const RING_WIDTH: f32 = 2.0;
const SHADOW_OFFSET: f32 = 3.0;
const SHADOW_ALPHA: u8 = 110;
/// Camera-off placeholder colours.
const OFF_DISC: Color32 = Color32::from_gray(28);
const OFF_LABEL: Color32 = Color32::from_gray(190);

/// What a bubble drag is changing.
///
/// The shell currently drives gestures through `cameo_input`; this stays the bubble's own entry
/// point so its geometry can be exercised without the shell's pointer plumbing.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Grab {
    Move,
    Resize,
}

/// Cameo bubble geometry. `center` is `[x, y]` in slide fractions; `radius` is a fraction of the
/// slide's shorter side. `placed` is false until the presenter moves the bubble, so the default
/// bottom-right anchor is computed from the slide's aspect ratio instead of baking in one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cameo {
    pub center: [f32; 2],
    pub radius: f32,
    placed: bool,
}

impl Default for Cameo {
    fn default() -> Self {
        Self { center: [0.0, 0.0], radius: DEFAULT_RADIUS, placed: false }
    }
}

impl Cameo {
    /// Centre in slide fractions, resolving the bottom-right default for this slide.
    pub(crate) fn center_on(&self, slide: Rect) -> [f32; 2] {
        if self.placed {
            return self.center;
        }
        let (w, h) = (slide.width().max(1.0), slide.height().max(1.0));
        let r = self.radius_px(slide);
        let edge = edge_px(slide);
        [((w - edge - r) / w).clamp(0.0, 1.0), ((h - edge - r) / h).clamp(0.0, 1.0)]
    }

    /// Centre in screen points.
    fn center_px(&self, slide: Rect) -> Pos2 {
        let [x, y] = self.center_on(slide);
        pos2(slide.left() + x * slide.width(), slide.top() + y * slide.height())
    }

    /// Radius in screen pixels for `slide`.
    fn radius_px(&self, slide: Rect) -> f32 {
        self.radius.clamp(MIN_RADIUS, MAX_RADIUS) * short_side(slide)
    }

    /// The circle's bounding square on screen, centred on the resolved centre.
    pub(crate) fn rect(&self, slide: Rect) -> Rect {
        let r = self.radius_px(slide);
        Rect::from_center_size(self.center_px(slide), vec2(2.0 * r, 2.0 * r))
    }

    /// The grab handle: a point on the circle at 45 degrees down-right, the corner a presenter
    /// drags.
    pub(crate) fn handle_rect(&self, slide: Rect) -> Rect {
        let r = self.radius_px(slide);
        let offset = vec2(r * FRAC_1_SQRT_2, r * FRAC_1_SQRT_2);
        Rect::from_center_size(self.center_px(slide) + offset, Vec2::splat(2.0 * HANDLE))
    }

    /// Whether `p` (screen points) is on the bubble or its resize handle, so a click there must
    /// not advance the show or start an ink stroke.
    pub(crate) fn hits(&self, slide: Rect, p: Pos2) -> bool {
        let r = self.radius_px(slide);
        (p - self.center_px(slide)).length() <= r + HANDLE || self.handle_rect(slide).contains(p)
    }

    /// Move by a screen-space delta and keep the circle inside `slide`.
    pub(crate) fn drag_by(&mut self, slide: Rect, delta: Vec2) {
        if !delta.x.is_finite() || !delta.y.is_finite() {
            return;
        }
        let (w, h) = (slide.width().max(1.0), slide.height().max(1.0));
        let [x, y] = self.center_on(slide);
        self.center = [x + delta.x / w, y + delta.y / h];
        self.placed = true;
        self.clamp(slide);
    }

    /// Set the radius from the pointer's distance to the centre (a corner drag), then clamp.
    pub(crate) fn resize_to(&mut self, slide: Rect, pointer: Pos2) {
        let center = self.rect(slide).center();
        let len = (pointer - center).length();
        if !len.is_finite() {
            return;
        }
        self.radius = (len / short_side(slide)).clamp(MIN_RADIUS, MAX_RADIUS);
        self.placed = true;
        self.clamp(slide);
    }

    /// Keep the whole circle inside the slide, shrinking it when the slide is too small for the
    /// usual radius and edge gap.
    pub(crate) fn clamp(&mut self, slide: Rect) {
        let (w, h) = (slide.width().max(1.0), slide.height().max(1.0));
        let short = w.min(h);
        let edge = EDGE * short;
        // The circle plus its edge gap must fit in half the shorter side. On a slide too small
        // for the usual radius the radius shrinks rather than the circle spilling out.
        let max_r = ((short * 0.5 - edge) / short).clamp(0.0, MAX_RADIUS);
        let min_r = MIN_RADIUS.min(max_r);
        self.radius = if self.radius.is_finite() { self.radius.clamp(min_r, max_r) } else { DEFAULT_RADIUS.clamp(min_r, max_r) };
        // Until the presenter moves the bubble the default anchor already keeps it inside; the
        // resolved centre is re-derived every frame, so the bubble stays bottom-right on resize.
        if !self.placed {
            return;
        }
        let r = self.radius_px(slide);
        let xlo = ((r + edge) / w).min(0.5);
        let ylo = ((r + edge) / h).min(0.5);
        let [x, y] = self.center;
        self.center = [x.clamp(xlo, 1.0 - xlo), y.clamp(ylo, 1.0 - ylo)];
    }

    /// Draw or resize the bubble from the pointer. `grab` remembers whether the drag started on
    /// the circle body (move) or on its handle (resize). Returns true when the pointer is over
    /// the bubble this frame.
    ///
    /// The shell's canvas delegates to `cameo_input`; this keeps the bubble usable on its own.
    #[allow(dead_code)]
    pub(crate) fn interact(&mut self, ui: &Ui, slide: Rect, grab: &mut Option<Grab>) -> bool {
        let rect = self.rect(slide);
        let resp = ui.interact(rect.expand(HANDLE), ui.id().with("cameo_bubble"), Sense::click_and_drag());
        let pointer = ui.ctx().input(|i| i.pointer.interact_pos());
        let over_handle = pointer.is_some_and(|p| self.handle_rect(slide).contains(p));
        if resp.dragged() {
            let icon = match *grab {
                Some(Grab::Resize) => egui::CursorIcon::ResizeNwSe,
                _ => egui::CursorIcon::Grabbing,
            };
            ui.ctx().set_cursor_icon(icon);
        } else if resp.hovered() {
            ui.ctx().set_cursor_icon(if over_handle { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::Grab });
        }
        if resp.drag_started() {
            *grab = Some(if over_handle { Grab::Resize } else { Grab::Move });
        }
        if resp.dragged() {
            match *grab {
                Some(Grab::Move) => self.drag_by(slide, resp.drag_delta()),
                Some(Grab::Resize) => {
                    if let Some(p) = resp.interact_pointer_pos() {
                        self.resize_to(slide, p);
                    }
                }
                None => {}
            }
        }
        if resp.drag_stopped() || !resp.dragged() {
            *grab = None;
        }
        resp.hovered() || resp.dragged()
    }
}

/// The slide's shorter side in points, never zero so fraction maths stays finite.
fn short_side(slide: Rect) -> f32 {
    slide.width().min(slide.height()).max(1.0)
}

/// The edge gap in screen points.
fn edge_px(slide: Rect) -> f32 {
    EDGE * short_side(slide)
}

/// A circular crop of `texture` inside `rect`: a triangle fan whose UVs trace the inscribed
/// circle of the frame, so the square camera picture reads as a round bubble.
pub(crate) fn circle_mesh(texture: TextureId, rect: Rect) -> Mesh {
    let mut mesh = Mesh::with_texture(texture);
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5;
    mesh.vertices.reserve(SEGMENTS + 2);
    mesh.indices.reserve(SEGMENTS * 3);
    mesh.vertices.push(egui::epaint::Vertex { pos: c, uv: pos2(0.5, 0.5), color: Color32::WHITE });
    for i in 0..=SEGMENTS {
        let angle = TAU * i as f32 / SEGMENTS as f32;
        let (sin, cos) = angle.sin_cos();
        mesh.vertices.push(egui::epaint::Vertex {
            pos: c + vec2(cos * r, sin * r),
            uv: pos2(0.5 + 0.5 * cos, 0.5 + 0.5 * sin),
            color: Color32::WHITE,
        });
    }
    for i in 1..=SEGMENTS as u32 {
        mesh.add_triangle(0, i, i + 1);
    }
    mesh
}

/// Whether a camera texture carries any pixels. A disconnected or empty camera still has a
/// handle, but its picture must not be drawn.
fn frame_ready(size: Vec2) -> bool {
    size.x.is_finite() && size.y.is_finite() && size.x > 0.0 && size.y > 0.0
}

/// The texture to draw for `frame`, if it has pixels.
fn frame_texture(frame: Option<&TextureHandle>) -> Option<TextureId> {
    frame.filter(|t| frame_ready(t.size_vec2())).map(TextureHandle::id)
}

/// The bubble's body: the drop shadow plus either the live frame or the camera-off disc.
fn bubble_shapes(rect: Rect, frame: Option<TextureId>) -> Vec<Shape> {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5;
    let shadow = Shape::circle_filled(c + vec2(0.0, SHADOW_OFFSET), r, Color32::from_black_alpha(SHADOW_ALPHA));
    let body = match frame {
        Some(id) => Shape::mesh(circle_mesh(id, rect)),
        None => Shape::circle_filled(c, r, OFF_DISC),
    };
    vec![shadow, body]
}

/// Paint the bubble: shadow, the live frame (or a neutral camera-off disc), and a thin ring.
pub(crate) fn paint(painter: &egui::Painter, rect: Rect, frame: Option<&TextureHandle>, ring: Color32, label: &str) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5;
    if r.is_nan() || r < 1.0 {
        return;
    }
    let texture = frame_texture(frame);
    for shape in bubble_shapes(rect, texture) {
        painter.add(shape);
    }
    if texture.is_none() && !label.is_empty() {
        let size = (r * 0.28).clamp(9.0, 14.0);
        painter.text(c, egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(size), OFF_LABEL);
    }
    painter.circle_stroke(c, r, Stroke::new(RING_WIDTH, ring));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn slide() -> Rect {
        Rect::from_min_size(pos2(100.0, 50.0), vec2(1600.0, 900.0))
    }

    #[test]
    fn default_bubble_is_round_bottom_right_with_equal_margins() {
        let cameo = Cameo::default();
        let r = cameo.rect(slide());
        let short = 900.0;
        assert!((r.width() - r.height()).abs() < 0.01, "circular");
        assert!((r.width() - 2.0 * DEFAULT_RADIUS * short).abs() < 0.01, "2.25 in Cameo radius");
        assert!(r.center().x > slide().center().x);
        assert!(r.center().y > slide().center().y);
        // The gap to the edge is the same on both axes and matches the Cameo placeholder margin.
        let gap_right = slide().right() - r.right();
        let gap_bottom = slide().bottom() - r.bottom();
        assert!((gap_right - gap_bottom).abs() < 0.01, "equal margins");
        assert!((gap_right - EDGE * short).abs() < 0.01, "edge gap");
        assert!(slide().contains_rect(r), "{r:?} must start fully inside {:?}", slide());

        // A portrait slide gets the same bottom-right anchor with equal margins.
        let portrait = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 1000.0));
        let p = cameo.rect(portrait);
        assert!((portrait.right() - p.right() - (portrait.bottom() - p.bottom())).abs() < 0.01);
        assert!(portrait.contains_rect(p));
    }

    #[test]
    fn resizing_stays_in_the_radius_band() {
        let mut cameo = Cameo::default();
        let short = 900.0;
        let center = cameo.rect(slide()).center();
        cameo.resize_to(slide(), center + Vec2::splat(10_000.0));
        let big = cameo.rect(slide());
        assert!((big.width() - 2.0 * MAX_RADIUS * short).abs() < 0.1, "largest radius");
        assert!(slide().contains_rect(big));
        cameo.resize_to(slide(), cameo.rect(slide()).center() + vec2(0.5, 0.0));
        let small = cameo.rect(slide());
        assert!((small.width() - 2.0 * MIN_RADIUS * short).abs() < 0.1, "smallest radius");
        assert!(slide().contains_rect(small));
    }

    #[test]
    fn extreme_drags_keep_the_circle_inside_the_slide() {
        let mut cameo = Cameo::default();
        let start = cameo.rect(slide()).center();
        cameo.drag_by(slide(), vec2(-100.0, -60.0));
        let moved = cameo.rect(slide());
        assert!(moved.center().x < start.x && moved.center().y < start.y, "dragging up-left moves the bubble");
        assert!((moved.width() - 2.0 * DEFAULT_RADIUS * 900.0).abs() < 0.01, "a move does not resize");

        let far = 10_000.0;
        for delta in [vec2(-far, -far), vec2(far, far), vec2(-far, far), vec2(far, -far)] {
            cameo.drag_by(slide(), delta);
            let rect = cameo.rect(slide());
            assert!(slide().contains_rect(rect), "delta {delta:?} left {rect:?} outside {:?}", slide());
        }
    }

    #[test]
    fn extreme_resizes_keep_the_circle_inside_the_slide() {
        let mut cameo = Cameo::default();
        for step in [0.5_f32, 2.0, 50.0, 10_000.0, -10_000.0] {
            let center = cameo.rect(slide()).center();
            cameo.resize_to(slide(), center + Vec2::splat(step));
            let rect = cameo.rect(slide());
            assert!(slide().contains_rect(rect), "resize {step} left {rect:?} outside {:?}", slide());
            assert!(rect.width() <= 2.0 * MAX_RADIUS * 900.0 + 0.01);
            assert!(rect.width() >= 2.0 * MIN_RADIUS * 900.0 - 0.01);
        }
    }

    #[test]
    fn a_tiny_slide_shrinks_the_bubble_to_fit() {
        let tiny = Rect::from_min_size(pos2(0.0, 0.0), vec2(120.0, 80.0));
        // An out-of-range radius (as stored geometry from another window size) is pulled back in.
        let mut cameo = Cameo { center: [0.0, 0.0], radius: 5.0, placed: true };
        cameo.clamp(tiny);
        let rect = cameo.rect(tiny);
        assert!(tiny.contains_rect(rect), "{rect:?} must fit in {tiny:?}");
        assert!(rect.width() <= 2.0 * MAX_RADIUS * 80.0 + 0.01);
        cameo.drag_by(tiny, vec2(10_000.0, 10_000.0));
        assert!(tiny.contains_rect(cameo.rect(tiny)));

        // A zero-sized slide (the shell before its first layout) has no room but must not panic.
        cameo.clamp(Rect::ZERO);
        assert!(cameo.radius.is_finite());
    }

    #[test]
    fn the_handle_sits_at_45_degrees_on_the_rim_and_hit_tests() {
        let cameo = Cameo::default();
        let rect = cameo.rect(slide());
        let r = rect.width() / 2.0;
        let handle = cameo.handle_rect(slide());
        let d = handle.center() - rect.center();
        let expected = r * FRAC_1_SQRT_2;
        assert!((d.x - expected).abs() < 0.01 && (d.y - expected).abs() < 0.01, "45 degrees down-right");
        assert!((d.length() - r).abs() < 0.01, "handle on the rim");
        assert!((handle.width() - 2.0 * HANDLE).abs() < 0.01 && (handle.height() - 2.0 * HANDLE).abs() < 0.01);

        assert!(cameo.hits(slide(), handle.center()), "the handle is grabbable");
        assert!(cameo.hits(slide(), rect.center()), "the bubble body is grabbable");
        assert!(cameo.hits(slide(), rect.center() + vec2(r * 0.99, 0.0)), "inside the circle");
        assert!(!cameo.hits(slide(), rect.center() - vec2(r + HANDLE + 5.0, 0.0)), "outside the rim");
        let past_handle = rect.center() + Vec2::splat((r + 60.0) * FRAC_1_SQRT_2);
        assert!(!cameo.hits(slide(), past_handle), "past the handle's grab target");
    }

    #[test]
    fn circle_mesh_uses_the_texture_id_and_rim_vertices_on_the_circle() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(200.0, 200.0));
        let id = TextureId::Managed(7);
        let mesh = circle_mesh(id, rect);
        assert_eq!(mesh.texture_id, id, "the camera texture id is preserved");
        assert_eq!(mesh.vertices.len(), SEGMENTS + 2);
        assert_eq!(mesh.indices.len(), SEGMENTS * 3);
        assert_eq!(mesh.vertices[0].pos, rect.center());
        assert_eq!(mesh.vertices[0].uv, pos2(0.5, 0.5));
        let r = rect.width() / 2.0;
        for (i, v) in mesh.vertices.iter().enumerate().skip(1) {
            let angle = TAU * (i - 1) as f32 / SEGMENTS as f32;
            let (sin, cos) = angle.sin_cos();
            assert!((v.pos.distance(rect.center()) - r).abs() < 0.01, "rim vertex {i} at {:?}", v.pos);
            assert!((v.uv.x - (0.5 + 0.5 * cos)).abs() < 1e-4, "u follows the circle");
            assert!((v.uv.y - (0.5 + 0.5 * sin)).abs() < 1e-4, "v follows the circle");
        }

        // A non-square frame is still cropped to its inscribed circle, not stretched.
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 200.0));
        let mesh = circle_mesh(id, wide);
        for v in mesh.vertices.iter().skip(1) {
            assert!((v.pos.distance(wide.center()) - 100.0).abs() < 0.01);
        }
    }

    #[test]
    fn a_loaded_color_image_drives_the_frame_mesh() {
        let ctx = egui::Context::default();
        let image = egui::ColorImage::filled([64, 48], Color32::from_rgb(10, 20, 30));
        let texture = ctx.load_texture("cameo-frame-test", image, egui::TextureOptions::LINEAR);
        assert_eq!(texture.size_vec2(), vec2(64.0, 48.0), "the fake frame's size reaches the handle");
        assert!(frame_ready(texture.size_vec2()));
        let id = frame_texture(Some(&texture)).expect("a non-empty frame is drawn");
        assert_eq!(id, texture.id());

        let rect = Cameo::default().rect(slide());
        let mesh = circle_mesh(id, rect);
        assert_eq!(mesh.texture_id, texture.id());
        let r = rect.width() / 2.0;
        for v in mesh.vertices.iter().skip(1) {
            assert!((v.pos.distance(rect.center()) - r).abs() < 0.01, "the frame fills the bubble rect");
        }
    }

    #[test]
    fn an_empty_color_image_falls_back_to_the_camera_off_disc() {
        let ctx = egui::Context::default();
        let empty = ctx.load_texture("cameo-empty-test", egui::ColorImage::filled([0, 0], Color32::WHITE), egui::TextureOptions::LINEAR);
        assert_eq!(empty.size_vec2(), Vec2::ZERO);
        assert!(!frame_ready(empty.size_vec2()), "zero-sized images are not drawn");
        assert_eq!(frame_texture(Some(&empty)), None);
        assert!(!frame_ready(vec2(4.0, 0.0)) && !frame_ready(vec2(0.0, 3.0)));
        assert!(frame_ready(vec2(1.0, 1.0)));

        let shapes = bubble_shapes(Cameo::default().rect(slide()), frame_texture(Some(&empty)));
        assert!(shapes.iter().all(|s| !matches!(s, Shape::Mesh(_))), "no fabricated image");
    }

    #[test]
    fn bubble_shapes_are_a_shadow_plus_a_frame_or_an_honest_disc() {
        let rect = Cameo::default().rect(slide());
        let off = bubble_shapes(rect, None);
        assert_eq!(off.len(), 2);
        assert!(matches!(off[0], Shape::Circle(_)), "shadow");
        assert!(matches!(off[1], Shape::Circle(_)), "camera-off disc");

        let id = TextureId::Managed(4);
        let live = bubble_shapes(rect, Some(id));
        assert_eq!(live.len(), 2);
        let mesh = live.iter().find_map(|s| if let Shape::Mesh(m) = s { Some(m) } else { None });
        let mesh = mesh.expect("the live frame is a textured mesh");
        assert_eq!(mesh.texture_id, id);
    }

    #[test]
    fn paint_draws_the_placeholder_label_without_a_camera() {
        let ctx = egui::Context::default();
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            paint(ui.painter(), Cameo::default().rect(slide()), None, Color32::WHITE, "Camera off");
        });
        let mut labelled = false;
        let mut mesh = false;
        for clipped in &out.shapes {
            match &clipped.shape {
                Shape::Text(t) if t.galley.text() == "Camera off" => labelled = true,
                Shape::Mesh(_) => mesh = true,
                _ => {}
            }
        }
        assert!(labelled, "the camera-off bubble is labelled");
        assert!(!mesh, "no frame is invented");
    }

    #[test]
    fn paint_uses_the_camera_texture_when_it_has_pixels() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture("cameo-paint-test", egui::ColorImage::filled([32, 24], Color32::RED), egui::TextureOptions::LINEAR);
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            paint(ui.painter(), Cameo::default().rect(slide()), Some(&texture), Color32::WHITE, "Camera off");
        });
        assert!(
            out.shapes.iter().any(|c| matches!(&c.shape, Shape::Mesh(m) if m.texture_id == texture.id())),
            "the live frame is painted with the camera texture"
        );
        assert!(
            !out.shapes.iter().any(|c| matches!(&c.shape, Shape::Text(t) if t.galley.text() == "Camera off")),
            "a live frame is not labelled camera off"
        );
    }
}
