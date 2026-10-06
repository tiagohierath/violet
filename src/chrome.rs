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

/// Panel heights and gaps, all from the blueprint's scale.
const ROW: f32 = 36.0;
const TITLE: f32 = 28.0;
const GAP: f32 = 8.0;
const TOOL: f32 = 44.0;

/// Inner padding of each panel's rect.
pub fn padding(name: &str) -> Vec2 {
    match name {
        "menu" => vec2(4.0, 0.0),
        "status" => vec2(12.0, 0.0),
        "tools" => vec2(4.0, 4.0),
        _ => vec2(12.0, 12.0),
    }
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
            "menu" => self.menu(ui, out),
            "status" => self.status(ui),
            "tools" => self.tools(ui, out),
            "color" => self.color(ui, out),
            "brush" => self.brush(ui, out),
            "layers" => self.layers(ui, out),
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

    // ---- menu bar ----------------------------------------------------

    fn menu(&self, ui: &mut Ui, out: &mut Vec<Invocation>) {
        let item = |ui: &mut Ui, label: &str, command: &str, out: &mut Vec<Invocation>| {
            let mut button = egui::Button::new(label).min_size(vec2(240.0, 28.0));
            if let Some(key) = self.key(command) {
                button = button.shortcut_text(key);
            }
            if ui.add(button).clicked() {
                out.push(Invocation::new(command, Value::Nil));
                ui.close();
            }
        };
        ui.spacing_mut().button_padding = vec2(12.0, 4.0);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        egui::MenuBar::new().ui(ui, |ui| {
            ui.set_min_height(28.0);
            ui.menu_button("File", |ui| {
                item(ui, "New", "file.new", out);
                item(ui, "Open…", "file.open", out);
                item(ui, "Save", "file.save", out);
                item(ui, "Save as…", "file.save-as", out);
                item(ui, "Export PNG", "file.export", out);
                ui.separator();
                item(ui, "Quit", "app.quit", out);
            });
            ui.menu_button("Edit", |ui| {
                item(ui, "Undo", "history.undo", out);
                item(ui, "Redo", "history.redo", out);
            });
            ui.menu_button("View", |ui| {
                item(ui, "Zoom in", "view.zoom-in", out);
                item(ui, "Zoom out", "view.zoom-out", out);
                item(ui, "Zoom to 100%", "view.zoom-100", out);
                item(ui, "Fit to window", "view.fit", out);
                ui.separator();
                item(ui, "Rotate left", "view.rotate-left", out);
                item(ui, "Rotate right", "view.rotate-right", out);
                item(ui, "Reset rotation", "view.rotate-reset", out);
                item(ui, "Mirror", "view.flip", out);
                ui.separator();
                item(ui, "Hide panels", "ui.chrome", out);
                item(ui, "Measurements", "ui.perf", out);
            });
            ui.menu_button("Layer", |ui| {
                item(ui, "New layer", "layer.add", out);
                item(ui, "Delete layer", "layer.delete", out);
                item(ui, "Move up", "layer.raise", out);
                item(ui, "Move down", "layer.lower", out);
            });
            ui.menu_button("Help", |ui| {
                item(ui, "Keyboard shortcuts", "ui.help", out);
            });
        });
    }

    // ---- status line -------------------------------------------------

    fn status(&self, ui: &mut Ui) {
        let st = self.st;
        let t = self.theme;
        let rect = ui.max_rect();
        let painter = ui.painter();
        let y = rect.center().y;
        let mut x = rect.left();
        let mut put = |text: String, font, color| {
            let r = painter.text(pos2(x, y), Align2::LEFT_CENTER, text, font, color);
            x = r.right() + 16.0;
        };
        put(tool_name(st), bold(12.0), t.text);
        put(format!("{} px", px(st.brushes.size(st.erasing))), regular(12.0), t.text);
        put(format!("{:.0}%", st.brushes.opacity() * 100.0), regular(12.0), t.text);
        put(format!("Zoom {:.0}%", st.view.zoom * 100.0), regular(12.0), t.text);
        let degrees = st.view.degrees() as i32;
        if degrees != 0 {
            put(format!("Rotated {degrees}°"), regular(12.0), t.text);
        }
        if st.view.flip {
            put("Mirrored".into(), regular(12.0), t.text);
        }
        if st.eyedropper {
            put("Eyedropper on".into(), bold(12.0), t.accent);
        }
        put(st.status.clone(), regular(12.0), t.text);

        // Right end: the document, and whether it is saved.
        let name = st
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(|| "Untitled".to_owned(), |n| n.to_string_lossy().into_owned());
        let mut right = rect.right();
        if st.modified() {
            let r = painter.text(pos2(right, y), Align2::RIGHT_CENTER, "Unsaved", bold(12.0), t.accent);
            right = r.left() - 12.0;
        }
        // The message may run under the name; the name wins.
        let galley = painter.layout_no_wrap(name, bold(12.0), t.text);
        let r = Align2::RIGHT_CENTER.anchor_size(pos2(right, y), galley.size());
        painter.rect_filled(r.expand2(vec2(12.0, 0.0)).with_max_x(rect.right() + 12.0).with_min_y(rect.top()).with_max_y(rect.bottom()), CornerRadius::ZERO, t.surface);
        if st.modified() {
            painter.text(pos2(rect.right(), y), Align2::RIGHT_CENTER, "Unsaved", bold(12.0), t.accent);
        }
        painter.galley(r.min, galley, t.text);
    }

    // ---- tool bar ----------------------------------------------------

    fn tools(&self, ui: &mut Ui, out: &mut Vec<Invocation>) {
        let st = self.st;
        let rect = ui.max_rect();
        let mut y = rect.top();
        let mut slot = |ui: &mut Ui, icon: Icon, selected: bool, hint: String| {
            let r = Rect::from_min_size(pos2(rect.center().x - TOOL / 2.0, y), Vec2::splat(TOOL));
            y += TOOL + 4.0;
            tool_button(ui, self.theme, r, icon, selected, &hint)
        };

        // Tools: what the pen does on the canvas.
        let picking = st.eyedropper;
        for (i, preset) in st.brushes.presets.iter().enumerate() {
            let selected = i == st.brushes.current && !st.erasing && !picking;
            let icon = match preset.name.as_str() {
                "ink" => Icon::Ink,
                "pencil" => Icon::Pencil,
                "soft" => Icon::Soft,
                _ => Icon::Letter(preset.name.chars().next().unwrap_or('?').to_ascii_uppercase()),
            };
            let hint = format!("{} brush (B for the next brush)", title_case(&preset.name));
            if slot(ui, icon, selected, hint).clicked() {
                out.push(Invocation::new("brush.select", Value::Str(preset.name.clone())));
                if st.erasing {
                    out.push(Invocation::new("tool.eraser", Value::Nil));
                }
                if picking {
                    out.push(Invocation::new("color.eyedropper", Value::Nil));
                }
            }
        }
        if slot(ui, Icon::Eraser, st.erasing && !picking, self.hint("Eraser", "tool.eraser")).clicked() {
            if !st.erasing {
                out.push(Invocation::new("tool.eraser", Value::Nil));
            }
            if picking {
                out.push(Invocation::new("color.eyedropper", Value::Nil));
            }
        }
        let hint = "Eyedropper (or hold Ctrl or Alt and click the canvas)".to_owned();
        if slot(ui, Icon::Eyedropper, picking, hint).clicked() {
            out.push(Invocation::new("color.eyedropper", Value::Nil));
        }
        let hint = "Pan: drag this button (or hold Space, or the middle button, and drag the canvas)";
        let pan = slot(ui, Icon::Pan, false, hint.to_owned());
        let delta = pan.drag_delta();
        if delta != Vec2::ZERO {
            out.push(Invocation::new(
                "view.pan",
                Value::List(vec![Value::Num(f64::from(delta.x)), Value::Num(f64::from(delta.y))]),
            ));
        }
        if pan.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }

        // Actions, at the bottom where the hand finds them without looking.
        let mut y = rect.bottom() - 2.0 * TOOL - 4.0;
        for (icon, name, command) in
            [(Icon::Undo, "Undo", "history.undo"), (Icon::Redo, "Redo", "history.redo")]
        {
            let r = Rect::from_min_size(pos2(rect.center().x - TOOL / 2.0, y), Vec2::splat(TOOL));
            y += TOOL + 4.0;
            if tool_button(ui, self.theme, r, icon, false, &self.hint(name, command)).clicked() {
                out.push(Invocation::new(command, Value::Nil));
            }
        }
    }

    // ---- color -------------------------------------------------------

    fn color(&self, ui: &mut Ui, out: &mut Vec<Invocation>) {
        let t = self.theme;
        let rect = ui.max_rect();
        let rgb = self.st.brushes.color;
        let painter = ui.painter().clone();

        // Title, with the current color and its hex code at the right.
        let title = Rect::from_min_size(rect.min, vec2(rect.width(), TITLE));
        painter.text(title.left_center(), Align2::LEFT_CENTER, "Color", bold(14.0), t.text);
        let hex = format!("#{:02X}{:02X}{:02X}", to8(rgb[0]), to8(rgb[1]), to8(rgb[2]));
        let code = painter.text(title.right_center(), Align2::RIGHT_CENTER, hex, regular(14.0), t.text);
        let chip = Rect::from_center_size(pos2(code.left() - 8.0 - 10.0, title.center().y), Vec2::splat(20.0));
        painter.rect_filled(chip, CornerRadius::ZERO, rgb8(rgb));
        painter.rect_stroke(chip, CornerRadius::ZERO, Stroke::new(1.0, t.border), StrokeKind::Inside);

        // The swatches sit at the bottom; the square takes what is left.
        let swatch_h = 28.0;
        let swatches_h = 2.0 * swatch_h + 4.0;
        let hue_h = 20.0;
        let top = title.bottom() + GAP;
        let square_h = (rect.bottom() - swatches_h - GAP - hue_h - GAP - top).max(32.0);
        let square = Rect::from_min_size(pos2(rect.left(), top), vec2(rect.width(), square_h));
        let hue_rect = Rect::from_min_size(pos2(rect.left(), square.bottom() + GAP), vec2(rect.width(), hue_h));

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

        let sq = ui.allocate_rect(square, Sense::click_and_drag());
        sv_mesh(&painter, square, hsv[0]);
        painter.rect_stroke(square, CornerRadius::ZERO, Stroke::new(1.0, t.border), StrokeKind::Inside);
        let mut changed = false;
        if sq.is_pointer_button_down_on()
            && let Some(p) = sq.interact_pointer_pos()
        {
            hsv[1] = ((p.x - square.left()) / square.width()).clamp(0.0, 1.0);
            hsv[2] = 1.0 - ((p.y - square.top()) / square.height()).clamp(0.0, 1.0);
            changed = true;
        }
        let at = pos2(square.left() + hsv[1] * square.width(), square.top() + (1.0 - hsv[2]) * square.height());
        marker(&painter.with_clip_rect(square.expand(6.0)), t, at);

        let hue = ui.allocate_rect(hue_rect, Sense::click_and_drag());
        hue_mesh(&painter, hue_rect);
        painter.rect_stroke(hue_rect, CornerRadius::ZERO, Stroke::new(1.0, t.border), StrokeKind::Inside);
        if hue.is_pointer_button_down_on()
            && let Some(p) = hue.interact_pointer_pos()
        {
            hsv[0] = ((p.x - hue_rect.left()) / hue_rect.width()).clamp(0.0, 1.0);
            changed = true;
        }
        let hx = hue_rect.left() + hsv[0] * hue_rect.width();
        let tick = Rect::from_center_size(pos2(hx, hue_rect.center().y), vec2(6.0, hue_h + 4.0));
        painter.rect_stroke(tick, CornerRadius::ZERO, Stroke::new(3.0, t.cursor_light), StrokeKind::Middle);
        painter.rect_stroke(tick, CornerRadius::ZERO, Stroke::new(1.0, t.cursor_dark), StrokeKind::Middle);

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

    // ---- brush -------------------------------------------------------

    fn brush(&self, ui: &mut Ui, out: &mut Vec<Invocation>) {
        let st = self.st;
        let t = self.theme;
        let rect = ui.max_rect();
        let painter = ui.painter().clone();
        let title = Rect::from_min_size(rect.min, vec2(rect.width(), TITLE));
        painter.text(title.left_center(), Align2::LEFT_CENTER, "Brush", bold(14.0), t.text);
        painter.text(title.right_center(), Align2::RIGHT_CENTER, tool_name(st), regular(14.0), t.text);

        let mut y = title.bottom() + GAP;
        let mut row = || {
            let r = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), ROW));
            y += ROW + GAP;
            r
        };

        let size = st.brushes.size(st.erasing);
        let span = (MAX_SIZE / MIN_SIZE).ln();
        let at = (size / MIN_SIZE).ln() / span;
        let size_hint = format!("Brush radius ({} smaller, {} bigger)", key_or(self, "brush.size-down"), key_or(self, "brush.size-up"));
        if let (Some(v), _) = field(ui, t, row(), "Size", &format!("{} px", px(size)), at, None, &size_hint) {
            let px = MIN_SIZE * (v * span).exp();
            out.push(Invocation::new("brush.size", Value::Num(f64::from(px))));
        }

        let opacity = st.brushes.opacity();
        let hint = format!("Stroke opacity ({} less, {} more)", key_or(self, "brush.opacity-down"), key_or(self, "brush.opacity-up"));
        if let (Some(v), _) = field(ui, t, row(), "Opacity", &format!("{:.0}%", opacity * 100.0), opacity_position(opacity), None, &hint) {
            out.push(Invocation::new("brush.opacity", Value::Num(f64::from(opacity_from_position(v)))));
        }

        let steps = SMOOTHING.len();
        let step = st.brushes.smoothing.min(steps - 1);
        let name = SMOOTHING_NAMES.get(step).copied().unwrap_or("On");
        let hint = self.hint("Smoothing steadies the line and lags behind the pen", "brush.smoothing");
        let at = step as f32 / (steps - 1) as f32;
        if let (Some(v), _) = field(ui, t, row(), "Smoothing", name, at, Some(steps), &hint) {
            let new = (v * (steps - 1) as f32).round() as usize;
            if new != step {
                out.push(Invocation::new("brush.smoothing", Value::Num(new as f64)));
            }
        }

        // Tip preview: the brush at its real size on screen, clipped to the box.
        let preview = Rect::from_min_max(pos2(rect.left(), y), rect.max);
        if preview.height() >= 24.0 {
            painter.rect_filled(preview, CornerRadius::ZERO, Color32::WHITE);
            painter.rect_stroke(preview, CornerRadius::ZERO, Stroke::new(1.0, t.border), StrokeKind::Inside);
            let r = (size * st.view.zoom / st.ppp.max(0.1)).max(1.0);
            let clip = painter.with_clip_rect(preview.shrink(1.0));
            if st.erasing {
                clip.circle_stroke(preview.center(), r, Stroke::new(1.0, t.cursor_dark));
            } else {
                let [cr, cg, cb] = st.brushes.color.map(to8);
                let paint = Color32::from_rgba_unmultiplied(cr, cg, cb, to8(opacity));
                clip.circle_filled(preview.center(), r, paint);
            }
        }
    }

    // ---- layers ------------------------------------------------------

    fn layers(&self, ui: &mut Ui, out: &mut Vec<Invocation>) {
        let st = self.st;
        let t = self.theme;
        let doc = &st.doc;
        let rect = ui.max_rect();
        let painter = ui.painter().clone();
        let title = Rect::from_min_size(rect.min, vec2(rect.width(), TITLE));
        painter.text(title.left_center(), Align2::LEFT_CENTER, "Layers", bold(14.0), t.text);
        painter.text(title.right_center(), Align2::RIGHT_CENTER, format!("{}", doc.layers.len()), regular(14.0), t.text);

        // Bottom up: the buttons, then the active layer's opacity.
        let buttons = Rect::from_min_max(pos2(rect.left(), rect.bottom() - ROW), rect.max);
        let opacity_row = Rect::from_min_size(pos2(rect.left(), buttons.top() - GAP - ROW), vec2(rect.width(), ROW));
        let list = Rect::from_min_max(pos2(rect.left(), title.bottom() + GAP), pos2(rect.right(), opacity_row.top() - GAP));

        let w = (rect.width() - 3.0 * 4.0) / 4.0;
        for (i, (label, command, hint)) in [
            ("New", "layer.add", "New layer above the active one"),
            ("Delete", "layer.delete", "Delete the active layer"),
            ("Up", "layer.raise", "Move the active layer up"),
            ("Down", "layer.lower", "Move the active layer down"),
        ]
        .into_iter()
        .enumerate()
        {
            let r = Rect::from_min_size(pos2(buttons.left() + i as f32 * (w + 4.0), buttons.top()), vec2(w, ROW));
            if text_button(ui, t, r, label, false, &self.hint(hint, command)).clicked() {
                out.push(Invocation::new(command, Value::Nil));
            }
        }

        if let Some(active) = doc.layer(doc.active) {
            let handle = Value::Handle(Handle { kind: Kind::Layer, id: active.id });
            let label = format!("{:.0}%", active.opacity * 100.0);
            let (v, response) = field(ui, t, opacity_row, "Opacity", &label, opacity_position(active.opacity), None, "Opacity of the active layer");
            if let Some(v) = v {
                out.push(Invocation::new("layer.opacity", Value::List(vec![Value::Num(f64::from(opacity_from_position(v))), handle])));
            }
            if response.drag_stopped() || response.clicked() {
                out.push(Invocation::new("layer.opacity-done", Value::Nil));
            }
        }

        if list.height() < ROW {
            return;
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                for layer in doc.layers.iter().rev() {
                    let handle = Value::Handle(Handle { kind: Kind::Layer, id: layer.id });
                    let (row, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                    let selected = doc.active == layer.id;
                    let (bg, ink) = if selected {
                        (t.accent, t.on_accent)
                    } else if response.hovered() {
                        (t.hover, t.text)
                    } else {
                        (t.surface, t.text)
                    };
                    let p = ui.painter();
                    p.rect_filled(row, CornerRadius::ZERO, bg);
                    p.line_segment([row.left_bottom(), row.right_bottom()], Stroke::new(1.0, t.border));
                    // Visibility box: its own 36px target at the left.
                    let eye = Rect::from_min_size(row.min, Vec2::splat(ROW));
                    let eye_response = ui.interact(eye, Id::new(("layer-visible", layer.id)), Sense::click())
                        .on_hover_text(if layer.visible { "Hide this layer" } else { "Show this layer" });
                    let boxr = Rect::from_center_size(eye.center(), Vec2::splat(16.0));
                    p.rect_stroke(boxr, CornerRadius::ZERO, Stroke::new(1.5, ink), StrokeKind::Inside);
                    if layer.visible {
                        let c = boxr.center();
                        p.add(Shape::line(
                            vec![c + vec2(-4.5, 0.0), c + vec2(-1.5, 3.5), c + vec2(4.5, -3.5)],
                            Stroke::new(2.0, ink),
                        ));
                    }
                    let font = if selected { bold(14.0) } else { regular(14.0) };
                    p.text(pos2(eye.right(), row.center().y), Align2::LEFT_CENTER, &layer.name, font, ink);
                    let note = if layer.visible { format!("{:.0}%", layer.opacity * 100.0) } else { "Hidden".to_owned() };
                    p.text(row.right_center() - vec2(12.0, 0.0), Align2::RIGHT_CENTER, note, regular(12.0), ink);
                    if eye_response.clicked() {
                        let args = Value::List(vec![Value::Bool(!layer.visible), handle.clone()]);
                        out.push(Invocation::new("layer.visible", args));
                    } else if response.clicked() && !selected {
                        out.push(Invocation::new("layer.select", handle));
                    }
                }
            });
        });
    }

    // ---- measurements ------------------------------------------------

    fn perf(&self, ui: &mut Ui) {
        let p = &self.st.perf;
        let f = &p.last;
        ui.label(egui::RichText::new("Measurements").font(bold(14.0)));
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
                ui.label(egui::RichText::new(key).font(bold(14.0)));
                ui.label(capitalize(what));
                ui.end_row();
            }
            for (chord, line) in self.keymap.describe() {
                let command = line.split_whitespace().next().unwrap_or(line);
                let what = self.registry.iter().find(|c| c.name == command).map_or(command, |c| c.help);
                ui.label(egui::RichText::new(pretty_chord(chord)).font(bold(14.0)));
                ui.label(capitalize(what));
                ui.end_row();
            }
        });
    }
}

