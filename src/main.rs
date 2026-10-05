mod pen;

use std::collections::{HashMap, HashSet};
use std::sync::{OnceLock, mpsc};

use eframe::egui;

use hokusai_core::surface::TiledSurface;
use hokusai_core::tile::{empty_tile, TilePixels, TILE_SIZE};
use hokusai_core::{Brush, BrushSetting, BrushState};
use pen::{PenSample, PenSource, WaylandPenSource};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

const W: usize = 800;
const H: usize = 600;

/// Sparse 64x64 fix15 document, per ARCHITECTURE §1/§2.
struct Canvas {
    tiles: HashMap<(i32, i32), Box<TilePixels>>,
    dirty: HashSet<(i32, i32)>,
    /// Tiles touched since the UI last uploaded them. Separate from `dirty`,
    /// which is stroke-scoped (§4/hokusai's `begin_atomic`/`end_atomic`
    /// contract) — this one is frame-scoped, so idle frames touch nothing.
    frame_dirty: HashSet<(i32, i32)>,
}

/// Every color the UI uses comes from here (ARCHITECTURE, theme tokens).
struct Theme {
    /// Paper under the document, linear light.
    paper: [f32; 3],
}

const LIGHT: Theme = Theme { paper: [1.0, 1.0, 1.0] };

const LUT_SIZE: usize = 4096;

/// Linear light to sRGB-encoded u8, sampled at `LUT_SIZE` steps.
fn linear_to_srgb8(v: f32) -> u8 {
    static LUT: OnceLock<Vec<u8>> = OnceLock::new();
    let lut = LUT.get_or_init(|| {
        (0..=LUT_SIZE)
            .map(|i| {
                let l = i as f32 / LUT_SIZE as f32;
                let s = if l <= 0.0031308 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
                (s * 255.0 + 0.5) as u8
            })
            .collect()
    });
    lut[(v.clamp(0.0, 1.0) * LUT_SIZE as f32) as usize]
}

/// Premultiplied linear fix15 composited over the paper, encoded for display.
fn fix15_to_display(px: [u16; 4], theme: &Theme) -> [u8; 4] {
    let a = px[3] as f32 / 32768.0;
    let mut out = [0u8; 4];
    for c in 0..3 {
        out[c] = linear_to_srgb8(px[c] as f32 / 32768.0 + theme.paper[c] * (1.0 - a));
    }
    out[3] = 255;
    out
}

fn paper_color(theme: &Theme) -> egui::Color32 {
    let [r, g, b] = theme.paper.map(linear_to_srgb8);
    egui::Color32::from_rgb(r, g, b)
}

/// Where every UI element goes. Elements render into the rect handed to
/// them and never place themselves (ARCHITECTURE, layout from data).
struct Layout {
    toolbar: egui::Rect,
    canvas: egui::Rect,
    status: egui::Rect,
}

fn layout(area: egui::Rect) -> Layout {
    const BAR: f32 = 28.0;
    let toolbar = egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, area.min.y + BAR));
    let status = egui::Rect::from_min_max(egui::pos2(area.min.x, area.max.y - BAR), area.max);
    let middle = egui::Rect::from_min_max(toolbar.left_bottom(), status.right_top());
    let canvas = egui::Rect::from_center_size(middle.center(), egui::vec2(W as f32, H as f32));
    Layout { toolbar, canvas, status }
}

impl Canvas {
    fn new() -> Self {
        Self {
            tiles: HashMap::new(),
            dirty: HashSet::new(),
            frame_dirty: HashSet::new(),
        }
    }

    fn take_frame_dirty(&mut self) -> Vec<(i32, i32)> {
        self.frame_dirty.drain().collect()
    }

    fn to_rgba8(&self, theme: &Theme) -> Vec<u8> {
        let mut out = vec![0u8; W * H * 4];
        for y in 0..H {
            for x in 0..W {
                let tx = (x / TILE_SIZE) as i32;
                let ty = (y / TILE_SIZE) as i32;
                let (ix, iy) = (x % TILE_SIZE, y % TILE_SIZE);
                let px = self
                    .tiles
                    .get(&(tx, ty))
                    .map(|t| t[iy][ix])
                    .unwrap_or([0, 0, 0, 0]);
                out[(y * W + x) * 4..(y * W + x) * 4 + 4]
                    .copy_from_slice(&fix15_to_display(px, theme));
            }
        }
        out
    }

