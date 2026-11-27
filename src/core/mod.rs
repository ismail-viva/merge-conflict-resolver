//! UI-independent merge engine: three-way diff, per-line resolution state,
//! undo/redo and output rendering.

pub mod diff3;
pub mod history;
pub mod session;
pub mod text;

pub use diff3::{Hunk, HunkKind};
pub use session::{
    Applied, HunkStatus, Labels, LineRef, LineState, MergeInput, MergeOutput, MergeSession, Op,
    OpError, Origin, ResultLine, Side, Snapshot, Stats,
};
