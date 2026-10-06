//! The app: state, the frame loop and the panels. Nothing in here changes
//! the document. Input and panels only ask for commands, and `commands` acts.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, CornerRadius, FontId, Key, Pos2, Rect, Stroke,
    StrokeKind, TextEdit, UiBuilder, pos2, vec2,
};

use violet::brushes::Brushes;
use violet::document::{DEFAULT_HEIGHT, DEFAULT_WIDTH, Document};
use violet::history::History;
use violet::perf::{Perf, Span};
use violet::stroke::LiveStroke;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::commands::{Invocation, Registry, Value};
use crate::display::{Display, MAX_LEVEL};
use crate::input::{Frame, Hold, Input, Sample, Source};
use crate::io::{self, Done, Io, Job, Paths};
use crate::keymap::Keymap;
use crate::layout::Layout;
use crate::pen::{PenSample, PenSource, WaylandPenSource};
use crate::theme::Theme;
use crate::view::View;

const AUTOSAVE: Duration = Duration::from_secs(30);
const HUD_FOR: Duration = Duration::from_millis(1200);

pub enum DialogKind {
    Open,
    SaveAs,
    /// Asks the size, and is the confirmation before the drawing is replaced.
    New(crate::settings::NewDrawing),
    /// Confirms quitting; holds the note saying where the drawing is saved.
    Quit(String),
}

pub struct Dialog {
    pub kind: DialogKind,
    pub text: String,
    pub focus_path: bool,
}

pub enum Transition {
    New(u32, u32),
    Open(PathBuf),
    Quit,
}

pub struct State {
    pub ctx: egui::Context,
    pub started: Instant,
    pub startup_ms: Option<f32>,
    pub doc: Document,
    pub path: Option<PathBuf>,
    pub paths: Paths,
    pub recent: Vec<PathBuf>,
    pub io: Io,
    pub history: History,
    pub display: Display,
    pub brushes: Brushes,
    pub stroke: Option<LiveStroke>,
    pub view: View,
    pub ppp: f32,
    pub canvas: Rect,
    pub pointer: Option<Pos2>,
    pub erasing: bool,
    pub eyedropper: bool,
    pub loading: bool,
    pub chrome: bool,
    pub perf_open: bool,
    pub perf: Perf,
    pub dialog: Option<Dialog>,
    pub status: String,
    pub hud: Option<(String, Instant)>,
    pub rev: u64,
    pub saved_rev: u64,
    pub fit_pending: bool,
    pub autosave_at: Instant,
    pub pending: Option<Transition>,
    pub close_ready: bool,
    pub help_open: bool,
    pub terminal_open: bool,
    pub terminal_input: String,
    pub terminal_log: Vec<String>,
}

impl State {
    pub fn changed(&mut self) {
        self.rev += 1;
    }

