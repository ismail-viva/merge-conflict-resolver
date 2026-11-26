//! Undo / redo history (memento pattern).
//!
//! Every edit records the complete before/after state of each hunk it
//! touched, so undo and redo are exact and can never drift from the model.

use super::session::HunkState;

/// One hunk's state transition.
#[derive(Debug, Clone)]
pub struct Change {
    pub hunk: usize,
    pub before: HunkState,
    pub after: HunkState,
}

/// One user-visible edit; may span several hunks (bulk operations).
#[derive(Debug, Clone)]
pub struct Entry {
    pub label: &'static str,
    pub changes: Vec<Change>,
}

#[derive(Debug, Default)]
pub struct History {
    done: Vec<Entry>,
    undone: Vec<Entry>,
}

impl History {
    /// Record a new edit. This invalidates the redo stack.
    pub fn record(&mut self, entry: Entry) {
        self.done.push(entry);
        self.undone.clear();
    }

    /// Move the latest edit to the redo stack and return it.
    pub fn undo(&mut self) -> Option<&Entry> {
        let entry = self.done.pop()?;
        self.undone.push(entry);
        self.undone.last()
    }

    /// Move the latest undone edit back and return it.
    pub fn redo(&mut self) -> Option<&Entry> {
        let entry = self.undone.pop()?;
        self.done.push(entry);
        self.done.last()
    }

    pub fn undo_depth(&self) -> usize {
        self.done.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.undone.len()
    }
}
