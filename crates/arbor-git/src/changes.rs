//! Bringing the operator's uncommitted changes into a Session without touching their checkout.
//!
//! The changes are captured with a temporary index, so the operator's index, working tree and
//! stashes stay as they were. They are applied onto the start point in a second temporary
//! index and committed with `commit-tree`, so nothing is checked out until the patch applies.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::command::{Git, GitError, git};

/// The message of the commit that holds the brought-in changes (ADR-0009).
const MESSAGE: &str = "Include uncommitted changes from the working tree.";

/// Commits the uncommitted changes of the checkout at `root` on top of the commit `start`,
/// which the operator knows as `start_name`. Returns `None` if there are no changes.
pub(crate) fn commit_onto(
    root: &Path,
    start: &str,
    start_name: &str,
) -> Result<Option<String>, GitError> {
    let patch = uncommitted_patch(root)?;
    if patch.is_empty() {
        return Ok(None);
    }
    let index = TempIndex::new(root)?;
    Git::new(root, &["read-tree", start])
        .index(&index.path)
        .bytes()?;
    let applied = Git::new(root, &["apply", "--cached", "--whitespace=nowarn", "-"])
        .index(&index.path)
        .input(&patch)
        .bytes();
    if let Err(GitError::Failed { stderr, .. }) = applied {
        return Err(GitError::Refused(format!(
            "the uncommitted changes don't apply cleanly onto {start_name}:\n{stderr}\nChoose \
             Leave them out or Abort."
        )));
    }
    applied?;
    let tree = Git::new(root, &["write-tree"]).index(&index.path).text()?;
    let commit = Git::new(root, &["commit-tree", &tree, "-p", start, "-F", "-"])
        .input(MESSAGE.as_bytes())
        .text()?;
    Ok(Some(commit))
}

/// A binary patch from the checkout's `HEAD` to its working tree, including untracked files
/// that aren't ignored.
fn uncommitted_patch(root: &Path) -> Result<Vec<u8>, GitError> {
    if git(root, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]).is_err() {
        return Err(GitError::Refused(
            "the checkout has no commits, so its changes can't be brought in".to_owned(),
        ));
    }
    let index = TempIndex::new(root)?;
    // Starting from a copy of the real index keeps staged renames and git's cached file stats.
    let real = git_dir(root)?.join("index");
    if real.exists() {
        fs::copy(&real, &index.path).map_err(GitError::Spawn)?;
    } else {
        Git::new(root, &["read-tree", "HEAD"])
            .index(&index.path)
            .bytes()?;
    }
    Git::new(root, &["add", "--all"])
        .index(&index.path)
        .bytes()?;
    let tree = Git::new(root, &["write-tree"]).index(&index.path).text()?;
    // Every option that the operator's diff config could change is pinned, so `git apply`
    // can read the patch.
    Git::new(
        root,
        &[
            "diff",
            "--binary",
            "--full-index",
            "--find-renames",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-relative",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "HEAD",
            &tree,
        ],
    )
    .bytes()
}

fn git_dir(root: &Path) -> Result<PathBuf, GitError> {
    git(root, &["rev-parse", "--absolute-git-dir"]).map(PathBuf::from)
}

/// An index file in the git directory, removed when dropped.
struct TempIndex {
    path: PathBuf,
}

impl TempIndex {
    fn new(root: &Path) -> Result<Self, GitError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let name = format!("session-index-{}-{nanos}", std::process::id());
        Ok(Self {
            path: git_dir(root)?.join(name),
        })
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.path.with_extension("lock"));
    }
}
