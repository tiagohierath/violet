//! The panels (ARCHITECTURE §12, §13). Each one draws into the rect the
//! layout hands it, takes every color from the theme, and only ever asks
//! for commands. Nothing here knows where it sits or touches the document.
//!
//! One control language everywhere: 44px icon buttons in the tool bar, 36px
//! fields and buttons in panels, 28px rows in the menu and status lines.
//! The selected state is always the accent fill with on_accent ink.

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Id, Layout, Mesh, Pos2, Rect, Response, Sense,
    Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

use violet::brushes::{MAX_SIZE, MIN_OPACITY, MIN_SIZE, SMOOTHING};
use violet::perf::Span;

use crate::app::State;
use crate::commands::{Handle, Invocation, Kind, Registry, Value};
use crate::keymap::Keymap;
use crate::theme::{Theme, bold, regular};

const PALETTE: [[f32; 3]; 12] = [
    [0.04, 0.04, 0.04],
    [1.0, 1.0, 1.0],
    [0.78, 0.12, 0.12],
    [0.95, 0.48, 0.12],
    [0.97, 0.84, 0.22],
    [0.22, 0.62, 0.28],
    [0.14, 0.56, 0.52],
    [0.16, 0.42, 0.82],
    [0.12, 0.20, 0.52],
    [0.50, 0.26, 0.66],
    [0.92, 0.50, 0.66],
    [0.46, 0.30, 0.18],
];

/// Names for the smoothing steps, in `SMOOTHING` order.
const SMOOTHING_NAMES: [&str; 4] = ["Off", "Low", "Medium", "High"];

const LINE: f32 = 24.0;
const GAP: f32 = 8.0;
const MONO: f32 = 14.0;

fn mono() -> egui::FontId {
    egui::FontId::monospace(MONO)
}

/// Inner padding of each panel's rect.
pub fn padding(_name: &str) -> Vec2 {
    vec2(8.0, 8.0)
}

pub struct Chrome<'a> {
    pub st: &'a State,
    pub keymap: &'a Keymap,
    pub registry: &'a Registry,
    pub theme: &'a Theme,
}

