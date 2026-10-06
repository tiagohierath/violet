//! The `.lily` file (ARCHITECTURE §8; own zip format decided 2026-10-05): a
//! zip holding every layer's tiles exactly as painted, a JSON manifest and a
//! PNG thumbnail. Raw fix15 tiles make a save and load round trip lossless
//! and cost no color conversion either way. Every write lands in a temp file
//! that replaces the target only once it is complete and on disk, so a crash
//! mid-save can never eat the previous save.

use std::collections::HashMap;
use std::fmt::Display;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use hokusai_core::tile::{TILE_SIZE, TilePixels};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::compositor::{self, LayerTile, ONE, Px, TILE_PX};
use crate::document::{Document, Layer, MAX_SIDE, Tile, TileKey};

pub const EXTENSION: &str = "lily";

/// First entry, stored uncompressed, so a file is recognized without
/// parsing anything else.
const MIMETYPE: &str = "application/x-violet";
/// Manifest format version this build writes and reads.
const FORMAT: u32 = 1;
/// The pixel encoding, spelled out in the manifest so a reader that does
/// not match refuses instead of misreading.
const PIXELS: &str = "rgba16 fix15 premultiplied linear-light little-endian";
const THUMB_SIDE: usize = 256;
/// One tile on disk: tx and ty as i32, then 64 * 64 * 4 u16.
const RECORD: usize = 8 + TILE_PX * 8;
/// Far past any real manifest; stops a hostile file from inflating one.
const MAX_MANIFEST: u64 = 16 << 20;
/// Entries this big get zip64 headers. Below it they are left out, so
/// ordinary files stay readable by tools without zip64.
const LARGE_ENTRY: u64 = 0xF000_0000;

/// `manifest.json`: everything but the pixels.
#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    width: u32,
    height: u32,
    paper: String,
    tile_size: u32,
    pixels: String,
    /// Index into `layers`, which is bottom first.
    active: usize,
    layers: Vec<LayerEntry>,
}

#[derive(Serialize, Deserialize)]
struct LayerEntry {
    name: String,
    opacity: f32,
    visible: bool,
    composite_op: String,
    /// Zip entry holding this layer's tile records.
    tiles: String,
    /// Number of records in that entry.
    count: usize,
}

/// Read on its own first, so a newer file is refused by its version even
/// when the rest of its manifest no longer fits this one.
#[derive(Deserialize)]
struct Version {
    format: u32,
}

/// `fast` is for autosave: the quickest deflate level, a bigger file.
pub fn save(doc: &Document, path: &Path, fast: bool) -> Result<(), String> {
    write_atomic(path, |w| write_lily(doc, w, fast))
        .map_err(|e| format!("Could not save {}: {e}", path.display()))
}

/// Reads a `.lily` file. Every way a file can be damaged is an error,
/// never a panic.
pub fn load(path: &Path) -> Result<Document, String> {
    File::open(path)
        .map_err(msg)
        .and_then(|f| read_lily(BufReader::new(f)))
        .map_err(|e| format!("Could not open {}: {e}", path.display()))
}

/// The visible layers flattened over the paper, as 8-bit sRGB.
pub fn export_png(doc: &Document, path: &Path) -> Result<(), String> {
    write_atomic(path, |w| write_png(doc, w))
        .map_err(|e| format!("Could not export {}: {e}", path.display()))
}

/// Error to message, for `map_err`.
fn msg(e: impl Display) -> String {
    e.to_string()
}

fn damaged(why: impl Display) -> String {
    format!("the file is damaged ({why})")
}

