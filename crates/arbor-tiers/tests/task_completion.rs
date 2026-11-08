//! The Checks are `git grep` commands, so they work wherever the tests do, Windows included.

use std::fs;
use std::path::Path;
use std::process::Command;

use arbor_config::Checks;
use arbor_core::{CommitOrigin, Event, LeftoverMarker, SessionCommit, WorkUnitId};
use arbor_git::{CommitMessage, Mainline, Repo, SessionBranch, Worktree};
use arbor_tiers::{
    MARKER_FIX_SUBJECT, NO_UNIT_CHECK, check_markers, commit_operator_fix, leftover_markers,
    run_task_check, task_check_warning, task_diff,
};
use tempfile::{TempDir, tempdir};

fn git(dir: &Path, args: &[&str]) {
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
}

/// A repository whose `main` already holds a line with `ARBOR:`, and a Session worktree on it.
struct Session {
    _dir: TempDir,
    mainline: Mainline,
    worktree: Worktree,
}

impl Session {
    fn new() -> Self {
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"]);
        git(&repo, &["config", "user.name", "Test Operator"]);
        git(&repo, &["config", "user.email", "operator@example.com"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        git(&repo, &["config", "core.autocrlf", "false"]);
        fs::write(repo.join("NOTES.md"), "ARBOR: is our Marker prefix\n").unwrap();
        git(&repo, &["add", "--all"]);
        git(&repo, &["commit", "--quiet", "-m", "Explain Markers"]);
        let opened = Repo::open(&repo).unwrap();
        let mainline = opened.mainline(None).unwrap();
        let branch = SessionBranch::for_feature("work").unwrap();
        let worktree = opened
            .create_worktree(&mainline, &branch, &dir.path().join("session"))
            .unwrap();
        Self {
            _dir: dir,
            mainline,
            worktree,
        }
    }

    fn write(&self, path: &str, text: &str) {
        fs::write(self.worktree.path().join(path), text).unwrap();
    }

    fn commit(&self, path: &str, text: &str) -> String {
        self.write(path, text);
        let message = CommitMessage::new("Change a file.").unwrap();
        let commit = self.worktree.commit(&message).unwrap().unwrap();
        commit.as_str().to_owned()
    }
}

fn work_unit_commit(id: u32, hash: String) -> SessionCommit {
    SessionCommit {
        hash,
        origin: CommitOrigin::WorkUnit(WorkUnitId(id)),
    }
}

fn marker(file: &str, line: usize, text: &str) -> LeftoverMarker {
    LeftoverMarker {
        file: file.into(),
        line,
        text: text.into(),
    }
}

#[test]
fn markers_the_branch_adds_are_found_but_not_those_on_the_mainline() {
    let session = Session::new();
    session.commit("calc.py", "def add(a, b):\n    return a + b\n");
    assert_eq!(
        check_markers(&session.worktree, &session.mainline).unwrap(),
        Event::MarkersChecked { remaining: vec![] }
    );

    session.commit(
        "calc.py",
        "def add(a, b):\n    # ARBOR:BEGIN WU-1\n    return a + b\n",
    );
    assert_eq!(
        check_markers(&session.worktree, &session.mainline).unwrap(),
        Event::MarkersChecked {
            remaining: vec![marker("calc.py", 2, "# ARBOR:BEGIN WU-1")]
        }
    );
}

#[test]
fn an_uncommitted_fix_does_not_pass_the_marker_check_until_it_is_committed() {
    let session = Session::new();
    session.commit("calc.py", "# ARBOR:END WU-1\n");
    session.write("calc.py", "def add(a, b):\n    return a + b\n");
    let Event::MarkersChecked { remaining } =
        check_markers(&session.worktree, &session.mainline).unwrap()
    else {
        panic!("expected MarkersChecked");
    };
    assert_eq!(
        remaining.len(),
        1,
        "the branch's commits still hold the Marker"
    );

    let message = CommitMessage::new(MARKER_FIX_SUBJECT).unwrap();
    let Event::OperatorFixed { commit: Some(fix) } =
        commit_operator_fix(&session.worktree, &message).unwrap()
    else {
        panic!("expected the fix to be committed");
    };
    assert_eq!(session.worktree.head().unwrap().as_str(), fix);
    assert_eq!(
        check_markers(&session.worktree, &session.mainline).unwrap(),
        Event::MarkersChecked { remaining: vec![] }
    );
    assert_eq!(
        commit_operator_fix(&session.worktree, &message).unwrap(),
        Event::OperatorFixed { commit: None },
        "with nothing changed there is nothing to commit"
    );
}

#[test]
fn the_task_diff_starts_where_the_tasks_first_commit_did() {
    let session = Session::new();
    session.commit("before.txt", "an earlier Task\n");
    assert_eq!(task_diff(&session.worktree, &[]).unwrap(), "");

    let first = session.commit("calc.py", "def add(a, b):\n    return a + b\n");
    let second = session.commit("calc.py", "def add(a, b):\n    return a - b\n");
    session.write("calc.py", "def add(a, b):\n    return a * b\n");
    let task_commits = [work_unit_commit(1, first), work_unit_commit(2, second)];
    let diff = task_diff(&session.worktree, &task_commits).unwrap();
    assert!(diff.contains("+    return a * b"), "{diff}");
    assert!(!diff.contains("before.txt"), "{diff}");
}

#[test]
fn added_lines_are_numbered_in_the_new_file() {
    let diff = "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,4 @@
 fn a() {
-    // ARBOR:BEGIN WU-1
+    // ARBOR:BEGIN WU-1 kept
     1
 }
@@ -10 +10,2 @@
-x
++++ ARBOR:END WU-4 in a line that starts with +++
+// ARBOR:END WU-1
\\ No newline at end of file
diff --git a/gone.rs b/gone.rs
--- a/gone.rs
+++ /dev/null
@@ -1 +0,0 @@
-// ARBOR:END WU-9
";
    assert_eq!(
        leftover_markers(diff),
        [
            marker("src/lib.rs", 2, "// ARBOR:BEGIN WU-1 kept"),
            marker(
                "src/lib.rs",
                10,
                "+++ ARBOR:END WU-4 in a line that starts with +++"
            ),
            marker("src/lib.rs", 11, "// ARBOR:END WU-1"),
        ]
    );
}

#[test]
fn only_well_formed_markers_are_leftovers() {
    let diff = "\
--- a/src/markers.rs
+++ b/src/markers.rs
@@ -0,0 +1,6 @@
+pub const BEGIN: &str = \"ARBOR:BEGIN\";
+// ARBOR:BEGINNING is not a Marker
+// ARBOR:END WU-oops
+Markers look like `ARBOR:BEGIN WU-<n>`.
+    # ARBOR:BEGIN WU-12
+// ARBOR:END WU-12
";
    assert_eq!(
        leftover_markers(diff),
        [
            marker("src/markers.rs", 5, "# ARBOR:BEGIN WU-12"),
            marker("src/markers.rs", 6, "// ARBOR:END WU-12"),
        ]
    );
}

fn checks(unit: Option<&str>) -> Checks {
    Checks {
        unit: unit.map(str::to_owned),
        ..Checks::default()
    }
}

#[tokio::test]
async fn the_full_unit_check_decides_whether_the_task_passed() {
    let session = Session::new();
    session.commit("calc.py", "def add(a, b):\n    return a + b\n");
    let path = session.worktree.path();

    let passing = checks(Some("git grep --quiet -e add -- calc.py"));
    assert_eq!(
        run_task_check(path, &passing).await.unwrap(),
        Event::TaskChecksPassed
    );

    let failing = checks(Some("git grep --quiet -e subtract -- calc.py"));
    let Event::TaskChecksFailed { output } = run_task_check(path, &failing).await.unwrap() else {
        panic!("expected the Task to fail");
    };
    assert!(output.contains("exit code 1"), "{output}");
}

#[tokio::test]
async fn without_a_unit_check_the_task_passes_with_a_warning() {
    let session = Session::new();
    let none = checks(None);
    assert_eq!(task_check_warning(&none), Some(NO_UNIT_CHECK));
    assert_eq!(
        run_task_check(session.worktree.path(), &none)
            .await
            .unwrap(),
        Event::TaskChecksPassed
    );

    let missing = checks(Some("no-such-program-for-arbor"));
    assert!(
        run_task_check(session.worktree.path(), &missing)
            .await
            .is_err()
    );
}