// ---- widgets -------------------------------------------------------------

/// A value field: label left, value right, the amount as a fill with an
/// accent edge. Press or drag anywhere sets it, so the whole 36px row is
/// the target. Returns the new position from 0 to 1 while pressed.
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
            p.line_segment([pos2(x, rect.bottom() - 6.0), pos2(x, rect.bottom())], Stroke::new(1.0, t.text));
        }
    }
    let marker = Rect::from_min_max(pos2((edge - 1.0).max(rect.left()), rect.top()), pos2((edge + 1.0).max(rect.left() + 2.0), rect.bottom()));
    p.rect_filled(marker, CornerRadius::ZERO, t.accent);
    if live {
        p.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    p.text(rect.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, label, regular(14.0), t.text);
    p.text(rect.right_center() - vec2(12.0, 0.0), Align2::RIGHT_CENTER, value, bold(14.0), t.text);
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
    p.text(rect.center(), Align2::CENTER_CENTER, label, regular(14.0), ink);
    response
}

fn tool_button(ui: &mut Ui, t: &Theme, rect: Rect, icon: Icon, selected: bool, hint: &str) -> Response {
    let response = ui.allocate_rect(rect, Sense::click_and_drag()).on_hover_text(hint);
    let (bg, ink) = if selected {
        (t.accent, t.on_accent)
    } else if response.hovered() || response.is_pointer_button_down_on() {
        (t.hover, t.text)
    } else {
        (t.surface, t.text)
    };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::ZERO, bg);
    draw_icon(p, rect.center(), 10.0, icon, ink);
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

