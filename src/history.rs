//! Undo and redo as swaps (ARCHITECTURE §7). A record holds whatever the
//! action displaced: the tiles a stroke replaced, a deleted layer, an old
//! opacity. Undo swaps it back into the document and keeps what that
//! displaces, so the same record then serves redo. Nothing is copied.
//! Fixed at 100 steps (decided 2026-10-05).

use std::collections::VecDeque;

use hokusai_core::tile::TilePixels;

use crate::document::{Document, Layer, Tile, TileKey};

pub const STEPS: usize = 100;

pub struct Change {
    pub layer: u64,
    pub key: TileKey,
    /// `None` means the tile did not exist.
    pub tile: Option<Tile>,
}

enum Op {
    Tiles(Vec<Change>),
    /// A layer at `index` that exists in exactly one place: the document
    /// (`layer` is `None`) or this record. Swapping moves it across, which
    /// makes one op serve both "add layer" and "delete layer".
    Presence { index: usize, layer: Option<Layer> },
    Move { from: usize, to: usize },
    Opacity { layer: u64, value: f32 },
}

pub struct Record {
    op: Op,
}

impl Record {
    pub fn tiles(changes: Vec<Change>) -> Self {
        Self { op: Op::Tiles(changes) }
    }

    /// For a layer just added at `index` (the document holds it).
    pub fn added(index: usize) -> Self {
        Self { op: Op::Presence { index, layer: None } }
    }

    /// For a layer just removed from `index` (the record holds it).
    pub fn removed(index: usize, layer: Layer) -> Self {
        Self { op: Op::Presence { index, layer: Some(layer) } }
    }

    /// For a layer just moved from `from` to `to`.
    pub fn moved(from: usize, to: usize) -> Self {
        Self { op: Op::Move { from, to } }
    }

    /// For an opacity change; `old` is the value before it.
    pub fn opacity(layer: u64, old: f32) -> Self {
        Self { op: Op::Opacity { layer, value: old } }
    }

    fn is_empty(&self) -> bool {
        matches!(&self.op, Op::Tiles(c) if c.is_empty())
    }

    fn bytes(&self) -> usize {
        let tile = size_of::<TilePixels>();
        match &self.op {
            Op::Tiles(c) => c.iter().filter(|c| c.tile.is_some()).count() * tile,
            Op::Presence { layer: Some(l), .. } => l.tiles.len() * tile,
            _ => 0,
        }
    }
}

#[derive(Default)]
pub struct History {
    undo: VecDeque<Record>,
    redo: Vec<Record>,
    /// Layer whose opacity is being dragged: further opacity changes to it
    /// fold into the step already recorded, which holds the value from
    /// before the drag began.
    dragging: Option<u64>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, record: Record) {
        if record.is_empty() {
            return;
        }
        self.redo.clear();
        match record.op {
            Op::Opacity { layer, .. } if self.dragging == Some(layer) => return,
            Op::Opacity { layer, .. } => self.dragging = Some(layer),
            _ => self.dragging = None,
        }
        self.undo.push_back(record);
        while self.undo.len() > STEPS {
            self.undo.pop_front();
        }
    }

    /// Ends an opacity drag, so the next change is its own step.
    pub fn seal(&mut self) {
        self.dragging = None;
    }

    /// Returns the tiles whose pixels changed, or `None` when there is
    /// nothing to undo.
    pub fn undo(&mut self, doc: &mut Document) -> Option<Vec<TileKey>> {
        self.dragging = None;
        let mut record = self.undo.pop_back()?;
        let keys = swap(&mut record.op, doc);
        self.redo.push(record);
        Some(keys)
    }

    pub fn redo(&mut self, doc: &mut Document) -> Option<Vec<TileKey>> {
        self.dragging = None;
        let mut record = self.redo.pop()?;
        let keys = swap(&mut record.op, doc);
        self.undo.push_back(record);
        Some(keys)
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    /// Pixel memory history alone keeps alive.
    pub fn bytes(&self) -> usize {
        self.undo.iter().chain(&self.redo).map(Record::bytes).sum()
    }
}

fn swap(op: &mut Op, doc: &mut Document) -> Vec<TileKey> {
    let keys = match op {
        Op::Tiles(changes) => {
            let mut keys = Vec::with_capacity(changes.len());
            for c in changes.iter_mut() {
                // A record for a layer that no longer exists changes nothing.
                let Some(layer) = doc.layer_mut(c.layer) else { continue };
                let current = layer.tiles.remove(&c.key);
                if let Some(t) = c.tile.take() {
                    layer.tiles.insert(c.key, t);
                }
                c.tile = current;
                keys.push(c.key);
            }
            keys
        }
        Op::Presence { index, layer } => match layer.take() {
            Some(l) => {
                let keys = l.tiles.keys().copied().collect();
                let at = (*index).min(doc.layers.len());
                doc.layers.insert(at, l);
                keys
            }
            None if *index < doc.layers.len() => {
                let l = doc.layers.remove(*index);
                let keys = l.tiles.keys().copied().collect();
                *layer = Some(l);
                keys
            }
            None => Vec::new(),
        },
        Op::Move { from, to } => {
            if *to < doc.layers.len() && *from < doc.layers.len() {
                let l = doc.layers.remove(*to);
                let keys = l.tiles.keys().copied().collect();
                doc.layers.insert(*from, l);
                std::mem::swap(from, to);
                keys
            } else {
                Vec::new()
            }
        }
        Op::Opacity { layer, value } => match doc.layer_mut(*layer) {
            Some(l) => {
                std::mem::swap(&mut l.opacity, value);
                l.tiles.keys().copied().collect()
            }
            None => Vec::new(),
        },
    };
    if doc.layer(doc.active).is_none()
        && let Some(top) = doc.layers.last()
    {
        doc.active = top.id;
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::new_shared_tile;

    #[test]
    fn undo_then_redo_restores_both_states() {
        let mut doc = Document::new(128, 128);
        let id = doc.active;
        let painted = new_shared_tile();
        doc.layer_mut(id).unwrap().tiles.insert((0, 0), painted.clone());

        let mut h = History::new();
        h.push(Record::tiles(vec![Change { layer: id, key: (0, 0), tile: None }]));

        assert_eq!(h.undo(&mut doc), Some(vec![(0, 0)]));
        assert!(doc.layer(id).unwrap().tiles.is_empty());
        assert_eq!(h.redo(&mut doc), Some(vec![(0, 0)]));
        assert!(std::sync::Arc::ptr_eq(&doc.layer(id).unwrap().tiles[&(0, 0)], &painted));
        assert_eq!(h.redo(&mut doc), None);
    }

    #[test]
    fn delete_layer_undoes() {
        let mut doc = Document::new(64, 64);
        let second = doc.push_layer("two");
        let removed = doc.layers.remove(1);
        let mut h = History::new();
        h.push(Record::removed(1, removed));
        assert_eq!(doc.layers.len(), 1);
        h.undo(&mut doc);
        assert_eq!(doc.layers[1].id, second);
        h.redo(&mut doc);
        assert_eq!(doc.layers.len(), 1);
    }

    #[test]
    fn keeps_a_hundred_steps() {
        let mut doc = Document::new(64, 64);
        let id = doc.active;
        let mut h = History::new();
        for _ in 0..150 {
            h.push(Record::tiles(vec![Change { layer: id, key: (0, 0), tile: None }]));
        }
        assert_eq!(h.depth(), (STEPS, 0));
        assert!(h.undo(&mut doc).is_some());
    }
}
