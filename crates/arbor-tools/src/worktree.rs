use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

use ignore::{Walk, WalkBuilder};

/// The Session worktree. Every path a model supplies is resolved through it.
pub(crate) struct Worktree {
    root: PathBuf,
}

impl Worktree {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The worktree root with symlinks resolved, so it compares equal to resolved paths.
    pub(crate) fn root(&self) -> Result<PathBuf, String> {
        fs::canonicalize(&self.root)
            .map_err(|err| format!("the worktree {} is unavailable: {err}", self.root.display()))
    }

    /// Resolves a path relative to the worktree. Refuses absolute paths, `..`, anything under
    /// `.git`, and symlinks that lead outside the worktree or into `.git`. The path itself need
    /// not exist, so writes can resolve new files.
    pub(crate) fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        for component in Path::new(path).components() {
            match component {
                Component::Normal(name) if is_git(name) => {
                    return Err(format!("{path}: .git is off limits"));
                }
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir => {
                    return Err(format!("{path}: paths may not contain `..`"));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(format!("{path}: use a path relative to the worktree root"));
                }
            }
        }
        let root = self.root()?;
        let full = root.join(path);

        // Resolve the deepest part that exists. What lies below it can't be a symlink yet.
        let mut existing = full.as_path();
        while fs::symlink_metadata(existing).is_err() {
            existing = existing
                .parent()
                .expect("the worktree root exists, so some ancestor does");
        }
        let real =
            fs::canonicalize(existing).map_err(|_| format!("{path}: is a broken symlink"))?;
        let inside = real
            .strip_prefix(&root)
            .map_err(|_| format!("{path}: resolves outside the worktree"))?;
        if inside
            .components()
            .any(|component| matches!(component, Component::Normal(name) if is_git(name)))
        {
            return Err(format!("{path}: .git is off limits"));
        }
        let rest = full
            .strip_prefix(existing)
            .expect("existing is an ancestor of full");
        // Joining an empty path would add a trailing separator, which Windows rejects for files.
        Ok(if rest.as_os_str().is_empty() {
            real
        } else {
            real.join(rest)
        })
    }

    /// A resolved path as the model should see it: relative, with `/` separators.
    pub(crate) fn display(&self, root: &Path, path: &Path) -> String {
        let relative = path.strip_prefix(root).unwrap_or(path);
        let parts: Vec<_> = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect();
        if parts.is_empty() {
            ".".to_owned()
        } else {
            parts.join("/")
        }
    }

    /// Walks `start` (a resolved path) in file-name order, skipping `.git` and whatever
    /// `.gitignore` excludes. The walk begins at the root so the root's ignore files apply to
    /// subdirectories too. Yields only paths strictly below `start`.
    pub(crate) fn walk(
        &self,
        root: &Path,
        start: &Path,
        max_depth: Option<usize>,
    ) -> impl Iterator<Item = ignore::DirEntry> {
        let start_depth = start
            .strip_prefix(root)
            .map_or(0, |relative| relative.components().count());
        let prune_start = start.to_path_buf();
        let walk: Walk = WalkBuilder::new(root)
            .hidden(false)
            .parents(false)
            .require_git(false)
            .max_depth(max_depth.map(|depth| start_depth + depth))
            .sort_by_file_name(OsStr::cmp)
            .filter_entry(move |entry| {
                let path = entry.path();
                !is_git(entry.file_name())
                    && (path.starts_with(&prune_start) || prune_start.starts_with(path))
            })
            .build();
        let start = start.to_path_buf();
        walk.filter_map(Result::ok)
            .filter(move |entry| entry.path().starts_with(&start) && entry.path() != start)
    }
}

/// Whether a path component names `.git`. Ignores case and trailing dots and spaces, which
/// Windows drops.
fn is_git(name: &OsStr) -> bool {
    name.to_string_lossy()
        .trim_end_matches(['.', ' '])
        .eq_ignore_ascii_case(".git")
}
