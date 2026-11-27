//! [`MergeBackend`] for plain files, as handed over by `git mergetool`.

use std::path::{Path, PathBuf};

use super::{
    decode_text, Blame, ConflictEntry, GitError, GitResult, MergeBackend, MergeContext, SaveOutcome,
};
use crate::core::{Labels, MergeInput, Side};

pub struct FileBackend {
    base: Option<PathBuf>,
    /// The current branch's version ("ours", `$LOCAL`).
    target: PathBuf,
    /// The incoming version ("theirs", `$REMOTE`).
    source: PathBuf,
    output: PathBuf,
}

impl FileBackend {
    pub fn new(base: Option<PathBuf>, target: PathBuf, source: PathBuf, output: PathBuf) -> Self {
        Self {
            base,
            target,
            source,
            output,
        }
    }

    fn display(&self) -> String {
        self.output.display().to_string()
    }

    fn read(path: &Path) -> GitResult<String> {
        decode_text(std::fs::read(path)?, &path.display().to_string())
    }
}

impl MergeBackend for FileBackend {
    fn context(&self) -> MergeContext {
        MergeContext {
            operation: "merge".to_owned(),
            source_label: "REMOTE".to_owned(),
            target_label: "LOCAL".to_owned(),
        }
    }

    fn conflicts(&self) -> GitResult<Vec<ConflictEntry>> {
        Ok(vec![ConflictEntry {
            path: self.display(),
            unsupported: None,
            resolved: false,
        }])
    }

    fn load(&self, _path: &str) -> GitResult<MergeInput> {
        Ok(MergeInput {
            base: match &self.base {
                Some(base) => Self::read(base)?,
                None => String::new(),
            },
            source: Self::read(&self.source)?,
            target: Self::read(&self.target)?,
            labels: Labels {
                source: "REMOTE".to_owned(),
                target: "LOCAL".to_owned(),
                base: "BASE".to_owned(),
            },
        })
    }

    fn save(&self, _path: &str, content: &str, _resolved: bool) -> GitResult<SaveOutcome> {
        std::fs::write(&self.output, content).map_err(GitError::from)?;
        Ok(SaveOutcome::default())
    }

    fn blame(&self, _path: &str, _side: Side) -> GitResult<Option<Blame>> {
        Ok(None)
    }
}
