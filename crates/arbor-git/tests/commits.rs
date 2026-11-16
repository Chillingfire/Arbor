mod common;

use std::path::Path;

use arbor_git::{CommitId, CommitMessage, Repo, SessionBranch, Worktree};
use common::{Local, git, write};
use tempfile::{TempDir, tempdir};

/// A Session worktree on `arbor/work`, created from a repository with no remote.
struct Session {
    _repo: Local,
    _place: TempDir,
    worktree: Worktree,
}

impl Session {
    fn new() -> Self {
        let repo = Local::new();
        let opened = Repo::open(repo.path()).unwrap();
        let mainline = opened.mainline(None).unwrap();
        let place = tempdir().unwrap();
        let branch = SessionBranch::for_feature("work").unwrap();
        let worktree = opened
            .create_worktree(&mainline, &branch, &place.path().join("session"))
            .unwrap();
        Self {
            _repo: repo,
            _place: place,
            worktree,
        }
    }

    fn path(&self) -> &Path {
        self.worktree.path()
    }

    fn commit(&self, subject: &str) -> CommitId {
        self.worktree
            .commit(&CommitMessage::new(subject).unwrap())
            .unwrap()
            .expect("there were changes to commit")
    }
}

#[test]
fn a_commit_uses_the_operators_identity() {
    let session = Session::new();
    write(session.path(), "src/lib.rs", "fn main() {}\n");
    session.commit("Add the entry point.");
    let who = git(
        session.path(),
        &["log", "-1", "--format=%an <%ae> / %cn <%ce>"],
    );
    assert_eq!(
        who,
        "Test Operator <operator@example.com> / Test Operator <operator@example.com>"
    );
}

#[test]
fn the_message_is_the_subject_and_description_with_nothing_added() {
    let session = Session::new();
    write(session.path(), "a.txt", "a\n");
    let message = CommitMessage::new("Add the parser.")
        .unwrap()
        .with_description("# Grammar\nIt handles nested lists.");
    session.worktree.commit(&message).unwrap();
    let body = git(session.path(), &["log", "-1", "--format=%B"]);
    assert_eq!(
        body,
        "Add the parser.\n\n# Grammar\nIt handles nested lists."
    );
}

#[test]
fn a_message_without_a_description_is_just_the_subject() {
    let session = Session::new();
    write(session.path(), "a.txt", "a\n");
    let message = CommitMessage::new("  Add a file.  ")
        .unwrap()
        .with_description("   ");
    assert_eq!(message.text(), "Add a file.");
    session.worktree.commit(&message).unwrap();
    assert_eq!(
        git(session.path(), &["log", "-1", "--format=%B"]),
        "Add a file."
    );
}

#[test]
fn a_subject_must_be_one_non_empty_line() {
    assert!(CommitMessage::new("").is_err());
    assert!(CommitMessage::new("   ").is_err());
    assert!(CommitMessage::new("Two\nlines").is_err());
}

#[test]
fn a_commit_takes_every_change_in_the_worktree() {
    let session = Session::new();
    write(session.path(), "new.txt", "new\n");
    write(session.path(), "README.md", "changed\n");
    let commit = session.commit("Change things.");
    let files = git(
        session.path(),
        &["show", "--name-status", "--format=", commit.as_str()],
    );
    assert!(files.contains("A\tnew.txt"), "{files}");
    assert!(files.contains("M\tREADME.md"), "{files}");
    assert!(git(session.path(), &["status", "--porcelain"]).is_empty());

    std::fs::remove_file(session.path().join("new.txt")).unwrap();
    let commit = session.commit("Remove a file.");
    let files = git(
        session.path(),
        &["show", "--name-status", "--format=", commit.as_str()],
    );
    assert_eq!(files, "D\tnew.txt");
}

#[test]
fn committing_with_no_changes_makes_no_commit() {
    let session = Session::new();
    let before = session.worktree.head().unwrap();
    let message = CommitMessage::new("Nothing.").unwrap();
    assert_eq!(session.worktree.commit(&message).unwrap(), None);
    assert_eq!(session.worktree.head().unwrap(), before);
}

#[test]
fn the_worktree_can_be_reset_to_a_recorded_commit() {
    let session = Session::new();
    write(session.path(), "kept.txt", "kept\n");
    let task_start = session.commit("Start the task.");
    write(session.path(), "kept.txt", "rewritten\n");
    write(session.path(), "later.txt", "later\n");
    session.commit("Do more.");
    write(session.path(), "kept.txt", "uncommitted\n");
    write(session.path(), "untracked/file.txt", "stray\n");

    let recorded = CommitId::from(task_start.as_str().to_owned());
    session.worktree.reset_to(&recorded).unwrap();

    assert_eq!(session.worktree.head().unwrap(), task_start);
    assert_eq!(
        std::fs::read_to_string(session.path().join("kept.txt")).unwrap(),
        "kept\n"
    );
    assert!(!session.path().join("later.txt").exists());
    assert!(!session.path().join("untracked").exists());
    assert_eq!(
        git(session.path(), &["branch", "--show-current"]),
        "arbor/work"
    );
}

#[test]
fn resetting_to_something_that_is_not_a_commit_hash_is_refused() {
    let session = Session::new();
    let head = session.worktree.head().unwrap();
    for bad in [
        "HEAD~1",
        "--hard",
        "main",
        "0123456789abcdef0123456789abcdef01234567",
    ] {
        assert!(
            session
                .worktree
                .reset_to(&CommitId::from(bad.to_owned()))
                .is_err(),
            "{bad}"
        );
    }
    assert_eq!(session.worktree.head().unwrap(), head);
}

#[test]
fn a_commit_runs_the_repositorys_hooks_and_a_refusing_hook_stops_it() {
    let session = Session::new();
    let hooks = git(
        session.path(),
        &["rev-parse", "--path-format=absolute", "--git-path", "hooks"],
    );
    write(
        Path::new(&hooks),
        "pre-commit",
        "#!/bin/sh\necho refused by the hook >&2\nexit 1\n",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hook = Path::new(&hooks).join("pre-commit");
        std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    write(session.path(), "a.txt", "a\n");

    let refused = session
        .worktree
        .commit(&CommitMessage::new("Add a file.").unwrap());

    let message = refused.unwrap_err().to_string();
    assert!(message.contains("refused by the hook"), "{message}");
}