impl Chrome<'_> {
    pub fn panel(&self, ui: &mut Ui, name: &str, out: &mut Vec<Invocation>) {
        match name {
            "color" => self.color(ui, out),
            "info" => self.info(ui, out),
            "terminal" => self.terminal(ui, out),
            "perf" => self.perf(ui),
            _ => {}
        }
    }

    /// `"Ctrl+Shift+Z"` for the first key bound to `command`, read from the
    /// keymap so the hints follow the config.
    pub fn key(&self, command: &str) -> Option<String> {
        self.keymap
            .describe()
            .find(|(_, line)| line.split_whitespace().next() == Some(command))
            .map(|(chord, _)| pretty_chord(chord))
    }

    fn hint(&self, name: &str, command: &str) -> String {
        match self.key(command) {
            Some(key) => format!("{name} ({key})"),
            None => name.to_owned(),
        }
    }

    // ---- color (hue ring + SV square) ---------------------------------

    fn color(&self, ui: &mut Ui, out: &mut Vec<Invocation>) {
        let t = self.theme;
        let rect = ui.max_rect();
        let rgb = self.st.brushes.color;
        let painter = ui.painter().clone();

        // Title, with the current color and its hex code at the right.
        let title = Rect::from_min_size(rect.min, vec2(rect.width(), LINE));
        painter.text(title.left_center(), Align2::LEFT_CENTER, "Color", bold(MONO), t.text);
        let hex = format!("#{:02X}{:02X}{:02X}", to8(rgb[0]), to8(rgb[1]), to8(rgb[2]));
        let code = painter.text(title.right_center(), Align2::RIGHT_CENTER, hex, mono(), t.text);
        let chip = Rect::from_center_size(pos2(code.left() - 8.0 - 10.0, title.center().y), Vec2::splat(20.0));
        painter.rect_filled(chip, CornerRadius::ZERO, rgb8(rgb));
        painter.rect_stroke(chip, CornerRadius::ZERO, Stroke::new(1.0, t.border), StrokeKind::Inside);

        // The swatches sit at the bottom; the ring and its square fill the rest.
        let swatch_h = 28.0;
        let swatches_h = 2.0 * swatch_h + 4.0;
        let top = title.bottom() + GAP;
        let bottom = rect.bottom() - swatches_h - GAP;
        let outer = ((bottom - top).min(rect.width()) / 2.0).max(40.0);
        let center = pos2(rect.center().x, top + (bottom - top) / 2.0);
        let ring_w = 16.0;
        let inner = outer - ring_w;
        let half = (inner - 8.0) / std::f32::consts::SQRT_2;
        let square = Rect::from_center_size(center, Vec2::splat(2.0 * half));
        let ring_box = Rect::from_center_size(center, Vec2::splat(2.0 * outer));

        // Hue survives black and gray, where the color alone forgets it.
        let id = Id::new("violet-color-hsv");
        let remembered: Option<([f32; 3], [f32; 3])> = ui.data(|d| d.get_temp(id));
        let mut hsv = match remembered {
            Some((seen, hsv)) if seen == rgb => hsv,
            Some((_, old)) => {
                let new = rgb_to_hsv(rgb);
                if new[1] == 0.0 || new[2] == 0.0 { [old[0], new[1], new[2]] } else { new }
            }
            None => rgb_to_hsv(rgb),
        };
        let mut changed = false;

        let ring = ui.allocate_rect(ring_box, Sense::click_and_drag());
        ring_mesh(&painter, center, inner, outer);
        if ring.is_pointer_button_down_on()
            && let (Some(origin), Some(p)) = (ui.input(|i| i.pointer.press_origin()), ring.interact_pointer_pos())
        {
            let from = origin.distance(center);
            if (inner..=outer).contains(&from) {
                let angle = (p.y - center.y).atan2(p.x - center.x);
                hsv[0] = (angle / std::f32::consts::TAU + 0.25).rem_euclid(1.0);
                changed = true;
            }
        }
        let mid = (inner + outer) / 2.0;
        let on_ring = pos2(
            center.x + mid * (hsv[0] * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2).cos(),
            center.y + mid * (hsv[0] * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2).sin(),
        );
        marker(&painter.with_clip_rect(ring_box.expand(6.0)), t, on_ring);

        let sq = ui.allocate_rect(square, Sense::click_and_drag());
        sv_mesh(&painter, square, hsv[0]);
        painter.rect_stroke(square, CornerRadius::ZERO, Stroke::new(1.0, t.border), StrokeKind::Inside);
        if sq.is_pointer_button_down_on()
            && let Some(p) = sq.interact_pointer_pos()
        {
            hsv[1] = ((p.x - square.left()) / square.width()).clamp(0.0, 1.0);
            hsv[2] = 1.0 - ((p.y - square.top()) / square.height()).clamp(0.0, 1.0);
            changed = true;
        }
        let at = pos2(square.left() + hsv[1] * square.width(), square.top() + (1.0 - hsv[2]) * square.height());
        marker(&painter.with_clip_rect(square.expand(6.0)), t, at);

        let new_rgb = if changed { hsv_to_rgb(hsv) } else { rgb };
        ui.data_mut(|d| d.insert_temp(id, (new_rgb, hsv)));
        if changed && new_rgb != rgb {
            out.push(color_set(new_rgb));
        }

        // Swatches: two rows of six, filling the width.
        let per_row = 6;
        let w = (rect.width() - 4.0 * (per_row - 1) as f32) / per_row as f32;
        let top = rect.bottom() - swatches_h;
        for (i, c) in PALETTE.iter().enumerate() {
            let (row, col) = (i / per_row, i % per_row);
            let r = Rect::from_min_size(
                pos2(rect.left() + col as f32 * (w + 4.0), top + row as f32 * (swatch_h + 4.0)),
                vec2(w, swatch_h),
            );
            let response = ui.allocate_rect(r, Sense::click());
            painter.rect_filled(r, CornerRadius::ZERO, rgb8(*c));
            let edge = if response.hovered() { t.accent } else { t.border };
            painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, edge), StrokeKind::Inside);
            if response.clicked() {
                out.push(color_set(*c));
            }
        }
    }

    // ---- measurements ------------------------------------------------

    fn perf(&self, ui: &mut Ui) {
        let p = &self.st.perf;
        let f = &p.last;
        ui.label(egui::RichText::new("Measurements").font(bold(MONO)));
        ui.monospace("span          last    avg   peak");
        for span in Span::ALL {
            let s = p.stat(span);
            ui.monospace(format!("{:<11}{:>7.2}{:>7.2}{:>7.2}", span.name(), s.last, s.avg, s.peak));
        }
        ui.separator();
        let fps = if p.interval.avg > 0.0 { 1000.0 / p.interval.avg } else { 0.0 };
        ui.monospace(format!("fps {fps:.0}   startup {:.0} ms", self.st.startup_ms.unwrap_or(0.0)));
        if let Some(cpu) = p.eframe_cpu {
            ui.monospace(format!("eframe cpu {cpu:.2} ms"));
        }
        ui.monospace(format!("allocs {}   {} KiB", f.allocs, f.alloc_bytes / 1024));
        ui.monospace(format!("tiles {}   uploads {}   {} KiB", f.tiles_composited, f.uploads, f.upload_bytes / 1024));
        ui.monospace(format!("draws {}   dabs {}   samples {}", f.draw_calls, f.dabs, f.samples));
        ui.monospace(format!("doc {} MiB   undo {} MiB", self.st.doc.bytes() >> 20, self.st.history.bytes() >> 20));
    }

    // ---- info (pure text, like cat-ing a status file) ----------------------

    fn info(&self, ui: &mut Ui, out: &mut Vec<Invocation>) {
        let st = self.st;
        let t = self.theme;
        let rect = ui.max_rect();
        let p = ui.painter().clone();
        let mut y = rect.top();

        let line = |p: &egui::Painter, y: &mut f32, text: &str, ink: Color32| {
            p.text(pos2(rect.left() + 8.0, *y + LINE / 2.0), Align2::LEFT_CENTER, text, mono(), ink);
            *y += LINE;
        };

        line(&p, &mut y, &format!("preset  {}", st.brushes.name()), t.text);
        line(&p, &mut y, &format!("tool    {}", tool_name(st)), t.text);

        let size = st.brushes.size(st.erasing);
        let span = (MAX_SIZE / MIN_SIZE).ln();
        let at = (size / MIN_SIZE).ln() / span;
        let size_hint = format!("Brush radius ({} smaller, {} bigger)", key_or(self, "brush.size-down"), key_or(self, "brush.size-up"));
        let r = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), LINE));
        y += LINE;
        if let (Some(v), _) = field(ui, t, r, "size", &format!("{} px", px(size)), at, None, &size_hint) {
            let px = MIN_SIZE * (v * span).exp();
            out.push(Invocation::new("brush.size", Value::Num(f64::from(px))));
        }

        let opacity = st.brushes.opacity();
        let hint = format!("Stroke opacity ({} less, {} more)", key_or(self, "brush.opacity-down"), key_or(self, "brush.opacity-up"));
        let r = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), LINE));
        y += LINE;
        if let (Some(v), _) = field(ui, t, r, "opacity", &format!("{:.0}%", opacity * 100.0), opacity_position(opacity), None, &hint) {
            out.push(Invocation::new("brush.opacity", Value::Num(f64::from(opacity_from_position(v)))));
        }

        let steps = SMOOTHING.len();
        let step = st.brushes.smoothing.min(steps - 1);
        let sname = SMOOTHING_NAMES.get(step).copied().unwrap_or("On");
        let sm_at = step as f32 / (steps - 1) as f32;
        let hint = self.hint("Smoothing steadies the line and lags behind the pen", "brush.smoothing");
        let r = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), LINE));
        y += LINE;
        if let (Some(v), _) = field(ui, t, r, "smooth", sname, sm_at, Some(steps), &hint) {
            let new = (v * (steps - 1) as f32).round() as usize;
            if new != step {
                out.push(Invocation::new("brush.smoothing", Value::Num(new as f64)));
            }
        }

        line(&p, &mut y, "", t.text);

        let actions = [("new", "layer.add"), ("up", "layer.raise"), ("down", "layer.lower"), ("del", "layer.delete")];
        for (i, (label, command)) in actions.into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(rect.left() + i as f32 * 64.0, y), vec2(64.0, LINE));
            let response = ui.allocate_rect(r, Sense::click()).on_hover_text(self.hint(label, command));
            let ink = if response.hovered() { t.accent } else { t.text };
            p.text(r.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, label, mono(), ink);
            if response.clicked() {
                out.push(Invocation::new(command, Value::Nil));
            }
        }
        y += LINE;

        let doc = &st.doc;
        for layer in doc.layers.iter().rev() {
            let selected = doc.active == layer.id;
            let marker = if selected { ">" } else { " " };
            let vis = if layer.visible { "*" } else { "." };
            let note = if layer.visible { format!("{:.0}%", layer.opacity * 100.0) } else { "hidden".into() };
            let ink = if selected { t.accent } else { t.text };
            let text = format!("{marker}{vis} {:<12} {note}", layer.name);
            p.text(pos2(rect.left() + 8.0, y + LINE / 2.0), Align2::LEFT_CENTER, &text, mono(), ink);
            let r = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), LINE));
            y += LINE;
            let response = ui.allocate_rect(r, Sense::click());
            if response.clicked() && !selected {
                out.push(Invocation::new("layer.select", Value::Handle(Handle { kind: Kind::Layer, id: layer.id })));
            }
        }

        if let Some(active) = doc.layer(doc.active) {
            let handle = Value::Handle(Handle { kind: Kind::Layer, id: active.id });
            let r = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), LINE));
            y += LINE;
            let (v, response) = field(ui, t, r, "layer opacity", &format!("{:.0}%", active.opacity * 100.0), opacity_position(active.opacity), None, "Opacity of the active layer");
            if let Some(v) = v {
                out.push(Invocation::new("layer.opacity", Value::List(vec![Value::Num(f64::from(opacity_from_position(v))), handle])));
            }
            if response.drag_stopped() || response.clicked() {
                out.push(Invocation::new("layer.opacity-done", Value::Nil));
            }
        }

        line(&p, &mut y, "", t.text);

        line(&p, &mut y, &format!("zoom    {:.0}%", st.view.zoom * 100.0), t.text);
        line(&p, &mut y, &format!("rotate  {}deg", st.view.degrees() as i32), t.text);
        line(&p, &mut y, &format!("mirror  {}", if st.view.flip { "on" } else { "off" }), t.text);
        let doc_name = st
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(|| "Untitled".to_owned(), |n| n.to_string_lossy().into_owned());
        line(&p, &mut y, &format!("file    {doc_name}"), t.text);
        if st.modified() {
            line(&p, &mut y, "        unsaved", t.accent);
        }

        let _ = y;
    }

    // ---- terminal ------------------------------------------------------

    fn terminal(&self, ui: &mut Ui, out: &mut Vec<Invocation>) {
        let t = self.theme;
        let rect = ui.max_rect();
        let painter = ui.painter().clone();

        let input_h = LINE;
        let input_rect = Rect::from_min_max(pos2(rect.left(), rect.bottom() - input_h), rect.max);
        let log_rect = Rect::from_min_max(rect.min, pos2(rect.right(), input_rect.top() - 2.0));

        if log_rect.height() > 0.0 {
            ui.scope_builder(egui::UiBuilder::new().max_rect(log_rect), |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                    for line in &self.st.terminal_log {
                        ui.label(egui::RichText::new(line).font(mono()).color(t.text));
                    }
                });
            });
        }

        let id = Id::new("violet-terminal-input");
        let mut text = self.st.terminal_input.clone();
        let edit_rect = input_rect.shrink2(vec2(4.0, 2.0));
        painter.rect_filled(input_rect, CornerRadius::ZERO, t.raised);
        painter.rect_stroke(input_rect, CornerRadius::ZERO, Stroke::new(1.0, t.border), StrokeKind::Inside);
        let response = ui.scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut text)
                    .id(id)
                    .font(mono())
                    .desired_width(edit_rect.width())
                    .hint_text("> "),
            )
        }).inner;
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !text.trim().is_empty() {
            let line = text.trim().to_owned();
            out.push(Invocation::new("terminal.log", Value::Str(format!("> {line}"))));
            if let Some(inv) = Invocation::parse(&line) {
                out.push(inv);
            } else {
                out.push(Invocation::new("terminal.log", Value::Str(format!("? could not parse: {line}"))));
            }
            text.clear();
            response.request_focus();
        }
        if text != self.st.terminal_input {
            out.push(Invocation::new("terminal.input", Value::Str(text)));
        }
    }

    // ---- keyboard shortcuts ------------------------------------------

    pub fn help(&self, ui: &mut Ui) {
        let pointer = [
            ("Space + drag", "pan"),
            ("Middle button drag", "pan"),
            ("Scroll", "zoom"),
            ("Ctrl or Alt + click", "pick a color from the canvas"),
        ];
        egui::Grid::new("shortcuts").num_columns(2).spacing(vec2(24.0, 8.0)).show(ui, |ui| {
            for (key, what) in pointer {
                ui.label(egui::RichText::new(key).font(bold(MONO)));
                ui.label(capitalize(what));
                ui.end_row();
            }
            for (chord, line) in self.keymap.describe() {
                let command = line.split_whitespace().next().unwrap_or(line);
                let what = self.registry.iter().find(|c| c.name == command).map_or(command, |c| c.help);
                ui.label(egui::RichText::new(pretty_chord(chord)).font(bold(MONO)));
                ui.label(capitalize(what));
                ui.end_row();
            }
        });
    }
}

