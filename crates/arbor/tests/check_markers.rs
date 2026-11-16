use std::fs;
use std::path::Path;
use std::process::{Command, Output};

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

fn check_markers(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_arbor"))
        .arg("check-markers")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn project() -> TempDir {
    let dir = tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "--quiet", "-b", "main"]);
    git(root, &["config", "user.name", "Test Operator"]);
    git(root, &["config", "user.email", "operator@example.com"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "core.autocrlf", "false"]);
    fs::write(root.join("lib.rs"), "fn one() {}\n").unwrap();
    git(root, &["add", "--all"]);
    git(root, &["commit", "--quiet", "-m", "Start"]);
    dir
}

const MARKED: &str = "fn two() {\n    // ARBOR:BEGIN WU-2\n    todo!()\n    // ARBOR:END WU-2\n}\n";

#[test]
fn a_clean_tree_passes_silently() {
    let dir = project();
    let checked = check_markers(dir.path(), &[]);
    assert!(checked.status.success(), "{checked:?}");
    assert!(checked.stdout.is_empty());
}

#[test]
fn markers_are_listed_and_fail_the_check() {
    let dir = project();
    fs::write(dir.path().join("two.rs"), MARKED).unwrap();
    git(dir.path(), &["add", "two.rs"]);

    let checked = check_markers(dir.path(), &["--staged"]);

    assert_eq!(checked.status.code(), Some(1), "{checked:?}");
    assert_eq!(
        String::from_utf8_lossy(&checked.stdout),
        "two.rs:2: // ARBOR:BEGIN WU-2\ntwo.rs:4: // ARBOR:END WU-2\n"
    );
    assert!(String::from_utf8_lossy(&checked.stderr).contains("found 2 Marker lines"));
}

#[test]
fn a_revision_is_checked_by_its_tree() {
    let dir = project();
    fs::write(dir.path().join("two.rs"), MARKED).unwrap();
    git(dir.path(), &["add", "two.rs"]);
    git(dir.path(), &["commit", "--quiet", "-m", "Add two"]);

    assert_eq!(
        check_markers(dir.path(), &["--rev", "HEAD"]).status.code(),
        Some(1)
    );
    assert!(
        check_markers(dir.path(), &["--rev", "HEAD~1"])
            .status
            .success()
    );
}

#[test]
fn a_check_that_cannot_run_fails_with_status_2() {
    let dir = project();
    let checked = check_markers(dir.path(), &["--rev", "no-such-branch"]);
    assert_eq!(checked.status.code(), Some(2), "{checked:?}");

    let outside = tempdir().unwrap();
    assert_eq!(check_markers(outside.path(), &[]).status.code(), Some(2));
}

#[test]
fn an_allowlist_that_allows_everything_is_refused_with_status_2() {
    let dir = project();
    fs::write(dir.path().join(".arbor-markers-allowed"), "*\n").unwrap();

    let checked = check_markers(dir.path(), &[]);

    assert_eq!(checked.status.code(), Some(2), "{checked:?}");
    let stderr = String::from_utf8_lossy(&checked.stderr);
    assert!(
        stderr.contains("would allow Markers everywhere"),
        "{stderr}"
    );
}

#[test]
fn markers_in_allowed_paths_pass() {
    let dir = project();
    fs::create_dir_all(dir.path().join("tests")).unwrap();
    fs::write(dir.path().join("tests/two.rs"), MARKED).unwrap();
    fs::write(dir.path().join(".arbor-markers-allowed"), "tests/\n").unwrap();
    git(dir.path(), &["add", "--all"]);

    assert!(check_markers(dir.path(), &["--staged"]).status.success());
}

#[test]
fn a_session_worktree_may_commit_markers_when_the_hook_allows_it() {
    let dir = project();
    let worktree = dir.path().join(".arbor/sessions/1-two/worktree");
    git(
        dir.path(),
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "arbor/two",
            worktree.to_str().unwrap(),
        ],
    );
    fs::write(worktree.join("two.rs"), MARKED).unwrap();
    git(&worktree, &["add", "two.rs"]);

    let allowed = check_markers(&worktree, &["--staged", "--allow-session-worktree"]);
    assert!(allowed.status.success(), "{allowed:?}");
    assert_eq!(
        check_markers(&worktree, &["--staged"]).status.code(),
        Some(1)
    );
}

#[test]
fn the_allowance_needs_both_a_session_branch_and_a_sessions_folder() {
    let dir = project();
    git(dir.path(), &["switch", "--quiet", "-c", "arbor/two"]);
    fs::write(dir.path().join("two.rs"), MARKED).unwrap();
    git(dir.path(), &["add", "two.rs"]);
    let on_branch_only = check_markers(dir.path(), &["--staged", "--allow-session-worktree"]);
    assert_eq!(on_branch_only.status.code(), Some(1));

    let worktree = dir.path().join(".arbor/sessions/1-two/worktree");
    git(
        dir.path(),
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feature",
            worktree.to_str().unwrap(),
        ],
    );
    fs::write(worktree.join("two.rs"), MARKED).unwrap();
    git(&worktree, &["add", "two.rs"]);
    let in_folder_only = check_markers(&worktree, &["--staged", "--allow-session-worktree"]);
    assert_eq!(in_folder_only.status.code(), Some(1));
}
