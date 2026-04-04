//! Undo/redo history — stores adjustment snapshots with auto-naming.

use super::adjustment::AdjustmentSnapshot;
use serde::{Deserialize, Serialize};

/// Maximum number of undo entries.
const MAX_HISTORY: usize = 50;

/// A named history entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryEntry {
    snapshot: AdjustmentSnapshot,
}

/// Undo/redo stack for editor adjustments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryModel {
    stack: Vec<HistoryEntry>,
    /// Current position in the stack. Points to the last applied entry.
    /// -1 means no history (initial state).
    index: i32,
}

impl Default for HistoryModel {
    fn default() -> Self {
        Self {
            stack: Vec::new(),
            index: -1,
        }
    }
}

impl HistoryModel {
    /// Push a new snapshot, truncating any redo entries beyond the current position.
    pub fn push(&mut self, snapshot: AdjustmentSnapshot) {
        let new_index = self.index + 1;
        // Truncate redo history
        self.stack.truncate(new_index as usize);
        self.stack.push(HistoryEntry { snapshot });
        self.index = new_index;

        // Evict oldest entries if over the limit
        if self.stack.len() > MAX_HISTORY {
            let excess = self.stack.len() - MAX_HISTORY;
            self.stack.drain(..excess);
            self.index -= excess as i32;
        }
    }

    /// Undo: return a clone of the previous snapshot, or None if at the beginning.
    pub fn undo(&mut self) -> Option<AdjustmentSnapshot> {
        if self.index <= 0 {
            return None;
        }
        self.index -= 1;
        Some(self.stack[self.index as usize].snapshot.clone())
    }

    /// Redo: return a clone of the next snapshot, or None if at the end.
    pub fn redo(&mut self) -> Option<AdjustmentSnapshot> {
        let next = self.index + 1;
        if next >= self.stack.len() as i32 {
            return None;
        }
        self.index = next;
        Some(self.stack[self.index as usize].snapshot.clone())
    }

    pub fn can_undo(&self) -> bool {
        self.index > 0
    }

    pub fn can_redo(&self) -> bool {
        (self.index + 1) < self.stack.len() as i32
    }

    /// Clear all history.
    pub fn clear(&mut self) {
        self.stack.clear();
        self.index = -1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AdjustmentModel;

    fn make_snapshot(value: f64) -> AdjustmentSnapshot {
        let mut adj = AdjustmentModel::default();
        adj.set("test", value);
        adj.snapshot()
    }

    #[test]
    fn undo_redo_cycle() {
        let mut h = HistoryModel::default();
        h.push(make_snapshot(0.0));
        h.push(make_snapshot(1.0));
        h.push(make_snapshot(2.0));

        assert!(h.can_undo());
        let _ = h.undo().unwrap();
        assert!(h.can_redo());
        let _ = h.redo().unwrap();
        assert!(!h.can_redo());
    }

    #[test]
    fn push_after_undo_truncates_redo() {
        let mut h = HistoryModel::default();
        h.push(make_snapshot(0.0));
        h.push(make_snapshot(1.0));
        h.push(make_snapshot(2.0));
        h.undo();
        h.push(make_snapshot(3.0));
        assert!(!h.can_redo());
    }
}