/// Runs `write` into a temp file beside `path`, then renames it over
/// `path` once it is complete and synced. On failure the temp file is
/// removed and `path` is untouched.
fn write_atomic(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<File>) -> Result<(), String>,
) -> Result<(), String> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let name = path.file_name().ok_or("the path names no file")?;
    // Unique per call, so an autosave and a save racing on one path never
    // share a temp file.
    let tmp = dir.join(format!(
        ".{}.{}-{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let file = OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(msg)?;
        let mut writer = BufWriter::with_capacity(1 << 20, file);
        write(&mut writer)?;
        let file = writer.into_inner().map_err(|e| msg(e.error()))?;
        file.sync_all().map_err(msg)?;
        drop(file);
        fs::rename(&tmp, path).map_err(msg)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written?;
    // The rename itself only survives a power cut once the directory is on
    // disk too.
    File::open(dir).and_then(|d| d.sync_all()).map_err(msg)
}

fn write_lily<W: Write + Seek>(doc: &Document, w: W, fast: bool) -> Result<(), String> {
    let mut zip = ZipWriter::new(w);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(if fast { 1 } else { 6 }));

    zip.start_file("mimetype", stored).map_err(msg)?;
    zip.write_all(MIMETYPE.as_bytes()).map_err(msg)?;

    let active = doc
        .layers
        .iter()
        .position(|l| l.id == doc.active)
        .unwrap_or(doc.layers.len().saturating_sub(1));
    let [r, g, b] = doc.paper;
    let manifest = Manifest {
        format: FORMAT,
        width: doc.width,
        height: doc.height,
        paper: format!("#{r:02x}{g:02x}{b:02x}"),
        tile_size: TILE_SIZE as u32,
        pixels: PIXELS.to_owned(),
        active,
        layers: doc
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| LayerEntry {
                name: l.name.clone(),
                opacity: l.opacity,
                visible: l.visible,
                composite_op: l.composite_op.clone(),
                tiles: format!("layers/{i}.tiles"),
                count: l.tiles.len(),
            })
            .collect(),
    };
    zip.start_file("manifest.json", deflated).map_err(msg)?;
    serde_json::to_writer(&mut zip, &manifest).map_err(msg)?;

    let mut record = vec![0u8; RECORD];
    for (layer, entry) in doc.layers.iter().zip(&manifest.layers) {
        // Row-major order: the same document always writes the same bytes.
        let mut tiles: Vec<_> = layer.tiles.iter().collect();
        tiles.sort_unstable_by_key(|((x, y), _)| (*y, *x));
        let large = (tiles.len() * RECORD) as u64 >= LARGE_ENTRY;
        zip.start_file(entry.tiles.as_str(), deflated.large_file(large)).map_err(msg)?;
        for (&key, tile) in tiles {
            encode_record(key, tile, &mut record);
            zip.write_all(&record).map_err(msg)?;
        }
    }

    let (tw, th, rgb) = thumbnail(doc);
    zip.start_file("thumbnail.png", stored).map_err(msg)?;
    let mut png = png_encoder(&mut zip, tw as u32, th as u32, png::Compression::Fast)
        .write_header()
        .map_err(msg)?;
    png.write_image_data(&rgb).map_err(msg)?;
    png.finish().map_err(msg)?;
    zip.finish().map_err(msg)?;
    Ok(())
}

/// One tile as a `RECORD`-sized byte record.
fn encode_record((tx, ty): TileKey, tile: &TilePixels, out: &mut [u8]) {
    out[..4].copy_from_slice(&tx.to_le_bytes());
    out[4..8].copy_from_slice(&ty.to_le_bytes());
    let channels = tile.as_flattened().as_flattened();
    for (dst, channel) in out[8..].chunks_exact_mut(2).zip(channels) {
        dst.copy_from_slice(&channel.to_le_bytes());
    }
}

/// Fills `tile` from one record's pixel bytes and returns its largest
/// channel, which must not pass 1.0.
fn decode_pixels(bytes: &[u8], tile: &mut TilePixels) -> u16 {
    let mut max = 0;
    let channels = tile.as_flattened_mut().as_flattened_mut();
    for (channel, pair) in channels.iter_mut().zip(bytes.chunks_exact(2)) {
        *channel = u16::from_le_bytes([pair[0], pair[1]]);
        max = max.max(*channel);
    }
    max
}

