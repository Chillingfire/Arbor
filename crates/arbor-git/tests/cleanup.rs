mod common;

use arbor_git::{CommitId, CommitMessage, Repo, SessionBranch, Worktree};
use common::{Local, git, write};
use tempfile::{TempDir, tempdir};

struct SessionSetup {
    project: Local,
    repo: Repo,
    place: TempDir,
    worktree: Worktree,
}

fn session_setup() -> SessionSetup {
    let project = Local::new();
    let repo = Repo::open(project.path()).unwrap();
    let mainline = repo.mainline(None).unwrap();
    let place = tempdir().unwrap();
    let branch = SessionBranch::for_feature("Add a thing").unwrap();
    let worktree = repo
        .create_worktree(&mainline, &branch, &place.path().join("worktree"))
        .unwrap();
    SessionSetup {
        project,
        repo,
        place,
        worktree,
    }
}

fn commit(worktree: &Worktree, path: &str, file_text: &str) -> CommitId {
    write(worktree.path(), path, file_text);
    let message = CommitMessage::new("Change a file.").unwrap();
    worktree.commit(&message).unwrap().unwrap()
}

#[test]
fn uncommitted_changes_include_untracked_files() {
    let setup = session_setup();
    assert!(!setup.worktree.has_uncommitted_changes().unwrap());
    write(setup.worktree.path(), "new.txt", "new\n");
    assert!(setup.worktree.has_uncommitted_changes().unwrap());
}

#[test]
fn removing_a_worktree_keeps_its_branch() {
    let setup = session_setup();
    let path = setup.place.path().join("worktree");
    setup.repo.remove_worktree(&path).unwrap();

    assert!(!path.exists());
    assert!(setup.repo.branch_exists("arbor/add-a-thing").unwrap());
    let listed = git(setup.project.path(), &["worktree", "list"]);
    assert!(!listed.contains("worktree"), "{listed}");
}

#[test]
fn a_worktree_folder_deleted_by_hand_only_loses_its_record() {
    let setup = session_setup();
    let path = setup.place.path().join("worktree");
    std::fs::remove_dir_all(&path).unwrap();
    setup.repo.remove_worktree(&path).unwrap();
    let listed = git(setup.project.path(), &["worktree", "list", "--porcelain"]);
    assert_eq!(listed.matches("worktree ").count(), 1, "{listed}");
}

#[test]
fn only_session_branches_can_be_deleted() {
    let setup = session_setup();
    setup
        .repo
        .remove_worktree(&setup.place.path().join("worktree"))
        .unwrap();
    assert!(setup.repo.delete_session_branch("main").is_err());
    setup
        .repo
        .delete_session_branch("arbor/add-a-thing")
        .unwrap();
    assert!(!setup.repo.branch_exists("arbor/add-a-thing").unwrap());
    assert!(setup.repo.branch_exists("main").unwrap());
}

#[test]
fn a_reset_with_uncommitted_edits_leaves_the_later_files_uncommitted() {
    let setup = session_setup();
    let start = setup.worktree.head().unwrap();
    let edits = commit(&setup.worktree, "skeleton.rs", "fn skeleton() {}\n");
    commit(&setup.worktree, "later.rs", "fn later() {}\n");

    setup
        .worktree
        .reset_with_uncommitted(&start, &edits)
        .unwrap();

    assert_eq!(setup.worktree.head().unwrap(), start);
    assert!(setup.worktree.path().join("skeleton.rs").exists());
    assert!(!setup.worktree.path().join("later.rs").exists());
    assert!(setup.worktree.has_uncommitted_changes().unwrap());
}
