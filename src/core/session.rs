//! The merge model: three inputs, the hunks between them and the user's
//! resolution of every hunk.
//!
//! The session is pure data + logic (no I/O, no UI) so that every merge
//! operation can be unit-tested. Invariants:
//!
//! * inputs are immutable — a resolution only ever *selects* lines;
//! * `picked` lists exactly the lines whose state is [`LineState::Accepted`];
//! * a hunk with any [`LineState::Unresolved`] line is never dropped from the
//!   output: it is written with conflict markers instead.

use super::diff3::{diff3, Hunk, HunkKind};
use super::history::{Change, Entry, History};
use super::text::{dominant_eol, split_lines};

/// One of the two branches being merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// The incoming branch ("theirs"), shown on the left.
    Source,
    /// The current branch ("ours"), shown on the right.
    Target,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Source => Side::Target,
            Side::Target => Side::Source,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Side::Source => "source",
            Side::Target => "target",
        }
    }
}

/// Resolution state of one line of one side inside a changed hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineState {
    Unresolved,
    Accepted,
    Rejected,
}

/// A line of a hunk, addressed relative to the hunk's own range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRef {
    pub side: Side,
    pub index: usize,
}

/// The user's resolution of one hunk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HunkState {
    source: Vec<LineState>,
    target: Vec<LineState>,
    /// Accepted lines in output order.
    picked: Vec<LineRef>,
    /// Hand-edited replacement; overrides `picked` when set.
    custom: Option<Vec<String>>,
}

impl HunkState {
    fn side(&self, side: Side) -> &[LineState] {
        match side {
            Side::Source => &self.source,
            Side::Target => &self.target,
        }
    }

    fn side_mut(&mut self, side: Side) -> &mut Vec<LineState> {
        match side {
            Side::Source => &mut self.source,
            Side::Target => &mut self.target,
        }
    }

    fn pending(&self) -> usize {
        self.source
            .iter()
            .chain(&self.target)
            .filter(|s| **s == LineState::Unresolved)
            .count()
    }

    /// Insert keeping lines of one side in file order while still allowing
    /// lines of the two sides to interleave in the order they were accepted.
    fn pick(&mut self, line: LineRef) {
        let at = self
            .picked
            .iter()
            .position(|p| p.side == line.side && p.index > line.index)
            .unwrap_or(self.picked.len());
        self.picked.insert(at, line);
    }

    fn unpick(&mut self, line: LineRef) {
        self.picked.retain(|p| *p != line);
    }

    fn fill(&mut self, side: Side, state: LineState) {
        self.side_mut(side).iter_mut().for_each(|s| *s = state);
    }

    fn refs(&self, side: Side) -> impl Iterator<Item = LineRef> {
        (0..self.side(side).len()).map(move |index| LineRef { side, index })
    }
}

/// Summary of a hunk's resolution, as shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkStatus {
    /// Not a change at all.
    Unchanged,
    /// Non-conflicting change, resolved automatically and left untouched.
    Auto,
    /// Nothing decided yet.
    Unresolved,
    /// Some lines decided, some still pending.
    Partial,
    /// Every line accepted or rejected by the user.
    Resolved,
    /// Replaced by a manual edit.
    Modified,
}

impl HunkStatus {
    /// Whether the hunk still blocks a clean merge result.
    pub fn is_open(self) -> bool {
        matches!(self, HunkStatus::Unresolved | HunkStatus::Partial)
    }
}

/// Where a line of the merged result comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Unchanged,
    Line(LineRef),
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultLine<'a> {
    pub text: &'a str,
    pub origin: Origin,
}

/// Names shown for the three inputs (and written into conflict markers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Labels {
    pub source: String,
    pub target: String,
    pub base: String,
}

impl Default for Labels {
    fn default() -> Self {
        Self {
            source: "source".into(),
            target: "target".into(),
            base: "base".into(),
        }
    }
}

/// The raw material of a three-way merge.
#[derive(Debug, Clone, Default)]
pub struct MergeInput {
    pub base: String,
    pub source: String,
    pub target: String,
    pub labels: Labels,
}