    /// Converts one dirty tile, clipped to the document bounds, for a
    /// partial texture upload. Returns `(x0, y0, width, height, rgba8)`.
    fn tile_rgba8(&self, tx: i32, ty: i32, theme: &Theme) -> Option<(usize, usize, usize, usize, Vec<u8>)> {
        let x0 = tx * TILE_SIZE as i32;
        let y0 = ty * TILE_SIZE as i32;
        if x0 >= W as i32 || y0 >= H as i32 || x0 + TILE_SIZE as i32 <= 0 || y0 + TILE_SIZE as i32 <= 0 {
            return None;
        }
        let cx0 = x0.max(0) as usize;
        let cy0 = y0.max(0) as usize;
        let cx1 = (x0 + TILE_SIZE as i32).min(W as i32) as usize;
        let cy1 = (y0 + TILE_SIZE as i32).min(H as i32) as usize;
        let (w, h) = (cx1 - cx0, cy1 - cy0);
        let tile = self.tiles.get(&(tx, ty));
        let mut out = vec![0u8; w * h * 4];
        for y in cy0..cy1 {
            for x in cx0..cx1 {
                let (ix, iy) = ((x as i32 - x0) as usize, (y as i32 - y0) as usize);
                let px = tile.map(|t| t[iy][ix]).unwrap_or([0, 0, 0, 0]);
                let o = ((y - cy0) * w + (x - cx0)) * 4;
                out[o..o + 4].copy_from_slice(&fix15_to_display(px, theme));
            }
        }
        Some((cx0, cy0, w, h, out))
    }
}

impl TiledSurface for Canvas {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels {
        self.dirty.insert((tx, ty));
        self.frame_dirty.insert((tx, ty));
        self.tiles.entry((tx, ty)).or_insert_with(empty_tile)
    }

    fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

    fn begin_atomic(&mut self) {
        self.dirty.clear();
    }

    fn end_atomic(&mut self) -> Vec<(i32, i32)> {
        self.dirty.drain().collect()
    }
}

fn ink_brush() -> Brush {
    let mut b = Brush::new();
    b.get_mut(BrushSetting::Opaque).base_value = 1.0;
    b.get_mut(BrushSetting::Radius).base_value = 8f32.ln();
    b.get_mut(BrushSetting::Hardness).base_value = 0.8;
    b.get_mut(BrushSetting::DabsPerBasicRadius).base_value = 2.0;
    b.get_mut(BrushSetting::ColorV).base_value = 0.0; // black, regardless of H/S
    b
}

/// Everything the document can be told to do. Nothing outside this function
/// touches the canvas or brush state directly.
enum Command {
    StrokeBegin,
    StrokeTo { x: f32, y: f32, pressure: f32, dtime: f64 },
    StrokeEnd,
    ExportPng(String),
}

struct App {
    canvas: Canvas,
    brush: Brush,
    brush_state: BrushState,
    theme: Theme,
    texture: Option<egui::TextureHandle>,
    /// One stroke at a time, whichever device started it.
    drawing: bool,
    /// Time of the last stroke sample, in seconds.
    last_time: f64,
    /// Last mouse position and time, for interpolating between frames.
    last_mouse: Option<(egui::Pos2, f64)>,
    pen: Option<WaylandPenSource>,
    pen_samples: Vec<PenSample>,
    status: String,
    status_tx: mpsc::Sender<String>,
    status_rx: mpsc::Receiver<String>,
}

impl App {
    fn dispatch(&mut self, cmd: Command) {
        match cmd {
            Command::StrokeBegin => {
                self.brush_state = BrushState::default();
                self.canvas.begin_atomic();
            }
            Command::StrokeTo { x, y, pressure, dtime } => {
                self.brush
                    .stroke_to(&mut self.brush_state, &mut self.canvas, x, y, pressure, 0.0, 0.0, dtime);
            }
            Command::StrokeEnd => {
                self.canvas.end_atomic();
            }
            Command::ExportPng(path) => {
                let pixels = self.canvas.to_rgba8(&self.theme);
                let tx = self.status_tx.clone();
                // Encoding is slow; never block the canvas on it.
                std::thread::spawn(move || {
                    let msg = match image::save_buffer(&path, &pixels, W as u32, H as u32, image::ColorType::Rgba8) {
                        Ok(()) => format!("Exported {path}"),
                        Err(e) => format!("Export failed: {e}"),
                    };
                    let _ = tx.send(msg);
                });
            }
        }
    }

