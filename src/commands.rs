//! The command layer (ARCHITECTURE §2, §17, approved 2026-10-05). Every
//! action is a named command taking and returning a `Value`; keys, panels
//! and pointer input only ever dispatch these, and Lua will call the same
//! table. Nouns are addressed by handle (`layer/3`), never by implicit
//! global state, where a command is about a specific thing.

use std::collections::BTreeMap;
use std::fmt;

use hokusai_core::BrushSetting;
use violet::compositor::{self, ONE, StrokeBlend};
use violet::document::{DEFAULT_HEIGHT, DEFAULT_WIDTH, MAX_SIDE};
use violet::history::{DirtyHint, Record};
use violet::perf::Span;
use violet::stroke::{LiveStroke, StrokePoint};

use crate::app::{DialogKind, State, Transition};
use crate::io::{self, Job};

/// What a `Handle` points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Layer,
}

/// A reference to a document noun by stable id, printed as `layer/3`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    pub kind: Kind,
    pub id: u64,
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            Kind::Layer => write!(f, "layer/{}", self.id),
        }
    }
}

/// What commands take and return. Shaped like a Lua value so the same
/// table can be called from scripts.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
    Handle(Handle),
    /// A stroke is a value handed to a command, never a stored object.
    Path(Vec<StrokePoint>),
}

impl Value {
    /// Positional argument `i`. A lone non-list value is argument 0, so
    /// `brush.size 12` and `brush.size [12]` read the same.
    fn arg(&self, i: usize) -> Option<&Value> {
        match self {
            Value::List(items) => items.get(i),
            Value::Nil => None,
            v if i == 0 => Some(v),
            _ => None,
        }
    }

    pub fn num(&self, i: usize) -> Option<f64> {
        match self.arg(i)? {
            Value::Num(n) => Some(*n),
            Value::Str(s) => s.parse().ok(),
            _ => None,
        }
    }

