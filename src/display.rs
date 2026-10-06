//! The canvas on screen (ARCHITECTURE §3): the document is cut into 512 px
//! chunks, each an egui texture per mip level, created only where there is
//! paint and only at the levels the view asks for. Dirty tiles are
//! re-flattened and uploaded; nothing else is touched, so an idle frame
//! costs nothing and a stroke costs exactly the tiles it dirties.
//! egui's wgpu backend has no mipmaps, hence the CPU levels: without them a
//! zoomed-out drawing shimmers.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use eframe::egui::epaint::Vertex;
use eframe::egui::{
    self, Color32, ColorImage, Mesh, Rect, Shape, TextureHandle, TextureOptions, pos2,
};

use violet::compositor::{self, LayerTile, N, ONE, Px, TILE_PX};
use violet::document::{Document, TileKey};
use violet::perf::Perf;
use violet::stroke::LiveStroke;

use crate::view::Xf;

/// Coarsest mip level; level `l` is `1 << l` document pixels per texel.
pub const MAX_LEVEL: usize = 5;
/// Tiles per chunk side.
const CHUNK: i32 = 8;
const CHUNK_PX: i32 = CHUNK * N as i32;
/// New chunks and zoom levels share this budget, checked between chunks.
const BUILD_BUDGET: Duration = Duration::from_millis(6);

type ChunkKey = (i32, i32);

/// One chunk's textures, by mip level; `None` for levels never asked for.
#[derive(Default)]
struct Chunk {
    tex: [Option<TextureHandle>; MAX_LEVEL + 1],
}

/// Reused per-tile buffers: level 0 is the flattened tile, each next level
/// half the size of the one before.
struct Levels {
    px: [Vec<Px>; MAX_LEVEL + 1],
}

impl Levels {
    fn new() -> Self {
        let mut px: [Vec<Px>; MAX_LEVEL + 1] =
            std::array::from_fn(|level| vec![[0; 4]; (N >> level) * (N >> level)]);
        px[0].resize(TILE_PX, [0; 4]);
        Self { px }
    }

    /// Flattens one tile and derives levels up to `upto`. Returns false for
    /// an empty tile, whose display is plain paper.
    fn compute(
        &mut self,
        doc: &Document,
        stroke: Option<&LiveStroke>,
        key: TileKey,
        upto: usize,
    ) -> bool {
        let layers = doc.layers.iter().filter(|l| l.visible).map(|l| LayerTile {
            tile: l.tiles.get(&key).map(|t| &**t),
            stroke: stroke
                .filter(|s| s.layer == l.id)
                .and_then(|s| s.scratch.get(key).map(|t| (t, s.opacity, s.blend))),
            opacity: (l.opacity.clamp(0.0, 1.0) * ONE as f32) as u32,
        });
        if !compositor::flatten(&mut self.px[0], layers) {
            return false;
        }
        for level in 1..=upto {
            let (finer, coarser) = self.px.split_at_mut(level);
            compositor::downsample(&finer[level - 1], N >> (level - 1), &mut coarser[0]);
        }
        true
    }
}

/// The canvas's GPU side: chunk textures plus the tiles waiting to be
/// re-uploaded.
pub struct Display {
    chunks: HashMap<ChunkKey, Chunk>,
    dirty: HashSet<TileKey>,
    /// Reused each frame to drain `dirty` without allocating.
    dirty_buf: Vec<TileKey>,
    levels: Levels,
    paper: Color32,
}

impl Display {
    pub fn new(doc: &Document) -> Self {
        let rgb = compositor::paper_rgb(doc.paper_linear());
        Self {
            chunks: HashMap::new(),
            dirty: HashSet::new(),
            dirty_buf: Vec::new(),
            levels: Levels::new(),
            paper: Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
        }
    }

    /// Forget every texture: a different document, or a different paper.
    pub fn reset(&mut self, doc: &Document) {
        let rgb = compositor::paper_rgb(doc.paper_linear());
        self.paper = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        self.chunks.clear();
        self.dirty.clear();
        self.dirty.extend(doc.content_keys());
    }

    /// Queue tiles for re-flattening and upload on the next `update`.
    pub fn mark(&mut self, keys: impl IntoIterator<Item = TileKey>) {
        self.dirty.extend(keys);
    }

    pub fn mark_doc(&mut self, doc: &Document) {
        self.dirty.extend(doc.content_keys());
    }