// ---- icons ---------------------------------------------------------------

#[derive(Clone, Copy)]
enum Icon {
    Ink,
    Pencil,
    Soft,
    Eraser,
    Eyedropper,
    Pan,
    Undo,
    Redo,
    Letter(char),
}

/// Line icons from plain geometry, 2px strokes, `s` is half the icon size.
fn draw_icon(p: &egui::Painter, c: Pos2, s: f32, icon: Icon, ink: Color32) {
    let at = |x: f32, y: f32| c + vec2(x * s, y * s);
    let line = Stroke::new(2.0, ink);
    let poly = |pts: Vec<Pos2>| Shape::closed_line(pts, line);
    // Along the diagonal from bottom left to top right.
    let diag = |u: f32, w: f32| {
        let k = std::f32::consts::FRAC_1_SQRT_2;
        c + vec2((u + w) * k * s, (-u + w) * k * s)
    };
    match icon {
        Icon::Ink => {
            p.add(poly(vec![at(0.0, 1.0), at(-0.6, 0.0), at(-0.35, -1.0), at(0.35, -1.0), at(0.6, 0.0)]));
            p.line_segment([at(0.0, 1.0), at(0.0, 0.1)], line);
            p.circle_filled(at(0.0, 0.0), 1.8, ink);
        }
        Icon::Pencil => {
            p.add(poly(vec![diag(-1.3, 0.0), diag(-0.6, 0.35), diag(1.2, 0.35), diag(1.2, -0.35), diag(-0.6, -0.35)]));
            p.line_segment([diag(0.8, 0.35), diag(0.8, -0.35)], line);
            p.line_segment([diag(-0.6, 0.35), diag(-0.6, -0.35)], line);
        }
        Icon::Soft => {
            p.circle_filled(c, 0.45 * s, ink);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                p.circle_filled(c + vec2(a.cos(), a.sin()) * 0.9 * s, 1.5, ink);
            }
        }
        Icon::Eraser => {
            p.add(poly(vec![diag(-1.1, 0.5), diag(1.1, 0.5), diag(1.1, -0.5), diag(-1.1, -0.5)]));
            p.line_segment([diag(-0.2, 0.5), diag(-0.2, -0.5)], line);
            p.line_segment([at(-1.0, 1.1), at(1.0, 1.1)], line);
        }
        Icon::Eyedropper => {
            p.line_segment([diag(-1.2, 0.0), diag(0.3, 0.0)], Stroke::new(2.0, ink));
            p.add(poly(vec![diag(-0.6, 0.22), diag(0.3, 0.22), diag(0.3, -0.22), diag(-0.6, -0.22)]));
            p.add(Shape::convex_polygon(vec![diag(0.5, 0.3), diag(1.2, 0.3), diag(1.2, -0.3), diag(0.5, -0.3)], ink, Stroke::NONE));
            p.line_segment([diag(0.4, 0.55), diag(0.4, -0.55)], line);
        }
        Icon::Pan => {
            p.line_segment([at(-1.0, 0.0), at(1.0, 0.0)], line);
            p.line_segment([at(0.0, -1.0), at(0.0, 1.0)], line);
            for (tip, a, b) in [
                (at(-1.0, 0.0), at(-0.65, -0.35), at(-0.65, 0.35)),
                (at(1.0, 0.0), at(0.65, -0.35), at(0.65, 0.35)),
                (at(0.0, -1.0), at(-0.35, -0.65), at(0.35, -0.65)),
                (at(0.0, 1.0), at(-0.35, 0.65), at(0.35, 0.65)),
            ] {
                p.add(Shape::line(vec![a, tip, b], line));
            }
        }
        Icon::Undo | Icon::Redo => {
            let m = if matches!(icon, Icon::Undo) { 1.0 } else { -1.0 };
            let mut pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = -std::f32::consts::PI * i as f32 / 12.0;
                    at(m * 0.7 * a.cos() * -1.0 * -1.0 + 0.0, 0.25 + 0.7 * a.sin())
                })
                .collect();
            // From the right foot over the top to the left foot (mirrored for redo).
            pts.reverse();
            let end = at(-m * 0.7, 0.25);
            pts.push(at(-m * 0.7, 0.85));
            p.add(Shape::line(pts, line));
            p.add(Shape::line(vec![at(-m * 0.7 - 0.35, 0.5), at(-m * 0.7, 0.85), at(-m * 0.7 + 0.35, 0.5)], line));
            let _ = end;
        }
        Icon::Letter(ch) => {
            p.text(c, Align2::CENTER_CENTER, ch, bold(18.0), ink);
        }
    }
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

fn hue_mesh(p: &egui::Painter, rect: Rect) {
    const N: usize = 36;
    let mut mesh = Mesh::default();
    for j in 0..=1 {
        for i in 0..=N {
            let h = i as f32 / N as f32;
            let pos = pos2(rect.left() + h * rect.width(), if j == 0 { rect.top() } else { rect.bottom() });
            mesh.colored_vertex(pos, rgb8(hsv_to_rgb([h, 1.0, 1.0])));
        }
    }
    grid_triangles(&mut mesh, N, 1);
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