// ---- widgets -------------------------------------------------------------

/// A value field: label left, value right, the amount as a fill with an
/// accent edge. Press or drag anywhere sets it. Returns the new position
/// from 0 to 1 while pressed.
#[allow(clippy::too_many_arguments)]
fn field(ui: &mut Ui, t: &Theme, rect: Rect, label: &str, value: &str, at: f32, steps: Option<usize>, hint: &str) -> (Option<f32>, Response) {
    let response = ui.allocate_rect(rect, Sense::click_and_drag()).on_hover_text(hint);
    let at = at.clamp(0.0, 1.0);
    let p = ui.painter();
    let live = response.is_pointer_button_down_on();
    p.rect_filled(rect, CornerRadius::ZERO, if response.hovered() || live { t.hover } else { t.raised });
    let edge = rect.left() + at * rect.width();
    p.rect_filled(Rect::from_min_max(rect.min, pos2(edge, rect.bottom())), CornerRadius::ZERO, t.fill);
    if let Some(n) = steps {
        for i in 1..n.saturating_sub(1) {
            let x = rect.left() + rect.width() * i as f32 / (n - 1) as f32;
            p.line_segment([pos2(x, rect.bottom() - 4.0), pos2(x, rect.bottom())], Stroke::new(1.0, t.text));
        }
    }
    let marker = Rect::from_min_max(pos2((edge - 1.0).max(rect.left()), rect.top()), pos2((edge + 1.0).max(rect.left() + 2.0), rect.bottom()));
    p.rect_filled(marker, CornerRadius::ZERO, t.accent);
    if live {
        p.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    p.text(rect.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, label, mono(), t.text);
    p.text(rect.right_center() - vec2(8.0, 0.0), Align2::RIGHT_CENTER, value, mono(), t.text);
    let v = if live {
        response.interact_pointer_pos().map(|pos| ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0))
    } else {
        None
    };
    (v, response)
}

