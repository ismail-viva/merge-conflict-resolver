//! Access to the things being merged.
//!
//! The application talks to a [`MergeBackend`] only; whether the inputs come
//! from a repository's index ([`GitBackend`]) or from plain files handed over
//! by `git mergetool` ([`FileBackend`]) is an implementation detail.

pub mod blame;
pub mod files;
pub mod repo;

use std::path::PathBuf;
use std::sync::Arc;

pub use blame::{Blame, CommitInfo};
pub use files::FileBackend;
pub use repo::GitBackend;

use crate::core::{MergeInput, Side};

/// One conflicted path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictEntry {
    pub path: String,
    /// Set when the conflict cannot be resolved line by line.
    pub unsupported: Option<String>,
}

/// What is being merged into what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeContext {
    /// "merge", "rebase", "cherry-pick", …
    pub operation: String,
    pub source_label: String,
    pub target_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SaveOutcome {
    /// The path was marked as resolved in the index.
    pub staged: bool,
    /// Where the previous working-tree content was preserved, if anywhere.
    pub backup: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("failed to run git: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("git {command} failed: {stderr}")]
    Command { command: String, stderr: String },
    #[error("{0}")]
    Unsupported(String),
    #[error("not inside a git repository")]
    NotARepository,
}

pub type GitResult<T> = Result<T, GitError>;

/// Source of merge inputs and sink for merge results.
pub trait MergeBackend: Send + Sync {
    fn context(&self) -> MergeContext;

    /// Paths that currently need resolving.
    fn conflicts(&self) -> GitResult<Vec<ConflictEntry>>;

    /// The three versions of `path`.
    fn load(&self, path: &str) -> GitResult<MergeInput>;

    /// Write a merge result. `resolved` tells whether `content` is free of
    /// conflict markers; only then may the path be marked as resolved.
    fn save(&self, path: &str, content: &str, resolved: bool) -> GitResult<SaveOutcome>;

    /// Per-line history of one side, or `None` when there is none to show.
    fn blame(&self, path: &str, side: Side) -> GitResult<Option<Blame>>;
}

pub type SharedBackend = Arc<dyn MergeBackend>;

/// Decode file content, refusing anything that is not line-mergeable text.
pub(crate) fn decode_text(bytes: Vec<u8>, what: &str) -> GitResult<String> {
    if bytes.contains(&0) {
        return Err(GitError::Unsupported(format!("{what} is a binary file")));
    }
    String::from_utf8(bytes)
        .map_err(|_| GitError::Unsupported(format!("{what} is not valid UTF-8 text")))
}