    /// Re-upload dirty tiles into every level their chunk has, then build
    /// chunks still missing `level`, within `BUILD_BUDGET` per frame.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        doc: &Document,
        stroke: Option<&LiveStroke>,
        level: usize,
        perf: &mut Perf,
    ) {
        let paper = self.paper;
        let (across, down) = doc.tiles_across();

        self.dirty_buf.clear();
        self.dirty_buf.extend(self.dirty.drain());
        for &key in &self.dirty_buf {
            if key.0 < 0 || key.1 < 0 || key.0 >= across || key.1 >= down {
                continue;
            }
            let chunk_key = (key.0.div_euclid(CHUNK), key.1.div_euclid(CHUNK));
            let chunk = self.chunks.entry(chunk_key).or_default();
            if chunk.tex.iter().all(Option::is_none) {
                // Build new chunks in the budgeted pass below. All dirty
                // tiles in a new chunk will be included in that one upload.
                continue;
            }
            let coarsest = (0..=MAX_LEVEL).rev().find(|&l| chunk.tex[l].is_some()).unwrap_or(0);
            let painted = self.levels.compute(doc, stroke, key, coarsest);
            perf.frame.tiles_composited += 1;
            for tex_level in 0..=MAX_LEVEL {
                let Some(tex) = chunk.tex[tex_level].as_mut() else { continue };
                upload_tile(
                    tex,
                    &mut self.levels,
                    key,
                    chunk_key,
                    tex_level,
                    painted,
                    doc,
                    paper,
                    perf,
                );
            }
        }

        let start = Instant::now();
        for (&chunk_key, chunk) in &mut self.chunks {
            if chunk.tex[level].is_some() {
                continue;
            }
            if start.elapsed() > BUILD_BUDGET {
                ctx.request_repaint();
                break;
            }
            build(ctx, &mut self.levels, chunk, chunk_key, level, doc, stroke, paper, perf);
        }
    }

    /// Paper, then every chunk at the level closest to `level` it has.
    /// Returns the number of meshes added.
    pub fn paint(&self, painter: &egui::Painter, xf: &Xf, level: usize, doc: &Document) -> u64 {
        let (w, h) = (doc.width as f32, doc.height as f32);
        let corners = [pos2(0.0, 0.0), pos2(w, 0.0), pos2(w, h), pos2(0.0, h)].map(|p| xf.apply(p));
        painter.add(Shape::convex_polygon(corners.to_vec(), self.paper, egui::Stroke::NONE));
        let clip = painter.clip_rect();
        let mut draws = 1;
        for (&(cx, cy), chunk) in &self.chunks {
            let pick = (0..=MAX_LEVEL)
                .filter(|&l| chunk.tex[l].is_some())
                .min_by_key(|&l| l.abs_diff(level));
            let Some((tex_level, tex)) = pick.and_then(|l| chunk.tex[l].as_ref().map(|t| (l, t)))
            else {
                continue;
            };
            let x0 = (cx * CHUNK_PX) as f32;
            let y0 = (cy * CHUNK_PX) as f32;
            let x1 = (x0 + CHUNK_PX as f32).min(w);
            let y1 = (y0 + CHUNK_PX as f32).min(h);
            let quad =
                [pos2(x0, y0), pos2(x1, y0), pos2(x1, y1), pos2(x0, y1)].map(|p| xf.apply(p));
            if !clip.intersects(Rect::from_points(&quad)) {
                continue;
            }
            let [tw, th] = tex.size();
            let scale = (1u32 << tex_level) as f32;
            let u = ((x1 - x0) / scale / tw.max(1) as f32).min(1.0);
            let v = ((y1 - y0) / scale / th.max(1) as f32).min(1.0);
            let uv = [pos2(0.0, 0.0), pos2(u, 0.0), pos2(u, v), pos2(0.0, v)];
            let mut mesh = Mesh::with_texture(tex.id());
            for i in 0..4 {
                mesh.vertices.push(Vertex { pos: quad[i], uv: uv[i], color: Color32::WHITE });
            }
            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            painter.add(Shape::mesh(mesh));
            draws += 1;
        }
        draws
    }
}

/// Texture size of a chunk at a level, clipped to the document.
fn chunk_size(doc: &Document, (cx, cy): ChunkKey, level: usize) -> [usize; 2] {
    let w = (doc.width as i64 - (cx * CHUNK_PX) as i64).clamp(1, CHUNK_PX as i64) as usize;
    let h = (doc.height as i64 - (cy * CHUNK_PX) as i64).clamp(1, CHUNK_PX as i64) as usize;
    [w.div_ceil(1 << level), h.div_ceil(1 << level)]
}