fn text_button(ui: &mut Ui, t: &Theme, rect: Rect, label: &str, selected: bool, hint: &str) -> Response {
    let response = ui.allocate_rect(rect, Sense::click()).on_hover_text(hint);
    let (bg, ink) = state_colors(t, &response, selected);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::ZERO, bg);
    p.text(rect.center(), Align2::CENTER_CENTER, label, mono(), ink);
    response
}

/// Default, hover, pressed and selected, the same for every button.
fn state_colors(t: &Theme, response: &Response, selected: bool) -> (Color32, Color32) {
    if selected || response.is_pointer_button_down_on() {
        (t.accent, t.on_accent)
    } else if response.hovered() {
        (t.hover, t.text)
    } else {
        (t.raised, t.text)
    }
}

/// A square ring, light under dark, so it shows on any color.
fn marker(p: &egui::Painter, t: &Theme, at: Pos2) {
    let r = Rect::from_center_size(at, Vec2::splat(10.0));
    p.rect_stroke(r, CornerRadius::ZERO, Stroke::new(3.0, t.cursor_light), StrokeKind::Middle);
    p.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, t.cursor_dark), StrokeKind::Middle);
}

// ---- color math (sRGB, 0..1) ---------------------------------------------

fn sv_mesh(p: &egui::Painter, rect: Rect, hue: f32) {
    const N: usize = 16;
    let mut mesh = Mesh::default();
    for j in 0..=N {
        for i in 0..=N {
            let (s, v) = (i as f32 / N as f32, 1.0 - j as f32 / N as f32);
            let pos = pos2(rect.left() + s * rect.width(), rect.top() + (1.0 - v) * rect.height());
            mesh.colored_vertex(pos, rgb8(hsv_to_rgb([hue, s, v])));
        }
    }
    grid_triangles(&mut mesh, N, N);
    p.add(Shape::mesh(mesh));
}