    pub fn modified(&self) -> bool {
        self.rev != self.saved_rev
    }

    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = text.into();
    }

    pub fn hud(&mut self) {
        let eraser = if self.erasing { "  eraser" } else { "" };
        let text = format!(
            "{}  {:.0} px  {:.0}%{eraser}",
            self.brushes.name(),
            self.brushes.size(self.erasing),
            self.brushes.opacity() * 100.0
        );
        self.hud = Some((text, Instant::now() + HUD_FOR));
    }

    pub fn open_dialog(&mut self, kind: DialogKind) {
        let text = match kind {
            DialogKind::Open => format!("{}/", self.paths.drawings.display()),
            DialogKind::SaveAs => self
                .path
                .clone()
                .unwrap_or_else(|| io::untitled(&self.paths))
                .display()
                .to_string(),
            DialogKind::New(_) | DialogKind::Quit(_) => String::new(),
        };
        self.dialog = Some(Dialog { kind, text, focus_path: true });
    }

    /// Where the drawing on screen ends up if it is replaced or closed.
    pub fn save_note(&self) -> String {
        let target = self.path.clone().unwrap_or_else(|| io::untitled(&self.paths));
        crate::settings::save_note(self.path.as_deref(), self.modified(), &target)
    }

    pub fn replace_document(&mut self, doc: Document, path: Option<PathBuf>) {
        self.doc = doc;
        self.path = path;
        self.history.clear();
        self.stroke = None;
        self.display.reset(&self.doc);
        self.rev += 1;
        self.saved_rev = self.rev;
        self.fit_pending = true;
    }

    pub fn save_to(&mut self, path: PathBuf) {
        self.set_status(format!("Saving {}…", path.display()));
        self.io.run(Job::Save { doc: self.doc.clone(), path, rev: self.rev, snapshot: false, origin: None });
    }

    /// Keep the document until its save has succeeded. A failed save
    /// cancels the transition and leaves the drawing editable.
    pub fn transition(&mut self, next: Transition) -> Result<(), String> {
        if self.loading { return Err("A file operation is still in progress".into()); }
        if self.modified() {
            self.pending = Some(next);
            self.loading = true;
            self.save_to(self.path.clone().unwrap_or_else(|| io::untitled(&self.paths)));
        } else {
            self.perform(next);
        }
        Ok(())
    }

    fn perform(&mut self, next: Transition) {
        match next {
            Transition::New(w, h) => {
                let kept = self.path.as_deref().map(crate::settings::saved_in);
                self.replace_document(Document::new(w, h), None);
                self.set_status(match kept {
                    Some(kept) => format!("New drawing. {kept}."),
                    None => "New drawing".into(),
                });
            }
            Transition::Open(path) => {
                self.loading = true;
                self.set_status(format!("Opening {}…", path.display()));
                self.io.run(Job::Load { path, recovered: None });
            }
            Transition::Quit => {
                self.close_ready = true;
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    pub fn clear_snapshot(&self) {
        let path = self.paths.snapshot();
        let _ = std::fs::remove_file(path.with_extension("path"));
        let _ = std::fs::remove_file(&path);
    }
}

pub struct App {
    st: State,
    input: Input,
    keymap: Keymap,
    registry: Registry,
    layout: Layout,
    theme: Theme,
    pen: Option<WaylandPenSource>,
    pen_samples: Vec<PenSample>,
    mouse_events: Vec<egui::Event>,
    mouse_down: bool,
    middle_down: bool,
    pen_down: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, started: Instant) -> Self {
        let ctx = cc.egui_ctx.clone();
        let mut app = Self::without_window(ctx.clone(), Paths::xdg(), started);
        app.pen = match (cc.display_handle(), cc.window_handle()) {
            (Ok(d), Ok(w)) => WaylandPenSource::connect(d.as_raw(), w.as_raw(), ctx),
            _ => None,
        };
        if let Some(path) = std::env::args_os().nth(1) {
            app.st.perform(Transition::Open(path.into()));
        } else if let Some((path, origin)) = io::orphaned_snapshot(&app.st.paths) {
            app.st.loading = true;
            app.st.set_status("Recovering an unsaved drawing");
            app.st.io.run(Job::Load { path, recovered: Some(origin) });
        }
        app
    }

    fn without_window(ctx: egui::Context, paths: Paths, started: Instant) -> Self {
        let theme = Theme::LIGHT;
        theme.apply(&ctx);
        let recent = io::recent(&paths);
        let doc = Document::new(DEFAULT_WIDTH, DEFAULT_HEIGHT);
        let st = State {
            ctx: ctx.clone(),
            started,
            startup_ms: None,
            display: Display::new(&doc),
            doc,
            path: None,
            paths,
            recent,
            io: Io::new(ctx),
            history: History::new(),
            brushes: Brushes::builtin(),
            stroke: None,
            view: View::default(),
            ppp: 1.0,
            canvas: Rect::NOTHING,
            pointer: None,
            erasing: false,
            eyedropper: false,
            loading: false,
            chrome: true,
            perf_open: false,
            perf: Perf::new(),
            dialog: None,
            status: String::new(),
            hud: None,
            rev: 0,
            saved_rev: 0,
            fit_pending: true,
            autosave_at: Instant::now() + AUTOSAVE,
            pending: None,
            close_ready: false,
            help_open: false,
            terminal_open: true,
            terminal_input: String::new(),
            terminal_log: Vec::new(),
        };
        Self {
            st,
            input: Input::new(),
            keymap: Keymap::load(),
            registry: Registry::builtin(),
            layout: Layout::default(),
            theme,
            pen: None,
            pen_samples: Vec::new(),
            mouse_events: Vec::new(),
            mouse_down: false,
            middle_down: false,
            pen_down: false,
        }
    }

    fn dispatch(&mut self, inv: Invocation) {
        if let Err(e) = self.registry.dispatch(&mut self.st, &inv) {
            self.st.set_status(e);
        }
    }

    fn finish_io(&mut self, done: Done) {
        match done {
            Done::Loaded { path, doc, recovered, ms } => {
                self.st.loading = false;
                self.st.perf.record(Span::Load, ms);
                match recovered {
                    Some(origin) => {
                        self.st.replace_document(*doc, origin);
                        self.st.rev += 1;
                        // Keep the source recovery file until the user has
                        // another durable copy; recovery itself can crash.
                        self.st.set_status("Recovered an unsaved drawing");
                    }
                    None => {
                        // The drawing being replaced, saved by the transition.
                        let kept = self.st.path.as_deref().filter(|p| *p != path).map(crate::settings::saved_in);
                        self.st.replace_document(*doc, Some(path.clone()));
                        io::remember(&self.st.paths, &mut self.st.recent, &path);
                        let opened = format!("Opened {}", path.display());
                        self.st.set_status(match kept {
                            Some(kept) => format!("{opened}. {kept}."),
                            None => opened,
                        });
                    }
                }
            }
            Done::Saved { path, rev, snapshot: false, ms } => {
                self.st.perf.record(Span::Save, ms);
                if rev >= self.st.saved_rev {
                    self.st.path = Some(path.clone());
                    self.st.saved_rev = rev;
                }
                io::remember(&self.st.paths, &mut self.st.recent, &path);
                self.st.set_status(format!("Saved. {}.", crate::settings::saved_in(&path)));
                if rev == self.st.rev && let Some(next) = self.st.pending.take() {
                    self.st.loading = false;
                    self.st.perform(next);
                }
            }
            Done::Saved { ms, .. } => self.st.perf.record(Span::Save, ms),
            Done::Exported { path, ms } => {
                self.st.perf.record(Span::Export, ms);
                self.st.set_status(format!("Exported {}", path.display()));
            }
            Done::Failed { what, path, err, snapshot } => {
                if snapshot {
                    self.st.set_status(format!("Autosave failed: {err}"));
                } else {
                    let quitting = matches!(self.st.pending, Some(Transition::Quit));
                    if matches!(what, "open" | "save") {
                        self.st.loading = false;
                        self.st.pending = None;
                    }
                    let stay = if quitting && what == "save" { " Violet did not quit." } else { "" };
                    self.st.set_status(format!("Could not {what} {}: {err}{stay}", path.display()));
                }
            }
        }
    }

    fn chrome(&self) -> crate::chrome::Chrome<'_> {
        crate::chrome::Chrome {
            st: &self.st,
            keymap: &self.keymap,
            registry: &self.registry,
            theme: &self.theme,
        }
    }

    fn dialog(&mut self, ctx: &egui::Context, out: &mut Vec<Invocation>) {
        let Some(d) = self.st.dialog.as_mut() else { return };
        let (title, verb, action) = match &mut d.kind {
            DialogKind::Open => ("Open", "file.open", "Open"),
            DialogKind::SaveAs => ("Save as", "file.save", "Save"),
            DialogKind::New(drawing) => {
                crate::settings::new_drawing(ctx, drawing, &self.theme, out);
                return;
            }
            DialogKind::Quit(note) => {
                crate::settings::quit(ctx, note, &self.theme, out);
                return;
            }
        };
        let mut submit = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("file-dialog"))
            .backdrop_color(egui::Color32::TRANSPARENT)
            .frame(egui::Frame::new().fill(self.theme.surface)
                .stroke(Stroke::new(1.0, self.theme.border)).inner_margin(16))
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(title).font(crate::theme::bold(18.0)));
                let edit = ui.add(
                    TextEdit::singleline(&mut d.text)
                        .desired_width(480.0)
                        .hint_text("path to a .lily file"),
                );
                if std::mem::take(&mut d.focus_path) {
                    edit.request_focus();
                }
                if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
                ui.horizontal(|ui| {
                    submit |= ui.add_enabled(!d.text.trim().is_empty(), egui::Button::new(action)).clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if submit && !d.text.trim().is_empty() {
            out.push(Invocation::new(verb, Value::Str(d.text.clone())));
            out.push(Invocation::new("ui.dismiss", Value::Nil));
        } else if cancel {
            out.push(Invocation::new("ui.dismiss", Value::Nil));
        }
    }
}

