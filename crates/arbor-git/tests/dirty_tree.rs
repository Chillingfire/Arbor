mod common;

use arbor_git::{CommitMessage, DirtyTreeAction, DirtyTreeOutcome, Repo};
use common::{Local, git, write};

fn dirty() -> (Local, Repo) {
    let local = Local::new();
    write(local.path(), "README.md", "edited\n");
    write(local.path(), "notes/new.txt", "new\n");
    let repo = Repo::open(local.path()).unwrap();
    (local, repo)
}

#[test]
fn a_clean_tree_has_no_changes() {
    let local = Local::new();
    let repo = Repo::open(local.path()).unwrap();
    assert!(repo.uncommitted_changes().unwrap().is_empty());
}

#[test]
fn a_dirty_tree_lists_modified_and_untracked_files() {
    let (_local, repo) = dirty();
    assert_eq!(
        repo.uncommitted_changes().unwrap(),
        ["README.md", "notes/new.txt"]
    );
}

#[test]
fn stash_sets_the_changes_aside_and_proceeds() {
    let (local, repo) = dirty();
    let mainline = repo.mainline(None).unwrap();
    let outcome = repo
        .resolve_dirty_tree(DirtyTreeAction::Stash, &mainline)
        .unwrap();
    assert_eq!(outcome, DirtyTreeOutcome::Proceed);
    assert!(repo.uncommitted_changes().unwrap().is_empty());
    let stashes = git(local.path(), &["stash", "list"]);
    assert_eq!(stashes.lines().count(), 1, "{stashes}");
    assert!(!stashes.to_lowercase().contains("arbor"), "{stashes}");
}

#[test]
fn commit_commits_the_changes_on_the_current_branch_and_proceeds() {
    let (local, repo) = dirty();
    git(local.path(), &["switch", "--quiet", "-c", "feature"]);
    let main = git(local.path(), &["rev-parse", "main"]);
    let mainline = repo.mainline(None).unwrap();
    let message = CommitMessage::new("Save work in progress.").unwrap();

    let outcome = repo
        .resolve_dirty_tree(DirtyTreeAction::Commit(message), &mainline)
        .unwrap();

    assert_eq!(outcome, DirtyTreeOutcome::Proceed);
    assert!(repo.uncommitted_changes().unwrap().is_empty());
    assert_eq!(
        git(local.path(), &["log", "-1", "--format=%B", "feature"]),
        "Save work in progress."
    );
    assert_eq!(git(local.path(), &["rev-parse", "main"]), main);
}

#[test]
fn commit_is_refused_on_the_local_mainline() {
    let (local, repo) = dirty();
    let main = git(local.path(), &["rev-parse", "main"]);
    let mainline = repo.mainline(None).unwrap();
    let message = CommitMessage::new("Save work in progress.").unwrap();

    let result = repo.resolve_dirty_tree(DirtyTreeAction::Commit(message), &mainline);

    assert!(result.is_err());
    assert_eq!(git(local.path(), &["rev-parse", "main"]), main);
    assert_eq!(repo.uncommitted_changes().unwrap().len(), 2);
}

#[test]
fn proceeding_without_the_changes_leaves_them_in_place() {
    let (_local, repo) = dirty();
    let mainline = repo.mainline(None).unwrap();
    let outcome = repo
        .resolve_dirty_tree(DirtyTreeAction::ProceedWithout, &mainline)
        .unwrap();
    assert_eq!(outcome, DirtyTreeOutcome::Proceed);
    assert_eq!(repo.uncommitted_changes().unwrap().len(), 2);
}

#[test]
fn abort_changes_nothing() {
    let (_local, repo) = dirty();
    let mainline = repo.mainline(None).unwrap();
    let outcome = repo
        .resolve_dirty_tree(DirtyTreeAction::Abort, &mainline)
        .unwrap();
    assert_eq!(outcome, DirtyTreeOutcome::Abort);
    assert_eq!(repo.uncommitted_changes().unwrap().len(), 2);
}