fn ring_mesh(p: &egui::Painter, c: Pos2, inner: f32, outer: f32) {
    const N: usize = 120;
    let mut mesh = Mesh::default();
    for i in 0..=N {
        let h = i as f32 / N as f32;
        let a = h * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let dir = vec2(a.cos(), a.sin());
        let color = rgb8(hsv_to_rgb([h, 1.0, 1.0]));
        mesh.colored_vertex(c + dir * outer, color);
        mesh.colored_vertex(c + dir * inner, color);
    }
    for i in 0..N as u32 {
        let a = 2 * i;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 3, a + 2);
    }
    p.add(Shape::mesh(mesh));
}

fn grid_triangles(mesh: &mut Mesh, cols: usize, rows: usize) {
    let w = (cols + 1) as u32;
    for j in 0..rows as u32 {
        for i in 0..cols as u32 {
            let a = j * w + i;
            mesh.add_triangle(a, a + 1, a + w);
            mesh.add_triangle(a + 1, a + w + 1, a + w);
        }
    }
}

fn rgb_to_hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    [h, s, max]
}

fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let h = (h.rem_euclid(1.0)) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

// ---- small helpers -------------------------------------------------------

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub fn rgb8(c: [f32; 3]) -> Color32 {
    Color32::from_rgb(to8(c[0]), to8(c[1]), to8(c[2]))
}