impl App {
    fn ui_frame(&mut self, ui: &mut egui::Ui) {
        self.st.perf.begin_frame();
        let ctx = ui.ctx().clone();
        let painter = ui.painter().clone();
        let screen = ui.max_rect();
        self.st.ppp = ctx.pixels_per_point();
        if self.st.startup_ms.is_none() {
            self.st.startup_ms = Some(self.st.started.elapsed().as_secs_f32() * 1000.0);
        }

        let layout = self.layout.resolve(screen, |name| match name {
            "perf" => self.st.perf_open,
            "terminal" => self.st.terminal_open,
            _ => self.st.chrome,
        });
        self.st.canvas = layout.canvas;
        let canvas = self.st.canvas;
        let ppp = self.st.ppp;
        if self.st.fit_pending && canvas.width() > 1.0 && canvas.height() > 1.0 {
            self.st.view.fit(canvas, ppp, self.st.doc.width, self.st.doc.height);
            self.st.fit_pending = false;
        }
        let mut panels: Vec<Rect> = layout.panels.iter().map(|(_, r)| *r).collect();
        if self.st.dialog.is_some() || self.st.help_open || self.st.loading || ctx.any_popup_open() {
            panels.push(canvas);
        }

        for done in self.st.io.poll() {
            self.finish_io(done);
        }

        let mut out: Vec<Invocation> = Vec::new();
        let typing = ctx.egui_wants_keyboard_input();
        let keymap = &self.keymap;
        let (scroll, pointer, space, modifiers, now) = ctx.input(|i| {
            for event in &i.events {
                if let egui::Event::Key { key, pressed: true, modifiers, .. } = event
                    && ((!typing && !self.st.loading && self.st.dialog.is_none() && !self.st.help_open) || *key == Key::Escape)
                    && let Some(inv) = keymap.lookup(*key, *modifiers)
                {
                    out.push(inv.clone());
                }
            }
            (
                i.smooth_scroll_delta.y,
                i.pointer.latest_pos(),
                i.key_down(Key::Space),
                i.modifiers,
                i.time,
            )
        });
        self.st.pointer = pointer;
        let over_panel = |p: Pos2| panels.iter().any(|r| r.contains(p));
        if scroll != 0.0
            && !self.input.busy()
            && let Some(p) = pointer
            && canvas.contains(p)
            && !over_panel(p)
        {
            let factor = f64::from((scroll * 0.002).exp());
            let args = Value::List(vec![Value::Num(factor), Value::Num(f64::from(p.x)), Value::Num(f64::from(p.y))]);
            out.push(Invocation::new("view.zoom-by", args));
        }

        for inv in out.drain(..) { self.dispatch(inv); }

        let started = Instant::now();
        let hold = if space {
            Hold::Pan
        } else if self.st.eyedropper || modifiers.ctrl || modifiers.alt {
            Hold::Pick
        } else {
            Hold::None
        };
        let to_doc_xf = self.st.view.xf(canvas, ppp).inverse();
        let to_doc = move |p: Pos2| to_doc_xf.apply(p);
        {
            let mut f = Frame { canvas, panels: &panels, hold, to_doc: &to_doc, out: &mut out };
            for event in self.mouse_events.drain(..) {
                let pos = match event {
                    egui::Event::PointerMoved(pos) => pos,
                    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, .. } => {
                        self.mouse_down = pressed;
                        pos
                    }
                    egui::Event::PointerButton { pos, button: egui::PointerButton::Middle, pressed, .. } => {
                        self.middle_down = pressed;
                        if pressed { self.input.press_pan(pos, &f); } else { self.input.release_pan(); }
                        pos
                    }
                    egui::Event::WindowFocused(false) | egui::Event::PointerGone => {
                        self.input.finish(&mut f);
                        self.mouse_down = false;
                        self.middle_down = false;
                        continue;
                    }
                    _ => continue,
                };
                self.input.sample(Sample { pos, pressure: 1.0, tilt: (0.0, 0.0), time: now,
                    down: self.mouse_down || self.middle_down, eraser: false }, Source::Mouse, &mut f);
            }
            for s in self.pen_samples.drain(..) {
                let s = Sample {
                    pos: pos2(s.x, s.y),
                    pressure: s.pressure,
                    tilt: ((s.tilt_x / 60.0).clamp(-1.0, 1.0), (s.tilt_y / 60.0).clamp(-1.0, 1.0)),
                    time: f64::from(s.time_ms) / 1000.0,
                    down: s.down,
                    eraser: false,
                };
                self.input.sample(s, Source::Pen, &mut f);
            }
            self.input.flush(&mut f);
        }
        self.st.perf.record_since(Span::Input, started);

