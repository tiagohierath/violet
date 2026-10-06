//! The view transform (ARCHITECTURE §1): pan, zoom, rotation and flip are a
//! transform sampling the raster, never a re-render. One matrix maps
//! document pixels to screen points; everything else derives from it.

use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};

pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 64.0;

/// Where the canvas is looking. Every change keeps some point fixed on
/// screen, so nothing jumps.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Physical pixels per document pixel: 1.0 is 100%, crisp.
    pub zoom: f32,
    /// Radians, clockwise on screen.
    pub angle: f32,
    /// Mirrored left to right about the canvas center.
    pub flip: bool,
    /// The document point shown at the center of the canvas.
    pub center: Pos2,
}

/// screen = m * doc + t, in egui points. `m` is a row-major 2x2 matrix.
#[derive(Clone, Copy, Debug)]
pub struct Xf {
    m: [f32; 4],
    t: Vec2,
}

impl Xf {
    /// Maps a point.
    #[inline]
    pub fn apply(&self, p: Pos2) -> Pos2 {
        let [a, b, c, d] = self.m;
        pos2(a * p.x + b * p.y + self.t.x, c * p.x + d * p.y + self.t.y)
    }

    /// Maps a direction: the matrix alone, no translation.
    #[inline]
    pub fn linear(&self, v: Vec2) -> Vec2 {
        let [a, b, c, d] = self.m;
        vec2(a * v.x + b * v.y, c * v.x + d * v.y)
    }

    pub fn inverse(&self) -> Xf {
        let [a, b, c, d] = self.m;
        // Guarded against zero so a degenerate view never produces NaN.
        let det = a * d - b * c;
        let det = if det.abs() < f32::EPSILON { f32::EPSILON } else { det };
        let m = [d / det, -b / det, -c / det, a / det];
        let t = Xf { m, t: Vec2::ZERO }.linear(-self.t);
        Xf { m, t }
    }

    /// Screen points per document pixel.
    pub fn scale(&self) -> f32 {
        let [a, _, c, _] = self.m;
        (a * a + c * c).sqrt()
    }
}

impl Default for View {
    fn default() -> Self {
        Self { zoom: 1.0, angle: 0.0, flip: false, center: Pos2::ZERO }
    }
}

impl View {
    /// Document pixels to screen points for a canvas at `rect`, with `ppp`
    /// physical pixels per point.
    pub fn xf(&self, rect: Rect, ppp: f32) -> Xf {
        // Screen points per document pixel.
        let scale = self.zoom / ppp;
        let (sin, cos) = self.angle.sin_cos();
        let flip_x = if self.flip { -1.0 } else { 1.0 };
        // Flip is applied after rotation, in screen space, so it mirrors
        // exactly what is on screen.
        let m = [flip_x * cos * scale, -flip_x * sin * scale, sin * scale, cos * scale];
        let partial = Xf { m, t: Vec2::ZERO };
        let t = rect.center().to_vec2() - partial.linear(self.center.to_vec2());
        Xf { m, t }
    }

    /// Screen point to document pixel.
    pub fn to_doc(&self, rect: Rect, ppp: f32, p: Pos2) -> Pos2 {
        self.xf(rect, ppp).inverse().apply(p)
    }

    /// Moves the view so `doc_point` lands on `screen_point`.
    fn pin(&mut self, rect: Rect, ppp: f32, doc_point: Pos2, screen_point: Pos2) {
        let inv = Xf { m: self.xf(rect, ppp).m, t: Vec2::ZERO }.inverse();
        self.center = doc_point - inv.linear(screen_point - rect.center());
    }

    pub fn pan(&mut self, rect: Rect, ppp: f32, delta: Vec2) {
        let inv = self.xf(rect, ppp).inverse();
        self.center -= inv.linear(delta);
    }

