mod common;

use arbor_git::{CommitId, CommitMessage, Mainline, Repo, SessionBranch, Worktree};
use common::{Local, git, write};
use tempfile::{TempDir, tempdir};

/// A Session worktree on `arbor/work`, created from a repository with no remote.
struct Session {
    repo: Local,
    _place: TempDir,
    mainline: Mainline,
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
            repo,
            _place: place,
            mainline,
            worktree,
        }
    }

    fn commit_file(&self, path: &str, text: &str) -> CommitId {
        write(self.worktree.path(), path, text);
        self.worktree
            .commit(&CommitMessage::new("Change a file.").unwrap())
            .unwrap()
            .expect("there were changes to commit")
    }
}

#[test]
fn the_parent_of_a_tasks_first_commit_is_where_the_task_started() {
    let session = Session::new();
    let start = session.worktree.head().unwrap();
    let first = session.commit_file("src/lib.rs", "one\n");
    session.commit_file("src/lib.rs", "two\n");
    assert_eq!(session.worktree.parent_of(&first).unwrap(), start);
}

#[test]
fn a_diff_covers_commits_and_uncommitted_changes_since_the_base() {
    let session = Session::new();
    let base = session.worktree.head().unwrap();
    session.commit_file("src/lib.rs", "fn a() {}\n");
    write(
        session.worktree.path(),
        "src/lib.rs",
        "fn a() {}\nfn b() {}\n",
    );
    write(session.worktree.path(), "untracked.txt", "left out\n");

    let diff = session.worktree.diff_from(&base).unwrap();
    assert!(diff.contains("+++ b/src/lib.rs"), "{diff}");
    assert!(diff.contains("+fn a() {}\n+fn b() {}\n"), "{diff}");
    assert!(!diff.contains("untracked.txt"), "{diff}");
}

#[test]
fn a_diff_c_quotes_non_ascii_paths_whatever_the_operators_quote_path_config() {
    let session = Session::new();
    git(
        session.worktree.path(),
        &["config", "core.quotePath", "false"],
    );
    let base = session.worktree.head().unwrap();
    session.commit_file("\u{e9}.rs", "fn a() {}\n");

    let diff = session.worktree.diff_from(&base).unwrap();
    assert!(diff.contains("+++ \"b/\\303\\251.rs\"\n"), "{diff}");
}

#[test]
fn a_committed_diff_leaves_out_uncommitted_changes() {
    let session = Session::new();
    let base = session.worktree.head().unwrap();
    session.commit_file("src/lib.rs", "fn a() {}\n");
    write(
        session.worktree.path(),
        "src/lib.rs",
        "fn uncommitted() {}\n",
    );

    let diff = session.worktree.committed_diff_from(&base).unwrap();
    assert!(diff.contains("+fn a() {}"), "{diff}");
    assert!(!diff.contains("uncommitted"), "{diff}");
}

#[test]
fn the_fork_point_ignores_later_mainline_commits() {
    let session = Session::new();
    let start = session.worktree.head().unwrap();
    session.commit_file("src/lib.rs", "work\n");
    git(
        session.repo.path(),
        &["commit", "--quiet", "--allow-empty", "-m", "Later"],
    );
    assert_eq!(
        session.worktree.fork_point(&session.mainline).unwrap(),
        start
    );
}

#[test]
fn a_base_that_is_not_a_commit_hash_is_refused() {
    let session = Session::new();
    for base in ["HEAD", "--output=x", "0000000"] {
        let base = CommitId::from(base.to_owned());
        assert!(session.worktree.diff_from(&base).is_err(), "{base}");
        assert!(session.worktree.parent_of(&base).is_err(), "{base}");
    }
}
