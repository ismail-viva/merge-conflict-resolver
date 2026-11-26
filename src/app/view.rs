//! Per-file view state: the merge session plus cursor and scrolling.
//!
//! The three panes are aligned on a shared grid of *rows*. Every hunk takes
//! as many rows as its tallest pane needs, shorter panes are padded, so one
//! row index addresses the same hunk in all three panes — that is what keeps
//! navigation and scrolling synchronized.

use crate::core::{HunkKind, MergeSession, Side};

/// One of the three panes, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pane {
    Source,
    Result,
    Target,
}

impl Pane {
    pub const ALL: [Pane; 3] = [Pane::Source, Pane::Result, Pane::Target];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn side(self) -> Option<Side> {
        match self {
            Pane::Source => Some(Side::Source),
            Pane::Target => Some(Side::Target),
            Pane::Result => None,
        }
    }

    pub fn left(self) -> Pane {
        Pane::ALL[self.index().saturating_sub(1)]
    }

    pub fn right(self) -> Pane {
        Pane::ALL[(self.index() + 1).min(2)]
    }

    pub fn next(self) -> Pane {
        Pane::ALL[(self.index() + 1) % 3]
    }

    pub fn prev(self) -> Pane {
        Pane::ALL[(self.index() + 2) % 3]
    }
}

/// Maps hunks to rows of the aligned three-pane grid.
#[derive(Debug, Clone, Default)]
pub struct RowLayout {
    /// First row of every hunk.
    starts: Vec<usize>,
    /// Number of result lines before every hunk.
    result_starts: Vec<usize>,
    total: usize,
}

impl RowLayout {
    pub fn build(session: &MergeSession) -> Self {
        let mut layout = RowLayout::default();
        let mut result_lines = 0;
        for (index, hunk) in session.hunks().iter().enumerate() {
            let result_len = session.result_len(index);
            let height = hunk
                .source
                .len()
                .max(hunk.target.len())
                .max(result_len)
                .max(1);
            layout.starts.push(layout.total);
            layout.result_starts.push(result_lines);
            layout.total += height;
            result_lines += result_len;
        }
        layout
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn hunk_count(&self) -> usize {
        self.starts.len()
    }

    pub fn start(&self, hunk: usize) -> usize {
        self.starts[hunk]
    }

    pub fn height(&self, hunk: usize) -> usize {
        self.starts.get(hunk + 1).copied().unwrap_or(self.total) - self.starts[hunk]
    }

    /// Number of result lines above `hunk`.
    pub fn result_start(&self, hunk: usize) -> usize {
        self.result_starts[hunk]
    }

    /// The hunk containing `row` and the row's offset inside it.
    pub fn locate(&self, row: usize) -> Option<(usize, usize)> {
        if row >= self.total {
            return None;
        }
        let hunk = self.starts.partition_point(|start| *start <= row) - 1;
        Some((hunk, row - self.starts[hunk]))
    }
}

/// What happened the last time the file was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveInfo {
    pub unresolved: usize,
    pub staged: bool,
}

pub struct FileView {
    pub session: MergeSession,
    pub rows: RowLayout,
    /// Cursor row in the aligned grid.
    pub cursor: usize,
    /// First visible row.
    pub scroll: usize,
    /// Horizontal scroll, in columns.
    pub hscroll: usize,
    /// Session revision that is on disk (0 = nothing written yet).
    pub saved_revision: u64,
    pub last_save: Option<SaveInfo>,
}

impl FileView {
    pub fn new(session: MergeSession) -> Self {
        let rows = RowLayout::build(&session);
        let mut view = Self {
            session,
            rows,
            cursor: 0,
            scroll: 0,
            hscroll: 0,
            saved_revision: 0,
            last_save: None,
        };
        // Start on the first thing that needs attention.
        if let Some(hunk) = view.find_hunk(0, true, |v, h| v.session.status(h).is_open()) {
            view.cursor = view.rows.start(hunk);
        }
        view
    }

    pub fn is_dirty(&self) -> bool {
        self.session.revision() != self.saved_revision
    }

    /// Hunk and in-hunk offset under the cursor.
    pub fn position(&self) -> (usize, usize) {
        self.rows.locate(self.cursor).unwrap_or((0, 0))
    }

    pub fn current_hunk(&self) -> Option<usize> {
        self.rows.locate(self.cursor).map(|(hunk, _)| hunk)
    }

    pub fn max_row(&self) -> usize {
        self.rows.total().saturating_sub(1)
    }

    pub fn move_to(&mut self, row: usize) {
        self.cursor = row.min(self.max_row());
    }

    pub fn move_by(&mut self, delta: isize) {
        self.move_to(self.cursor.saturating_add_signed(delta));
    }

    pub fn goto_hunk(&mut self, hunk: usize) {
        if hunk < self.rows.hunk_count() {
            self.cursor = self.rows.start(hunk);
        }
    }

    /// Rebuild the row grid after an edit, keeping the cursor on the same
    /// hunk and (as far as the new height allows) the same offset.
    pub fn relayout(&mut self) {
        let (hunk, offset) = self.position();
        self.rows = RowLayout::build(&self.session);
        if hunk < self.rows.hunk_count() {
            self.cursor = self.rows.start(hunk) + offset.min(self.rows.height(hunk) - 1);
        }
        self.cursor = self.cursor.min(self.max_row());
    }

