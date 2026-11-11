mod common;

use std::fs;
use std::path::{Path, PathBuf};

use arbor_git::{Repo, SessionBranch, Worktree};
use common::{Cloned, Local, commit_file, git, write};
use tempfile::{TempDir, tempdir};

const BINARY: &[u8] = &[0, 159, 146, 150, 255, 0, 1, 2, 13, 10, 0];

/// Everything about the operator's checkout that bringing changes in must leave alone.
#[derive(Debug, PartialEq)]
struct Snapshot {
    index: Vec<u8>,
    head: String,
    stashes: String,
    files: Vec<(PathBuf, Vec<u8>)>,
}

fn snapshot(dir: &Path) -> Snapshot {
    let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    let mut files = Vec::new();
    collect(dir, dir, &mut files);
    files.sort();
    Snapshot {
        index: fs::read(git_dir.join("index")).unwrap(),
        head: git(dir, &["rev-parse", "HEAD"]),
        stashes: fs::read_to_string(git_dir.join("logs/refs/stash")).unwrap_or_default(),
        files,
    }
}

fn collect(root: &Path, dir: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap() == ".git" {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            let relative = path.strip_prefix(root).unwrap().to_owned();
            files.push((relative, fs::read(&path).unwrap()));
        }
    }
}

/// Brings the checkout's changes into a new worktree on `arbor/work`.
fn bring_in(repo_dir: &Path) -> (TempDir, Result<Worktree, arbor_git::GitError>) {
    let repo = Repo::open(repo_dir).unwrap();
    repo.fetch().unwrap();
    let mainline = repo.mainline(Some("main")).unwrap();
    let place = tempdir().unwrap();
    let branch = SessionBranch::for_feature("work").unwrap();
    let worktree_result =
        repo.create_worktree_with_changes(&mainline, &branch, &place.path().join("session"));
    (place, worktree_result)
}

fn read(dir: &Path, path: &str) -> String {
    fs::read_to_string(dir.join(path)).unwrap()
}

#[test]
fn a_clean_tree_has_no_changes() {
    let local = Local::new();
    let repo = Repo::open(local.path()).unwrap();
    assert!(repo.uncommitted_changes().unwrap().is_empty());
}

#[test]
fn a_dirty_tree_lists_modified_and_untracked_files() {
    let local = Local::new();
    write(local.path(), "README.md", "edited\n");
    write(local.path(), "notes/new.txt", "new\n");
    let repo = Repo::open(local.path()).unwrap();
    assert_eq!(
        repo.uncommitted_changes().unwrap(),
        ["README.md", "notes/new.txt"]
    );
}

#[test]
fn every_kind_of_change_is_brought_in_as_the_first_commit() {
    let local = Local::new();
    let dir = local.path();
    commit_file(dir, "gone.txt", "gone\n", "Add gone");
    commit_file(dir, "old-name.txt", "a\nb\nc\nd\ne\n", "Add old name");
    fs::write(dir.join("image.bin"), BINARY).unwrap();
    write(dir, ".gitignore", "*.log\n");
    commit_file(dir, "staged.txt", "one\n", "Add staged");
    // Modified, staged then edited again, deleted, renamed, untracked, binary, and ignored.
    write(dir, "README.md", "edited\n");
    write(dir, "staged.txt", "two\n");
    git(dir, &["add", "staged.txt"]);
    write(dir, "staged.txt", "three\n");
    fs::remove_file(dir.join("gone.txt")).unwrap();
    git(dir, &["mv", "old-name.txt", "new-name.txt"]);
    write(dir, "notes/untracked.txt", "untracked\n");
    let mut binary = BINARY.to_vec();
    binary.push(7);
    fs::write(dir.join("image.bin"), &binary).unwrap();
    fs::write(dir.join("new.bin"), BINARY).unwrap();
    write(dir, "debug.log", "ignored\n");
    let main = git(dir, &["rev-parse", "main"]);
    let before = snapshot(dir);

    let (_place, worktree) = bring_in(dir);
    let worktree = worktree.unwrap();
    let wt = worktree.path();

    assert_eq!(
        snapshot(dir),
        before,
        "the operator's checkout must be untouched"
    );
    assert_eq!(git(dir, &["rev-parse", "main"]), main);
    assert_eq!(git(wt, &["rev-parse", "HEAD~1"]), main);
    assert_eq!(
        git(wt, &["log", "-1", "--format=%B%n%an <%ae>"]),
        "Include uncommitted changes from the working tree.\nTest Operator <operator@example.com>"
    );
    assert_eq!(read(wt, "README.md"), "edited\n");
    assert_eq!(read(wt, "staged.txt"), "three\n");
    assert!(!wt.join("gone.txt").exists());
    assert!(!wt.join("old-name.txt").exists());
    assert_eq!(read(wt, "new-name.txt"), "a\nb\nc\nd\ne\n");
    assert_eq!(read(wt, "notes/untracked.txt"), "untracked\n");
    assert_eq!(fs::read(wt.join("image.bin")).unwrap(), binary);
    assert_eq!(fs::read(wt.join("new.bin")).unwrap(), BINARY);
    assert!(!wt.join("debug.log").exists());
    assert!(git(wt, &["status", "--porcelain"]).is_empty());
}