/// A merge operation (command pattern); see [`MergeSession::apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Accept a line, or return it to unresolved if already accepted.
    ToggleAccept { hunk: usize, line: LineRef },
    /// Reject a line, or return it to unresolved if already rejected.
    ToggleReject { hunk: usize, line: LineRef },
    /// Use one side's lines only.
    TakeSide { hunk: usize, side: Side },
    /// Use both sides, `first` on top.
    TakeBoth { hunk: usize, first: Side },
    /// Use neither side.
    TakeNone { hunk: usize },
    /// Reject whatever is still pending, keeping the accepted lines.
    ResolveRemaining { hunk: usize },
    /// Return the hunk to its initial state.
    Reset { hunk: usize },
    /// Replace the hunk's result with hand-edited lines.
    SetCustom { hunk: usize, lines: Vec<String> },
    /// `TakeSide` for every hunk that is still open.
    TakeSideEverywhere { side: Side },
}

impl Op {
    fn label(&self) -> &'static str {
        match self {
            Op::ToggleAccept { .. } => "accept line",
            Op::ToggleReject { .. } => "reject line",
            Op::TakeSide {
                side: Side::Source, ..
            } => "take source hunk",
            Op::TakeSide {
                side: Side::Target, ..
            } => "take target hunk",
            Op::TakeBoth { .. } => "take both sides",
            Op::TakeNone { .. } => "take neither side",
            Op::ResolveRemaining { .. } => "resolve remaining lines",
            Op::Reset { .. } => "reset hunk",
            Op::SetCustom { .. } => "edit hunk",
            Op::TakeSideEverywhere { side: Side::Source } => "take source for all conflicts",
            Op::TakeSideEverywhere { side: Side::Target } => "take target for all conflicts",
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OpError {
    #[error("nothing to resolve here — this line is unchanged")]
    NotAChange,
    #[error("hunk was edited by hand — reset it before picking lines")]
    CustomLocked,
    #[error("no line here")]
    OutOfRange,
    #[error("nothing changed")]
    NoChange,
}

/// What an applied / undone / redone edit touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub label: &'static str,
    pub hunks: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    /// Hunks where at least one side differs from the base.
    pub changes: usize,
    /// Hunks git could not merge on its own.
    pub conflicts: usize,
    /// Hunks that still need a decision.
    pub open: usize,
}

/// The merged text plus how many hunks were written as conflict markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutput {
    pub text: String,
    pub unresolved: usize,
}

const MARKER_LEN: usize = 7;

#[derive(Debug)]
pub struct MergeSession {
    base: Vec<String>,
    source: Vec<String>,
    target: Vec<String>,
    labels: Labels,
    eol: &'static str,
    hunks: Vec<Hunk>,
    states: Vec<HunkState>,
    history: History,
    revision: u64,
}

impl MergeSession {
    pub fn new(input: MergeInput) -> Self {
        let base = split_lines(&input.base);
        let source = split_lines(&input.source);
        let target = split_lines(&input.target);
        let eol = dominant_eol(target.iter().chain(&source).map(String::as_str));
        let hunks = diff3(&base, &source, &target);
        let states = hunks.iter().map(initial_state).collect();
        Self {
            base,
            source,
            target,
            labels: input.labels,
            eol,
            hunks,
            states,
            history: History::default(),
            revision: 0,
        }
    }

    // ---- read access -----------------------------------------------------

    pub fn labels(&self) -> &Labels {
        &self.labels
    }

