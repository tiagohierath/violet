//! Pointer input to commands. Mouse and pen arrive as the same kind of
//! sample; whichever presses first owns the gesture until it lifts. A press
//! on the canvas paints, unless a held key turns it into a pan (Space, or
//! the middle button) or an eyedropper (Ctrl or Alt). Stroke samples are
//! batched and dispatched once per frame as one path (ARCHITECTURE §17).

use eframe::egui::{Pos2, Rect};

use violet::stroke::StrokePoint;

use crate::commands::{Invocation, Value};

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// Screen points.
    pub pos: Pos2,
    pub pressure: f32,
    pub tilt: (f32, f32),
    /// Seconds.
    pub time: f64,
    pub down: bool,
    pub eraser: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Mouse,
    Pen,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Idle,
    Stroke,
    Pan(Pos2),
    /// Zooms around `origin` as the pen moves: up zooms in.
    Zoom { origin: Pos2, last: Pos2 },
    Pick,
    /// Pressed on a panel: the panel's own widgets handle it.
    Panel,
}

/// What a press turns into, decided by the keys held when it lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    None,
    Pan,
    Zoom,
    Pick,
}

/// Zoom factor per screen pixel of pen travel: 100 px is about 1.65x.
const ZOOM_PER_PIXEL: f32 = 0.005;

/// Upward travel (negative `dy`) zooms in.
fn zoom_factor(dy: f32) -> f32 {
    (-dy * ZOOM_PER_PIXEL).exp()
}

#[cfg(test)]
mod tests {
    use super::zoom_factor;

    #[test]
    fn dragging_up_zooms_in() {
        assert!(zoom_factor(-50.0) > 1.0);
        assert!(zoom_factor(50.0) < 1.0);
        assert_eq!(zoom_factor(0.0), 1.0);
    }
}

pub struct Frame<'a> {
    pub canvas: Rect,
    pub panels: &'a [Rect],
    pub hold: Hold,
    pub to_doc: &'a dyn Fn(Pos2) -> Pos2,
    pub out: &'a mut Vec<Invocation>,
}

pub struct Input {
    mode: Mode,
    owner: Option<Source>,
    batch: Vec<StrokePoint>,
}

impl Input {
    pub fn new() -> Self {
        Self { mode: Mode::Idle, owner: None, batch: Vec::new() }
    }

    pub fn busy(&self) -> bool {
        self.mode != Mode::Idle
    }

    pub fn painting(&self) -> bool {
        self.mode == Mode::Stroke
    }

    pub fn sample(&mut self, s: Sample, src: Source, f: &mut Frame) {
        if self.mode == Mode::Idle {
            if s.down {
                self.press(s, src, f);
            }
            return;
        }
        if self.owner != Some(src) {
            return;
        }
        if !s.down {
            self.release(f);
            return;
        }
        match self.mode {
            Mode::Stroke => self.batch.push(point(s, f)),
            Mode::Pan(last) => {
                let d = s.pos - last;
                f.out.push(Invocation::new(
                    "view.pan",
                    Value::List(vec![Value::Num(d.x as f64), Value::Num(d.y as f64)]),
                ));
                self.mode = Mode::Pan(s.pos);
            }
            Mode::Zoom { origin, last } => {
                let factor = zoom_factor(s.pos.y - last.y);
                let args = Value::List(vec![Value::Num(f64::from(factor)), Value::Num(origin.x as f64), Value::Num(origin.y as f64)]);
                f.out.push(Invocation::new("view.zoom-by", args));
                self.mode = Mode::Zoom { origin, last: s.pos };
            }
            Mode::Pick => f.out.push(pick(s, f)),
            Mode::Panel | Mode::Idle => {}
        }
    }

    /// A middle-button press always pans.
    pub fn press_pan(&mut self, pos: Pos2, f: &Frame) {
        if self.mode == Mode::Idle && f.canvas.contains(pos) && !f.panels.iter().any(|r| r.contains(pos)) {
            self.mode = Mode::Pan(pos);
            self.owner = Some(Source::Mouse);
        }
    }

    pub fn release_pan(&mut self) {
        if matches!(self.mode, Mode::Pan(_)) && self.owner == Some(Source::Mouse) {
            self.mode = Mode::Idle;
            self.owner = None;
        }
    }

    fn press(&mut self, s: Sample, src: Source, f: &mut Frame) {
        self.owner = Some(src);
        if !f.canvas.contains(s.pos) || f.panels.iter().any(|r| r.contains(s.pos)) {
            self.mode = Mode::Panel;
            return;
        }
        self.mode = match f.hold {
            Hold::Pan => Mode::Pan(s.pos),
            Hold::Zoom => Mode::Zoom { origin: s.pos, last: s.pos },
            Hold::Pick => {
                f.out.push(pick(s, f));
                Mode::Pick
            }
            Hold::None => {
                let args = if s.eraser { Value::Str("erase".into()) } else { Value::Nil };
                f.out.push(Invocation::new("stroke.begin", args));
                self.batch.push(point(s, f));
                Mode::Stroke
            }
        };
    }

    fn release(&mut self, f: &mut Frame) {
        if self.mode == Mode::Stroke {
            self.flush(f);
            f.out.push(Invocation::new("stroke.end", Value::Nil));
        }
        self.mode = Mode::Idle;
        self.owner = None;
    }

    /// Focus loss finishes the gesture even if the release happened outside
    /// the window. A later hover must not continue painting.
    pub fn finish(&mut self, f: &mut Frame) { self.release(f); }

    /// Dispatches the frame's stroke samples as one path.
    pub fn flush(&mut self, f: &mut Frame) {
        if !self.batch.is_empty() {
            f.out.push(Invocation::new("stroke.extend", Value::Path(std::mem::take(&mut self.batch))));
        }
    }
}

fn point(s: Sample, f: &Frame) -> StrokePoint {
    let d = (f.to_doc)(s.pos);
    StrokePoint { x: d.x, y: d.y, pressure: s.pressure, xtilt: s.tilt.0, ytilt: s.tilt.1, time: s.time }
}

fn pick(s: Sample, f: &Frame) -> Invocation {
    let d = (f.to_doc)(s.pos);
    Invocation::new("color.pick", Value::List(vec![Value::Num(d.x as f64), Value::Num(d.y as f64)]))
}