fn read_lily<R: Read + Seek>(r: R) -> Result<Document, String> {
    let not_lily = || "it is not a Violet file, or it is damaged".to_owned();
    let mut zip = ZipArchive::new(r).map_err(|_| not_lily())?;
    let mut kind = Vec::new();
    zip.by_name("mimetype")
        .map_err(|_| not_lily())?
        .take(64)
        .read_to_end(&mut kind)
        .map_err(|_| not_lily())?;
    if kind != MIMETYPE.as_bytes() {
        return Err(not_lily());
    }

    let mut text = Vec::new();
    zip.by_name("manifest.json")
        .map_err(|_| damaged("it has no manifest"))?
        .take(MAX_MANIFEST)
        .read_to_end(&mut text)
        .map_err(|_| damaged("its manifest is unreadable"))?;
    let version: Version =
        serde_json::from_slice(&text).map_err(|e| damaged(format!("manifest: {e}")))?;
    if version.format > FORMAT {
        return Err("it was made by a newer Violet".to_owned());
    }
    let manifest: Manifest =
        serde_json::from_slice(&text).map_err(|e| damaged(format!("manifest: {e}")))?;
    if manifest.format != FORMAT {
        return Err(damaged(format!("unknown format {}", manifest.format)));
    }
    let sides = 1..=MAX_SIDE;
    if !sides.contains(&manifest.width) || !sides.contains(&manifest.height) {
        return Err(damaged(format!(
            "a {} by {} canvas is not possible",
            manifest.width, manifest.height
        )));
    }
    if manifest.tile_size != TILE_SIZE as u32 || manifest.pixels != PIXELS {
        return Err(format!(
            "it stores {} pixels in {}px tiles, which this Violet cannot read",
            manifest.pixels, manifest.tile_size
        ));
    }
    let paper = parse_color(&manifest.paper)
        .ok_or_else(|| damaged(format!("paper color {:?}", manifest.paper)))?;

    let mut doc = Document::empty(manifest.width, manifest.height);
    doc.paper = paper;
    let mut record = vec![0u8; RECORD];
    for (i, entry) in manifest.layers.iter().enumerate() {
        let tiles = read_layer_tiles(&mut zip, entry, i + 1, doc.tiles_across(), &mut record)?;
        let id = doc.push_layer(&entry.name);
        if let Some(layer) = doc.layer_mut(id) {
            layer.opacity = entry.opacity.clamp(0.0, 1.0);
            layer.visible = entry.visible;
            layer.composite_op.clone_from(&entry.composite_op);
            layer.tiles = tiles;
        }
    }
    if doc.layers.is_empty() {
        doc.push_layer("Layer 1");
    }
    doc.active = doc.layers.get(manifest.active).or(doc.layers.last()).map_or(0, |l| l.id);
    Ok(doc)
}

/// Reads and validates one layer's tile records. `number` is the layer's
/// 1-based position, for messages; `record` is a reused `RECORD` buffer.
fn read_layer_tiles<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    layer: &LayerEntry,
    number: usize,
    (cols, rows): (i32, i32),
    record: &mut [u8],
) -> Result<HashMap<TileKey, Tile>, String> {
    let most = cols as usize * rows as usize;
    if layer.count > most {
        return Err(damaged(format!("layer {number} has more tiles than the canvas")));
    }
    let mut tiles = HashMap::with_capacity(layer.count);
    let mut entry =
        zip.by_name(&layer.tiles).map_err(|_| damaged(format!("layer {number} is missing")))?;
    for _ in 0..layer.count {
        entry.read_exact(record).map_err(|_| damaged(format!("layer {number} is cut short")))?;
        let tx = i32::from_le_bytes([record[0], record[1], record[2], record[3]]);
        let ty = i32::from_le_bytes([record[4], record[5], record[6], record[7]]);
        if tx < 0 || ty < 0 || tx >= cols || ty >= rows {
            return Err(damaged(format!("layer {number} has a tile outside the canvas")));
        }
        let mut tile = compositor::new_shared_tile();
        let pixels = Arc::get_mut(&mut tile).ok_or("a new tile was shared")?;
        if decode_pixels(&record[8..], pixels) > ONE as u16 {
            return Err(damaged(format!("layer {number} has pixels out of range")));
        }
        if tiles.insert((tx, ty), tile).is_some() {
            return Err(damaged(format!("layer {number} lists a tile twice")));
        }
    }
    // Reading to the end is what makes zip check the CRC.
    match entry.read(&mut [0u8; 1]) {
        Ok(0) => {}
        Ok(_) => return Err(damaged(format!("layer {number} holds more tiles than it says"))),
        Err(_) => return Err(damaged(format!("layer {number} fails its checksum"))),
    }
    Ok(tiles)
}