    pub fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }

    pub fn hunk(&self, hunk: usize) -> &Hunk {
        &self.hunks[hunk]
    }

    /// Increases with every edit, undo and redo.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn can_undo(&self) -> bool {
        self.history.undo_depth() > 0
    }

    pub fn can_redo(&self) -> bool {
        self.history.redo_depth() > 0
    }

    pub fn side_lines(&self, hunk: usize, side: Side) -> &[String] {
        let h = &self.hunks[hunk];
        match side {
            Side::Source => &self.source[h.source.clone()],
            Side::Target => &self.target[h.target.clone()],
        }
    }

    pub fn base_lines(&self, hunk: usize) -> &[String] {
        &self.base[self.hunks[hunk].base.clone()]
    }

    pub fn side_len(&self, side: Side) -> usize {
        match side {
            Side::Source => self.source.len(),
            Side::Target => self.target.len(),
        }
    }

    pub fn line_state(&self, hunk: usize, line: LineRef) -> Option<LineState> {
        self.states[hunk].side(line.side).get(line.index).copied()
    }

    pub fn is_custom(&self, hunk: usize) -> bool {
        self.states[hunk].custom.is_some()
    }

    /// Lines of the hunk that still await a decision.
    pub fn pending(&self, hunk: usize) -> usize {
        if self.states[hunk].custom.is_some() {
            0
        } else {
            self.states[hunk].pending()
        }
    }

    pub fn status(&self, hunk: usize) -> HunkStatus {
        let h = &self.hunks[hunk];
        let state = &self.states[hunk];
        if state.custom.is_some() {
            return HunkStatus::Modified;
        }
        if h.kind == HunkKind::Unchanged {
            return HunkStatus::Unchanged;
        }
        let pending = state.pending();
        if pending == 0 {
            if h.kind != HunkKind::Conflict && *state == initial_state(h) {
                HunkStatus::Auto
            } else {
                HunkStatus::Resolved
            }
        } else if pending == state.source.len() + state.target.len() {
            HunkStatus::Unresolved
        } else {
            HunkStatus::Partial
        }
    }

    pub fn result_len(&self, hunk: usize) -> usize {
        let state = &self.states[hunk];
        match &state.custom {
            Some(lines) => lines.len(),
            None if self.hunks[hunk].kind == HunkKind::Unchanged => self.hunks[hunk].target.len(),
            None => state.picked.len(),
        }
    }

    /// The `index`-th line of the hunk's merged result.
    pub fn result_line(&self, hunk: usize, index: usize) -> Option<ResultLine<'_>> {
        let state = &self.states[hunk];
        if let Some(lines) = &state.custom {
            return lines.get(index).map(|text| ResultLine {
                text,
                origin: Origin::Custom,
            });
        }
        if self.hunks[hunk].kind == HunkKind::Unchanged {
            return self
                .side_lines(hunk, Side::Target)
                .get(index)
                .map(|text| ResultLine {
                    text,
                    origin: Origin::Unchanged,
                });
        }
        let line = *state.picked.get(index)?;
        Some(ResultLine {
            text: &self.side_lines(hunk, line.side)[line.index],
            origin: Origin::Line(line),
        })
    }

    pub fn result_lines(&self, hunk: usize) -> impl Iterator<Item = ResultLine<'_>> {
        (0..self.result_len(hunk)).filter_map(move |i| self.result_line(hunk, i))
    }

    pub fn stats(&self) -> Stats {
        let mut stats = Stats::default();
        for (i, h) in self.hunks.iter().enumerate() {
            if h.kind != HunkKind::Unchanged {
                stats.changes += 1;
            }
            if h.kind == HunkKind::Conflict {
                stats.conflicts += 1;
            }
            if self.status(i).is_open() {
                stats.open += 1;
            }
        }
        stats
    }

    // ---- editing ---------------------------------------------------------

    /// Apply `op`, recording it for undo. Fails without side effects.
    pub fn apply(&mut self, op: Op) -> Result<Applied, OpError> {
        let label = op.label();
        let mut changes = Vec::new();
        match op {
            Op::TakeSideEverywhere { side } => {
                for hunk in 0..self.hunks.len() {
                    if !self.status(hunk).is_open() {
                        continue;
                    }
                    let after = self.next_state(hunk, &Op::TakeSide { hunk, side })?;
                    changes.push(Change {
                        hunk,
                        before: self.states[hunk].clone(),
                        after,
                    });
                }
            }
            op => {
                let hunk = op_hunk(&op);
                let after = self.next_state(hunk, &op)?;
                changes.push(Change {
                    hunk,
                    before: self.states[hunk].clone(),
                    after,
                });
            }
        }
        changes.retain(|c| c.before != c.after);
        if changes.is_empty() {
            return Err(OpError::NoChange);
        }
        let hunks = changes.iter().map(|c| c.hunk).collect();
        for change in &changes {
            self.states[change.hunk] = change.after.clone();
        }
        self.history.record(Entry { label, changes });
        self.revision += 1;
        Ok(Applied { label, hunks })
    }

    pub fn undo(&mut self) -> Option<Applied> {
        let entry = self.history.undo()?;
        for change in &entry.changes {
            self.states[change.hunk] = change.before.clone();
        }
        self.revision += 1;
        Some(Applied {
            label: entry.label,
            hunks: entry.changes.iter().map(|c| c.hunk).collect(),
        })
    }

    pub fn redo(&mut self) -> Option<Applied> {
        let entry = self.history.redo()?;
        for change in &entry.changes {
            self.states[change.hunk] = change.after.clone();
        }
        self.revision += 1;
        Some(Applied {
            label: entry.label,
            hunks: entry.changes.iter().map(|c| c.hunk).collect(),
        })
    }

    /// Compute the state `op` leads to, without mutating anything.
    fn next_state(&self, hunk: usize, op: &Op) -> Result<HunkState, OpError> {
        let kind = self.hunks[hunk].kind;
        let mut state = self.states[hunk].clone();
        match op {
            Op::SetCustom { lines, .. } => {
                state.custom = Some(lines.clone());
                return Ok(state);
            }
            Op::Reset { .. } => return Ok(initial_state(&self.hunks[hunk])),
            _ => {}
        }
        if state.custom.is_some() {
            return Err(OpError::CustomLocked);
        }
        if kind == HunkKind::Unchanged {
            return Err(OpError::NotAChange);
        }
        match *op {
            Op::ToggleAccept { line, .. } => {
                let current = *state
                    .side(line.side)
                    .get(line.index)
                    .ok_or(OpError::OutOfRange)?;
                if current == LineState::Accepted {
                    state.side_mut(line.side)[line.index] = LineState::Unresolved;
                    state.unpick(line);
                } else {
                    state.side_mut(line.side)[line.index] = LineState::Accepted;
                    state.pick(line);
                }
            }
            Op::ToggleReject { line, .. } => {
                let current = *state
                    .side(line.side)
                    .get(line.index)
                    .ok_or(OpError::OutOfRange)?;
                state.unpick(line);
                state.side_mut(line.side)[line.index] = if current == LineState::Rejected {
                    LineState::Unresolved
                } else {
                    LineState::Rejected
                };
            }
            Op::TakeSide { side, .. } => {
                state.fill(side, LineState::Accepted);
                state.fill(side.other(), LineState::Rejected);
                state.picked = state.refs(side).collect();
            }
            Op::TakeBoth { first, .. } => {
                state.fill(Side::Source, LineState::Accepted);
                state.fill(Side::Target, LineState::Accepted);
                state.picked = state.refs(first).chain(state.refs(first.other())).collect();
            }
            Op::TakeNone { .. } => {
                state.fill(Side::Source, LineState::Rejected);
                state.fill(Side::Target, LineState::Rejected);
                state.picked.clear();
            }
            Op::ResolveRemaining { .. } => {
                for s in state.source.iter_mut().chain(state.target.iter_mut()) {
                    if *s == LineState::Unresolved {
                        *s = LineState::Rejected;
                    }
                }
            }
            Op::SetCustom { .. } | Op::Reset { .. } | Op::TakeSideEverywhere { .. } => {
                unreachable!("handled by the caller")
            }
        }
        Ok(state)
    }

    // ---- output ----------------------------------------------------------

    /// The hunk as a standard (diff3-style) conflict block.
    pub fn conflict_block(&self, hunk: usize) -> String {
        let mut out = Emitter::new(self.eol);
        self.emit_conflict(hunk, &mut out);
        out.text
    }

    /// The hunk's current result as plain text.
    pub fn result_text(&self, hunk: usize) -> String {
        let mut out = Emitter::new(self.eol);
        for line in self.result_lines(hunk) {
            out.line(line.text);
        }
        out.text
    }

    /// Render the whole merge. Open hunks are written as conflict markers so
    /// that no side of an undecided hunk is ever lost.
    pub fn output(&self) -> MergeOutput {
        let mut out = Emitter::new(self.eol);
        let mut unresolved = 0;
        for hunk in 0..self.hunks.len() {
            if self.status(hunk).is_open() {
                unresolved += 1;
                self.emit_conflict(hunk, &mut out);
            } else {
                for line in self.result_lines(hunk) {
                    out.line(line.text);
                }
            }
        }
        MergeOutput {
            text: out.text,
            unresolved,
        }
    }

    fn emit_conflict(&self, hunk: usize, out: &mut Emitter) {
        let marker = |c: char, label: &str| {
            let mut m: String = std::iter::repeat_n(c, MARKER_LEN).collect();
            if !label.is_empty() {
                m.push(' ');
                m.push_str(label);
            }
            m.push_str(self.eol);
            m
        };
        out.line(&marker('<', &self.labels.target));
        self.side_lines(hunk, Side::Target)
            .iter()
            .for_each(|l| out.line(l));
        out.line(&marker('|', &self.labels.base));
        self.base_lines(hunk).iter().for_each(|l| out.line(l));
        out.line(&marker('=', ""));
        self.side_lines(hunk, Side::Source)
            .iter()
            .for_each(|l| out.line(l));
        out.line(&marker('>', &self.labels.source));
    }
}

