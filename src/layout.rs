//! Where panels go (ARCHITECTURE §13): no panel decides where it lives.
//! Layout is data: which panels show, the edge each docks to or where it
//! floats, and its size. These defaults stand in until the Lua config
//! supplies the same data (decided 2026-10-05: the UI is edited only
//! through config, and the app never writes it).

use eframe::egui::{Pos2, Rect, pos2, vec2};

/// Where a panel sits relative to the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dock {
    Top,
    Bottom,
    Left,
    Right,
    /// Over the canvas, offset from its top-left corner; negative values
    /// measure from the right or bottom edge instead.
    Float {
        x: f32,
        y: f32,
    },
}

/// One panel's entry in the layout data.
#[derive(Clone, Debug)]
pub struct Placement {
    pub panel: &'static str,
    pub dock: Dock,
    /// Docked: `[thickness, length]`, where a length of 0 shares whatever
    /// the edge has left. Floating: `[width, height]`.
    pub size: [f32; 2],
    pub visible: bool,
}

/// Placements in order. Panels on the same edge stack in this order, and
/// edges are carved off the screen in the order they first appear.
pub struct Layout {
    pub placements: Vec<Placement>,
}

impl Default for Layout {
    fn default() -> Self {
        let place = |panel, dock, size, visible| Placement { panel, dock, size, visible };
        Self {
            placements: vec![
                place("bar", Dock::Top, [28.0, 0.0], true),
                place("color", Dock::Right, [300.0, 340.0], true),
                place("info", Dock::Right, [300.0, 400.0], true),
                place("terminal", Dock::Right, [300.0, 200.0], true),
                place("wheel", Dock::Float { x: -340.0, y: 60.0 }, [300.0, 340.0], false),
                place("perf", Dock::Float { x: 12.0, y: 12.0 }, [360.0, 420.0], true),
            ],
        }
    }
}

/// The narrowest the canvas may be made by docked panels.
const MIN_CANVAS: f32 = 240.0;

/// A layout applied to one screen size: the rect each element renders into.
pub struct Resolved {
    /// What is left once the docked panels are carved off.
    pub canvas: Rect,
    pub panels: Vec<(&'static str, Rect)>,
}

impl Layout {
    /// `shown` says whether a panel wants to be on screen this frame (the
    /// perf overlay is summoned, the whole chrome can be hidden).
    pub fn resolve(&self, screen: Rect, shown: impl Fn(&str) -> bool) -> Resolved {
        let visible: Vec<&Placement> =
            self.placements.iter().filter(|p| p.visible && shown(p.panel)).collect();
        let mut rest = screen;
        let mut panels = Vec::new();
        let mut done: Vec<Dock> = Vec::new();
        for placement in &visible {
            if matches!(placement.dock, Dock::Float { .. }) || done.contains(&placement.dock) {
                continue;
            }
            done.push(placement.dock);
            // Every visible panel on this edge, in layout order.
            let edge: Vec<&&Placement> =
                visible.iter().filter(|other| other.dock == placement.dock).collect();
            let thickness = edge.iter().map(|member| member.size[0]).fold(0.0, f32::max);
            let thickness = match placement.dock {
                Dock::Left | Dock::Right => thickness.min((rest.width() - MIN_CANVAS).max(0.0)),
                _ => thickness,
            };
            let Some(band) = carve(&mut rest, placement.dock, thickness) else { continue };
            // Left and right edges stack their panels top to bottom.
            let vertical = matches!(placement.dock, Dock::Left | Dock::Right);
            let total = if vertical { band.height() } else { band.width() };
            let fixed: f32 = edge.iter().map(|m| m.size[1]).filter(|&len| len > 0.0).sum();
            let fills = edge.iter().filter(|m| m.size[1] <= 0.0).count().max(1) as f32;
            let share = ((total - fixed) / fills).max(0.0);
            let mut at = 0.0;
            for member in edge {
                let len = if member.size[1] > 0.0 { member.size[1].min(total - at) } else { share };
                let rect = if vertical {
                    Rect::from_min_size(pos2(band.min.x, band.min.y + at), vec2(band.width(), len))
                } else {
                    Rect::from_min_size(pos2(band.min.x + at, band.min.y), vec2(len, band.height()))
                };
                at += len;
                panels.push((member.panel, rect));
            }
        }
        for placement in &visible {
            if let Dock::Float { x, y } = placement.dock {
                let [w, h] = placement.size;
                let x = if x < 0.0 { screen.max.x + x - w } else { screen.min.x + x };
                let y = if y < 0.0 { screen.max.y + y - h } else { screen.min.y + y };
                let x = x.clamp(screen.min.x, (screen.max.x - w).max(screen.min.x));
                let y = y.clamp(screen.min.y, (screen.max.y - h).max(screen.min.y));
                panels.push((placement.panel, Rect::from_min_size(Pos2::new(x, y), vec2(w, h))));
            }
        }
        Resolved { canvas: rest, panels }
    }
}

/// Cuts a band `thickness` thick off `dock`'s side of `rest` and returns
/// it. `None` for a floating dock, which takes no room.
fn carve(rest: &mut Rect, dock: Dock, thickness: f32) -> Option<Rect> {
    let band = match dock {
        Dock::Bottom => {
            let band = Rect::from_min_max(pos2(rest.min.x, rest.max.y - thickness), rest.max);
            rest.max.y -= thickness;
            band
        }
        Dock::Top => {
            let band = Rect::from_min_max(rest.min, pos2(rest.max.x, rest.min.y + thickness));
            rest.min.y += thickness;
            band
        }
        Dock::Left => {
            let band = Rect::from_min_max(rest.min, pos2(rest.min.x + thickness, rest.max.y));
            rest.min.x += thickness;
            band
        }
        Dock::Right => {
            let band = Rect::from_min_max(pos2(rest.max.x - thickness, rest.min.y), rest.max);
            rest.max.x -= thickness;
            band
        }
        Dock::Float { .. } => return None,
    };
    Some(band)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_carves_canvas() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0));
        let resolved = Layout::default().resolve(screen, |p| p != "perf");
        assert_eq!(
            resolved.canvas,
            Rect::from_min_max(pos2(0.0, 28.0), pos2(1280.0 - 300.0, 800.0))
        );
        let color = resolved.panels.iter().find(|(n, _)| *n == "color").map(|(_, r)| *r);
        assert_eq!(color.map(|c| c.height()), Some(340.0));
    }
}