fn color_set(c: [f32; 3]) -> Invocation {
    Invocation::new("color.set", Value::List(c.iter().map(|&v| Value::Num(f64::from(v))).collect()))
}

/// Brush radius for display: tenths below 10 px.
/// Slider position (0 to 1) of an opacity on a log scale from MIN_OPACITY to 1.
/// Zero sits at the bottom, for layers that can be fully transparent.
fn opacity_position(opacity: f32) -> f32 {
    if opacity <= 0.0 { 0.0 } else { (opacity / MIN_OPACITY).ln() / (1.0 / MIN_OPACITY).ln() }
}

/// Inverse of `opacity_position`.
fn opacity_from_position(position: f32) -> f32 {
    if position <= 0.0 { 0.0 } else { MIN_OPACITY * (position * (1.0 / MIN_OPACITY).ln()).exp() }
}

fn px(size: f32) -> String {
    if size < 10.0 { format!("{size:.1}") } else { format!("{size:.0}") }
}

fn tool_name(st: &State) -> String {
    if st.eyedropper {
        "Eyedropper".into()
    } else if st.erasing {
        "Eraser".into()
    } else {
        title_case(st.brushes.name())
    }
}

fn title_case(s: &str) -> String {
    capitalize(s)
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().collect::<String>() + chars.as_str())
}

fn key_or(chrome: &Chrome, command: &str) -> String {
    chrome.key(command).unwrap_or_else(|| "no key".into())
}

/// `"ctrl+shift+z"` to `"Ctrl+Shift+Z"`.
pub fn pretty_chord(chord: &str) -> String {
    if chord == "+" {
        return "+".into();
    }
    let (mods, key) = match chord.rsplit_once('+') {
        Some((mods, "")) => (mods.trim_end_matches('+'), "+"),
        Some((mods, key)) => (mods, key),
        None => ("", chord),
    };
    let mut parts: Vec<String> = mods.split('+').filter(|m| !m.is_empty()).map(capitalize).collect();
    parts.push(match key {
        "escape" => "Esc".into(),
        k if k.len() == 1 => k.to_uppercase(),
        k => capitalize(k),
    });
    parts.join("+")
}

#[allow(dead_code)]
fn _layout_unused(_: Layout, _: Align) {}
