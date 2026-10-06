//! The document: fixed bounds, a stack of layers, each a sparse grid of
//! 64x64 fix15 tiles (ARCHITECTURE §1, §9). Tiles are shared (`Arc`) and
//! never mutated in place once committed: a stroke replaces the tiles it
//! touched. That one rule makes undo a pointer swap and lets a save thread
//! hold a consistent snapshot while painting carries on.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use hokusai_core::tile::{TILE_SIZE, TilePixels};

use crate::compositor;

/// Tile column and row; tile `(tx, ty)` starts at pixel `(tx * 64, ty * 64)`.
pub type TileKey = (i32, i32);
/// Shared and immutable once committed; see the module comment.
pub type Tile = Arc<TilePixels>;

pub const DEFAULT_WIDTH: u32 = 3000;
pub const DEFAULT_HEIGHT: u32 = 2000;
/// Largest side accepted from a file or a command. 30000 px is past any
/// print size, and keeps a full layer's tile index in the low millions.
pub const MAX_SIDE: u32 = 30_000;

/// One layer. Tiles that hold no paint are absent, not stored transparent.
#[derive(Clone)]
pub struct Layer {
    /// Stable for the session; what history records and commands address.
    pub id: u64,
    pub name: String,
    pub opacity: f32,
    pub visible: bool,
    /// OpenRaster composite op, kept so files from other apps round-trip.
    /// Violet composites every layer as source-over for now.
    pub composite_op: String,
    pub tiles: HashMap<TileKey, Tile>,
}

#[derive(Clone)]
pub struct Document {
    pub width: u32,
    pub height: u32,
    /// sRGB. Part of the document: exports are flattened over it.
    pub paper: [u8; 3],
    /// Bottom first.
    pub layers: Vec<Layer>,
    /// Id of the layer strokes land on.
    pub active: u64,
    next_id: u64,
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        let mut doc = Self::empty(width, height);
        doc.active = doc.push_layer("Layer 1");
        doc
    }

    /// No layers yet; for loaders, which add them.
    pub fn empty(width: u32, height: u32) -> Self {
        Self {
            width: width.clamp(1, MAX_SIDE),
            height: height.clamp(1, MAX_SIDE),
            paper: [255; 3],
            layers: Vec::new(),
            active: 0,
            next_id: 1,
        }
    }

    /// Adds an empty layer on top and returns its id.
    pub fn push_layer(&mut self, name: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.layers.push(Layer {
            id,
            name: name.to_owned(),
            opacity: 1.0,
            visible: true,
            composite_op: "svg:src-over".to_owned(),
            tiles: HashMap::new(),
        });
        id
    }

    pub fn layer(&self, id: u64) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn layer_mut(&mut self, id: u64) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    /// Number of tile columns and rows covering the document.
    pub fn tiles_across(&self) -> (i32, i32) {
        let tile = TILE_SIZE as u32;
        (self.width.div_ceil(tile) as i32, self.height.div_ceil(tile) as i32)
    }

    /// How many columns and rows of this tile lie inside the document.
    pub fn valid_in_tile(&self, key: TileKey) -> (usize, usize) {
        valid_in_tile(self.width, self.height, key)
    }

    pub fn is_blank(&self) -> bool {
        self.layers.iter().all(|l| l.tiles.is_empty())
    }

    /// The paper color in the compositor's linear fix15.
    pub fn paper_linear(&self) -> [u16; 3] {
        self.paper.map(compositor::srgb8_to_fix15)
    }

    /// Every tile key with content on any layer.
    pub fn content_keys(&self) -> HashSet<TileKey> {
        self.layers.iter().flat_map(|l| l.tiles.keys().copied()).collect()
    }

    /// Bytes of pixel data the document owns, shared tiles counted once per
    /// layer that holds them.
    pub fn bytes(&self) -> usize {
        self.layers.iter().map(|l| l.tiles.len()).sum::<usize>() * size_of::<TilePixels>()
    }
}

/// How many columns and rows of tile `(tx, ty)` lie inside a `width` by
/// `height` document. Only the last column and row of tiles are partial.
pub fn valid_in_tile(width: u32, height: u32, (tx, ty): TileKey) -> (usize, usize) {
    let tile = TILE_SIZE as i64;
    let w = (width as i64 - tx as i64 * tile).clamp(0, tile) as usize;
    let h = (height as i64 - ty as i64 * tile).clamp(0, tile) as usize;
    (w, h)
}
