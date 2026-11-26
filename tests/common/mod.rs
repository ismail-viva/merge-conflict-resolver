//! A throw-away git repository for integration tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

pub const BASE: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
/// Changes `two` (conflict) and `six` (source only).
pub const SOURCE: &str =
    "one\ntwo from feature\nextra from feature\nthree\nfour\nfive\nsix (feature)\nseven\n";
/// Changes `two` (conflict) and adds `zero` (target only).
pub const TARGET: &str = "zero\none\ntwo from main\nthree\nfour\nfive\nsix\nseven\n";

pub struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    /// An empty repository on branch `main`.
    pub fn new() -> Self {
        let repo = Self {
            dir: TempDir::new().expect("temp dir"),
        };
        repo.git(&["init", "-q", "-b", "main", "."]);
        repo
    }

    /// `main` and `feature` diverged on `file.txt` and `other.txt`, and
    /// `git merge feature` stopped with conflicts in both.
    pub fn conflicted() -> Self {
        let repo = Self::new();
        repo.write("file.txt", BASE);
        repo.write("other.txt", "x\n");
        repo.commit_all("base commit", "Ada Base");
        repo.git(&["checkout", "-q", "-b", "feature"]);
        repo.write("file.txt", SOURCE);
        repo.write("other.txt", "x source\n");
        repo.commit_all(
            "feature work\n\nA body line explaining the feature.",
            "Grace Feature",
        );
        repo.git(&["checkout", "-q", "main"]);
        repo.write("file.txt", TARGET);
        repo.write("other.txt", "x target\n");
        repo.commit_all("main work", "Linus Main");
        let (merged, _) = repo.try_git(&["merge", "feature"]);
        assert!(!merged, "the merge is expected to conflict");
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    pub fn write(&self, name: &str, content: &str) {
        let path = self.file(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(path, content).expect("write file");
    }

    pub fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.file(name)).expect("read file")
    }

    pub fn commit_all(&self, message: &str, author: &str) {
        self.git(&["add", "-A"]);
        let (ok, out) = self.run(&["commit", "-q", "-m", message], Some(author));
        assert!(ok, "commit failed: {out}");
    }

    /// Run git, panicking on failure; returns stdout.
    pub fn git(&self, args: &[&str]) -> String {
        let (ok, out) = self.try_git(args);
        assert!(ok, "git {args:?} failed: {out}");
        out
    }

    /// Run git; returns success and stdout (or stderr on failure).
    pub fn try_git(&self, args: &[&str]) -> (bool, String) {
        self.run(args, None)
    }

    fn run(&self, args: &[&str], author: Option<&str>) -> (bool, String) {
        let author = author.unwrap_or("Test User");
        let email = format!("{}@example.org", author.to_lowercase().replace(' ', "."));
        let output = Command::new("git")
            .arg("-C")
            .arg(self.dir.path())
            .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", author)
            .env("GIT_AUTHOR_EMAIL", &email)
            .env("GIT_COMMITTER_NAME", author)
            .env("GIT_COMMITTER_EMAIL", &email)
            .env("GIT_EDITOR", "true")
            .output()
            .expect("git is installed");
        let text = if output.status.success() {
            &output.stdout
        } else {
            &output.stderr
        };
        (
            output.status.success(),
            String::from_utf8_lossy(text).into_owned(),
        )
    }

    /// Paths with unmerged index entries.
    pub fn unmerged(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .git(&["diff", "--name-only", "--diff-filter=U"])
            .lines()
            .map(str::to_owned)
            .collect();
        paths.dedup();
        paths
    }
}
