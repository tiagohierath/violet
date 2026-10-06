//! Undo and redo as swaps (ARCHITECTURE §7). A record holds whatever the
//! action displaced: the tiles a stroke replaced, a deleted layer, an old
//! opacity. Undo swaps it back into the document and keeps what that
//! displaces, so the same record then serves redo. Nothing is copied.
//! Fixed at 100 steps (decided 2026-10-05).

use std::collections::VecDeque;

use hokusai_core::tile::TilePixels;

use crate::document::{Document, Layer, Tile, TileKey};

/// Undo steps kept; older ones are dropped.
pub const STEPS: usize = 100;

/// One tile a stroke replaced.
pub struct Change {
    pub layer: u64,
    pub key: TileKey,
    /// `None` means the tile did not exist.
    pub tile: Option<Tile>,
}

/// What a record undoes. Each variant holds the displaced state, and
/// `swap` exchanges it with the document's.
enum Op {
    Tiles(Vec<Change>),
    /// A layer at `index` that exists in exactly one place: the document
    /// (`layer` is `None`) or this record. Swapping moves it across, which
    /// makes one op serve both "add layer" and "delete layer".
    Presence {
        index: usize,
        layer: Option<Layer>,
    },
    Move {
        from: usize,
        to: usize,
    },
    Opacity {
        layer: u64,
        value: f32,
    },
}

/// One undo step.
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
            Op::Tiles(changes) => changes.iter().filter(|c| c.tile.is_some()).count() * tile,
            Op::Presence { layer: Some(layer), .. } => layer.tiles.len() * tile,
            _ => 0,
        }
    }
}

/// The undo and redo stacks.
#[derive(Default)]
pub struct History {
    /// Oldest first; a deque so the oldest step can drop off the front.
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

    /// Records a step that just happened. Clears redo, like every editor.
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
    pub fn undo(&mut self, doc: &mut Document) -> Option<DirtyHint> {
        self.dragging = None;
        let mut record = self.undo.pop_back()?;
        let hint = swap(&mut record.op, doc);
        self.redo.push(record);
        Some(hint)
    }

    /// As `undo`, the other way.
    pub fn redo(&mut self, doc: &mut Document) -> Option<DirtyHint> {
        self.dragging = None;
        let mut record = self.redo.pop()?;
        let hint = swap(&mut record.op, doc);
        self.undo.push_back(record);
        Some(hint)
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// `(undo steps, redo steps)`.
    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    /// Pixel memory history alone keeps alive.
    pub fn bytes(&self) -> usize {
        self.undo.iter().chain(&self.redo).map(Record::bytes).sum()
    }
}

/// What an undo or redo needs repainted.
#[derive(Debug, PartialEq)]
pub enum DirtyHint {
    Keys(Vec<TileKey>),
    AllDoc,
}

/// Exchanges the record's state with the document's, so the same record
/// turns around to serve the opposite direction.
fn swap(op: &mut Op, doc: &mut Document) -> DirtyHint {
    let hint = match op {
        Op::Tiles(changes) => {
            let mut keys = Vec::with_capacity(changes.len());
            for change in changes.iter_mut() {
                // A record for a layer that no longer exists changes nothing.
                let Some(layer) = doc.layer_mut(change.layer) else { continue };
                let current = layer.tiles.remove(&change.key);
                if let Some(tile) = change.tile.take() {
                    layer.tiles.insert(change.key, tile);
                }
                change.tile = current;
                keys.push(change.key);
            }
            DirtyHint::Keys(keys)
        }
        Op::Presence { index, layer } => match layer.take() {
            Some(held) => {
                let keys = held.tiles.keys().copied().collect();
                let at = (*index).min(doc.layers.len());
                doc.layers.insert(at, held);
                DirtyHint::Keys(keys)
            }
            None if *index < doc.layers.len() => {
                let removed = doc.layers.remove(*index);
                let keys = removed.tiles.keys().copied().collect();
                *layer = Some(removed);
                DirtyHint::Keys(keys)
            }
            None => DirtyHint::Keys(Vec::new()),
        },
        Op::Move { from, to } => {
            if *to < doc.layers.len() && *from < doc.layers.len() {
                let moved = doc.layers.remove(*to);
                let keys = moved.tiles.keys().copied().collect();
                doc.layers.insert(*from, moved);
                std::mem::swap(from, to);
                DirtyHint::Keys(keys)
            } else {
                DirtyHint::Keys(Vec::new())
            }
        }
        Op::Opacity { layer, value } => match doc.layer_mut(*layer) {
            Some(target) => {
                std::mem::swap(&mut target.opacity, value);
                DirtyHint::AllDoc
            }
            None => DirtyHint::Keys(Vec::new()),
        },
    };
    // Undoing an "add layer" can remove the active one.
    if doc.layer(doc.active).is_none()
        && let Some(top) = doc.layers.last()
    {
        doc.active = top.id;
    }
    hint
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

        let mut history = History::new();
        history.push(Record::tiles(vec![Change { layer: id, key: (0, 0), tile: None }]));

        assert!(matches!(history.undo(&mut doc), Some(DirtyHint::Keys(k)) if k == vec![(0, 0)]));
        assert!(doc.layer(id).unwrap().tiles.is_empty());
        assert!(matches!(history.redo(&mut doc), Some(DirtyHint::Keys(k)) if k == vec![(0, 0)]));
        assert!(std::sync::Arc::ptr_eq(&doc.layer(id).unwrap().tiles[&(0, 0)], &painted));
        assert_eq!(history.redo(&mut doc), None);
    }

    #[test]
    fn delete_layer_undoes() {
        let mut doc = Document::new(64, 64);
        let second = doc.push_layer("two");
        let removed = doc.layers.remove(1);
        let mut history = History::new();
        history.push(Record::removed(1, removed));
        assert_eq!(doc.layers.len(), 1);
        history.undo(&mut doc);
        assert_eq!(doc.layers[1].id, second);
        history.redo(&mut doc);
        assert_eq!(doc.layers.len(), 1);
    }

    #[test]
    fn keeps_a_hundred_steps() {
        let mut doc = Document::new(64, 64);
        let id = doc.active;
        let mut history = History::new();
        for _ in 0..150 {
            history.push(Record::tiles(vec![Change { layer: id, key: (0, 0), tile: None }]));
        }
        assert_eq!(history.depth(), (STEPS, 0));
        assert!(history.undo(&mut doc).is_some());
    }
}