/// Creates a chunk's texture at `level` from scratch.
#[allow(clippy::too_many_arguments)]
fn build(
    ctx: &egui::Context,
    levels: &mut Levels,
    chunk: &mut Chunk,
    chunk_key: ChunkKey,
    level: usize,
    doc: &Document,
    stroke: Option<&LiveStroke>,
    paper: Color32,
    perf: &mut Perf,
) {
    let [tw, th] = chunk_size(doc, chunk_key, level);
    let mut image = ColorImage::filled([tw, th], paper);
    // Tile side at this level.
    let side = N >> level;
    let paper_lin = doc.paper_linear();
    for ty in 0..CHUNK {
        for tx in 0..CHUNK {
            let key = (chunk_key.0 * CHUNK + tx, chunk_key.1 * CHUNK + ty);
            if !levels.compute(doc, stroke, key, level) {
                continue;
            }
            perf.frame.tiles_composited += 1;
            let (ox, oy) = (tx as usize * side, ty as usize * side);
            let src = &levels.px[level];
            compositor::to_display(src, paper_lin, |i, rgb| {
                let (x, y) = (ox + i % side, oy + i / side);
                if x < tw && y < th {
                    image.pixels[y * tw + x] = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                }
            });
        }
    }
    perf.frame.uploads += 1;
    perf.frame.upload_bytes += (tw * th * 4) as u64;
    let name = format!("chunk {} {} level {level}", chunk_key.0, chunk_key.1);
    chunk.tex[level] = Some(ctx.load_texture(name, image, TextureOptions::LINEAR));
}

/// Writes one freshly computed tile into an existing chunk texture.
#[allow(clippy::too_many_arguments)]
fn upload_tile(
    tex: &mut TextureHandle,
    levels: &mut Levels,
    key: TileKey,
    chunk_key: ChunkKey,
    level: usize,
    painted: bool,
    doc: &Document,
    paper: Color32,
    perf: &mut Perf,
) {
    // Tile side at this level.
    let side = N >> level;
    let [tw, th] = tex.size();
    let x0 = (key.0 - chunk_key.0 * CHUNK) as usize * side;
    let y0 = (key.1 - chunk_key.1 * CHUNK) as usize * side;
    if x0 >= tw || y0 >= th {
        return;
    }
    let (w, h) = ((tw - x0).min(side), (th - y0).min(side));
    // egui takes ownership of upload pixels, so this one Vec per tile is
    // the floor; it is moved into the image, never copied.
    let mut colors = vec![paper; w * h];
    if painted {
        compositor::to_display(&levels.px[level], doc.paper_linear(), |i, rgb| {
            let (x, y) = (i % side, i / side);
            if x < w && y < h {
                colors[y * w + x] = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
            }
        });
    }
    perf.frame.uploads += 1;
    perf.frame.upload_bytes += (w * h * 4) as u64;
    tex.set_partial([x0, y0], ColorImage::new([w, h], colors), TextureOptions::LINEAR);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_chunk_uploads_its_dirty_tiles_once_and_idle_frames_upload_nothing() {
        let ctx = egui::Context::default();
        let mut doc = Document::new(128, 64);
        for key in [(0, 0), (1, 0)] {
            let mut tile = compositor::new_tile();
            tile[0][0] = [0, 0, 0, ONE as u16];
            doc.layers[0].tiles.insert(key, tile.into());
        }
        let mut display = Display::new(&doc);
        display.reset(&doc);
        let mut perf = Perf::new();
        ctx.run_ui(Default::default(), |_| {
            display.update(&ctx, &doc, None, 0, &mut perf);
        }).textures_delta.clear();
        assert_eq!(perf.frame.tiles_composited, 2);
        assert_eq!(perf.frame.uploads, 1);
        assert!(display.chunks[&(0, 0)].tex[0].is_some());

        perf.frame = Default::default();
        ctx.run_ui(Default::default(), |_| {
            display.update(&ctx, &doc, None, 0, &mut perf);
        }).textures_delta.clear();
        assert_eq!(perf.frame.tiles_composited, 0);
        assert_eq!(perf.frame.uploads, 0);

        display.mark([(0, 0)]);
        ctx.run_ui(Default::default(), |_| {
            display.update(&ctx, &doc, None, 0, &mut perf);
        }).textures_delta.clear();
        assert_eq!(perf.frame.tiles_composited, 1);
        assert_eq!(perf.frame.uploads, 1);
    }
}
