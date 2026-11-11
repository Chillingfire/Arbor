//! The tiny sample repo the pipeline runs against: a copy of `tests/fixtures/sample-repo` committed
//! on `main`, and a Session worktree on its own Session branch.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use arbor_git::{Mainline, Repo, SessionBranch, Worktree};
use tempfile::{TempDir, tempdir};

const SAMPLE_REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample-repo");

pub struct SampleRepo {
    /// Holds the operator's checkout and the Session worktree; both go when it drops.
    _dir: TempDir,
    pub worktree: Worktree,
    pub mainline: Mainline,
    /// The `main` commit the Session branch started from.
    pub start: String,
}

impl SampleRepo {
    /// Copies the sample repo into a new repository, commits it on `main`, and creates the Session
    /// branch and worktree from it as a Session would.
    pub fn new(feature: &str) -> Self {
        let dir = tempdir().unwrap();
        let checkout = dir.path().join("wordy");
        copy_dir(Path::new(SAMPLE_REPO), &checkout);
        git(&checkout, &["init", "--quiet", "-b", "main"]);
        configure(&checkout);
        git(&checkout, &["add", "--all"]);
        git(&checkout, &["commit", "--quiet", "-m", "Start wordy"]);
        let start = git(&checkout, &["rev-parse", "HEAD"]);

        let repo = Repo::open(&checkout).unwrap();
        let mainline = repo.mainline(None).unwrap();
        let branch = SessionBranch::for_feature(feature).unwrap();
        let worktree = repo
            .create_worktree(&mainline, &branch, &dir.path().join("session"))
            .unwrap();
        Self {
            _dir: dir,
            worktree,
            mainline,
            start,
        }
    }

    pub fn path(&self) -> &Path {
        self.worktree.path()
    }

    pub fn read(&self, file: &str) -> String {
        fs::read_to_string(self.path().join(file)).unwrap()
    }

    /// The subjects of the commits the Session branch added, oldest first.
    pub fn session_commit_subjects(&self) -> Vec<String> {
        let range = format!("{}..HEAD", self.start);
        let subjects = git(self.path(), &["log", "--reverse", "--format=%s", &range]);
        subjects.lines().map(str::to_owned).collect()
    }

    /// Uncommitted changes in the worktree, as `git status --porcelain` lists them.
    pub fn uncommitted_changes(&self) -> String {
        git(self.path(), &["status", "--porcelain"])
    }
}

/// Copies the fixture with `\n` line endings, whatever the checkout of Arbor itself used, so the
/// prompts built from it are the same on every machine. Build output is left behind.
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "target" || name == "Cargo.lock" {
            continue;
        }
        let source: PathBuf = entry.path();
        let destination = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&source, &destination);
        } else {
            let source_text = fs::read_to_string(&source).unwrap();
            fs::write(&destination, source_text.replace("\r\n", "\n")).unwrap();
        }
    }
}

/// A fixed identity, and none of the operator's own settings that would change what git writes
/// or the Task diff a Revision is shown.
fn configure(dir: &Path) {
    let settings = [
        ("user.name", "Test Operator"),
        ("user.email", "operator@example.com"),
        ("commit.gpgsign", "false"),
        ("core.autocrlf", "false"),
        ("core.abbrev", "7"),
        ("diff.algorithm", "myers"),
        ("diff.indentHeuristic", "true"),
        ("diff.interHunkContext", "0"),
        ("diff.suppressBlankEmpty", "false"),
    ];
    for (key, value) in settings {
        git(dir, &["config", key, value]);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let git_output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        git_output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&git_output.stderr)
    );
    String::from_utf8(git_output.stdout)
        .unwrap()
        .trim()
        .to_owned()
}
