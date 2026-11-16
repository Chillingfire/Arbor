//! `arbor init`'s git hooks, run for real with this build of `arbor` on `PATH`. The operator's own
//! git config is kept out with an empty global config.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::{TempDir, tempdir};

/// A git repository with a clean first commit, and an isolated home for git and arbor config.
struct Project {
    dir: TempDir,
    home: TempDir,
}

impl Project {
    fn new() -> Self {
        let project = Self {
            dir: tempdir().unwrap(),
            home: tempdir().unwrap(),
        };
        fs::write(project.global_config(), "").unwrap();
        project.git(&["init", "--quiet", "-b", "main"]);
        project.git(&["config", "user.name", "Test Operator"]);
        project.git(&["config", "user.email", "operator@example.com"]);
        project.git(&["config", "commit.gpgsign", "false"]);
        project.git(&["config", "core.autocrlf", "false"]);
        project.write("lib.rs", "fn one() {}\n");
        project.git(&["add", "--all"]);
        project.git(&["commit", "--quiet", "-m", "Start"]);
        project
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn global_config(&self) -> PathBuf {
        self.home.path().join("gitconfig")
    }

    fn hook(&self, name: &str) -> PathBuf {
        self.root().join(".git/hooks").join(name)
    }

    fn write(&self, path: &str, text: &str) {
        fs::write(self.root().join(path), text).unwrap();
    }

    /// A command with this build of arbor first on `PATH` and no machine-wide config.
    fn command(&self, program: &str, dir: &Path) -> Command {
        let arbor_dir = Path::new(env!("CARGO_BIN_EXE_arbor")).parent().unwrap();
        let mut paths = vec![arbor_dir.to_owned()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let path: OsString = std::env::join_paths(paths).unwrap();
        let mut command = Command::new(program);
        command
            .current_dir(dir)
            .env("PATH", path)
            .env("GIT_CONFIG_GLOBAL", self.global_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("OPENROUTER_API_KEY")
            .env("APPDATA", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path())
            .env("HOME", self.home.path());
        command
    }

    fn git_in(&self, dir: &Path, args: &[&str]) -> Output {
        self.command("git", dir).args(args).output().unwrap()
    }

    fn git(&self, args: &[&str]) -> String {
        let git_output = self.git_in(self.root(), args);
        assert!(git_output.status.success(), "git {args:?}: {git_output:?}");
        String::from_utf8_lossy(&git_output.stdout)
            .trim()
            .to_owned()
    }

    fn init(&self, answers: &str) -> Output {
        let mut child = self
            .command(env!("CARGO_BIN_EXE_arbor"), self.root())
            .arg("init")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(answers.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

const MARKED: &str = "fn two() {\n    // ARBOR:BEGIN WU-2\n    todo!()\n    // ARBOR:END WU-2\n}\n";

#[test]
fn init_installs_the_three_hooks_by_default() {
    let project = Project::new();

    let init_run = project.init("");

    let stdout = String::from_utf8_lossy(&init_run.stdout);
    assert!(
        stdout.contains("Installed the pre-commit, pre-merge-commit, pre-push hooks"),
        "{init_run:?}"
    );
    for name in ["pre-commit", "pre-merge-commit", "pre-push"] {
        let script = fs::read_to_string(project.hook(name)).unwrap();
        assert!(script.contains("arbor check-markers"), "{name}: {script}");
    }
}

#[test]
fn the_operator_can_decline_the_hooks() {
    let project = Project::new();
    project.init("n\n");
    assert!(!project.hook("pre-commit").exists());
}

#[test]
fn an_existing_hook_is_never_overwritten() {
    let project = Project::new();
    fs::create_dir_all(project.root().join(".git/hooks")).unwrap();
    fs::write(project.hook("pre-commit"), "#!/bin/sh\nexit 0\n").unwrap();

    let init_run = project.init("y\n");

    assert_eq!(
        fs::read_to_string(project.hook("pre-commit")).unwrap(),
        "#!/bin/sh\nexit 0\n"
    );
    let stdout = String::from_utf8_lossy(&init_run.stdout);
    assert!(
        stdout.contains("Left your own pre-commit hooks as they are"),
        "{stdout}"
    );
    assert!(
        stdout.contains("arbor check-markers --staged --allow-session-worktree"),
        "{stdout}"
    );
    assert!(project.hook("pre-push").exists());
}

#[test]
fn with_core_hooks_path_set_init_prints_the_lines_instead() {
    let project = Project::new();
    project.git(&["config", "core.hooksPath", "my-hooks"]);

    let init_run = project.init("y\n");

    let stdout = String::from_utf8_lossy(&init_run.stdout);
    assert!(
        stdout.contains("core.hooksPath is set to my-hooks"),
        "{stdout}"
    );
    assert!(stdout.contains("arbor check-markers --rev"), "{stdout}");
    assert!(!project.hook("pre-commit").exists());
    assert!(!project.root().join("my-hooks").exists());
}

#[test]
fn the_hooks_refuse_markers_outside_a_session_worktree() {
    let project = Project::new();
    project.init("y\n");
    project.write("two.rs", MARKED);
    project.git(&["add", "two.rs"]);

    let commit = project.git_in(project.root(), &["commit", "--quiet", "-m", "Add two"]);

    assert!(!commit.status.success(), "{commit:?}");
    // git sends a hook's output to stderr.
    let stderr = String::from_utf8_lossy(&commit.stderr);
    assert!(
        stderr.contains("two.rs:2: // ARBOR:BEGIN WU-2"),
        "{commit:?}"
    );
}

#[test]
fn a_session_worktree_may_commit_markers_but_not_push_them() {
    let project = Project::new();
    let origin = project.home.path().join("origin.git");
    project.git(&["init", "--quiet", "--bare", origin.to_str().unwrap()]);
    project.git(&["remote", "add", "origin", origin.to_str().unwrap()]);
    project.init("y\n");
    let worktree = project.root().join(".arbor/sessions/1-two/worktree");
    project.git(&[
        "worktree",
        "add",
        "--quiet",
        "-b",
        "arbor/two",
        worktree.to_str().unwrap(),
    ]);
    fs::write(worktree.join("two.rs"), MARKED).unwrap();
    assert!(
        project
            .git_in(&worktree, &["add", "two.rs"])
            .status
            .success()
    );

    let commit = project.git_in(&worktree, &["commit", "--quiet", "-m", "Add two"]);
    assert!(commit.status.success(), "{commit:?}");

    let push = project.git_in(&worktree, &["push", "--quiet", "origin", "arbor/two"]);
    assert!(!push.status.success(), "{push:?}");
    assert!(
        String::from_utf8_lossy(&push.stdout).contains("two.rs:2:"),
        "{push:?}"
    );
    let clean_push = project.git_in(project.root(), &["push", "--quiet", "origin", "main"]);
    assert!(clean_push.status.success(), "{clean_push:?}");
}