    /// The single path from any input device to a stroke.
    fn stroke_input(&mut self, down: bool, pos: egui::Pos2, pressure: f32, time: f64) {
        if down && !self.drawing {
            self.dispatch(Command::StrokeBegin);
            self.drawing = true;
            self.last_time = time;
        }
        if down {
            let dtime = (time - self.last_time).max(1e-3);
            self.last_time = time;
            self.dispatch(Command::StrokeTo { x: pos.x, y: pos.y, pressure, dtime });
        }
        if !down && self.drawing {
            self.dispatch(Command::StrokeEnd);
            self.drawing = false;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let lay = layout(ui.max_rect());

        ui.scope_builder(egui::UiBuilder::new().max_rect(lay.toolbar), |ui| {
            ui.horizontal(|ui| {
                if ui.button("Export PNG").clicked() {
                    self.dispatch(Command::ExportPng("lilyfield-export.png".into()));
                }
            });
        });

        let rect = lay.canvas;
        let response = ui.allocate_rect(rect, egui::Sense::drag());
        let now = ctx.input(|i| i.time);

        if response.dragged()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let local = pos - rect.min.to_vec2();
            match self.last_mouse {
                Some((prev, prev_t)) => {
                    let steps = (prev.distance(local) / 2.0).ceil().max(1.0) as i32;
                    for i in 1..=steps {
                        let f = i as f32 / steps as f32;
                        let t = prev_t + (now - prev_t) * f as f64;
                        self.stroke_input(true, prev.lerp(local, f), 1.0, t);
                    }
                }
                None => self.stroke_input(true, local, 1.0, now),
            }
            self.last_mouse = Some((local, now));
        }
        if response.drag_stopped() {
            self.stroke_input(false, egui::Pos2::ZERO, 0.0, now);
            self.last_mouse = None;
        }

        if let Some(pen) = self.pen.as_mut() {
            pen.drain(&mut self.pen_samples);
        }
        for s in std::mem::take(&mut self.pen_samples) {
            let local = egui::pos2(s.x, s.y) - rect.min.to_vec2();
            self.stroke_input(s.down, local, s.pressure, s.time_ms as f64 / 1000.0);
        }

        match self.texture.as_mut() {
            None => {
                // Nothing is drawn before the first frame: upload plain paper.
                let image = egui::ColorImage::filled([W, H], paper_color(&self.theme));
                self.texture = Some(ctx.load_texture("canvas", image, Default::default()));
                self.canvas.take_frame_dirty();
            }
            Some(texture) => {
                for (tx, ty) in self.canvas.take_frame_dirty() {
                    if let Some((x0, y0, w, h, pixels)) = self.canvas.tile_rgba8(tx, ty, &self.theme) {
                        let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels);
                        texture.set_partial([x0, y0], image, Default::default());
                    }
                }
            }
        }
        if let Some(texture) = &self.texture {
            ui.painter().image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        if let Ok(msg) = self.status_rx.try_recv() {
            self.status = msg;
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(lay.status), |ui| {
            ui.label(&self.status);
        });
    }

    fn on_exit(&mut self) {
        if let Some(pen) = self.pen.as_mut() {
            pen.shutdown();
        }
    }
}

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "Lilyfield",
        eframe::NativeOptions::default(),
        Box::new(|cc| {
            let pen = match (cc.display_handle(), cc.window_handle()) {
                (Ok(dh), Ok(wh)) => {
                    WaylandPenSource::connect(dh.as_raw(), wh.as_raw(), cc.egui_ctx.clone())
                }
                _ => None,
            };
            let (status_tx, status_rx) = mpsc::channel();
            Ok(Box::new(App {
                canvas: Canvas::new(),
                brush: ink_brush(),
                brush_state: BrushState::default(),
                theme: LIGHT,
                texture: None,
                drawing: false,
                last_time: 0.0,
                last_mouse: None,
                pen,
                pen_samples: Vec::new(),
                status: String::new(),
                status_tx,
                status_rx,
            }))
        }),
    )
}