/// `#rrggbb` to sRGB bytes.
fn parse_color(s: &str) -> Option<[u8; 3]> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

fn layer_opacity(l: &Layer) -> u32 {
    (l.opacity.clamp(0.0, 1.0) * ONE as f32) as u32
}

/// Flattens the visible layers at `key` into `out` (one tile). False when
/// none of them has anything there.
fn flatten_at(doc: &Document, key: TileKey, out: &mut [Px]) -> bool {
    compositor::flatten(
        out,
        doc.layers.iter().filter(|l| l.visible).map(|l| LayerTile {
            tile: l.tiles.get(&key).map(|t| &**t),
            stroke: None,
            opacity: layer_opacity(l),
        }),
    )
}

/// The flattened page over its paper, box-filtered in linear light down to
/// at most `THUMB_SIDE` on its longest side. Returns width, height, RGB8.
fn thumbnail(doc: &Document) -> (usize, usize, Vec<u8>) {
    let (w, h) = (doc.width as usize, doc.height as usize);
    // Each thumbnail pixel covers a `factor` by `factor` cell of the page.
    let factor = w.max(h).div_ceil(THUMB_SIDE).max(1);
    let (tw, th) = (w.div_ceil(factor), h.div_ceil(factor));
    // Sums of premultiplied pixels: averaging them and then compositing
    // over the paper equals compositing first. At most 118 * 118 * 32768
    // per sum at MAX_SIDE, inside u32.
    let mut sums = vec![[0u32; 4]; tw * th];
    let mut tile = vec![[0u16; 4]; TILE_PX];
    // Thumbnail column of each pixel column in the current tile.
    let mut thumb_col = [0usize; TILE_SIZE];
    for key in doc.content_keys() {
        if key.0 < 0 || key.1 < 0 || !flatten_at(doc, key, &mut tile) {
            continue;
        }
        let (vw, vh) = doc.valid_in_tile(key);
        let (x0, y0) = (key.0 as usize * TILE_SIZE, key.1 as usize * TILE_SIZE);
        for (x, col) in thumb_col.iter_mut().enumerate().take(vw) {
            *col = (x0 + x) / factor;
        }
        for y in 0..vh {
            let row_start = (y0 + y) / factor * tw;
            let tile_row = &tile[y * TILE_SIZE..y * TILE_SIZE + vw];
            for (px, col) in tile_row.iter().zip(&thumb_col) {
                let sum = &mut sums[row_start + col];
                for k in 0..4 {
                    sum[k] += px[k] as u32;
                }
            }
        }
    }
    let paper = doc.paper_linear();
    let mut rgb = vec![0u8; tw * th * 3];
    let mut line = vec![[0u16; 4]; tw];
    for row in 0..th {
        // Cells on the last row and column can be cut short by the page edge.
        let cell_h = ((row + 1) * factor).min(h) - row * factor;
        for (col, out) in line.iter_mut().enumerate() {
            let cell_w = ((col + 1) * factor).min(w) - col * factor;
            let count = (cell_w * cell_h) as u32;
            *out = sums[row * tw + col].map(|s| ((s + count / 2) / count) as u16);
        }
        let dst = &mut rgb[row * tw * 3..(row + 1) * tw * 3];
        compositor::to_display(&line, paper, |i, px| dst[i * 3..i * 3 + 3].copy_from_slice(&px));
    }
    (tw, th, rgb)
}

fn png_encoder<W: Write>(
    w: W,
    width: u32,
    height: u32,
    compression: png::Compression,
) -> png::Encoder<'static, W> {
    let mut encoder = png::Encoder::new(w, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(compression);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    encoder
}