fn op_hunk(op: &Op) -> usize {
    match op {
        Op::ToggleAccept { hunk, .. }
        | Op::ToggleReject { hunk, .. }
        | Op::TakeSide { hunk, .. }
        | Op::TakeBoth { hunk, .. }
        | Op::TakeNone { hunk }
        | Op::ResolveRemaining { hunk }
        | Op::Reset { hunk }
        | Op::SetCustom { hunk, .. } => *hunk,
        Op::TakeSideEverywhere { .. } => unreachable!("bulk op has no single hunk"),
    }
}

/// The state a hunk starts in: non-conflicting changes are resolved the way
/// git would resolve them, conflicts are left entirely open.
fn initial_state(hunk: &Hunk) -> HunkState {
    let mut state = HunkState {
        source: vec![LineState::Unresolved; hunk.source.len()],
        target: vec![LineState::Unresolved; hunk.target.len()],
        picked: Vec::new(),
        custom: None,
    };
    let winner = match hunk.kind {
        HunkKind::Unchanged => return HunkState::default(),
        HunkKind::Conflict => return state,
        HunkKind::SourceOnly => Side::Source,
        HunkKind::TargetOnly | HunkKind::BothSame => Side::Target,
    };
    state.fill(winner, LineState::Accepted);
    state.fill(winner.other(), LineState::Rejected);
    state.picked = state.refs(winner).collect();
    state
}

