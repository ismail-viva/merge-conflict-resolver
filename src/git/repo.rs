//! [`MergeBackend`] backed by a git repository with a merge in progress.
//!
//! All access goes through the `git` executable: the index is only ever
//! modified by git itself (`git add`, `git update-index --unresolve`), which
//! keeps the repository consistent with what any other git tool expects.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use super::blame::{parse_bodies, parse_porcelain};
use super::{
    decode_text, Blame, ConflictEntry, GitError, GitResult, MergeBackend, MergeContext, SaveOutcome,
};
use crate::core::{Labels, MergeInput, Side};

const MODE_SYMLINK: &str = "120000";
const MODE_SUBMODULE: &str = "160000";
/// Directory under `.git/` holding one progress file per conflicted path.
const PROGRESS_DIR: &str = "mcr-session";

#[derive(Debug, Clone, Copy)]
pub struct GitOptions {
    /// `git add` a file once it is saved without conflicts.
    pub auto_stage: bool,
    /// Keep the previous working-tree content under `.git/mcr-backup/`.
    pub backup: bool,
}

impl Default for GitOptions {
    fn default() -> Self {
        Self {
            auto_stage: true,
            backup: true,
        }
    }
}

/// The three index stages of a conflicted path (base, ours, theirs).
type Stages = [Option<StageEntry>; 3];

#[derive(Debug, Clone)]
struct StageEntry {
    mode: String,
    oid: String,
}

/// The commit being merged in, as far as git's state files tell.
#[derive(Debug, Clone)]
struct Incoming {
    operation: &'static str,
    /// Ref usable for `git blame`; `None` when the incoming side is not a
    /// commit's own content (e.g. a revert).
    rev: Option<&'static str>,
    label: String,
}

pub struct GitBackend {
    root: PathBuf,
    git_dir: PathBuf,
    options: GitOptions,
    /// Paths already backed up during this run.
    backed_up: Mutex<HashSet<String>>,
}

