//! Brushes are data (ARCHITECTURE §4): MyPaint `.myb` files, parsed by
//! hokusai-brush. The built-in presets are embedded `.myb` files, so a
//! brush someone downloads for MyPaint is the same kind of thing.
//!
//! Lilyfield layers three controls of its own over a preset: size (the
//! radius at full pressure), opacity (applied once per stroke, see
//! `stroke`), and smoothing. Each preset remembers its own size, with a
//! separate size for erasing.

use hokusai_core::color::rgb_to_hsv;
use hokusai_core::{Brush, BrushSetting};

use crate::compositor::srgb_to_linear;

const BUILTIN: [(&str, &str); 3] = [
    ("ink", include_str!("../assets/brushes/ink.myb")),
    ("pencil", include_str!("../assets/brushes/pencil.myb")),
    ("soft", include_str!("../assets/brushes/soft.myb")),
];

/// Radius limits, in document pixels.
pub const MIN_SIZE: f32 = 0.5;
pub const MAX_SIZE: f32 = 800.0;

/// Smoothing steps the smoothing key cycles through; libmypaint's
/// slow_tracking. Off by default (decided 2026-10-05).
pub const SMOOTHING: [f32; 4] = [0.0, 1.0, 2.5, 5.0];

pub struct Preset {
    pub name: String,
    pub brush: Brush,
    pub size: f32,
    pub erase_size: f32,
    pub opacity: f32,
}

pub struct Brushes {
    pub presets: Vec<Preset>,
    pub current: usize,
    /// sRGB, 0..1: what the color picker shows and swatches store.
    pub color: [f32; 3],
    pub smoothing: usize,
}

impl Brushes {
    pub fn builtin() -> Self {
        let presets = BUILTIN
            .iter()
            .filter_map(|(name, json)| {
                let brush = hokusai_brush::from_str(json).ok()?;
                let size = brush.get(BrushSetting::Radius).base_value.exp();
                Some(Preset {
                    name: (*name).to_owned(),
                    brush,
                    size,
                    erase_size: (size * 3.0).min(MAX_SIZE),
                    opacity: 1.0,
                })
            })
            .collect();
        Self { presets, current: 0, color: [0.0; 3], smoothing: 0 }
    }

    pub fn preset(&self) -> Option<&Preset> {
        self.presets.get(self.current)
    }

    pub fn name(&self) -> &str {
        self.preset().map_or("none", |p| &p.name)
    }

    pub fn size(&self, erase: bool) -> f32 {
        self.preset().map_or(4.0, |p| if erase { p.erase_size } else { p.size })
    }

    pub fn set_size(&mut self, erase: bool, size: f32) {
        let size = size.clamp(MIN_SIZE, MAX_SIZE);
        if let Some(p) = self.presets.get_mut(self.current) {
            if erase { p.erase_size = size } else { p.size = size }
        }
    }

    pub fn opacity(&self) -> f32 {
        self.preset().map_or(1.0, |p| p.opacity)
    }

    pub fn set_opacity(&mut self, opacity: f32) {
        if let Some(p) = self.presets.get_mut(self.current) {
            p.opacity = opacity.clamp(0.01, 1.0);
        }
    }

    pub fn select(&mut self, name: &str) -> bool {
        match self.presets.iter().position(|p| p.name == name) {
            Some(i) => {
                self.current = i;
                true
            }
            None => false,
        }
    }

    pub fn next(&mut self) {
        if !self.presets.is_empty() {
            self.current = (self.current + 1) % self.presets.len();
        }
    }

    /// The hokusai brush for one stroke: the preset with Lilyfield's size,
    /// color and smoothing applied. hokusai paints in linear light, so the
    /// sRGB color is linearized before it becomes the brush's HSV.
    pub fn stroke_brush(&self, erase: bool) -> Brush {
        let mut b = self.preset().map_or_else(Brush::new, |p| p.brush.clone());
        b.get_mut(BrushSetting::Radius).base_value = self.size(erase).ln();
        let [r, g, bl] = self.color.map(|c| srgb_to_linear(c as f64) as f32);
        let hsv = rgb_to_hsv(r, g, bl);
        b.get_mut(BrushSetting::ColorH).base_value = hsv.h;
        b.get_mut(BrushSetting::ColorS).base_value = hsv.s;
        b.get_mut(BrushSetting::ColorV).base_value = hsv.v;
        b.get_mut(BrushSetting::SlowTracking).base_value = SMOOTHING[self.smoothing % SMOOTHING.len()];
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_brush_parses() {
        assert_eq!(Brushes::builtin().presets.len(), BUILTIN.len());
    }

    #[test]
    fn size_is_the_full_pressure_radius() {
        let mut b = Brushes::builtin();
        b.set_size(false, 10.0);
        let brush = b.stroke_brush(false);
        assert!((brush.get(BrushSetting::Radius).base_value.exp() - 10.0).abs() < 1e-3);
    }
}
