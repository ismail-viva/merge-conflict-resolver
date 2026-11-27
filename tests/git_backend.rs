//! The git layer against real repositories.

mod common;

use common::{TestRepo, BASE, SOURCE, TARGET};
use mcr::core::{MergeSession, Op, Side};
use mcr::git::repo::GitOptions;
use mcr::git::{FileBackend, GitBackend, GitError, MergeBackend};

fn backend(repo: &TestRepo) -> GitBackend {
    GitBackend::discover(repo.path(), GitOptions::default()).expect("repository")
}

#[test]
fn lists_conflicts_and_describes_the_merge() {
    let repo = TestRepo::conflicted();
    let git = backend(&repo);
    let conflicts = git.conflicts().unwrap();
    let paths: Vec<&str> = conflicts.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["file.txt", "other.txt"]);
    assert!(conflicts.iter().all(|c| c.unsupported.is_none()));

    let context = git.context();
    assert_eq!(context.operation, "merge");
    assert_eq!(context.source_label, "feature");
    assert_eq!(context.target_label, "main");
}

#[test]
fn loads_the_three_versions_from_the_index() {
    let repo = TestRepo::conflicted();
    let input = backend(&repo).load("file.txt").unwrap();
    assert_eq!(input.base, BASE);
    assert_eq!(input.source, SOURCE);
    assert_eq!(input.target, TARGET);
    assert_eq!(input.labels.source, "feature");
    assert_eq!(input.labels.target, "main");
}

#[test]
fn works_from_a_subdirectory() {
    let repo = TestRepo::conflicted();
    std::fs::create_dir(repo.file("sub")).unwrap();
    let git = GitBackend::discover(&repo.file("sub"), GitOptions::default()).unwrap();
    assert_eq!(git.conflicts().unwrap().len(), 2);
    assert_eq!(git.load("file.txt").unwrap().base, BASE);
}

#[test]
fn outside_a_repository_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let result = GitBackend::discover(dir.path(), GitOptions::default());
    assert!(matches!(result, Err(GitError::NotARepository)));
}

#[test]
fn unresolved_save_keeps_the_conflict_and_backs_up_the_worktree() {
    let repo = TestRepo::conflicted();
    let git = backend(&repo);
    let before = repo.read("file.txt");
    let session = MergeSession::new(git.load("file.txt").unwrap());
    let output = session.output();
    assert_eq!(output.unresolved, 1);

    let outcome = git.save("file.txt", &output.text, false).unwrap();
    assert!(!outcome.staged);
    assert_eq!(repo.unmerged(), ["file.txt", "other.txt"]);
    let written = repo.read("file.txt");
    assert!(written.contains("<<<<<<< main\ntwo from main\n||||||| base\ntwo\n=======\n"));
    assert!(written.contains("extra from feature\n>>>>>>> feature\n"));
    // Non-conflicting changes of both sides are merged around the markers.
    assert!(written.starts_with("zero\none\n") && written.contains("six (feature)\n"));

    let backup = outcome.backup.expect("backup path");
    assert_eq!(std::fs::read_to_string(backup).unwrap(), before);
}

#[test]
fn resolved_save_stages_and_the_merge_can_be_committed() {
    let repo = TestRepo::conflicted();
    let git = backend(&repo);
    for (path, side) in [("file.txt", Side::Source), ("other.txt", Side::Target)] {
        let mut session = MergeSession::new(git.load(path).unwrap());
        session.apply(Op::TakeSideEverywhere { side }).unwrap();
        let output = session.output();
        assert_eq!(output.unresolved, 0);
        assert!(git.save(path, &output.text, true).unwrap().staged);
    }
    assert!(repo.unmerged().is_empty());
    assert!(git.conflicts().unwrap().is_empty());
    assert_eq!(
        repo.read("file.txt"),
        "zero\none\ntwo from feature\nextra from feature\nthree\nfour\nfive\nsix (feature)\nseven\n"
    );
    assert_eq!(repo.read("other.txt"), "x target\n");

    repo.git(&["commit", "-q", "--no-edit"]);
    let parents = repo.git(&["log", "-1", "--format=%P"]);
    assert_eq!(parents.split_whitespace().count(), 2, "a real merge commit");
    assert_eq!(repo.git(&["status", "--porcelain"]).trim(), "");
    assert_eq!(repo.git(&["show", "HEAD:other.txt"]), "x target\n");
}

#[test]
fn saving_unresolved_after_staging_restores_the_conflict() {
    let repo = TestRepo::conflicted();
    let git = backend(&repo);
    let mut session = MergeSession::new(git.load("other.txt").unwrap());
    session
        .apply(Op::TakeSideEverywhere { side: Side::Source })
        .unwrap();
    git.save("other.txt", &session.output().text, true).unwrap();
    assert_eq!(repo.unmerged(), ["file.txt"]);

    session.undo().unwrap();
    let output = session.output();
    assert_eq!(output.unresolved, 1);
    git.save("other.txt", &output.text, false).unwrap();
    assert_eq!(
        repo.unmerged(),
        ["file.txt", "other.txt"],
        "git must see the conflict again"
    );
    assert!(repo.read("other.txt").contains("<<<<<<< main"));
    // …and all three versions are still there to resolve from.
    let input = git.load("other.txt").unwrap();
    assert_eq!(
        (input.base.as_str(), input.source.as_str()),
        ("x\n", "x source\n")
    );
}

