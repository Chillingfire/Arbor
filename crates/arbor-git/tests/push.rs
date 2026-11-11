mod common;

use arbor_git::{CommitMessage, RemoteApproval, Repo, SessionBranch, Worktree};
use common::{Cloned, Local, git, write};
use tempfile::tempdir;

#[test]
fn an_approved_push_sends_only_the_session_branch_to_origin() {
    let cloned = Cloned::new();
    let origin_main = git(&cloned.origin, &["rev-parse", "main"]);
    let repo = Repo::open(&cloned.clone).unwrap();
    repo.fetch().unwrap();
    let mainline = repo.mainline(None).unwrap();
    let place = tempdir().unwrap();
    let branch = SessionBranch::for_feature("ship it").unwrap();
    let worktree = repo
        .create_worktree(&mainline, &branch, &place.path().join("session"))
        .unwrap();
    write(worktree.path(), "feature.txt", "done\n");
    let head = worktree
        .commit(&CommitMessage::new("Add the feature.").unwrap())
        .unwrap()
        .unwrap();

    worktree
        .push(RemoteApproval::granted_by_operator())
        .unwrap();

    assert_eq!(
        git(&cloned.origin, &["rev-parse", "arbor/ship-it"]),
        head.as_str()
    );
    assert_eq!(git(&cloned.origin, &["rev-parse", "main"]), origin_main);
}

#[test]
fn only_a_session_branch_can_be_pushed() {
    let cloned = Cloned::new();
    let origin_main = git(&cloned.origin, &["rev-parse", "main"]);
    common::commit_file(&cloned.clone, "local.txt", "local\n", "Local change");
    let checkout = Worktree::open(&cloned.clone).unwrap();

    let push_result = checkout.push(RemoteApproval::granted_by_operator());

    assert!(push_result.is_err());
    assert_eq!(git(&cloned.origin, &["rev-parse", "main"]), origin_main);
}

#[test]
fn pushing_without_a_remote_fails() {
    let local = Local::new();
    let repo = Repo::open(local.path()).unwrap();
    let mainline = repo.mainline(None).unwrap();
    let place = tempdir().unwrap();
    let branch = SessionBranch::for_feature("nowhere").unwrap();
    let worktree = repo
        .create_worktree(&mainline, &branch, &place.path().join("session"))
        .unwrap();
    assert!(
        worktree
            .push(RemoteApproval::granted_by_operator())
            .is_err()
    );
}