    /// Adjust scrolling so the cursor is inside the viewport with a margin.
    pub fn ensure_visible(&mut self, height: usize, scrolloff: usize) {
        if height == 0 {
            return;
        }
        let margin = scrolloff.min(height.saturating_sub(1) / 2);
        if self.cursor < self.scroll + margin {
            self.scroll = self.cursor.saturating_sub(margin);
        } else if self.cursor + margin >= self.scroll + height {
            self.scroll = self.cursor + margin + 1 - height;
        }
        self.scroll = self.scroll.min(self.rows.total().saturating_sub(height));
    }

    /// Scroll the viewport, dragging the cursor along to stay visible.
    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        let max_scroll = self.rows.total().saturating_sub(height);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max_scroll);
        let last_visible = (self.scroll + height).saturating_sub(1);
        self.cursor = self
            .cursor
            .clamp(self.scroll, last_visible.max(self.scroll));
        self.cursor = self.cursor.min(self.max_row());
    }

    /// First hunk matching `pred`, searching forward from `from` (inclusive)
    /// or backward from `from` (inclusive).
    fn find_hunk(
        &self,
        from: usize,
        forward: bool,
        pred: impl Fn(&Self, usize) -> bool,
    ) -> Option<usize> {
        let count = self.rows.hunk_count();
        if forward {
            (from..count).find(|h| pred(self, *h))
        } else {
            (0..=from.min(count.checked_sub(1)?))
                .rev()
                .find(|h| pred(self, *h))
        }
    }

    /// The next (or previous) changed hunk relative to the cursor.
    pub fn neighbor_change(&self, forward: bool) -> Option<usize> {
        self.neighbor(forward, |v, h| {
            v.session.hunk(h).kind != HunkKind::Unchanged
        })
    }

    /// The next (or previous) hunk that still needs a decision.
    pub fn neighbor_open(&self, forward: bool) -> Option<usize> {
        self.neighbor(forward, |v, h| v.session.status(h).is_open())
    }

    /// Any open hunk, preferring ones after the cursor.
    pub fn any_open(&self) -> Option<usize> {
        self.neighbor_open(true)
            .or_else(|| self.find_hunk(0, true, |v, h| v.session.status(h).is_open()))
    }

    fn neighbor(&self, forward: bool, pred: impl Fn(&Self, usize) -> bool) -> Option<usize> {
        let current = self.current_hunk()?;
        if forward {
            self.find_hunk(current + 1, true, pred)
        } else {
            self.find_hunk(current.checked_sub(1)?, false, pred)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{MergeInput, Op};

    fn view() -> FileView {
        FileView::new(MergeSession::new(MergeInput {
            base: "a\nb\nc\nd\ne\n".into(),
            source: "a\nB1\nB2\nB3\nc\nd\nE-src\n".into(),
            target: "a\nB-tgt\nc\nd\nE-tgt\n".into(),
            ..MergeInput::default()
        }))
    }

    #[test]
    fn rows_align_hunks_to_their_tallest_pane() {
        let v = view();
        // a | conflict (3 source lines vs 1) | c d | conflict
        assert_eq!(v.rows.total(), 1 + 3 + 2 + 1);
        assert_eq!(v.rows.locate(0), Some((0, 0)));
        assert_eq!(v.rows.locate(3), Some((1, 2)));
        assert_eq!(v.rows.locate(4), Some((2, 0)));
        assert_eq!(v.rows.locate(6), Some((3, 0)));
        assert_eq!(v.rows.locate(7), None);
        assert_eq!(
            v.rows.result_start(2),
            1,
            "unresolved hunk contributes no result lines"
        );
    }

    #[test]
    fn cursor_starts_on_the_first_open_hunk() {
        let v = view();
        assert_eq!(v.position(), (1, 0));
    }

    #[test]
    fn relayout_keeps_the_cursor_in_its_hunk() {
        let mut v = view();
        v.move_to(3);
        assert_eq!(v.position(), (1, 2));
        v.session
            .apply(Op::TakeSide {
                hunk: 1,
                side: Side::Target,
            })
            .unwrap();
        v.relayout();
        assert_eq!(
            v.position(),
            (1, 2),
            "hunk keeps its height: source still has 3 lines"
        );
        v.session
            .apply(Op::TakeBoth {
                hunk: 1,
                first: Side::Source,
            })
            .unwrap();
        v.relayout();
        assert_eq!(v.rows.height(1), 4);
    }

    #[test]
    fn navigation_between_changes_and_open_hunks() {
        let mut v = view();
        assert_eq!(v.neighbor_open(true), Some(3));
        assert_eq!(v.neighbor_open(false), None);
        v.session
            .apply(Op::TakeSide {
                hunk: 3,
                side: Side::Source,
            })
            .unwrap();
        assert_eq!(v.neighbor_open(true), None);
        assert_eq!(v.neighbor_change(true), Some(3));
        v.goto_hunk(3);
        assert_eq!(v.neighbor_change(false), Some(1));
        assert_eq!(v.any_open(), Some(1));
    }

    #[test]
    fn scrolling_keeps_the_cursor_visible() {
        let mut v = view();
        v.move_to(6);
        v.ensure_visible(3, 1);
        assert_eq!(v.scroll, 4);
        v.move_to(0);
        v.ensure_visible(3, 1);
        assert_eq!(v.scroll, 0);
        v.scroll_by(2, 3);
        assert_eq!((v.scroll, v.cursor), (2, 2));
        v.scroll_by(100, 3);
        assert_eq!(v.scroll, 4);
    }

    #[test]
    fn pane_focus_moves_and_wraps() {
        assert_eq!(Pane::Source.left(), Pane::Source);
        assert_eq!(Pane::Source.right(), Pane::Result);
        assert_eq!(Pane::Target.next(), Pane::Source);
        assert_eq!(Pane::Source.prev(), Pane::Target);
    }
}
