//! Three-way line diff.
//!
//! Both sides are diffed against the common base; the two change lists are
//! then swept in base order and overlapping (or touching) changes are grouped
//! into a single hunk. The resulting hunks tile all three inputs: joining the
//! `source` ranges of every hunk yields the whole source file, and likewise
//! for `target` and `base`.

use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{capture_diff_slices_deadline, Algorithm, DiffTag};

/// How the two sides relate to the base inside one hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HunkKind {
    /// Neither side changed these lines.
    Unchanged,
    /// Only the source (incoming, "theirs") branch changed them.
    SourceOnly,
    /// Only the target (current, "ours") branch changed them.
    TargetOnly,
    /// Both sides made the identical change.
    BothSame,
    /// Both sides changed the same region differently.
    Conflict,
}

/// A region of the three files that is handled as one unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub kind: HunkKind,
    pub base: Range<usize>,
    pub source: Range<usize>,
    pub target: Range<usize>,
}

#[derive(Debug, Clone)]
struct Change {
    base: Range<usize>,
    side: Range<usize>,
}

/// Map every distinct line to a small integer so diffing compares integers.
fn intern<'a, T: AsRef<str>>(lines: &'a [T], table: &mut HashMap<&'a str, u32>) -> Vec<u32> {
    lines
        .iter()
        .map(|line| {
            let next = table.len() as u32;
            *table.entry(line.as_ref()).or_insert(next)
        })
        .collect()
}

/// Upper bound on the time spent searching for a minimal diff of one side.
/// Past it the diff is completed with a coarser (but still correct) result,
/// so pathological inputs cannot freeze the application.
const DIFF_DEADLINE: Duration = Duration::from_secs(2);

fn changes(base: &[u32], side: &[u32]) -> Vec<Change> {
    let deadline = Instant::now() + DIFF_DEADLINE;
    let ops = capture_diff_slices_deadline(Algorithm::Myers, base, side, Some(deadline));
    let mut out: Vec<Change> = Vec::new();
    for op in ops {
        let (tag, b, s) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.base.end == b.start && last.side.end == s.start => {
                last.base.end = b.end;
                last.side.end = s.end;
            }
            _ => out.push(Change { base: b, side: s }),
        }
    }
    out
}

/// Walks one side's change list in base order.
struct Cursor<'a> {
    changes: &'a [Change],
    next: usize,
    /// Base / side positions just past the last consumed change.
    base_end: usize,
    side_end: usize,
}

impl<'a> Cursor<'a> {
    fn new(changes: &'a [Change]) -> Self {
        Self {
            changes,
            next: 0,
            base_end: 0,
            side_end: 0,
        }
    }

    fn peek_start(&self) -> Option<usize> {
        self.changes.get(self.next).map(|c| c.base.start)
    }

    /// Side position matching `base_pos`, valid in the unchanged stretch
    /// between the last consumed change and the next one.
    fn map(&self, base_pos: usize) -> usize {
        self.side_end + (base_pos - self.base_end)
    }

    /// Consume every change starting at or before `end`; returns the new end.
    fn absorb(&mut self, mut end: usize, touched: &mut bool) -> usize {
        while let Some(change) = self.changes.get(self.next) {
            if change.base.start > end {
                break;
            }
            end = end.max(change.base.end);
            self.base_end = change.base.end;
            self.side_end = change.side.end;
            self.next += 1;
            *touched = true;
        }
        end
    }
}

/// Compute the three-way diff of `source` and `target` against `base`.
pub fn diff3<T: AsRef<str>>(base: &[T], source: &[T], target: &[T]) -> Vec<Hunk> {
    let mut table = HashMap::new();
    let base_ids = intern(base, &mut table);
    let source_ids = intern(source, &mut table);
    let target_ids = intern(target, &mut table);

    let source_changes = changes(&base_ids, &source_ids);
    let target_changes = changes(&base_ids, &target_ids);
    let mut src = Cursor::new(&source_changes);
    let mut tgt = Cursor::new(&target_changes);

    let mut hunks = Vec::new();
    let mut pos = 0usize;

    loop {
        let start = match (src.peek_start(), tgt.peek_start()) {
            (None, None) => break,
            (Some(a), None) | (None, Some(a)) => a,
            (Some(a), Some(b)) => a.min(b),
        };
        if start > pos {
            hunks.push(Hunk {
                kind: HunkKind::Unchanged,
                base: pos..start,
                source: src.map(pos)..src.map(start),
                target: tgt.map(pos)..tgt.map(start),
            });
        }
        let source_start = src.map(start);
        let target_start = tgt.map(start);

        let (mut in_source, mut in_target) = (false, false);
        let mut end = start;
        loop {
            let grown = tgt.absorb(src.absorb(end, &mut in_source), &mut in_target);
            // A change absorbed on one side may now touch one on the other.
            let more_source = src.peek_start().is_some_and(|s| s <= grown);
            end = grown;
            if !more_source {
                break;
            }
        }

        let base_range = start..end;
        let source_range = source_start..src.map(end);
        let target_range = target_start..tgt.map(end);
        match (in_source, in_target) {
            (true, false) => hunks.push(Hunk {
                kind: HunkKind::SourceOnly,
                base: base_range,
                source: source_range,
                target: target_range,
            }),
            (false, true) => hunks.push(Hunk {
                kind: HunkKind::TargetOnly,
                base: base_range,
                source: source_range,
                target: target_range,
            }),
            _ => push_two_sided(
                &mut hunks,
                base_range,
                source_range,
                target_range,
                &source_ids,
                &target_ids,
            ),
        }
        pos = end;
    }

    if pos < base.len() {
        hunks.push(Hunk {
            kind: HunkKind::Unchanged,
            base: pos..base.len(),
            source: src.map(pos)..source.len(),
            target: tgt.map(pos)..target.len(),
        });
    }
    hunks
}