    /// Zooms by `factor`, keeping the screen point `pivot` still.
    pub fn zoom_at(&mut self, rect: Rect, ppp: f32, pivot: Pos2, factor: f32) {
        let doc_point = self.to_doc(rect, ppp, pivot);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.pin(rect, ppp, doc_point, pivot);
    }

    pub fn set_zoom(&mut self, rect: Rect, ppp: f32, zoom: f32) {
        self.zoom_at(rect, ppp, rect.center(), zoom / self.zoom);
    }

    /// `delta` is clockwise on screen whether or not the view is flipped.
    pub fn rotate_at(&mut self, rect: Rect, ppp: f32, pivot: Pos2, delta: f32) {
        let doc_point = self.to_doc(rect, ppp, pivot);
        self.angle = wrap_angle(self.angle + if self.flip { -delta } else { delta });
        self.pin(rect, ppp, doc_point, pivot);
    }

    pub fn set_angle(&mut self, rect: Rect, ppp: f32, angle: f32) {
        let doc_point = self.to_doc(rect, ppp, rect.center());
        self.angle = wrap_angle(angle);
        self.pin(rect, ppp, doc_point, rect.center());
    }

    pub fn toggle_flip(&mut self, rect: Rect, ppp: f32) {
        let doc_point = self.to_doc(rect, ppp, rect.center());
        self.flip = !self.flip;
        self.pin(rect, ppp, doc_point, rect.center());
    }

    /// Whole document in view, rotation kept.
    pub fn fit(&mut self, rect: Rect, ppp: f32, width: u32, height: u32) {
        let (w, h) = (width as f32, height as f32);
        let (sin, cos) = self.angle.sin_cos();
        // Screen-aligned bounding box of the rotated document.
        let bounds_w = (w * cos).abs() + (h * sin).abs();
        let bounds_h = (w * sin).abs() + (h * cos).abs();
        // 0.94 leaves a margin of workspace around the page.
        let fit = (rect.width() / bounds_w).min(rect.height() / bounds_h) * 0.94;
        self.zoom = (fit * ppp).clamp(MIN_ZOOM, MAX_ZOOM);
        self.center = pos2(w / 2.0, h / 2.0);
    }

    /// Display mip level: each level halves the resolution, chosen so the
    /// texture is never minified by much more than 1.4x.
    pub fn level(&self, max: usize) -> usize {
        let level = (1.0 / self.zoom).log2().round();
        if level <= 0.0 { 0 } else { (level as usize).min(max) }
    }

    /// The angle in whole degrees, for display.
    pub fn degrees(&self) -> f32 {
        self.angle.to_degrees().round()
    }
}

/// Into `0..TAU`.
fn wrap_angle(angle: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let angle = angle.rem_euclid(tau);
    // Snap the float drift that fifteen-degree steps accumulate.
    let step = 15f32.to_radians();
    let snapped = (angle / step).round() * step;
    if (angle - snapped).abs() < 1e-4 { snapped.rem_euclid(tau) } else { angle }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 600.0))
    }

    #[test]
    fn to_doc_inverts_xf() {
        let mut view = View::default();
        view.fit(rect(), 1.5, 2480, 3508);
        view.rotate_at(rect(), 1.5, pos2(100.0, 100.0), 0.7);
        view.toggle_flip(rect(), 1.5);
        let screen = pos2(321.0, 456.0);
        let doc = view.to_doc(rect(), 1.5, screen);
        let back = view.xf(rect(), 1.5).apply(doc);
        assert!((back - screen).length() < 1e-3);
    }

    #[test]
    fn zoom_keeps_pivot_fixed() {
        let mut view = View::default();
        view.fit(rect(), 1.0, 1000, 1000);
        let pivot = pos2(200.0, 300.0);
        let before = view.to_doc(rect(), 1.0, pivot);
        view.zoom_at(rect(), 1.0, pivot, 3.0);
        assert!((view.to_doc(rect(), 1.0, pivot) - before).length() < 1e-2);
    }
}