        for inv in out.drain(..) {
            self.dispatch(inv);
        }

        let level = self.st.view.level(MAX_LEVEL);
        let t = Instant::now();
        self.st.display.update(&ctx, &self.st.doc, self.st.stroke.as_ref(), level, &mut self.st.perf);
        self.st.perf.record_since(Span::Composite, t);

        if self.st.modified() && !self.st.loading {
            let at = Instant::now();
            if at >= self.st.autosave_at && !self.st.io.snapshotting {
                self.st.autosave_at = at + AUTOSAVE;
                let path = self.st.paths.snapshot();
                let origin = self.st.path.clone();
                let doc = self.st.doc.clone();
                let rev = self.st.rev;
                self.st.io.run(Job::Save { doc, path, rev, snapshot: true, origin });
            }
            ctx.request_repaint_after(self.st.autosave_at.saturating_duration_since(at));
        }

        painter.rect_filled(screen, CornerRadius::ZERO, self.theme.workspace);
        let xf = self.st.view.xf(canvas, ppp);
        let canvas_painter = painter.with_clip_rect(canvas);
        let t = Instant::now();
        let draws = self.st.display.paint(&canvas_painter, &xf, level, &self.st.doc);
        self.st.perf.frame.draw_calls += draws;
        self.st.perf.record_since(Span::Paint, t);
        if let Some(p) = self.st.pointer.filter(|p| canvas.contains(*p) && !over_panel(*p)) {
            ctx.set_cursor_icon(egui::CursorIcon::None);
            let r = (self.st.brushes.size(self.st.erasing) * xf.scale()).max(2.0);
            canvas_painter.circle_stroke(p, r, Stroke::new(3.0, self.theme.cursor_light));
            canvas_painter.circle_stroke(p, r, Stroke::new(1.0, self.theme.cursor_dark));
        }