#[test]
fn rewind_can_return_to_the_brought_in_commit() {
    let local = Local::new();
    write(local.path(), "README.md", "edited\n");
    let (_place, worktree) = bring_in(local.path());
    let worktree = worktree.unwrap();
    let start = worktree.head().unwrap();
    write(worktree.path(), "README.md", "later\n");
    worktree
        .commit(&arbor_git::CommitMessage::new("Later.").unwrap())
        .unwrap();

    worktree.reset_to(&start).unwrap();

    assert_eq!(read(worktree.path(), "README.md"), "edited\n");
}

#[test]
fn a_clean_tree_brings_in_nothing_and_starts_from_the_mainline() {
    let local = Local::new();
    let main = git(local.path(), &["rev-parse", "main"]);
    let (_place, worktree) = bring_in(local.path());
    assert_eq!(worktree.unwrap().head().unwrap().as_str(), main);
}

#[test]
fn only_uncommitted_changes_come_from_a_checkout_on_another_branch() {
    let local = Local::new();
    let dir = local.path();
    git(dir, &["switch", "--quiet", "-c", "feature"]);
    commit_file(dir, "committed.txt", "on feature\n", "Feature commit");
    write(dir, "README.md", "edited on feature\n");

    let (_place, worktree) = bring_in(dir);
    let worktree = worktree.unwrap();

    assert_eq!(read(worktree.path(), "README.md"), "edited on feature\n");
    assert!(!worktree.path().join("committed.txt").exists());
    assert_eq!(git(dir, &["branch", "--show-current"]), "feature");
}

#[test]
fn changes_made_on_a_checkout_behind_origin_apply_onto_the_newer_mainline() {
    let cloned = Cloned::new();
    let upstream = cloned.advance_origin("upstream.txt");
    write(&cloned.clone, "README.md", "edited locally\n");
    write(&cloned.clone, "local.txt", "local\n");

    let (_place, worktree) = bring_in(&cloned.clone);
    let worktree = worktree.unwrap();

    assert_eq!(git(worktree.path(), &["rev-parse", "HEAD~1"]), upstream);
    assert!(worktree.path().join("upstream.txt").exists());
    assert_eq!(read(worktree.path(), "README.md"), "edited locally\n");
    assert_eq!(read(worktree.path(), "local.txt"), "local\n");
}

#[test]
fn changes_that_conflict_with_the_mainline_are_refused_and_create_nothing() {
    let cloned = Cloned::new();
    cloned.push_to_origin("README.md", "changed upstream\n");
    write(&cloned.clone, "README.md", "changed locally\n");
    let before = snapshot(&cloned.clone);

    let (place, worktree_result) = bring_in(&cloned.clone);

    let err = worktree_result.unwrap_err().to_string();
    assert!(err.contains("README.md"), "{err}");
    assert!(err.contains("Leave them out"), "{err}");
    assert!(!place.path().join("session").exists());
    let branches = git(&cloned.clone, &["branch", "--list", "arbor/*"]);
    assert!(branches.is_empty(), "{branches}");
    assert_eq!(snapshot(&cloned.clone), before);
}

#[test]
fn leaving_changes_out_starts_clean_from_the_mainline() {
    let local = Local::new();
    write(local.path(), "README.md", "edited\n");
    let repo = Repo::open(local.path()).unwrap();
    let mainline = repo.mainline(None).unwrap();
    let place = tempdir().unwrap();
    let branch = SessionBranch::for_feature("work").unwrap();
    let worktree = repo
        .create_worktree(&mainline, &branch, &place.path().join("session"))
        .unwrap();
    assert_eq!(read(worktree.path(), "README.md"), "hello\n");
    assert_eq!(read(local.path(), "README.md"), "edited\n");
}
