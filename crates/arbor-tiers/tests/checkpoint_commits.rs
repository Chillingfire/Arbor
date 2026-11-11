use std::fs;
use std::path::Path;
use std::process::Command;

use arbor_core::{Event, Task};
use arbor_git::{Mainline, Repo, SessionBranch, Worktree};
use arbor_tiers::{commit_revision, commit_skeleton, discard_revision};
use tempfile::{TempDir, tempdir};

fn git(dir: &Path, args: &[&str]) -> String {
    let finished = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        finished.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&finished.stderr)
    );
    String::from_utf8_lossy(&finished.stdout).trim().to_owned()
}

/// A Session worktree on a repository whose `main` holds `src/lib.rs`.
struct Session {
    _dir: TempDir,
    _mainline: Mainline,
    worktree: Worktree,
}

impl Session {
    fn new() -> Self {
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join("src")).unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"]);
        git(&repo, &["config", "user.name", "Test Operator"]);
        git(&repo, &["config", "user.email", "operator@example.com"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        git(&repo, &["config", "core.autocrlf", "false"]);
        fs::write(repo.join("src/lib.rs"), "pub fn one() -> u32 {\n    1\n}\n").unwrap();
        git(&repo, &["add", "--all"]);
        git(&repo, &["commit", "--quiet", "-m", "Start"]);
        let opened = Repo::open(&repo).unwrap();
        let mainline = opened.mainline(None).unwrap();
        let branch = SessionBranch::for_feature("work").unwrap();
        let worktree = opened
            .create_worktree(&mainline, &branch, &dir.path().join("session"))
            .unwrap();
        Self {
            _dir: dir,
            _mainline: mainline,
            worktree,
        }
    }

    fn write(&self, path: &str, text: &str) {
        fs::write(self.worktree.path().join(path), text).unwrap();
    }

    fn git(&self, args: &[&str]) -> String {
        git(self.worktree.path(), args)
    }
}

fn task(title: &str) -> Task {
    Task {
        title: title.into(),
        description: "Parse the config file".into(),
    }
}

const SKELETON: &str =
    "pub fn parse() -> u32 {\n    // ARBOR:BEGIN WU-1\n    todo!()\n    // ARBOR:END WU-1\n}\n";

#[test]
fn the_approved_skeleton_is_committed_with_the_task_title() {
    let session = Session::new();
    session.write("src/parse.rs", SKELETON);

    let event = commit_skeleton(&session.worktree, &task("Add config parsing.")).unwrap();

    let head = session.git(&["rev-parse", "HEAD"]);
    assert_eq!(event, Event::SkeletonCommitted { commit: Some(head) });
    assert_eq!(
        session.git(&["log", "-1", "--format=%s"]),
        "Add config parsing (structure and tests)"
    );
    assert_eq!(session.git(&["status", "--porcelain"]), "");
}

#[test]
fn a_skeleton_that_changes_nothing_makes_no_commit() {
    let session = Session::new();
    let before = session.git(&["rev-parse", "HEAD"]);

    let event = commit_skeleton(&session.worktree, &task("Add config parsing")).unwrap();

    assert_eq!(event, Event::SkeletonCommitted { commit: None });
    assert_eq!(session.git(&["rev-parse", "HEAD"]), before);
}

fn read(session: &Session, path: &str) -> String {
    fs::read_to_string(session.worktree.path().join(path)).unwrap()
}

#[test]
fn an_accepted_revision_is_committed() {
    let session = Session::new();
    session.write("src/parse.rs", SKELETON);
    commit_skeleton(&session.worktree, &task("Add config parsing")).unwrap();
    session.write("src/parse.rs", &SKELETON.replace("u32", "u64"));

    let event = commit_revision(&session.worktree, &task("Add config parsing")).unwrap();

    let head = session.git(&["rev-parse", "HEAD"]);
    assert_eq!(event, Event::RevisionCommitted { commit: Some(head) });
    assert_eq!(
        session.git(&["log", "-1", "--format=%s"]),
        "Add config parsing (revised structure and tests)"
    );
}

#[test]
fn a_rejected_revision_is_discarded_back_to_the_last_accepted_state() {
    let session = Session::new();
    session.write("src/parse.rs", SKELETON);
    commit_skeleton(&session.worktree, &task("Add config parsing")).unwrap();
    let accepted = session.git(&["rev-parse", "HEAD"]);
    // The rejected Revision re-marked a passing Region of src/lib.rs and added a file.
    session.write(
        "src/lib.rs",
        "pub fn one() -> u32 {
    // ARBOR:BEGIN WU-2
    todo!()
    // ARBOR:END WU-2
}
",
    );
    session.write(
        "src/extra.rs",
        "pub fn extra() {}
",
    );
    fs::remove_file(session.worktree.path().join("src/parse.rs")).unwrap();

    let event = discard_revision(&session.worktree).unwrap();

    assert_eq!(event, Event::RevisionDiscarded);
    assert_eq!(session.git(&["rev-parse", "HEAD"]), accepted);
    assert_eq!(
        read(&session, "src/lib.rs"),
        "pub fn one() -> u32 {
    1
}
"
    );
    assert_eq!(read(&session, "src/parse.rs"), SKELETON);
    assert!(!session.worktree.path().join("src/extra.rs").exists());
}