        if let Some((text, until)) = self.st.hud.clone() {
            let at = Instant::now();
            if at < until {
                canvas_painter.text(
                    canvas.left_top() + vec2(16.0, 16.0),
                    Align2::LEFT_TOP,
                    text,
                    FontId::proportional(18.0),
                    self.theme.accent,
                );
                ctx.request_repaint_after(until - at);
            } else {
                self.st.hud = None;
            }
        }

        for (name, rect) in &layout.panels {
            let rect = *rect;
            painter.rect_filled(rect, CornerRadius::ZERO, self.theme.surface);
            painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, self.theme.border), StrokeKind::Inside);
            let padding = crate::chrome::padding(name);
            ui.scope_builder(UiBuilder::new().id_salt(name).max_rect(rect.shrink2(padding)), |ui| {
                ui.set_clip_rect(rect);
                if self.st.loading {
                    ui.disable();
                }
                self.chrome().panel(ui, name, &mut out);
            });
        }

        if self.st.help_open {
            let mut help_open = self.st.help_open;
            egui::Window::new("Violet · Keyboard shortcuts").open(&mut help_open)
                .default_width(460.0).vscroll(true).show(&ctx, |ui| {
                    self.chrome().help(ui);
                });
            self.st.help_open = help_open;
        }

        self.dialog(&ctx, &mut out);
        for inv in out.drain(..) {
            self.dispatch(inv);
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.st.close_ready {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.st.pending.is_none() && !self.st.loading {
                self.dispatch(Invocation::new("app.quit", Value::Nil));
            }
        }
        let name = self.st.path.as_ref().and_then(|p| p.file_name())
            .map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{name}{} — Violet", if self.st.modified() { " *" } else { "" })));
        self.st.perf.end_frame();
    }
}

