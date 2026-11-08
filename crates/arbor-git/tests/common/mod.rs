#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::{TempDir, tempdir};

/// Runs git in `dir` and returns its trimmed stdout. Panics if git fails.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Gives a repository a fixed identity and turns off anything in the operator's own config
/// that would make the tests depend on this machine.
pub fn configure(dir: &Path) {
    git(dir, &["config", "user.name", "Test Operator"]);
    git(dir, &["config", "user.email", "operator@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "core.autocrlf", "false"]);
}

pub fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

pub fn commit_file(dir: &Path, path: &str, text: &str, subject: &str) -> String {
    write(dir, path, text);
    git(dir, &["add", "--all"]);
    git(dir, &["commit", "--quiet", "-m", subject]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A repository with one commit on `main` and no remote.
pub struct Local {
    pub dir: TempDir,
}

impl Local {
    pub fn new() -> Self {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "--quiet", "-b", "main"]);
        configure(dir.path());
        commit_file(dir.path(), "README.md", "hello\n", "Initial commit");
        Self { dir }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// A bare `origin` whose default branch is `main`, and a clone of it.
pub struct Cloned {
    pub dir: TempDir,
    pub origin: PathBuf,
    pub clone: PathBuf,
}

impl Cloned {
    pub fn new() -> Self {
        Self::with_default_branch("main")
    }

    /// `branch` becomes origin's default branch. Origin also has `main`.
    pub fn with_default_branch(branch: &str) -> Self {
        let dir = tempdir().unwrap();
        let origin = dir.path().join("origin.git");
        let seed = dir.path().join("seed");
        let clone = dir.path().join("clone");
        fs::create_dir_all(&origin).unwrap();
        fs::create_dir_all(&seed).unwrap();
        git(&origin, &["init", "--quiet", "--bare", "-b", "main"]);
        git(&seed, &["init", "--quiet", "-b", "main"]);
        configure(&seed);
        commit_file(&seed, "README.md", "hello\n", "Initial commit");
        git(
            &seed,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&seed, &["push", "--quiet", "origin", "main"]);
        if branch != "main" {
            git(&seed, &["switch", "--quiet", "-c", branch]);
            commit_file(&seed, "trunk.txt", "trunk\n", "Start trunk");
            git(&seed, &["push", "--quiet", "origin", branch]);
            git(
                &origin,
                &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
            );
        }
        clone_into(&origin, &clone);
        Self { dir, origin, clone }
    }

    /// Commits a file to origin's `main` from a second clone, so `clone` is behind.
    pub fn advance_origin(&self, path: &str) -> String {
        self.push_to_origin(path, "upstream\n")
    }

    /// Commits `text` as `path` on origin's `main` from a second clone.
    pub fn push_to_origin(&self, path: &str, text: &str) -> String {
        let other = self.dir.path().join("other");
        if !other.exists() {
            clone_into(&self.origin, &other);
        }
        git(&other, &["switch", "--quiet", "main"]);
        git(&other, &["pull", "--quiet", "--ff-only"]);
        let commit = commit_file(&other, path, text, "Upstream change");
        git(&other, &["push", "--quiet", "origin", "main"]);
        commit
    }
}

/// Clones `origin` into `dest` with [`configure`] applied from the start, so the checkout
/// doesn't depend on this machine's line-ending settings.
fn clone_into(origin: &Path, dest: &Path) {
    git(
        origin.parent().unwrap(),
        &[
            "clone",
            "--quiet",
            "--config",
            "core.autocrlf=false",
            origin.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
    );
    configure(dest);
}
