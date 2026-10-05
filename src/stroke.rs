//! A live stroke (ARCHITECTURE §4): hokusai turns input samples into dabs,
//! the dabs land in a scratch buffer at full strength, and the buffer is
//! composited onto its layer once, when the stroke ends. Until then the
//! document is untouched, so a stroke can be cancelled for free.

use std::collections::HashMap;

use hokusai_core::color::{Hsv, RgbaF32, hsv_to_rgb};
use hokusai_core::surface::{Dab, TiledSurface};
use hokusai_core::tile::TilePixels;
use hokusai_core::{Brush, BrushInput, BrushSetting, BrushState};

use crate::compositor::{self, StrokeBlend};
use crate::dab;
use crate::document::{Document, TileKey, valid_in_tile};
use crate::history::{Change, Record};

/// One input sample, in document pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokePoint {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    /// -1..1, libmypaint's convention.
    pub xtilt: f32,
    pub ytilt: f32,
    /// Seconds, on the input device's own clock.
    pub time: f64,
}

/// The scratch buffer. Tiles outside the document are routed to one
/// throwaway tile, so a stroke off the edge allocates nothing.
pub struct Scratch {
    tiles: HashMap<TileKey, (Box<TilePixels>, bool)>,
    dirty: Vec<TileKey>,
    bounds: (i32, i32),
    trash: Box<TilePixels>,
    pub dabs: u64,
}

impl Scratch {
    pub fn new(bounds: (i32, i32)) -> Self {
        Self {
            tiles: HashMap::new(),
            dirty: Vec::new(),
            bounds,
            trash: compositor::new_tile(),
            dabs: 0,
        }
    }

    pub fn get(&self, key: TileKey) -> Option<&TilePixels> {
        self.tiles.get(&key).map(|(t, _)| &**t)
    }

    pub fn keys(&self) -> impl Iterator<Item = TileKey> + '_ {
        self.tiles.keys().copied()
    }

    /// Tiles touched since the last call.
    pub fn take_dirty(&mut self) -> Vec<TileKey> {
        let dirty = std::mem::take(&mut self.dirty);
        for k in &dirty {
            if let Some(t) = self.tiles.get_mut(k) {
                t.1 = false;
            }
        }
        dirty
    }
}

impl TiledSurface for Scratch {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels {
        if tx < 0 || ty < 0 || tx >= self.bounds.0 || ty >= self.bounds.1 {
            return &mut self.trash;
        }
        let entry = self
            .tiles
            .entry((tx, ty))
            .or_insert_with(|| (compositor::new_tile(), false));
        if !entry.1 {
            entry.1 = true;
            self.dirty.push((tx, ty));
        }
        &mut entry.0
    }

    fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

    fn draw_dab(&mut self, d: &Dab) -> bool {
        self.dabs += 1;
        if dab::is_plain(d) {
            dab::draw(self, d)
        } else {
            hokusai_core::brushmodes::draw_dab_default(self, d)
        }
    }

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.get((tx, ty))
    }
}

pub struct LiveStroke {
    pub layer: u64,
    pub blend: StrokeBlend,
    /// fix15
    pub opacity: u32,
    pub scratch: Scratch,
    brush: Brush,
    state: BrushState,
    last: Option<StrokePoint>,
}

impl LiveStroke {
    pub fn new(brush: Brush, layer: u64, blend: StrokeBlend, opacity: f32, bounds: (i32, i32)) -> Self {
        Self {
            layer,
            blend,
            opacity: (opacity.clamp(0.0, 1.0) * compositor::ONE as f32) as u32,
            scratch: Scratch::new(bounds),
            brush,
            state: BrushState::default(),
            last: None,
        }
    }

    pub fn extend(&mut self, points: &[StrokePoint]) {
        for p in points {
            let dtime = self.last.map_or(0.0, |l| (p.time - l.time).max(0.0));
            self.brush.stroke_to(
                &mut self.state,
                &mut self.scratch,
                p.x,
                p.y,
                p.pressure,
                p.xtilt,
                p.ytilt,
                dtime,
            );
            self.last = Some(*p);
        }
    }

    /// Lets smoothing catch up with the pen, and makes a tap leave a dot:
    /// libmypaint only places dabs after the pen has moved a dab's spacing.
    pub fn finish(&mut self) {
        self.brush.finish_stroke(&mut self.state, &mut self.scratch);
        if self.scratch.dabs == 0
            && let Some(p) = self.last
        {
            let d = tap_dab(&self.brush, p);
            self.scratch.draw_dab(&d);
        }
    }

    /// Composites the buffer onto its layer. Returns the undo record and
    /// every tile whose display must be refreshed.
    pub fn commit(self, doc: &mut Document) -> (Record, Vec<TileKey>) {
        let (w, h) = (doc.width, doc.height);
        let keys: Vec<TileKey> = self.scratch.keys().collect();
        let mut changes = Vec::new();
        if let Some(layer) = doc.layer_mut(self.layer) {
            for (key, (tile, _)) in self.scratch.tiles {
                let before = layer.tiles.get(&key).cloned();
                let after = compositor::commit_tile(
                    before.as_deref(),
                    &tile,
                    self.opacity,
                    self.blend,
                    valid_in_tile(w, h, key),
                );
                if before.is_none() && after.is_none() {
                    continue;
                }
                match after {
                    Some(t) => layer.tiles.insert(key, t),
                    None => layer.tiles.remove(&key),
                };
                changes.push(Change { layer: self.layer, key, tile: before });
            }
        }
        (Record::tiles(changes), keys)
    }
}

/// The dab a stationary tap produces: the brush evaluated at the tap's
/// pressure with every other input at rest.
fn tap_dab(brush: &Brush, p: StrokePoint) -> Dab {
    let eval = |s: BrushSetting| {
        let v = brush.get(s);
        v.base_value
            + v.inputs
                .iter()
                .filter(|m| m.input == BrushInput::Pressure)
                .map(|m| m.eval(p.pressure))
                .sum::<f32>()
    };
    let multiply = {
        let v = brush.get(BrushSetting::OpaqueMultiply);
        if v.base_value == 0.0 && v.inputs.is_empty() {
            1.0
        } else {
            eval(BrushSetting::OpaqueMultiply).clamp(0.0, 1.0)
        }
    };
    let c = hsv_to_rgb(Hsv {
        h: brush.get(BrushSetting::ColorH).base_value,
        s: brush.get(BrushSetting::ColorS).base_value,
        v: brush.get(BrushSetting::ColorV).base_value,
    });
    Dab {
        x: p.x,
        y: p.y,
        radius: eval(BrushSetting::Radius).exp().clamp(0.2, 1000.0),
        color: RgbaF32::new(c.r, c.g, c.b, 1.0),
        opaque: (eval(BrushSetting::Opaque) * multiply).clamp(0.0, 1.0),
        hardness: eval(BrushSetting::Hardness).clamp(0.0, 1.0),
        alpha_eraser: 1.0,
        aspect_ratio: 1.0,
        angle: 0.0,
        lock_alpha: 0.0,
        colorize: 0.0,
        posterize: 0.0,
        posterize_num: 0.0,
        paint: 0.0,
        anti_aliasing: eval(BrushSetting::AntiAliasing),
    }
}