impl App {
    /// Scales this frame's pen samples to egui points and mirrors them as
    /// pointer events, so panels react to the pen like a mouse. The canvas
    /// still consumes the original pressure samples exactly once, through
    /// Source::Pen.
    fn pen_to_egui(&mut self, raw: &mut egui::RawInput, scale: f32) {
        for s in &mut self.pen_samples {
            s.x *= scale;
            s.y *= scale;
            let pos = pos2(s.x, s.y);
            raw.events.push(egui::Event::PointerMoved(pos));
            if self.pen_down != s.down {
                raw.events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary,
                    pressed: s.down, modifiers: egui::Modifiers::default() });
                self.pen_down = s.down;
            }
        }
    }
}

impl eframe::App for App {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.mouse_events = raw.events.clone();
        self.pen_samples.clear();
        if let Some(pen) = self.pen.as_mut() { pen.drain(&mut self.pen_samples); }
        let native = raw.viewports.get(&raw.viewport_id).and_then(|v| v.native_pixels_per_point)
            .unwrap_or(ctx.pixels_per_point());
        self.pen_to_egui(raw, native / ctx.pixels_per_point());
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.st.perf.eframe_cpu = frame.info().cpu_usage.map(|s| s * 1000.0);
        if self.keymap.reload_if_changed() {
            ui.ctx().request_repaint();
        }
        // Polls the keys file a few times a second; a stat is cheap.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
        self.ui_frame(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(pen) = self.pen.as_mut() {
            pen.shutdown();
        }
        self.dispatch(Invocation::new("stroke.end", Value::Nil));
        self.st.io.finish();
        if self.st.modified() {
            let path = self.st.path.clone().unwrap_or_else(|| io::untitled(&self.st.paths));
            if let Err(e) = Io::save_now(&self.st.doc, &path) {
                eprintln!("Violet: could not save {}: {e}", path.display());
                let snapshot = self.st.paths.snapshot();
                if let Err(e) = Io::save_now(&self.st.doc, &snapshot) {
                    eprintln!("Violet: recovery save failed: {e}");
                }
                return;
            }
        }
        self.st.clear_snapshot();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn app() -> (App, PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!("violet-app-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&root).unwrap();
        let paths = Paths { drawings: root.join("drawings"), state: root.join("state"), recovery: root.join("recovery") };
        (App::without_window(egui::Context::default(), paths, Instant::now()), root)
    }

    fn finish(app: &mut App) {
        app.st.io.finish();
        for done in app.st.io.poll() { app.finish_io(done); }
    }

    /// One frame. There is no renderer, so the texture uploads are dropped
    /// here; egui panics if they are dropped unhandled.
    fn run(ctx: &egui::Context, raw: egui::RawInput, app: &mut App) {
        ctx.run_ui(raw, |ui| app.ui_frame(ui)).textures_delta.clear();
    }

    #[test]
    fn new_document_waits_for_a_successful_save() {
        let (mut app, root) = app();
        app.dispatch(Invocation::new("layer.add", Value::Nil));
        app.dispatch(Invocation::new("file.new", Value::List(vec![Value::Num(320.0), Value::Num(240.0)])));
        assert!(app.st.loading);
        assert_eq!(app.st.doc.layers.len(), 2);
        finish(&mut app);
        assert!(!app.st.loading);
        assert_eq!((app.st.doc.width, app.st.doc.height), (320, 240));
        let saved = violet::file::load(&app.st.paths.drawings.join("drawing-001.lily")).unwrap();
        assert_eq!(saved.layers.len(), 2);
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_save_keeps_the_current_drawing_and_cancels_quit() {
        let (mut app, root) = app();
        let blocker = root.join("not-a-directory");
        std::fs::write(&blocker, "keep").unwrap();
        app.st.path = Some(blocker.join("drawing.lily"));
        app.dispatch(Invocation::new("layer.add", Value::Nil));
        app.dispatch(Invocation::new("app.quit", Value::Bool(true)));
        finish(&mut app);
        assert!(!app.st.close_ready);
        assert!(!app.st.loading);
        assert!(app.st.modified());
        assert_eq!(app.st.doc.layers.len(), 2);
        assert!(app.st.status.contains("Could not save"));
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn queued_saves_keep_the_newest_document() {
        let (mut app, root) = app();
        let path = root.join("drawing.lily");
        app.st.save_to(path.clone());
        app.dispatch(Invocation::new("layer.add", Value::Nil));
        app.st.save_to(path.clone());
        finish(&mut app);
        assert_eq!(violet::file::load(&path).unwrap().layers.len(), 2);
        assert!(!app.st.modified());
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_and_quit_show_dialogs_without_replacing_or_closing_the_drawing() {
        let (mut app, root) = app();
        app.dispatch(Invocation::new("layer.add", Value::Nil));
        let size = (app.st.doc.width, app.st.doc.height);

        app.dispatch(Invocation::new("file.new", Value::Nil));
        assert!(matches!(app.st.dialog.as_ref().map(|d| &d.kind), Some(DialogKind::New(_))));
        run(&app.st.ctx.clone(), screen(Vec::new()), &mut app);
        assert_eq!((app.st.doc.width, app.st.doc.height), size);
        assert_eq!(app.st.doc.layers.len(), 2);
        assert!(!app.st.loading);
        app.dispatch(Invocation::new("ui.dismiss", Value::Nil));

        app.dispatch(Invocation::new("app.quit", Value::Nil));
        assert!(matches!(app.st.dialog.as_ref().map(|d| &d.kind), Some(DialogKind::Quit(_))));
        run(&app.st.ctx.clone(), screen(Vec::new()), &mut app);
        assert!(!app.st.close_ready);
        assert!(!app.st.loading);
        assert!(app.st.modified());
        app.dispatch(Invocation::new("ui.dismiss", Value::Nil));
        assert!(app.st.dialog.is_none());
        assert_eq!(app.st.doc.layers.len(), 2);
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn confirming_quit_saves_before_closing() {
        let (mut app, root) = app();
        app.dispatch(Invocation::new("layer.add", Value::Nil));
        app.dispatch(Invocation::new("app.quit", Value::Nil));
        assert!(!app.st.close_ready);
        app.dispatch(Invocation::new("ui.dismiss", Value::Nil));
        app.dispatch(Invocation::new("app.quit", Value::Bool(true)));
        assert!(app.st.loading);
        assert!(!app.st.close_ready);
        finish(&mut app);
        assert!(app.st.close_ready);
        assert!(!app.st.modified());
        let saved = violet::file::load(&app.st.paths.drawings.join("drawing-001.lily")).unwrap();
        assert_eq!(saved.layers.len(), 2);
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mouse_click_in_one_frame_paints_and_undo_restores_blank_canvas() {
        let (mut app, root) = app();
        let ctx = app.st.ctx.clone();
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 840.0))), ..Default::default() };
        run(&ctx, raw.clone(), &mut app);
        let pos = app.st.canvas.center();
        let events = vec![egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }];
        app.mouse_events = events.clone();
        run(&ctx, egui::RawInput { events, ..raw }, &mut app);
        assert!(!app.st.doc.is_blank(), "a press and release between frames must paint a dot");
        assert_eq!(app.st.history.depth(), (1, 0));
        app.dispatch(Invocation::new("history.undo", Value::Nil));
        assert!(app.st.doc.is_blank());
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn screen(events: Vec<egui::Event>) -> egui::RawInput {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 840.0));
        egui::RawInput { screen_rect: Some(rect), events, ..Default::default() }
    }

    /// One mouse frame: the canvas and egui see the same events.
    fn mouse(app: &mut App, pos: Pos2, pressed: Option<bool>) {
        let mut events = vec![egui::Event::PointerMoved(pos)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        }
        app.mouse_events = events.clone();
        app.pen_samples.clear();
        run(&app.st.ctx.clone(), screen(events), app);
    }

    /// One pen frame, fed the way raw_input_hook feeds a real tablet.
    fn pen(app: &mut App, pos: Pos2, down: bool, time_ms: u32) {
        let mut raw = screen(Vec::new());
        app.mouse_events.clear();
        app.pen_samples = vec![PenSample { x: pos.x, y: pos.y, pressure: 0.8, tilt_x: 0.0, tilt_y: 0.0, down, time_ms }];
        app.pen_to_egui(&mut raw, 1.0);
        run(&app.st.ctx.clone(), raw, app);
    }

    /// The color square, found the way a user finds it: the spot in the color
    /// panel where a press alone changes the color. Swatches need a full
    /// click, and the hue slider leaves black black.
    fn color_square(app: &mut App) -> Pos2 {
        let (perf, chrome) = (app.st.perf_open, app.st.chrome);
        let rect = screen(Vec::new()).screen_rect.unwrap();
        let resolved = app.layout.resolve(rect, |name| if name == "perf" { perf } else { chrome });
        let panel = resolved.panels.iter().find(|(name, _)| *name == "color").map(|(_, r)| *r).unwrap();
        for y in (0..panel.height() as usize).step_by(6) {
            for x in (0..panel.width() as usize).step_by(6) {
                let pos = panel.min + vec2(x as f32, y as f32);
                app.st.brushes.color = [0.0; 3];
                mouse(app, pos, Some(true));
                let hit = app.st.brushes.color != [0.0; 3];
                mouse(app, pos, Some(false));
                app.st.brushes.color = [0.0; 3];
                if hit {
                    return pos + vec2(12.0, 12.0);
                }
            }
        }
        panic!("no color square in the color panel {panel:?}");
    }

    /// Tiago, 2026-10-05: dragging the pen in the color square, then painting, painted nothing.
    #[test]
    fn pen_paints_after_picking_a_color_in_the_square() {
        let (mut app, root) = app();
        run(&app.st.ctx.clone(), screen(Vec::new()), &mut app);
        let square = color_square(&mut app);
        pen(&mut app, square, true, 10);
        pen(&mut app, square + vec2(10.0, 10.0), true, 20);
        pen(&mut app, square + vec2(10.0, 10.0), false, 30);
        assert_ne!(app.st.brushes.color, [0.0; 3], "the pen drag must pick a color");
        let c = app.st.canvas.center();
        pen(&mut app, c, true, 40);
        pen(&mut app, c + vec2(30.0, 0.0), true, 50);
        pen(&mut app, c + vec2(30.0, 0.0), false, 60);
        assert!(!app.st.doc.is_blank(), "the pen must paint after picking a color; status: {}", app.st.status);
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mouse_picks_a_color_in_the_square_then_paints() {
        let (mut app, root) = app();
        run(&app.st.ctx.clone(), screen(Vec::new()), &mut app);
        let square = color_square(&mut app);
        mouse(&mut app, square, Some(true));
        mouse(&mut app, square + vec2(10.0, 10.0), None);
        mouse(&mut app, square + vec2(10.0, 10.0), Some(false));
        assert_ne!(app.st.brushes.color, [0.0; 3], "the mouse drag must pick a color");
        let c = app.st.canvas.center();
        mouse(&mut app, c, Some(true));
        mouse(&mut app, c + vec2(30.0, 0.0), None);
        mouse(&mut app, c + vec2(30.0, 0.0), Some(false));
        assert!(!app.st.doc.is_blank(), "the mouse must paint after picking a color; status: {}", app.st.status);
        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }
}