    pub fn text(&self, i: usize) -> Option<&str> {
        match self.arg(i)? {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    fn bool(&self, i: usize) -> Option<bool> {
        match self.arg(i)? {
            Value::Bool(b) => Some(*b),
            Value::Str(s) => s.parse().ok(),
            Value::Num(n) => Some(*n != 0.0),
            _ => None,
        }
    }

    fn layer(&self, i: usize) -> Option<u64> {
        match self.arg(i)? {
            Value::Handle(Handle { kind: Kind::Layer, id }) => Some(*id),
            Value::Str(s) => s.strip_prefix("layer/")?.parse().ok(),
            _ => None,
        }
    }
}

/// A command name plus its arguments: what a key, a panel or the pointer
/// asks for.
#[derive(Clone, Debug)]
pub struct Invocation {
    pub name: String,
    pub args: Value,
}

impl Invocation {
    pub fn new(name: &str, args: Value) -> Self {
        Self { name: name.to_owned(), args }
    }

    /// `"brush.size 12"`: the command-line form keymaps use.
    pub fn parse(line: &str) -> Option<Self> {
        let mut words = line.split_whitespace();
        let name = words.next()?.to_owned();
        let args: Vec<Value> = words
            .map(|w| w.parse::<f64>().map_or_else(|_| Value::Str(w.to_owned()), Value::Num))
            .collect();
        let args = if args.is_empty() { Value::Nil } else { Value::List(args) };
        Some(Self { name, args })
    }
}

/// A command's body. A plain fn pointer, so the builtin table is a const.
type Run = fn(&mut State, &Value) -> Result<Value, String>;

/// A registered command; `help` is the one-line text the help listing shows.
pub struct Command {
    pub name: &'static str,
    pub help: &'static str,
    run: Run,
}

/// Every command by name. A map at runtime, not an enum, so scripts can
/// add commands the binary has never heard of.
pub struct Registry {
    commands: BTreeMap<&'static str, Command>,
}

impl Registry {
    pub fn builtin() -> Self {
        let mut commands = BTreeMap::new();
        for (name, help, run) in BUILTIN {
            commands.insert(*name, Command { name, help, run: *run });
        }
        Self { commands }
    }

    /// Run a command. While a file is loading, anything that would touch
    /// the document is refused: the load is about to replace it.
    pub fn dispatch(&self, st: &mut State, inv: &Invocation) -> Result<Value, String> {
        if st.loading
            && (inv.name.starts_with("layer.")
                || inv.name.starts_with("history.")
                || inv.name.starts_with("file.")
                || inv.name == "stroke.begin")
        {
            return Err("A file operation is still in progress".into());
        }
        let cmd = self
            .commands
            .get(inv.name.as_str())
            .ok_or_else(|| format!("no command {}", inv.name))?;
        (cmd.run)(st, &inv.args)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Command> {
        self.commands.values()
    }
}

/// `(name, help, body)` for every builtin command.
const BUILTIN: &[(&str, &str, Run)] = &[
    ("stroke.begin", "start a stroke; \"erase\" erases", stroke_begin),
    ("stroke.extend", "add a path to the live stroke", stroke_extend),
    ("stroke.end", "finish the live stroke", stroke_end),
    ("stroke.cancel", "drop the live stroke", stroke_cancel),
    ("history.undo", "undo", undo),
    ("history.redo", "redo", redo),
    ("view.pan", "move the view by dx dy points", view_pan),
    ("view.zoom-by", "zoom by a factor around x y", view_zoom_by),
    ("view.zoom-in", "zoom in", |st, _| zoom_step(st, 1.25)),
    ("view.zoom-out", "zoom out", |st, _| zoom_step(st, 0.8)),
    ("view.zoom-100", "pixels 1:1", view_zoom_100),
    ("view.fit", "whole document in view", view_fit),
    ("view.rotate-by", "rotate by radians around x y", view_rotate_by),
    ("view.rotate-left", "rotate 15 degrees left", |st, _| rotate_step(st, -15.0)),
    ("view.rotate-right", "rotate 15 degrees right", |st, _| rotate_step(st, 15.0)),
    ("view.rotate-reset", "no rotation", view_rotate_reset),
    ("view.flip", "mirror the view", view_flip),
    ("brush.next", "next brush", brush_next),
    ("brush.select", "brush by name", brush_select),
    ("brush.size", "brush radius in pixels", brush_size),
    ("brush.size-up", "bigger brush", |st, _| size_step(st, 1.25)),
    ("brush.size-down", "smaller brush", |st, _| size_step(st, 0.8)),
    ("brush.opacity", "brush opacity 0 to 1", brush_opacity),
    ("brush.opacity-up", "more opaque", |st, _| opacity_step(st, 1.25)),
    ("brush.opacity-down", "less opaque", |st, _| opacity_step(st, 0.8)),
    ("brush.smoothing", "next smoothing step", brush_smoothing),
    ("tool.eraser", "toggle the eraser", tool_eraser),
    ("color.set", "brush color, sRGB r g b from 0 to 1", color_set),
    ("color.pick", "brush color from the canvas at x y", color_pick),
    ("color.eyedropper", "toggle the eyedropper", |st, _| {
        st.eyedropper = !st.eyedropper;
        Ok(Value::Bool(st.eyedropper))
    }),
    ("layer.add", "new layer above the active one", layer_add),
    ("layer.delete", "delete a layer", layer_delete),
    ("layer.raise", "move a layer up", |st, a| layer_move(st, a, 1)),
    ("layer.lower", "move a layer down", |st, a| layer_move(st, a, -1)),
    ("layer.select", "make a layer active", layer_select),
    ("layer.opacity", "layer opacity 0 to 1", layer_opacity),
    ("layer.opacity-done", "end an opacity drag", |st, _| {
        st.history.seal();
        Ok(Value::Nil)
    }),
    ("layer.visible", "show or hide a layer", layer_visible),
    ("file.new", "new document, width height", file_new),
    ("file.open", "open a .lily file", file_open),
    ("file.save", "save, to a path if given", file_save),
    ("file.save-as", "save under a new name", |st, _| {
        st.open_dialog(DialogKind::SaveAs);
        Ok(Value::Nil)
    }),
    ("file.export", "flattened PNG", file_export),
    ("ui.chrome", "show or hide every panel", |st, _| {
        st.chrome = !st.chrome;
        Ok(Value::Bool(st.chrome))
    }),
    ("ui.perf", "show or hide the measurements", |st, _| {
        st.perf_open = !st.perf_open;
        Ok(Value::Bool(st.perf_open))
    }),
    ("ui.dismiss", "close the dialog", |st, _| {
        st.dialog = None;
        st.help_open = false;
        st.eyedropper = false;
        Ok(Value::Nil)
    }),
    ("ui.help", "keyboard shortcuts", |st, _| {
        st.help_open = !st.help_open;
        Ok(Value::Nil)
    }),
    ("ui.terminal", "show or hide the terminal", |st, _| {
        st.terminal_open = !st.terminal_open;
        Ok(Value::Bool(st.terminal_open))
    }),
    ("terminal.input", "update terminal input text", |st, args| {
        if let Some(s) = args.text(0) { st.terminal_input = s.to_owned(); }
        Ok(Value::Nil)
    }),
    ("terminal.log", "append a line to the terminal log", |st, args| {
        if let Some(s) = args.text(0) { st.terminal_log.push(s.to_owned()); }
        Ok(Value::Nil)
    }),
    ("app.quit", "confirm, save and quit", |st, args| {
        stroke_end(st, &Value::Nil)?;
        if args.bool(0) != Some(true) {
            st.open_dialog(DialogKind::Quit(st.save_note()));
            return Ok(Value::Nil);
        }
        st.transition(Transition::Quit)?;
        Ok(Value::Nil)
    }),
];

fn stroke_begin(st: &mut State, args: &Value) -> Result<Value, String> {
    if st.loading {
        return Err("still loading".into());
    }
    let color = st.brushes.color;
    st.note_recent(color);
    let erase = st.erasing || args.text(0) == Some("erase");
    let layer = st.doc.layer(st.doc.active).ok_or("no layer to paint on")?;
    if !layer.visible {
        return Err(format!("{} is hidden", layer.name));
    }
    let blend = if erase { StrokeBlend::Erase } else { StrokeBlend::Normal };
    let brush = st.brushes.stroke_brush(erase);
    let base = if brush.get(BrushSetting::Smudge).base_value > 0.0 {
        layer.tiles.clone()
    } else {
        Default::default()
    };
    st.stroke = Some(LiveStroke::new(
        brush,
        layer.id,
        blend,
        st.brushes.opacity(),
        st.doc.tiles_across(),
        base,
    ));
    Ok(Value::Nil)
}

fn stroke_extend(st: &mut State, args: &Value) -> Result<Value, String> {
    let Value::Path(points) = args else { return Err("stroke.extend wants a path".into()) };
    let Some(stroke) = st.stroke.as_mut() else { return Ok(Value::Nil) };
    let started = std::time::Instant::now();
    let dabs_before = stroke.scratch.dabs;
    stroke.extend(points);
    st.perf.frame.dabs += stroke.scratch.dabs - dabs_before;
    st.perf.frame.samples += points.len() as u64;
    let dirty = stroke.scratch.take_dirty();
    st.perf.record_since(Span::Stroke, started);
    st.display.mark(dirty);
    Ok(Value::Nil)
}

fn stroke_end(st: &mut State, _: &Value) -> Result<Value, String> {
    let Some(mut stroke) = st.stroke.take() else { return Ok(Value::Nil) };
    let started = std::time::Instant::now();
    stroke.finish();
    let (record, keys) = stroke.commit(&mut st.doc);
    st.history.push(record);
    st.display.mark(keys);
    st.changed();
    st.perf.record_since(Span::Commit, started);
    Ok(Value::Nil)
}

fn stroke_cancel(st: &mut State, _: &Value) -> Result<Value, String> {
    if let Some(stroke) = st.stroke.take() {
        st.display.mark(stroke.scratch.keys().collect::<Vec<_>>());
    }
    Ok(Value::Nil)
}

fn undo(st: &mut State, _: &Value) -> Result<Value, String> {
    stroke_cancel(st, &Value::Nil)?;
    let started = std::time::Instant::now();
    let keys = st.history.undo(&mut st.doc);
    st.perf.record_since(Span::Undo, started);
    Ok(Value::Bool(apply_history(st, keys)))
}

fn redo(st: &mut State, _: &Value) -> Result<Value, String> {
    stroke_cancel(st, &Value::Nil)?;
    let started = std::time::Instant::now();
    let keys = st.history.redo(&mut st.doc);
    st.perf.record_since(Span::Undo, started);
    Ok(Value::Bool(apply_history(st, keys)))
}

/// Repaint what an undo or redo touched. False when there was nothing to
/// undo or redo.
fn apply_history(st: &mut State, hint: Option<DirtyHint>) -> bool {
    match hint {
        Some(DirtyHint::Keys(keys)) => {
            st.display.mark(keys);
            st.changed();
            true
        }
        Some(DirtyHint::AllDoc) => {
            st.display.mark_doc(&st.doc);
            st.changed();
            true
        }
        None => false,
    }
}

/// The point to zoom or rotate around: `x y` from the arguments at `at`,
/// else the pointer when it is over the canvas, else the canvas center.
fn pivot(st: &State, args: &Value, at: usize) -> eframe::egui::Pos2 {
    match (args.num(at), args.num(at + 1)) {
        (Some(x), Some(y)) => eframe::egui::pos2(x as f32, y as f32),
        _ => st.pointer.filter(|p| st.canvas.contains(*p)).unwrap_or(st.canvas.center()),
    }
}

fn view_pan(st: &mut State, args: &Value) -> Result<Value, String> {
    let dx = args.num(0).unwrap_or(0.0) as f32;
    let dy = args.num(1).unwrap_or(0.0) as f32;
    st.view.pan(st.canvas, st.ppp, eframe::egui::vec2(dx, dy));
    Ok(Value::Nil)
}

fn view_zoom_by(st: &mut State, args: &Value) -> Result<Value, String> {
    let factor = args.num(0).ok_or("view.zoom-by wants a factor")? as f32;
    let around = pivot(st, args, 1);
    st.view.zoom_at(st.canvas, st.ppp, around, factor);
    Ok(Value::Num(st.view.zoom as f64))
}

fn zoom_step(st: &mut State, factor: f32) -> Result<Value, String> {
    let center = st.canvas.center();
    st.view.zoom_at(st.canvas, st.ppp, center, factor);
    Ok(Value::Num(st.view.zoom as f64))
}

fn view_zoom_100(st: &mut State, _: &Value) -> Result<Value, String> {
    st.view.set_zoom(st.canvas, st.ppp, 1.0);
    Ok(Value::Nil)
}

fn view_fit(st: &mut State, _: &Value) -> Result<Value, String> {
    st.view.fit(st.canvas, st.ppp, st.doc.width, st.doc.height);
    Ok(Value::Nil)
}

fn view_rotate_by(st: &mut State, args: &Value) -> Result<Value, String> {
    let radians = args.num(0).ok_or("view.rotate-by wants radians")? as f32;
    let around = pivot(st, args, 1);
    st.view.rotate_at(st.canvas, st.ppp, around, radians);
    Ok(Value::Nil)
}

fn rotate_step(st: &mut State, degrees: f32) -> Result<Value, String> {
    let center = st.canvas.center();
    st.view.rotate_at(st.canvas, st.ppp, center, degrees.to_radians());
    Ok(Value::Num(st.view.degrees() as f64))
}

fn view_rotate_reset(st: &mut State, _: &Value) -> Result<Value, String> {
    st.view.set_angle(st.canvas, st.ppp, 0.0);
    Ok(Value::Nil)
}

fn view_flip(st: &mut State, _: &Value) -> Result<Value, String> {
    st.view.toggle_flip(st.canvas, st.ppp);
    Ok(Value::Bool(st.view.flip))
}

fn brush_next(st: &mut State, _: &Value) -> Result<Value, String> {
    st.brushes.next();
    st.hud();
    Ok(Value::Str(st.brushes.name().to_owned()))
}

fn brush_select(st: &mut State, args: &Value) -> Result<Value, String> {
    let name = args.text(0).ok_or("brush.select wants a name")?;
    if !st.brushes.select(name) {
        return Err(format!("no brush {name}"));
    }
    st.erasing = false;
    st.hud();
    Ok(Value::Nil)
}

fn brush_size(st: &mut State, args: &Value) -> Result<Value, String> {
    let px = args.num(0).ok_or("brush.size wants pixels")? as f32;
    st.brushes.set_size(st.erasing, px);
    st.hud();
    Ok(Value::Num(st.brushes.size(st.erasing) as f64))
}

fn size_step(st: &mut State, factor: f32) -> Result<Value, String> {
    let size = st.brushes.size(st.erasing) * factor;
    st.brushes.set_size(st.erasing, size);
    st.hud();
    Ok(Value::Num(st.brushes.size(st.erasing) as f64))
}

fn brush_opacity(st: &mut State, args: &Value) -> Result<Value, String> {
    let opacity = args.num(0).ok_or("brush.opacity wants 0 to 1")? as f32;
    st.brushes.set_opacity(opacity);
    st.hud();
    Ok(Value::Num(st.brushes.opacity() as f64))
}

fn opacity_step(st: &mut State, factor: f32) -> Result<Value, String> {
    // Multiplicative, like size: each press is the same ratio at any opacity.
    let opacity = (st.brushes.opacity() * factor).clamp(violet::brushes::MIN_OPACITY, 1.0);
    st.brushes.set_opacity(opacity);
    st.hud();
    Ok(Value::Num(st.brushes.opacity() as f64))
}

/// `brush.smoothing 2` sets that step; with no argument, the next step.
fn brush_smoothing(st: &mut State, args: &Value) -> Result<Value, String> {
    let steps = violet::brushes::SMOOTHING.len();
    st.brushes.smoothing = match args.num(0) {
        Some(step) => (step.max(0.0) as usize).min(steps - 1),
        None => (st.brushes.smoothing + 1) % steps,
    };
    st.hud();
    Ok(Value::Num(st.brushes.smoothing as f64))
}

fn tool_eraser(st: &mut State, _: &Value) -> Result<Value, String> {
    st.erasing = !st.erasing;
    st.hud();
    Ok(Value::Bool(st.erasing))
}

fn color_set(st: &mut State, args: &Value) -> Result<Value, String> {
    let channel = |i| args.num(i).map(|v| v.clamp(0.0, 1.0) as f32);
    let (Some(r), Some(g), Some(b)) = (channel(0), channel(1), channel(2)) else {
        return Err("color.set wants r g b from 0 to 1".into());
    };
    st.brushes.color = [r, g, b];
    Ok(Value::Nil)
}

fn color_pick(st: &mut State, args: &Value) -> Result<Value, String> {
    let (Some(x), Some(y)) = (args.num(0), args.num(1)) else {
        return Err("color.pick wants x y".into());
    };
    let (x, y) = (x.floor() as i64, y.floor() as i64);
    if x < 0 || y < 0 || x >= st.doc.width as i64 || y >= st.doc.height as i64 {
        return Ok(Value::Nil);
    }
    let tile_size = compositor::N as i64;
    let key = ((x / tile_size) as i32, (y / tile_size) as i32);
    let (ix, iy) = ((x % tile_size) as usize, (y % tile_size) as usize);
    let visible = st.doc.layers.iter().filter(|l| l.visible);
    let pixel = compositor::flatten_px(visible.map(|l| {
        let layer_pixel = l.tiles.get(&key).map_or([0; 4], |t| t[iy][ix]);
        (layer_pixel, (l.opacity.clamp(0.0, 1.0) * ONE as f32) as u32)
    }));
    let mut rgb = [0u8; 3];
    compositor::to_display(&[pixel], st.doc.paper_linear(), |_, c| rgb = c);
    st.brushes.color = rgb.map(|c| c as f32 / 255.0);
    Ok(Value::List(rgb.iter().map(|&c| Value::Num(c as f64 / 255.0)).collect()))
}

/// `(stack index, id)` of the layer named by argument `at`, defaulting to
/// the active layer.
fn target_layer(st: &State, args: &Value, at: usize) -> Result<(usize, u64), String> {
    let id = args.layer(at).unwrap_or(st.doc.active);
    let index =
        st.doc.layers.iter().position(|l| l.id == id).ok_or_else(|| format!("no layer/{id}"))?;
    Ok((index, id))
}

fn layer_add(st: &mut State, _: &Value) -> Result<Value, String> {
    stroke_cancel(st, &Value::Nil)?;
    let above = st.doc.layers.iter().position(|l| l.id == st.doc.active).map_or(0, |i| i + 1);
    let name = format!("Layer {}", st.doc.layers.len() + 1);
    let id = st.doc.push_layer(&name);
    // push_layer appends on top; move the new layer to just above the active one.
    if let Some(new_layer) = st.doc.layers.pop() {
        st.doc.layers.insert(above.min(st.doc.layers.len()), new_layer);
    }
    st.doc.active = id;
    st.history.push(Record::added(above));
    st.changed();
    Ok(Value::Handle(Handle { kind: Kind::Layer, id }))
}

fn layer_delete(st: &mut State, args: &Value) -> Result<Value, String> {
    stroke_cancel(st, &Value::Nil)?;
    if st.doc.layers.len() < 2 {
        return Err("the last layer stays".into());
    }
    let (index, _) = target_layer(st, args, 0)?;
    let layer = st.doc.layers.remove(index);
    st.display.mark(layer.tiles.keys().copied().collect::<Vec<_>>());
    let below = st.doc.layers.get(index.saturating_sub(1));
    if let Some(next_active) = below.or(st.doc.layers.first()) {
        st.doc.active = next_active.id;
    }
    st.history.push(Record::removed(index, layer));
    st.changed();
    Ok(Value::Nil)
}

fn layer_move(st: &mut State, args: &Value, by: i64) -> Result<Value, String> {
    stroke_cancel(st, &Value::Nil)?;
    let (from, _) = target_layer(st, args, 0)?;
    let to = from as i64 + by;
    if to < 0 || to >= st.doc.layers.len() as i64 {
        return Ok(Value::Nil);
    }
    let to = to as usize;
    let layer = st.doc.layers.remove(from);
    st.display.mark(layer.tiles.keys().copied().collect::<Vec<_>>());
    st.doc.layers.insert(to, layer);
    st.history.push(Record::moved(from, to));
    st.changed();
    Ok(Value::Nil)
}

fn layer_select(st: &mut State, args: &Value) -> Result<Value, String> {
    let (_, id) = target_layer(st, args, 0)?;
    st.doc.active = id;
    Ok(Value::Nil)
}

fn layer_opacity(st: &mut State, args: &Value) -> Result<Value, String> {
    let opacity = args.num(0).ok_or("layer.opacity wants 0 to 1")?.clamp(0.0, 1.0) as f32;
    let (index, id) = target_layer(st, args, 1)?;
    let layer = &mut st.doc.layers[index];
    let old = std::mem::replace(&mut layer.opacity, opacity);
    let keys: Vec<_> = layer.tiles.keys().copied().collect();
    st.display.mark(keys);
    st.history.push(Record::opacity(id, old));
    st.changed();
    Ok(Value::Nil)
}

fn layer_visible(st: &mut State, args: &Value) -> Result<Value, String> {
    let (index, _) = target_layer(st, args, 1)?;
    let layer = &mut st.doc.layers[index];
    layer.visible = args.bool(0).unwrap_or(!layer.visible);
    let keys: Vec<_> = layer.tiles.keys().copied().collect();
    st.display.mark(keys);
    st.changed();
    Ok(Value::Nil)
}

fn file_new(st: &mut State, args: &Value) -> Result<Value, String> {
    stroke_end(st, &Value::Nil)?;
    if matches!(args, Value::Nil) {
        st.open_dialog(DialogKind::New(crate::settings::NewDrawing {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            note: st.save_note(),
        }));
        return Ok(Value::Nil);
    }
    let side = |i, default| args.num(i).map_or(default, |v| (v as u32).clamp(1, MAX_SIDE));
    st.transition(Transition::New(side(0, DEFAULT_WIDTH), side(1, DEFAULT_HEIGHT)))?;
    Ok(Value::Nil)
}

fn file_open(st: &mut State, args: &Value) -> Result<Value, String> {
    let Some(path) = args.text(0) else {
        st.open_dialog(DialogKind::Open);
        return Ok(Value::Nil);
    };
    let path = io::expand(path);
    stroke_end(st, &Value::Nil)?;
    st.transition(Transition::Open(path))?;
    Ok(Value::Nil)
}

fn file_save(st: &mut State, args: &Value) -> Result<Value, String> {
    stroke_end(st, &Value::Nil)?;
    let path = match args.text(0) {
        Some(p) => io::with_extension(io::expand(p)),
        None => st.path.clone().unwrap_or_else(|| io::untitled(&st.paths)),
    };
    st.save_to(path);
    Ok(Value::Nil)
}

fn file_export(st: &mut State, args: &Value) -> Result<Value, String> {
    stroke_end(st, &Value::Nil)?;
    let path = match args.text(0) {
        Some(p) => io::expand(p),
        None => st.path.clone().unwrap_or_else(|| io::untitled(&st.paths)).with_extension("png"),
    };
    st.io.run(Job::Export { doc: st.doc.clone(), path });
    Ok(Value::Nil)
}
