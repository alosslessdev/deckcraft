//! The live Cameo camera bubble drawn over the recorded slide.
//!
//! PowerPoint keeps the bubble circular, places it at the bottom-right of the slide by default
//! and lets the presenter move or resize it while the show runs. The geometry here is stored in
//! slide fractions so it survives slide-size changes and stays inside the slide when dragged or
//! resized. Recording itself is separate: this module only paints the live preview and turns
//! pointer drags into clamped geometry changes.

use egui::{Color32, Mesh, Pos2, Rect, Sense, Stroke, TextureHandle, TextureId, Ui, Vec2, pos2, vec2};

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

/// What a bubble drag is changing.
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
            self.center
        } else {
            let r = self.radius_px(slide);
            let (w, h) = (slide.width().max(1.0), slide.height().max(1.0));
            let edge = EDGE * w.min(h);
            [(w - edge - r) / w, (h - edge - r) / h]
        }
    }

    /// Radius in screen pixels for `slide`.
    fn radius_px(&self, slide: Rect) -> f32 {
        self.radius.clamp(MIN_RADIUS, MAX_RADIUS) * slide.width().min(slide.height()).max(1.0)
    }

    /// The circle's bounding square on screen, centred on the resolved centre.
    pub(crate) fn rect(&self, slide: Rect) -> Rect {
        let r = self.radius_px(slide);
        let c = self.center_on(slide);
        let center = pos2(slide.left() + c[0] * slide.width(), slide.top() + c[1] * slide.height());
        Rect::from_center_size(center, vec2(2.0 * r, 2.0 * r))
    }

    /// The grab handle: a point on the circle at 45° down-right, the corner a presenter drags.
    pub(crate) fn handle_rect(&self, slide: Rect) -> Rect {
        let r = self.radius_px(slide);
        let c = self.center_on(slide);
        let center = pos2(slide.left() + c[0] * slide.width(), slide.top() + c[1] * slide.height());
        Rect::from_center_size(center + vec2(r * std::f32::consts::FRAC_1_SQRT_2, r * std::f32::consts::FRAC_1_SQRT_2), Vec2::splat(2.0 * HANDLE))
    }

    /// Whether `p` (screen points) is on the bubble or its resize handle, so a click there must
    /// not advance the show.
    pub(crate) fn hits(&self, slide: Rect, p: Pos2) -> bool {
        self.rect(slide).expand(HANDLE).contains(p) || self.handle_rect(slide).contains(p)
    }

    /// Move by a screen-space delta and keep the circle inside `slide`.
    pub(crate) fn drag_by(&mut self, slide: Rect, delta: Vec2) {
        let (w, h) = (slide.width().max(1.0), slide.height().max(1.0));
        let c = self.center_on(slide);
        self.center = [(c[0] + delta.x / w).clamp(0.0, 1.0), (c[1] + delta.y / h).clamp(0.0, 1.0)];
        self.placed = true;
        self.clamp(slide);
    }

    /// Set the radius from the pointer's distance to the centre (a corner drag), then clamp.
    pub(crate) fn resize_to(&mut self, slide: Rect, pointer: Pos2) {
        let rect = self.rect(slide);
        let d = pointer - rect.center();
        let len = d.length();
        if !len.is_finite() {
            return;
        }
        self.radius = (len / slide.width().min(slide.height()).max(1.0)).clamp(MIN_RADIUS, MAX_RADIUS);
        self.placed = true;
        self.clamp(slide);
    }

    /// Keep the whole circle inside the slide, shrinking it when the slide is too small.
    pub(crate) fn clamp(&mut self, slide: Rect) {
        let (w, h) = (slide.width().max(1.0), slide.height().max(1.0));
        let edge = EDGE * w.min(h);
        let max_r = ((w.min(h) / 2.0 - edge) / w.min(h)).max(MIN_RADIUS);
        self.radius = self.radius.clamp(MIN_RADIUS, MAX_RADIUS.min(max_r));
        let r = self.radius_px(slide);
        let c = self.center_on(slide);
        let mut x = c[0];
        let mut y = c[1];
        let xlo = (r + edge) / w;
        let xhi = 1.0 - xlo;
        let ylo = (r + edge) / h;
        let yhi = 1.0 - ylo;
        if xlo <= xhi {
            x = x.clamp(xlo, xhi);
        } else {
            x = 0.5;
        }
        if ylo <= yhi {
            y = y.clamp(ylo, yhi);
        } else {
            y = 0.5;
        }
        self.center = [x, y];
        self.placed = true;
    }

    /// Draw or resize the bubble from the pointer. `grab` remembers whether the drag started on
    /// the circle body (move) or on its handle (resize). Returns true when the pointer is over
    /// the bubble this frame.
    pub(crate) fn interact(&mut self, ui: &Ui, slide: Rect, grab: &mut Option<Grab>) -> bool {
        let rect = self.rect(slide);
        let resp = ui.interact(rect.expand(HANDLE), ui.id().with("cameo_bubble"), Sense::click_and_drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
        }
        if resp.drag_started() {
            let on_handle = ui.ctx().input(|i| i.pointer.interact_pos()).is_some_and(|p| self.handle_rect(slide).contains(p));
            *grab = Some(if on_handle { Grab::Resize } else { Grab::Move });
        }
        if resp.dragged()
            && let Some(kind) = *grab
        {
            match kind {
                Grab::Move => self.drag_by(slide, resp.drag_delta()),
                Grab::Resize => {
                    if let Some(p) = resp.interact_pointer_pos() {
                        self.resize_to(slide, p);
                    }
                }
            }
        }
        if resp.drag_stopped() || !resp.dragged() {
            *grab = None;
        }
        resp.hovered() || resp.dragged()
    }
}