#[test]
fn auto_stage_can_be_turned_off() {
    let repo = TestRepo::conflicted();
    let options = GitOptions {
        auto_stage: false,
        backup: false,
    };
    let git = GitBackend::discover(repo.path(), options).unwrap();
    let outcome = git.save("other.txt", "x resolved\n", true).unwrap();
    assert!(!outcome.staged);
    assert_eq!(outcome.backup, None);
    assert_eq!(repo.read("other.txt"), "x resolved\n");
    assert_eq!(repo.unmerged(), ["file.txt", "other.txt"]);
}

#[test]
fn blame_reports_the_last_commit_of_every_line() {
    let repo = TestRepo::conflicted();
    let git = backend(&repo);

    let source = git
        .blame("file.txt", Side::Source)
        .unwrap()
        .expect("source blame");
    assert_eq!(source.len(), SOURCE.lines().count());
    assert_eq!(source.line(0).unwrap().author, "Ada Base");
    let changed = source.line(1).unwrap();
    assert_eq!(changed.author, "Grace Feature");
    assert_eq!(changed.author_mail, "grace.feature@example.org");
    assert_eq!(changed.summary, "feature work");
    assert_eq!(changed.body, "A body line explaining the feature.");
    assert!(changed.author_time > 1_600_000_000);
    assert_eq!(changed.sha.len(), 40);

    let target = git
        .blame("file.txt", Side::Target)
        .unwrap()
        .expect("target blame");
    assert_eq!(target.len(), TARGET.lines().count());
    assert_eq!(target.line(0).unwrap().author, "Linus Main");
    assert_eq!(target.line(0).unwrap().summary, "main work");
    assert_eq!(target.line(1).unwrap().author, "Ada Base");
}

#[test]
fn delete_modify_and_binary_conflicts_are_not_line_merged() {
    let repo = TestRepo::new();
    repo.write("gone.txt", "keep\n");
    std::fs::write(repo.file("blob.bin"), b"\x00\x01base").unwrap();
    repo.commit_all("base", "Ada Base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("gone.txt", "changed\n");
    std::fs::write(repo.file("blob.bin"), b"\x00\x01feature").unwrap();
    repo.commit_all("feature", "Grace Feature");
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["rm", "-q", "gone.txt"]);
    std::fs::write(repo.file("blob.bin"), b"\x00\x01main").unwrap();
    repo.commit_all("main", "Linus Main");
    assert!(!repo.try_git(&["merge", "feature"]).0);

    let git = backend(&repo);
    let conflicts = git.conflicts().unwrap();
    let gone = conflicts.iter().find(|c| c.path == "gone.txt").unwrap();
    assert!(gone
        .unsupported
        .as_deref()
        .unwrap()
        .contains("deleted on one side"));
    assert!(
        matches!(git.load("blob.bin"), Err(GitError::Unsupported(msg)) if msg.contains("binary"))
    );
}