/// Joins lines, supplying a terminator wherever a line that lacked one (the
/// last line of an input) ends up in the middle of the output.
struct Emitter {
    text: String,
    eol: &'static str,
}

impl Emitter {
    fn new(eol: &'static str) -> Self {
        Self {
            text: String::new(),
            eol,
        }
    }

    fn line(&mut self, line: &str) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push_str(self.eol);
        }
        self.text.push_str(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "one\ntwo\nthree\nfour\nfive\n";
    const SOURCE: &str = "one\nTWO-src\nTWO-src-b\nthree\nfour\nfive\nsix\n";
    const TARGET: &str = "zero\none\nTWO-tgt\nthree\nfour\nfive\n";

    fn session() -> MergeSession {
        MergeSession::new(MergeInput {
            base: BASE.into(),
            source: SOURCE.into(),
            target: TARGET.into(),
            labels: Labels {
                source: "feature".into(),
                target: "main".into(),
                base: "base".into(),
            },
        })
    }

    fn conflict(s: &MergeSession) -> usize {
        s.hunks()
            .iter()
            .position(|h| h.kind == HunkKind::Conflict)
            .unwrap()
    }

    fn src(index: usize) -> LineRef {
        LineRef {
            side: Side::Source,
            index,
        }
    }

    fn tgt(index: usize) -> LineRef {
        LineRef {
            side: Side::Target,
            index,
        }
    }

    fn texts(s: &MergeSession, hunk: usize) -> Vec<&str> {
        s.result_lines(hunk).map(|l| l.text.trim_end()).collect()
    }

    #[test]
    fn non_conflicting_changes_start_resolved() {
        let s = session();
        let stats = s.stats();
        assert_eq!(
            stats,
            Stats {
                changes: 3,
                conflicts: 1,
                open: 1
            }
        );
        let auto = s
            .hunks()
            .iter()
            .enumerate()
            .filter(|(i, _)| s.status(*i) == HunkStatus::Auto);
        assert_eq!(auto.count(), 2);
        assert_eq!(s.status(conflict(&s)), HunkStatus::Unresolved);
    }

    #[test]
    fn unresolved_output_keeps_both_sides_in_markers() {
        let out = session().output();
        assert_eq!(out.unresolved, 1);
        assert_eq!(
            out.text,
            "zero\none\n<<<<<<< main\nTWO-tgt\n||||||| base\ntwo\n=======\nTWO-src\nTWO-src-b\n\
             >>>>>>> feature\nthree\nfour\nfive\nsix\n"
        );
    }

    #[test]
    fn take_side_resolves_the_hunk() {
        let mut s = session();
        let h = conflict(&s);
        s.apply(Op::TakeSide {
            hunk: h,
            side: Side::Source,
        })
        .unwrap();
        assert_eq!(s.status(h), HunkStatus::Resolved);
        assert_eq!(s.line_state(h, tgt(0)), Some(LineState::Rejected));
        let out = s.output();
        assert_eq!(out.unresolved, 0);
        assert_eq!(
            out.text,
            "zero\none\nTWO-src\nTWO-src-b\nthree\nfour\nfive\nsix\n"
        );
    }

    #[test]
    fn take_both_orders_by_first_side() {
        let mut s = session();
        let h = conflict(&s);
        s.apply(Op::TakeBoth {
            hunk: h,
            first: Side::Target,
        })
        .unwrap();
        assert_eq!(texts(&s, h), ["TWO-tgt", "TWO-src", "TWO-src-b"]);
        s.apply(Op::TakeBoth {
            hunk: h,
            first: Side::Source,
        })
        .unwrap();
        assert_eq!(texts(&s, h), ["TWO-src", "TWO-src-b", "TWO-tgt"]);
    }

    #[test]
    fn lines_can_be_accepted_independently_and_combined() {
        let mut s = session();
        let h = conflict(&s);
        s.apply(Op::ToggleAccept {
            hunk: h,
            line: src(1),
        })
        .unwrap();
        assert_eq!(s.status(h), HunkStatus::Partial);
        s.apply(Op::ToggleAccept {
            hunk: h,
            line: tgt(0),
        })
        .unwrap();
        // Accepting an earlier source line keeps source lines in file order.
        s.apply(Op::ToggleAccept {
            hunk: h,
            line: src(0),
        })
        .unwrap();
        assert_eq!(texts(&s, h), ["TWO-src", "TWO-src-b", "TWO-tgt"]);
        assert_eq!(s.status(h), HunkStatus::Resolved);
    }

    #[test]
    fn sides_interleave_in_acceptance_order() {
        let mut s = session();
        let h = conflict(&s);
        s.apply(Op::ToggleAccept {
            hunk: h,
            line: src(0),
        })
        .unwrap();
        s.apply(Op::ToggleAccept {
            hunk: h,
            line: tgt(0),
        })
        .unwrap();
        s.apply(Op::ToggleAccept {
            hunk: h,
            line: src(1),
        })
        .unwrap();
        assert_eq!(texts(&s, h), ["TWO-src", "TWO-tgt", "TWO-src-b"]);
    }

    #[test]
    fn partial_hunks_stay_open_until_every_line_is_decided() {
        let mut s = session();
        let h = conflict(&s);
        s.apply(Op::ToggleAccept {
            hunk: h,
            line: src(0),
        })
        .unwrap();
        assert_eq!(s.pending(h), 2);
        let out = s.output();
        assert_eq!(
            out.unresolved, 1,
            "a partial pick must not drop the pending lines"
        );
        assert!(out.text.contains("TWO-tgt") && out.text.contains("TWO-src-b"));

        s.apply(Op::ToggleReject {
            hunk: h,
            line: tgt(0),
        })
        .unwrap();
        s.apply(Op::ResolveRemaining { hunk: h }).unwrap();
        assert_eq!(s.status(h), HunkStatus::Resolved);
        assert_eq!(texts(&s, h), ["TWO-src"]);
    }

    #[test]
    fn toggles_return_lines_to_unresolved() {
        let mut s = session();
        let h = conflict(&s);
        s.apply(Op::ToggleAccept {
            hunk: h,
            line: src(0),
        })
        .unwrap();
        s.apply(Op::ToggleAccept {
            hunk: h,
            line: src(0),
        })
        .unwrap();
        assert_eq!(s.line_state(h, src(0)), Some(LineState::Unresolved));
        assert_eq!(s.status(h), HunkStatus::Unresolved);
        s.apply(Op::ToggleReject {
            hunk: h,
            line: src(0),
        })
        .unwrap();
        assert_eq!(s.line_state(h, src(0)), Some(LineState::Rejected));
        s.apply(Op::ToggleReject {
            hunk: h,
            line: src(0),
        })
        .unwrap();
        assert_eq!(s.line_state(h, src(0)), Some(LineState::Unresolved));
    }

    #[test]
    fn rejecting_an_accepted_line_removes_it_from_the_result() {
        let mut s = session();
        let h = conflict(&s);
        s.apply(Op::TakeBoth {
            hunk: h,
            first: Side::Source,
        })
        .unwrap();
        s.apply(Op::ToggleReject {
            hunk: h,
            line: src(1),
        })
        .unwrap();
        assert_eq!(texts(&s, h), ["TWO-src", "TWO-tgt"]);
    }

    #[test]
    fn take_none_resolves_to_nothing() {
        let mut s = session();
        let h = conflict(&s);
        s.apply(Op::TakeNone { hunk: h }).unwrap();
        assert_eq!(s.status(h), HunkStatus::Resolved);
        assert_eq!(s.output().text, "zero\none\nthree\nfour\nfive\nsix\n");
    }

    #[test]
    fn undo_and_redo_restore_exact_states() {
        let mut s = session();
        let h = conflict(&s);
        let initial = s.output();
        s.apply(Op::ToggleAccept {
            hunk: h,
            line: src(0),
        })
        .unwrap();
        let one = s.output();
        s.apply(Op::TakeSide {
            hunk: h,
            side: Side::Target,
        })
        .unwrap();
        let two = s.output();

        assert_eq!(s.undo().unwrap().hunks, [h]);
        assert_eq!(s.output(), one);
        assert_eq!(s.undo().unwrap().label, "accept line");
        assert_eq!(s.output(), initial);
        assert!(s.undo().is_none());

        s.redo().unwrap();
        s.redo().unwrap();
        assert_eq!(s.output(), two);
        assert!(s.redo().is_none());
    }

    #[test]
    fn a_new_edit_clears_the_redo_stack() {
        let mut s = session();
        let h = conflict(&s);
        s.apply(Op::TakeSide {
            hunk: h,
            side: Side::Source,
        })
        .unwrap();
        s.undo().unwrap();
        assert!(s.can_redo());
        s.apply(Op::TakeSide {
            hunk: h,
            side: Side::Target,
        })
        .unwrap();
        assert!(!s.can_redo());
    }

    #[test]
    fn failed_and_no_op_edits_leave_no_history() {
        let mut s = session();
        let h = conflict(&s);
        let unchanged = s
            .hunks()
            .iter()
            .position(|x| x.kind == HunkKind::Unchanged)
            .unwrap();
        assert_eq!(
            s.apply(Op::TakeSide {
                hunk: unchanged,
                side: Side::Source
            }),
            Err(OpError::NotAChange)
        );
        assert_eq!(
            s.apply(Op::ToggleAccept {
                hunk: h,
                line: src(9)
            }),
            Err(OpError::OutOfRange)
        );
        assert_eq!(s.apply(Op::Reset { hunk: h }), Err(OpError::NoChange));
        assert!(!s.can_undo());
        assert_eq!(s.revision(), 0);
    }

    #[test]
    fn auto_resolved_hunks_can_be_overridden_and_reset() {
        let mut s = session();
        let h = s
            .hunks()
            .iter()
            .position(|x| x.kind == HunkKind::TargetOnly)
            .unwrap();
        assert_eq!(texts(&s, h), ["zero"]);
        s.apply(Op::TakeNone { hunk: h }).unwrap();
        assert_eq!(s.status(h), HunkStatus::Resolved);
        assert!(texts(&s, h).is_empty());
        s.apply(Op::Reset { hunk: h }).unwrap();
        assert_eq!(s.status(h), HunkStatus::Auto);
    }

    #[test]
    fn manual_edit_overrides_and_locks_line_picks() {
        let mut s = session();
        let h = conflict(&s);
        s.apply(Op::SetCustom {
            hunk: h,
            lines: vec!["merged by hand\n".into()],
        })
        .unwrap();
        assert_eq!(s.status(h), HunkStatus::Modified);
        assert_eq!(s.result_line(h, 0).unwrap().origin, Origin::Custom);
        assert_eq!(
            s.apply(Op::ToggleAccept {
                hunk: h,
                line: src(0)
            }),
            Err(OpError::CustomLocked)
        );
        assert_eq!(s.output().unresolved, 0);
        assert!(s.output().text.contains("merged by hand\n"));
        s.undo().unwrap();
        assert_eq!(s.status(h), HunkStatus::Unresolved);
    }

    #[test]
    fn take_side_everywhere_is_one_undo_step() {
        let mut s = MergeSession::new(MergeInput {
            base: "a\nb\nc\nd\ne\n".into(),
            source: "A1\nb\nc\nd\nE1\n".into(),
            target: "A2\nb\nc\nd\nE2\n".into(),
            labels: Labels::default(),
        });
        assert_eq!(s.stats().open, 2);
        let applied = s
            .apply(Op::TakeSideEverywhere { side: Side::Source })
            .unwrap();
        assert_eq!(applied.hunks.len(), 2);
        assert_eq!(s.output().text, "A1\nb\nc\nd\nE1\n");
        s.undo().unwrap();
        assert_eq!(s.stats().open, 2);
        assert_eq!(
            s.apply(Op::TakeSideEverywhere { side: Side::Source })
                .map(|a| a.hunks.len()),
            Ok(2)
        );
        assert_eq!(
            s.apply(Op::TakeSideEverywhere { side: Side::Source }),
            Err(OpError::NoChange)
        );
    }

    #[test]
    fn missing_final_newline_is_repaired_only_mid_file() {
        let mut s = MergeSession::new(MergeInput {
            base: "a\n".into(),
            source: "a\nsrc".into(),
            target: "a\ntgt".into(),
            labels: Labels::default(),
        });
        let h = conflict(&s);
        s.apply(Op::TakeBoth {
            hunk: h,
            first: Side::Source,
        })
        .unwrap();
        assert_eq!(s.output().text, "a\nsrc\ntgt");
    }

    #[test]
    fn crlf_files_get_crlf_markers() {
        let s = MergeSession::new(MergeInput {
            base: "a\r\n".into(),
            source: "b\r\n".into(),
            target: "c\r\n".into(),
            labels: Labels::default(),
        });
        let text = s.output().text;
        assert!(text.contains("<<<<<<< target\r\n"));
        assert!(!text.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn resolving_every_hunk_with_target_reproduces_target() {
        let mut s = session();
        let source_only: Vec<usize> = (0..s.hunks().len())
            .filter(|&i| s.hunk(i).kind != HunkKind::Unchanged)
            .collect();
        for h in source_only {
            let _ = s.apply(Op::TakeSide {
                hunk: h,
                side: Side::Target,
            });
        }
        assert_eq!(s.output().text, TARGET);
    }
}