impl GitBackend {
    /// Open the repository containing `dir`.
    pub fn discover(dir: &Path, options: GitOptions) -> GitResult<Self> {
        let probe = |arg: &str| -> GitResult<PathBuf> {
            let out = run_in(dir, &["rev-parse", arg], None).map_err(|err| match err {
                GitError::Command { .. } => GitError::NotARepository,
                other => other,
            })?;
            Ok(PathBuf::from(
                String::from_utf8_lossy(&out).trim_end_matches(['\n', '\r']),
            ))
        };
        let root = probe("--show-toplevel")?;
        let git_dir = probe("--absolute-git-dir")?;
        Ok(Self {
            root,
            git_dir,
            options,
            backed_up: Mutex::default(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Run `git merge <args>` attached to the terminal. Returns whether the
    /// merge completed without conflicts.
    pub fn start_merge(&self, args: &[String]) -> GitResult<bool> {
        let status = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .arg("merge")
            .args(args)
            .status()?;
        Ok(status.success())
    }

    fn git(&self, args: &[&str]) -> GitResult<Vec<u8>> {
        run_in(&self.root, args, None)
    }

    fn git_text(&self, args: &[&str]) -> GitResult<String> {
        Ok(String::from_utf8_lossy(&self.git(args)?).trim().to_owned())
    }

    /// Index stages of the paths git still considers unmerged.
    fn stages(&self, path: Option<&str>) -> GitResult<BTreeMap<String, Stages>> {
        self.list_stages("-u", path)
    }

    /// Stages git kept for paths whose conflict was already resolved
    /// (`git add`ed) during the operation in progress.
    fn resolved_stages(&self, path: Option<&str>) -> GitResult<BTreeMap<String, Stages>> {
        self.list_stages("--resolve-undo", path)
    }

    fn list_stages(&self, which: &str, path: Option<&str>) -> GitResult<BTreeMap<String, Stages>> {
        let mut args = vec!["ls-files", which, "-z"];
        if let Some(path) = path {
            args.extend(["--", path]);
        }
        let out = self.git(&args)?;
        let mut map: BTreeMap<String, Stages> = BTreeMap::new();
        for record in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let record = String::from_utf8_lossy(record);
            let Some((meta, path)) = record.split_once('\t') else {
                continue;
            };
            let mut fields = meta.split(' ');
            let (Some(mode), Some(oid), Some(stage)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let Some(slot) = stage.parse::<usize>().ok().and_then(|s| s.checked_sub(1)) else {
                continue;
            };
            if slot < 3 {
                map.entry(path.to_owned()).or_default()[slot] = Some(StageEntry {
                    mode: mode.to_owned(),
                    oid: oid.to_owned(),
                });
            }
        }
        Ok(map)
    }

    fn incoming(&self) -> Option<Incoming> {
        let exists = |name: &str| self.git_dir.join(name).exists();
        let describe = |rev: &str| {
            self.git_text(&["log", "-1", "--format=%h %s", rev])
                .unwrap_or_else(|_| rev.to_owned())
        };
        if exists("MERGE_HEAD") {
            let label = self
                .merge_msg_branch()
                .or_else(|| {
                    self.git_text(&["name-rev", "--name-only", "--no-undefined", "MERGE_HEAD"])
                        .ok()
                })
                .unwrap_or_else(|| describe("MERGE_HEAD"));
            return Some(Incoming {
                operation: "merge",
                rev: Some("MERGE_HEAD"),
                label,
            });
        }
        if exists("CHERRY_PICK_HEAD") {
            return Some(Incoming {
                operation: "cherry-pick",
                rev: Some("CHERRY_PICK_HEAD"),
                label: describe("CHERRY_PICK_HEAD"),
            });
        }
        if exists("REVERT_HEAD") {
            return Some(Incoming {
                operation: "revert",
                rev: None,
                label: format!("revert of {}", describe("REVERT_HEAD")),
            });
        }
        if exists("REBASE_HEAD") {
            return Some(Incoming {
                operation: "rebase",
                rev: Some("REBASE_HEAD"),
                label: describe("REBASE_HEAD"),
            });
        }
        None
    }

    /// The branch name git wrote into `MERGE_MSG` ("Merge branch 'x' …").
    fn merge_msg_branch(&self) -> Option<String> {
        let msg = std::fs::read_to_string(self.git_dir.join("MERGE_MSG")).ok()?;
        let first = msg.lines().next()?;
        let rest = first.strip_prefix("Merge ")?;
        let start = rest.find('\'')? + 1;
        let len = rest[start..].find('\'')?;
        Some(rest[start..start + len].to_owned())
    }

    fn target_label(&self) -> String {
        self.git_text(&["symbolic-ref", "--short", "-q", "HEAD"])
            .ok()
            .filter(|name| !name.is_empty())
            .or_else(|| {
                self.git_text(&["rev-parse", "--short", "HEAD"])
                    .ok()
                    .map(|s| format!("HEAD ({s})"))
            })
            .unwrap_or_else(|| "HEAD".to_owned())
    }

    fn blob(&self, entry: &Option<StageEntry>, what: &str) -> GitResult<String> {
        match entry {
            None => Ok(String::new()),
            Some(entry) => decode_text(self.git(&["cat-file", "blob", &entry.oid])?, what),
        }
    }

    /// Where the resolution progress of `path` is kept between runs.
    fn progress_file(&self, path: &str) -> PathBuf {
        self.git_dir.join(PROGRESS_DIR).join(format!("{path}.toml"))
    }

    fn backup(&self, path: &str) -> GitResult<Option<PathBuf>> {
        if !self.options.backup {
            return Ok(None);
        }
        let worktree_file = self.root.join(path);
        let backup = self.git_dir.join("mcr-backup").join(path);
        let mut done = self.backed_up.lock().unwrap_or_else(|e| e.into_inner());
        if done.contains(path) {
            return Ok(Some(backup));
        }
        if !worktree_file.is_file() {
            return Ok(None);
        }
        if let Some(parent) = backup.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&worktree_file, &backup)?;
        done.insert(path.to_owned());
        Ok(Some(backup))
    }
}

impl MergeBackend for GitBackend {
    fn context(&self) -> MergeContext {
        let incoming = self.incoming();
        MergeContext {
            operation: incoming
                .as_ref()
                .map_or("merge", |i| i.operation)
                .to_owned(),
            source_label: incoming.map_or_else(|| "incoming".to_owned(), |i| i.label),
            target_label: self.target_label(),
        }
    }

    fn conflicts(&self) -> GitResult<Vec<ConflictEntry>> {
        let unmerged = self.stages(None)?;
        let in_progress = self.incoming().is_some();
        if unmerged.is_empty() && !in_progress {
            // No merge is under way any more: saved progress is obsolete.
            let _ = std::fs::remove_dir_all(self.git_dir.join(PROGRESS_DIR));
        }
        let mut entries: BTreeMap<String, ConflictEntry> = BTreeMap::new();
        if in_progress {
            // Paths resolved in an earlier run stay listed, so the work can
            // be reviewed or picked up again — but only our own: a path
            // the user resolved by other means has no progress file.
            for (path, _) in self.resolved_stages(None)? {
                if !unmerged.contains_key(&path) && self.progress_file(&path).is_file() {
                    let entry = ConflictEntry {
                        path: path.clone(),
                        unsupported: None,
                        resolved: true,
                    };
                    entries.insert(path, entry);
                }
            }
        }
        for (path, stages) in unmerged {
            let mode_is = |mode: &str| stages.iter().flatten().any(|s| s.mode == mode);
            let unsupported = if stages[1].is_none() || stages[2].is_none() {
                Some("deleted on one side — resolve with `git add` or `git rm`".to_owned())
            } else if mode_is(MODE_SYMLINK) {
                Some("symbolic link conflict".to_owned())
            } else if mode_is(MODE_SUBMODULE) {
                Some("submodule conflict".to_owned())
            } else {
                None
            };
            let entry = ConflictEntry {
                path: path.clone(),
                unsupported,
                resolved: false,
            };
            entries.insert(path, entry);
        }
        Ok(entries.into_values().collect())
    }

    fn load(&self, path: &str) -> GitResult<MergeInput> {
        let stages = match self.stages(Some(path))?.remove(path) {
            Some(stages) => stages,
            // Already staged: git keeps the three versions until the commit.
            None => self
                .resolved_stages(Some(path))?
                .remove(path)
                .ok_or_else(|| {
                    GitError::Unsupported(format!("{path} has no conflict in the index"))
                })?,
        };
        let context = self.context();
        Ok(MergeInput {
            base: self.blob(&stages[0], "the common ancestor")?,
            target: self.blob(&stages[1], "the target version")?,
            source: self.blob(&stages[2], "the source version")?,
            labels: Labels {
                source: context.source_label,
                target: context.target_label,
                base: "base".to_owned(),
            },
        })
    }

    fn save(&self, path: &str, content: &str, resolved: bool) -> GitResult<SaveOutcome> {
        let backup = self.backup(path)?;
        if !resolved && self.stages(Some(path))?.is_empty() {
            // The path was staged as resolved earlier (in this run or a
            // previous one); hand the conflict back to git before writing
            // markers again.
            self.git(&["update-index", "--unresolve", "--", path])?;
        }
        std::fs::write(self.root.join(path), content)?;
        let stage = resolved && self.options.auto_stage;
        if stage {
            self.git(&["add", "--", path])?;
        }
        Ok(SaveOutcome {
            staged: stage,
            backup,
        })
    }

    fn current_content(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(path)).ok()
    }

    fn load_progress(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.progress_file(path)).ok()
    }

    fn save_progress(&self, path: &str, progress: &str) -> GitResult<()> {
        let file = self.progress_file(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(file, progress)?;
        Ok(())
    }

    fn blame(&self, path: &str, side: Side) -> GitResult<Option<Blame>> {
        let rev = match side {
            Side::Target => Some("HEAD"),
            Side::Source => self.incoming().and_then(|i| i.rev),
        };
        let Some(rev) = rev else { return Ok(None) };
        let out = self.git(&["blame", "--porcelain", rev, "--", path])?;
        let blame = parse_porcelain(&String::from_utf8_lossy(&out));
        let commits = blame.commits();
        if commits.is_empty() {
            return Ok(Some(blame));
        }
        let log = run_in(
            &self.root,
            &[
                "log",
                "--no-walk=unsorted",
                "--stdin",
                "--format=%H%x00%b%x01",
            ],
            Some(commits.join("\n").as_bytes()),
        )?;
        Ok(Some(blame.with_bodies(&parse_bodies(
            &String::from_utf8_lossy(&log),
        ))))
    }
}

/// Run git in `dir`, optionally feeding `stdin`, and return its stdout.
fn run_in(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> GitResult<Vec<u8>> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .arg("--literal-pathspecs")
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(input)?;
    }
    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(GitError::Command {
            command: args.first().copied().unwrap_or_default().to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}