/// A circular crop of `texture` inside `rect`: a triangle fan whose UVs trace the inscribed
/// circle of the frame, so the square camera picture reads as a round bubble.
pub(crate) fn circle_mesh(texture: TextureId, rect: Rect) -> Mesh {
    let mut mesh = Mesh::default();
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5;
    mesh.texture_id = texture;
    mesh.vertices.push(egui::epaint::Vertex { pos: c, uv: pos2(0.5, 0.5), color: Color32::WHITE });
    for i in 0..=SEGMENTS {
        let a = std::f32::consts::TAU * i as f32 / SEGMENTS as f32;
        let (s, co) = a.sin_cos();
        mesh.vertices.push(egui::epaint::Vertex {
            pos: c + vec2(co * r, s * r),
            uv: pos2(0.5 + 0.5 * co, 0.5 + 0.5 * s),
            color: Color32::WHITE,
        });
    }
    for i in 1..=SEGMENTS as u32 {
        mesh.indices.extend_from_slice(&[0, i, i + 1]);
    }
    mesh
}

/// Paint the bubble: shadow, the live frame (or a neutral camera-off disc), and a thin ring.
pub(crate) fn paint(painter: &egui::Painter, rect: Rect, frame: Option<&TextureHandle>, ring: Color32, label: &str) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5;
    if r < 1.0 {
        return;
    }
    painter.circle_filled(c + vec2(0.0, 3.0), r, Color32::from_black_alpha(110));
    match frame.filter(|t| t.size_vec2().x > 0.0 && t.size_vec2().y > 0.0) {
        Some(tex) => painter.add(egui::Shape::mesh(circle_mesh(tex.id(), rect))),
        None => {
            painter.circle_filled(c, r, Color32::from_gray(28));
            painter.text(c, egui::Align2::CENTER_CENTER, label, egui::FontId::proportional((r * 0.28).clamp(9.0, 14.0)), Color32::from_gray(190));
        }
    }
    painter.circle_stroke(c, r, Stroke::new(2.0, ring));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn slide() -> Rect {
        Rect::from_min_size(pos2(100.0, 50.0), vec2(1600.0, 900.0))
    }

    #[test]
    fn default_bubble_is_bottom_right_and_round() {
        let cameo = Cameo::default();
        let r = cameo.rect(slide());
        assert!((r.width() - r.height()).abs() < 0.01, "circular");
        assert!(r.right() < slide().right());
        assert!(r.bottom() < slide().bottom());
        assert!(r.center().x > slide().center().x);
        assert!(r.center().y > slide().center().y);
        // The gap to the edge is the same on both axes for a 16:9 slide.
        assert!((slide().right() - r.right() - (slide().bottom() - r.bottom())).abs() < 0.01);
    }

    #[test]
    fn dragging_moves_the_bubble_and_clamps_it_inside_the_slide() {
        let mut cameo = Cameo::default();
        let start = cameo.rect(slide()).center();
        cameo.drag_by(slide(), vec2(-100.0, -60.0));
        let moved = cameo.rect(slide());
        assert!(moved.center().x < start.x && moved.center().y < start.y, "dragging up-left moves the bubble");
        cameo.drag_by(slide(), vec2(-5000.0, -5000.0));
        let top_left = cameo.rect(slide());
        assert!(slide().contains_rect(top_left), "{top_left:?} must stay inside {:?}", slide());
        cameo.drag_by(slide(), vec2(5000.0, 5000.0));
        let bottom_right = cameo.rect(slide());
        assert!(slide().contains_rect(bottom_right), "{bottom_right:?} must stay inside {:?}", slide());
    }

    #[test]
    fn resizing_clamps_between_the_limits_and_keeps_the_circle_inside() {
        let mut cameo = Cameo::default();
        let center = cameo.rect(slide()).center();
        cameo.resize_to(slide(), center + vec2(1000.0, 1000.0));
        let big = cameo.rect(slide());
        assert!(big.width() <= MAX_RADIUS * 900.0 + 0.01);
        assert!(slide().contains_rect(big));
        cameo.resize_to(slide(), center + Vec2::splat(1.0));
        let small = cameo.rect(slide());
        assert!(small.width() >= MIN_RADIUS * 900.0 - 0.01);
        assert!(slide().contains_rect(small));
    }

    #[test]
    fn circle_mesh_crops_the_texture_and_stays_on_the_circle() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(200.0, 200.0));
        let mesh = circle_mesh(TextureId::Managed(7), rect);
        assert_eq!(mesh.texture_id, TextureId::Managed(7));
        assert_eq!(mesh.vertices.len(), SEGMENTS + 2);
        assert_eq!(mesh.indices.len(), SEGMENTS * 3);
        let c = rect.center();
        for v in &mesh.vertices {
            assert!((v.pos.distance(c) - 100.0).abs() < 0.01 || v.pos == c, "vertex {:?} on the rim", v.pos);
        }
    }

    #[test]
    fn the_handle_sits_on_the_circle_and_counts_as_a_hit() {
        let cameo = Cameo::default();
        let rect = cameo.rect(slide());
        let handle = cameo.handle_rect(slide());
        let d = handle.center() - rect.center();
        assert!((d.length() - rect.width() / 2.0).abs() < 0.01, "handle on the rim");
        assert!(cameo.hits(slide(), handle.center()));
        assert!(cameo.hits(slide(), rect.center()));
        assert!(!cameo.hits(slide(), pos2(rect.left() - 40.0, rect.top() - 40.0)));
    }
}