/// Emit a region both sides changed, peeling identical leading / trailing
/// lines off into `BothSame` hunks so the conflict is as small as possible.
fn push_two_sided(
    hunks: &mut Vec<Hunk>,
    base: Range<usize>,
    source: Range<usize>,
    target: Range<usize>,
    source_ids: &[u32],
    target_ids: &[u32],
) {
    let s = &source_ids[source.clone()];
    let t = &target_ids[target.clone()];
    if s == t {
        hunks.push(Hunk {
            kind: HunkKind::BothSame,
            base,
            source,
            target,
        });
        return;
    }
    let prefix = s.iter().zip(t).take_while(|(a, b)| a == b).count();
    let max_suffix = s.len().min(t.len()) - prefix;
    let suffix = s
        .iter()
        .rev()
        .zip(t.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();

    if prefix > 0 {
        hunks.push(Hunk {
            kind: HunkKind::BothSame,
            base: base.start..base.start,
            source: source.start..source.start + prefix,
            target: target.start..target.start + prefix,
        });
    }
    hunks.push(Hunk {
        kind: HunkKind::Conflict,
        base: base.clone(),
        source: source.start + prefix..source.end - suffix,
        target: target.start + prefix..target.end - suffix,
    });
    if suffix > 0 {
        hunks.push(Hunk {
            kind: HunkKind::BothSame,
            base: base.end..base.end,
            source: source.end - suffix..source.end,
            target: target.end - suffix..target.end,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.split_whitespace().map(|w| format!("{w}\n")).collect()
    }

    fn kinds(hunks: &[Hunk]) -> Vec<HunkKind> {
        hunks.iter().map(|h| h.kind).collect()
    }

    /// Every hunk list must tile the three inputs without gaps or overlap.
    fn assert_tiles(hunks: &[Hunk], base: usize, source: usize, target: usize) {
        let (mut b, mut s, mut t) = (0, 0, 0);
        for h in hunks {
            assert_eq!(
                (h.base.start, h.source.start, h.target.start),
                (b, s, t),
                "{hunks:#?}"
            );
            assert!(h.base.start <= h.base.end);
            assert!(h.source.start <= h.source.end);
            assert!(h.target.start <= h.target.end);
            b = h.base.end;
            s = h.source.end;
            t = h.target.end;
        }
        assert_eq!((b, s, t), (base, source, target), "{hunks:#?}");
    }

    #[test]
    fn identical_inputs_are_one_unchanged_hunk() {
        let a = lines("1 2 3");
        let hunks = diff3(&a, &a, &a);
        assert_eq!(kinds(&hunks), [HunkKind::Unchanged]);
        assert!(diff3::<String>(&[], &[], &[]).is_empty());
    }

    #[test]
    fn one_sided_changes_are_attributed() {
        let base = lines("1 2 3 4 5");
        let source = lines("1 two 3 4 5");
        let target = lines("1 2 3 4 five");
        let hunks = diff3(&base, &source, &target);
        assert_eq!(
            kinds(&hunks),
            [
                HunkKind::Unchanged,
                HunkKind::SourceOnly,
                HunkKind::Unchanged,
                HunkKind::TargetOnly
            ]
        );
        assert_tiles(&hunks, 5, 5, 5);
    }

    #[test]
    fn overlapping_changes_conflict() {
        let base = lines("1 2 3");
        let source = lines("1 src 3");
        let target = lines("1 tgt 3");
        let hunks = diff3(&base, &source, &target);
        assert_eq!(
            kinds(&hunks),
            [HunkKind::Unchanged, HunkKind::Conflict, HunkKind::Unchanged]
        );
        assert_eq!(hunks[1].base, 1..2);
        assert_eq!(hunks[1].source, 1..2);
        assert_eq!(hunks[1].target, 1..2);
    }

    #[test]
    fn identical_changes_are_both_same() {
        let base = lines("1 2 3");
        let both = lines("1 new 3");
        let hunks = diff3(&base, &both, &both);
        assert_eq!(
            kinds(&hunks),
            [HunkKind::Unchanged, HunkKind::BothSame, HunkKind::Unchanged]
        );
    }

    #[test]
    fn adjacent_changes_conflict_like_git() {
        let base = lines("1 2 3 4");
        let source = lines("1 two 3 4");
        let target = lines("1 2 three 4");
        let hunks = diff3(&base, &source, &target);
        assert!(kinds(&hunks).contains(&HunkKind::Conflict));
        assert_tiles(&hunks, 4, 4, 4);
    }

    #[test]
    fn insertions_at_the_same_point_conflict() {
        let base = lines("1 2");
        let source = lines("1 a 2");
        let target = lines("1 b 2");
        let hunks = diff3(&base, &source, &target);
        assert_eq!(
            kinds(&hunks),
            [HunkKind::Unchanged, HunkKind::Conflict, HunkKind::Unchanged]
        );
        assert!(hunks[1].base.is_empty());
    }

    #[test]
    fn conflict_is_trimmed_to_the_differing_lines() {
        let base = lines("1 9");
        let source = lines("1 a b c 9");
        let target = lines("1 a x c 9");
        let hunks = diff3(&base, &source, &target);
        assert_eq!(
            kinds(&hunks),
            [
                HunkKind::Unchanged,
                HunkKind::BothSame,
                HunkKind::Conflict,
                HunkKind::BothSame,
                HunkKind::Unchanged
            ]
        );
        assert_eq!(hunks[2].source, 2..3);
        assert_tiles(&hunks, 2, 5, 5);
    }

    #[test]
    fn add_add_without_base() {
        let hunks = diff3(&[], &lines("a b"), &lines("a c"));
        assert_eq!(kinds(&hunks), [HunkKind::BothSame, HunkKind::Conflict]);
        assert_tiles(&hunks, 0, 2, 2);
    }

    #[test]
    fn both_deleted_is_both_same() {
        let base = lines("1 2 3");
        let side = lines("1 3");
        let hunks = diff3(&base, &side, &side);
        assert_eq!(hunks[1].kind, HunkKind::BothSame);
        assert!(hunks[1].source.is_empty() && hunks[1].target.is_empty());
    }

    /// Deterministic fuzz: random edits must always tile, and merging a side
    /// against an untouched base must reproduce that side exactly.
    #[test]
    fn fuzz_tiling_and_trivial_merges() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for _ in 0..400 {
            let base: Vec<String> = (0..rnd(12)).map(|_| format!("l{}\n", rnd(6))).collect();
            let mut mutate = |input: &[String]| {
                let mut out = Vec::new();
                for line in input {
                    match rnd(6) {
                        0 => {}
                        1 => out.push(format!("m{}\n", rnd(6))),
                        2 => {
                            out.push(line.clone());
                            out.push(format!("i{}\n", rnd(6)));
                        }
                        _ => out.push(line.clone()),
                    }
                }
                out
            };
            let source = mutate(&base);
            let target = mutate(&base);
            let hunks = diff3(&base, &source, &target);
            assert_tiles(&hunks, base.len(), source.len(), target.len());
            for h in &hunks {
                match h.kind {
                    HunkKind::Unchanged => {
                        assert_eq!(source[h.source.clone()], base[h.base.clone()]);
                        assert_eq!(target[h.target.clone()], base[h.base.clone()]);
                    }
                    HunkKind::SourceOnly => {
                        assert_eq!(target[h.target.clone()], base[h.base.clone()])
                    }
                    HunkKind::TargetOnly => {
                        assert_eq!(source[h.source.clone()], base[h.base.clone()])
                    }
                    HunkKind::BothSame => {
                        assert_eq!(source[h.source.clone()], target[h.target.clone()])
                    }
                    HunkKind::Conflict => {
                        assert_ne!(source[h.source.clone()], target[h.target.clone()])
                    }
                }
            }
            let trivial = diff3(&base, &source, &base);
            assert!(trivial
                .iter()
                .all(|h| matches!(h.kind, HunkKind::Unchanged | HunkKind::SourceOnly)));
        }
    }
}