#[test]
fn add_add_conflicts_have_an_empty_base() {
    let repo = TestRepo::new();
    repo.write("readme", "hi\n");
    repo.commit_all("base", "Ada Base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("new.txt", "shared\nfrom feature\n");
    repo.commit_all("feature", "Grace Feature");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("new.txt", "shared\nfrom main\n");
    repo.commit_all("main", "Linus Main");
    assert!(!repo.try_git(&["merge", "feature"]).0);

    let input = backend(&repo).load("new.txt").unwrap();
    assert_eq!(input.base, "");
    let session = MergeSession::new(input);
    assert_eq!(session.stats().open, 1);
    assert!(session
        .output()
        .text
        .starts_with("shared\n<<<<<<< main\nfrom main\n"));
}

#[test]
fn unusual_file_names_are_handled_literally() {
    let repo = TestRepo::new();
    let name = "dir with space/we[i]rd *name?.txt";
    repo.write(name, "a\n");
    repo.write("dir with space/weird name1.txt", "decoy\n");
    repo.commit_all("base", "Ada Base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write(name, "a feature\n");
    repo.commit_all("feature", "Grace Feature");
    repo.git(&["checkout", "-q", "main"]);
    repo.write(name, "a main\n");
    repo.commit_all("main", "Linus Main");
    assert!(!repo.try_git(&["merge", "feature"]).0);

    let git = backend(&repo);
    assert_eq!(git.conflicts().unwrap()[0].path, name);
    assert_eq!(git.load(name).unwrap().source, "a feature\n");
    assert_eq!(git.blame(name, Side::Source).unwrap().unwrap().len(), 1);
    assert!(git.save(name, "a merged\n", true).unwrap().staged);
    assert!(repo.unmerged().is_empty());
    assert_eq!(repo.git(&["show", &format!(":{name}")]), "a merged\n");
}

#[test]
fn rebase_conflicts_are_described_and_blamed() {
    let repo = TestRepo::new();
    repo.write("f.txt", "line\n");
    repo.commit_all("base", "Ada Base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("f.txt", "line feature\n");
    repo.commit_all("feature change", "Grace Feature");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("f.txt", "line main\n");
    repo.commit_all("main change", "Linus Main");
    repo.git(&["checkout", "-q", "feature"]);
    assert!(!repo.try_git(&["rebase", "main"]).0);

    let git = backend(&repo);
    let context = git.context();
    assert_eq!(context.operation, "rebase");
    assert!(
        context.source_label.ends_with("feature change"),
        "{context:?}"
    );
    // While rebasing, "ours" is the branch being rebased onto.
    let input = git.load("f.txt").unwrap();
    assert_eq!(
        (input.target.as_str(), input.source.as_str()),
        ("line main\n", "line feature\n")
    );
    let blame = git.blame("f.txt", Side::Source).unwrap().unwrap();
    assert_eq!(blame.line(0).unwrap().author, "Grace Feature");

    assert!(git.save("f.txt", "line both\n", true).unwrap().staged);
    assert!(repo.try_git(&["rebase", "--continue"]).0);
    assert_eq!(repo.git(&["show", "HEAD:f.txt"]), "line both\n");
}

#[test]
fn file_backend_merges_plain_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| dir.path().join(name);
    std::fs::write(path("base"), "a\n").unwrap();
    std::fs::write(path("local"), "a local\n").unwrap();
    std::fs::write(path("remote"), "a remote\n").unwrap();
    let files = FileBackend::new(
        Some(path("base")),
        path("local"),
        path("remote"),
        path("merged"),
    );

    let conflicts = files.conflicts().unwrap();
    assert_eq!(conflicts.len(), 1);
    let input = files.load(&conflicts[0].path).unwrap();
    assert_eq!(
        (input.target.as_str(), input.source.as_str()),
        ("a local\n", "a remote\n")
    );
    assert!(files
        .blame(&conflicts[0].path, Side::Source)
        .unwrap()
        .is_none());

    let mut session = MergeSession::new(input);
    session
        .apply(Op::TakeBoth {
            hunk: 0,
            first: Side::Target,
        })
        .unwrap();
    files
        .save(&conflicts[0].path, &session.output().text, true)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(path("merged")).unwrap(),
        "a local\na remote\n"
    );
}

#[test]
fn progress_survives_a_restart_and_resolved_files_stay_listed() {
    let repo = TestRepo::conflicted();
    {
        let git = backend(&repo);
        let mut session = MergeSession::new(git.load("other.txt").unwrap());
        session
            .apply(Op::TakeSideEverywhere { side: Side::Source })
            .unwrap();
        git.save("other.txt", &session.output().text, true).unwrap();
        git.save_progress("other.txt", &session.snapshot().to_toml())
            .unwrap();
    }

    // A new process: the resolved file is still listed and loadable.
    let git = backend(&repo);
    let conflicts = git.conflicts().unwrap();
    let state: Vec<(&str, bool)> = conflicts
        .iter()
        .map(|c| (c.path.as_str(), c.resolved))
        .collect();
    assert_eq!(state, [("file.txt", false), ("other.txt", true)]);
    assert_eq!(
        repo.unmerged(),
        ["file.txt"],
        "git still sees it as resolved"
    );

    let mut session = MergeSession::new(git.load("other.txt").unwrap());
    let snapshot =
        mcr::core::Snapshot::from_toml(&git.load_progress("other.txt").unwrap()).unwrap();
    assert!(session.restore(&snapshot));
    assert_eq!(session.output().text, "x source\n");
    assert_eq!(git.load_progress("file.txt"), None);

    // Reopening the decision hands the conflict back to git.
    session.apply(Op::Reset { hunk: 0 }).unwrap();
    git.save("other.txt", &session.output().text, false)
        .unwrap();
    assert_eq!(repo.unmerged(), ["file.txt", "other.txt"]);
}

#[test]
fn files_resolved_without_mcr_are_not_listed_and_progress_is_cleaned_up() {
    let repo = TestRepo::conflicted();
    let git = backend(&repo);
    // Resolved by hand with plain git: none of our business.
    repo.write("other.txt", "x by hand\n");
    repo.git(&["add", "other.txt"]);
    let paths: Vec<String> = git
        .conflicts()
        .unwrap()
        .into_iter()
        .map(|c| c.path)
        .collect();
    assert_eq!(paths, ["file.txt"]);

    git.save("file.txt", "all merged\n", true).unwrap();
    git.save_progress("file.txt", "version = 1").unwrap();
    assert_eq!(
        git.conflicts().unwrap().len(),
        1,
        "listed while the merge is open"
    );
    repo.git(&["commit", "-q", "--no-edit"]);
    assert!(
        git.conflicts().unwrap().is_empty(),
        "nothing is listed after the commit"
    );
    assert_eq!(
        git.load_progress("file.txt"),
        None,
        "stale progress is removed"
    );
}