/// Streams the page one 64-row band of tiles at a time, so an export never
/// holds more than one band of pixels.
fn write_png<W: Write>(doc: &Document, w: W) -> Result<(), String> {
    let (width, height) = (doc.width as usize, doc.height as usize);
    let mut png = png_encoder(w, doc.width, doc.height, png::Compression::Balanced)
        .write_header()
        .map_err(msg)?;
    let mut stream = png.stream_writer_with_size(1 << 16).map_err(msg)?;
    let paper = doc.paper_linear();
    let blank = compositor::paper_rgb(paper);
    let mut band = vec![0u8; width * TILE_SIZE * 3];
    let mut tile = vec![[0u16; 4]; TILE_PX];
    let (cols, rows) = doc.tiles_across();
    for ty in 0..rows {
        let band_rows = (height - ty as usize * TILE_SIZE).min(TILE_SIZE);
        for tx in 0..cols {
            let x0 = tx as usize * TILE_SIZE;
            let tile_cols = (width - x0).min(TILE_SIZE);
            let painted = flatten_at(doc, (tx, ty), &mut tile);
            for y in 0..band_rows {
                let start = (y * width + x0) * 3;
                let out = &mut band[start..start + tile_cols * 3];
                if painted {
                    let src = &tile[y * TILE_SIZE..y * TILE_SIZE + tile_cols];
                    compositor::to_display(src, paper, |i, px| {
                        out[i * 3..i * 3 + 3].copy_from_slice(&px)
                    });
                } else {
                    for px in out.chunks_exact_mut(3) {
                        px.copy_from_slice(&blank);
                    }
                }
            }
        }
        stream.write_all(&band[..band_rows * width * 3]).map_err(msg)?;
    }
    stream.finish().map_err(msg)?;
    png.finish().map_err(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use serde_json::json;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("violet-file-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Valid premultiplied pixels, every channel varying across the tile.
    fn varied_tile(seed: u32) -> Arc<TilePixels> {
        let mut tile = compositor::new_shared_tile();
        // xorshift32
        let mut state = seed.wrapping_mul(2_654_435_761) | 1;
        for px in Arc::get_mut(&mut tile).unwrap().as_flattened_mut() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let alpha = state % (ONE + 1);
            let channel = |k: u32| ((state >> k) % (alpha + 1)) as u16;
            *px = [channel(3), channel(9), channel(15), alpha as u16];
        }
        tile
    }

    fn sample_doc() -> Document {
        let mut doc = Document::new(300, 200);
        doc.paper = [250, 240, 3];
        let bottom = doc.active;
        let top = doc.push_layer("Ink, \"wet\" ✒");
        let l = doc.layer_mut(bottom).unwrap();
        l.opacity = 0.35;
        l.visible = false;
        l.composite_op = "svg:multiply".into();
        l.tiles.insert((0, 0), varied_tile(1));
        l.tiles.insert((4, 3), varied_tile(2));
        let l = doc.layer_mut(top).unwrap();
        l.tiles.insert((1, 0), varied_tile(3));
        l.tiles.insert((2, 2), varied_tile(4));
        l.tiles.insert((0, 3), varied_tile(5));
        // Not the top layer, so the index really has to round-trip.
        doc.active = bottom;
        doc
    }

    fn assert_same(a: &Document, b: &Document) {
        assert_eq!((a.width, a.height, a.paper), (b.width, b.height, b.paper));
        assert_eq!(a.layers.len(), b.layers.len());
        for (x, y) in a.layers.iter().zip(&b.layers) {
            assert_eq!(x.name, y.name);
            assert_eq!(x.opacity, y.opacity);
            assert_eq!(x.visible, y.visible);
            assert_eq!(x.composite_op, y.composite_op);
            assert_eq!(x.tiles.len(), y.tiles.len());
            for (k, t) in &x.tiles {
                assert!(y.tiles.get(k).is_some_and(|u| **u == **t), "tile {k:?} differs");
            }
        }
        let index = |d: &Document| d.layers.iter().position(|l| l.id == d.active);
        assert_eq!(index(a), index(b));
    }

    #[test]
    fn round_trip_is_exact() {
        let dir = temp_dir("round-trip");
        let path = dir.join("page.lily");
        let doc = sample_doc();
        for fast in [true, false] {
            save(&doc, &path, fast).unwrap();
            assert_same(&doc, &load(&path).unwrap());
        }
        let names: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["page.lily"], "a temp file was left behind");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_save_leaves_nothing_behind() {
        let dir = temp_dir("failed-save");
        let path = dir.join("missing-dir").join("page.lily");
        assert!(save(&sample_doc(), &path, true).is_err());
        assert!(export_png(&sample_doc(), &path).is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn damaged_files_are_errors() {
        let dir = temp_dir("damaged");
        let good = dir.join("good.lily");
        save(&sample_doc(), &good, true).unwrap();
        let bytes = fs::read(&good).unwrap();
        let bad = dir.join("bad.lily");
        let load_bytes = |b: &[u8]| {
            fs::write(&bad, b).unwrap();
            load(&bad)
        };

        for len in [0, 1, 30, bytes.len() / 3, bytes.len() / 2, bytes.len() - 1] {
            assert!(load_bytes(&bytes[..len]).is_err(), "truncated to {len} bytes");
        }
        assert!(load_bytes(b"not a zip at all").is_err());
        assert!(load(&dir.join("nowhere.lily")).is_err());

        // A flipped byte anywhere must never panic.
        for i in (0..bytes.len()).step_by(37) {
            let mut b = bytes.clone();
            b[i] ^= 0x5a;
            let _ = load_bytes(&b);
        }
        // Inside a layer's compressed tiles it is always an error.
        let mut zip = ZipArchive::new(File::open(&good).unwrap()).unwrap();
        let entry = zip.by_name("layers/1.tiles").unwrap();
        let start = entry.data_start().unwrap() as usize;
        let len = entry.compressed_size() as usize;
        drop(entry);
        for at in [start, start + len / 2, start + len - 1] {
            let mut b = bytes.clone();
            b[at] ^= 0x01;
            assert!(load_bytes(&b).is_err(), "byte {at} flipped");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    fn record(tx: i32, ty: i32, value: u16) -> Vec<u8> {
        let mut tile = compositor::new_tile();
        for px in tile.as_flattened_mut() {
            *px = [value; 4];
        }
        let mut r = vec![0u8; RECORD];
        encode_record((tx, ty), &tile, &mut r);
        r
    }

    fn manifest(count: usize) -> serde_json::Value {
        json!({
            "format": 1, "width": 200, "height": 100, "paper": "#ffffff",
            "tile_size": 64, "pixels": PIXELS, "active": 0,
            "layers": [{"name": "a", "opacity": 1.0, "visible": true,
                        "composite_op": "svg:src-over", "tiles": "layers/0.tiles", "count": count}]
        })
    }

    fn craft(
        path: &Path,
        mimetype: &str,
        manifest: &serde_json::Value,
        tiles: &[u8],
    ) -> Result<Document, String> {
        let mut zip = ZipWriter::new(File::create(path).unwrap());
        let options = SimpleFileOptions::default();
        let stored = options.compression_method(CompressionMethod::Stored);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(mimetype.as_bytes()).unwrap();
        zip.start_file("manifest.json", options).unwrap();
        zip.write_all(manifest.to_string().as_bytes()).unwrap();
        zip.start_file("layers/0.tiles", options).unwrap();
        zip.write_all(tiles).unwrap();
        zip.finish().unwrap();
        load(path)
    }

    #[test]
    fn manifests_are_validated() {
        let dir = temp_dir("manifests");
        let path = dir.join("crafted.lily");
        let one = record(3, 1, 100);
        assert!(craft(&path, MIMETYPE, &manifest(1), &one).is_ok());
        assert!(craft(&path, "application/zip", &manifest(1), &one).is_err());

        let mut m = manifest(1);
        m["format"] = json!(2);
        m["layers"] = json!("whatever a newer Violet writes");
        let e = craft(&path, MIMETYPE, &m, &one).err().unwrap();
        assert!(e.contains("made by a newer Violet"), "{e}");

        for (field, value) in [
            ("format", json!(0)),
            ("width", json!(0)),
            ("height", json!(MAX_SIDE + 1)),
            ("width", json!(-5)),
            ("tile_size", json!(128)),
            ("pixels", json!("rgba8")),
            ("paper", json!("#12345")),
            ("paper", json!("white")),
        ] {
            let mut m = manifest(1);
            m[field] = value.clone();
            assert!(craft(&path, MIMETYPE, &m, &one).is_err(), "{field} = {value}");
        }

        let outside = [record(4, 0, 100), record(0, 2, 100), record(-1, 0, 100)];
        for r in &outside {
            assert!(craft(&path, MIMETYPE, &manifest(1), r).is_err());
        }
        assert!(craft(&path, MIMETYPE, &manifest(1), &record(0, 0, ONE as u16 + 1)).is_err());
        let two = [one.clone(), record(0, 0, 7)].concat();
        assert!(craft(&path, MIMETYPE, &manifest(2), &one).is_err(), "count too high");
        assert!(craft(&path, MIMETYPE, &manifest(1), &two).is_err(), "count too low");
        assert!(craft(&path, MIMETYPE, &manifest(1), &one[..RECORD - 1]).is_err());
        assert!(
            craft(&path, MIMETYPE, &manifest(2), &[one.clone(), one.clone()].concat()).is_err()
        );
        assert!(craft(&path, MIMETYPE, &manifest(1_000_000), &one).is_err());

        let mut m = manifest(2);
        m["mood"] = json!({"unknown": "fields are ignored"});
        m["layers"][0]["blend"] = json!(3);
        let doc = craft(&path, MIMETYPE, &m, &two).unwrap();
        assert_eq!(doc.layers[0].tiles.len(), 2);
        assert_eq!(doc.layers[0].tiles[&(3, 1)][5][7], [100; 4]);

        let mut m = manifest(0);
        m["layers"] = json!([]);
        m["active"] = json!(4);
        let doc = craft(&path, MIMETYPE, &m, &[]).unwrap();
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.active, doc.layers[0].id);
        fs::remove_dir_all(&dir).unwrap();
    }

    fn read_png(bytes: &[u8]) -> (png::OutputInfo, Vec<u8>) {
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        (info, buf)
    }

    #[test]
    fn export_is_the_flattened_page() {
        let dir = temp_dir("export");
        let path = dir.join("page.png");
        let mut doc = sample_doc();
        let top = doc.layers[1].id;
        let mut red = compositor::new_shared_tile();
        for px in Arc::get_mut(&mut red).unwrap().as_flattened_mut() {
            *px = [ONE as u16, 0, 0, ONE as u16];
        }
        doc.layer_mut(top).unwrap().tiles.insert((4, 3), red);
        export_png(&doc, &path).unwrap();

        let (info, rgb) = read_png(&fs::read(&path).unwrap());
        assert_eq!((info.width, info.height), (300, 200));
        assert_eq!((info.color_type, info.bit_depth), (png::ColorType::Rgb, png::BitDepth::Eight));
        let at = |x: usize, y: usize| &rgb[(y * 300 + x) * 3..(y * 300 + x) * 3 + 3];
        // Blank, and covered only by the hidden layer: plain paper.
        assert_eq!(at(150, 70), doc.paper);
        assert_eq!(at(10, 10), doc.paper);
        // The last, partial tile in both directions.
        assert_eq!(at(299, 199), [255, 0, 0]);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn thumbnail_fits_256() {
        let dir = temp_dir("thumbnail");
        let path = dir.join("a4.lily");
        let mut doc = Document::new(2480, 3508);
        let id = doc.active;
        doc.layer_mut(id).unwrap().tiles.insert((0, 0), varied_tile(9));
        save(&doc, &path, true).unwrap();
        let mut zip = ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let mut bytes = Vec::new();
        zip.by_name("thumbnail.png").unwrap().read_to_end(&mut bytes).unwrap();
        let (info, rgb) = read_png(&bytes);
        assert_eq!((info.width, info.height), (178, 251));
        assert_eq!(&rgb[rgb.len() - 3..], [255; 3]);
        assert_ne!(&rgb[..3], [255; 3]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
