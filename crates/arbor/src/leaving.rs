//! Remembers the Sessions this run of arbor started, so leaving with one unfinished asks first and
//! leaves the warning on stderr once the TUI has closed.

use std::path::PathBuf;

use arbor_git::{Repo, TreeSource};
use arbor_tiers::scan_markers;
use arbor_tui::UnfinishedSession;

/// A Session's branch and worktree, from the moment they exist.
pub struct StartedSession {
    pub branch: String,
    pub worktree: PathBuf,
    pub finished: bool,
}

impl StartedSession {
    /// The warning to give on leaving, or `None` for a finished Session with no Marker left or a
    /// worktree that is gone.
    pub fn unfinished(&self, running: bool) -> Option<UnfinishedSession> {
        let repo = Repo::open(&self.worktree).ok()?;
        let marker_lines = scan_markers(&repo, &TreeSource::WorkingTree).unwrap_or_default();
        if self.finished && !running && marker_lines.is_empty() {
            return None;
        }
        Some(UnfinishedSession {
            branch: self.branch.clone(),
            worktree: self.worktree.clone(),
            markers: marker_lines.len(),
            placeholders: marker_lines
                .iter()
                .filter(|marker_line| marker_line.opens_region())
                .count(),
            running,
        })
    }
}

/// Prints the warning for each unfinished Session, once the terminal is back to normal.
pub fn warn_on_stderr(started_sessions: &[StartedSession]) {
    for unfinished in started_sessions
        .iter()
        .filter_map(|started| started.unfinished(false))
    {
        eprintln!();
        for paragraph in unfinished.warning() {
            eprintln!("arbor: {paragraph}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use tempfile::tempdir;

    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let finished = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(finished.status.success(), "git {args:?}: {finished:?}");
    }

    fn started(worktree: &Path, finished: bool) -> StartedSession {
        StartedSession {
            branch: "arbor/two".to_owned(),
            worktree: worktree.to_owned(),
            finished,
        }
    }

    #[test]
    fn markers_and_placeholders_are_counted_from_the_worktree() {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        fs::write(
            dir.path().join("two.rs"),
            "// ARBOR:BEGIN WU-1\n// ARBOR:END WU-1\n// ARBOR:BEGIN WU-2\n// ARBOR:END WU-2\n",
        )
        .unwrap();
        git(dir.path(), &["add", "two.rs"]);

        let unfinished = started(dir.path(), true).unfinished(false).unwrap();

        assert_eq!((unfinished.markers, unfinished.placeholders), (4, 2));
    }

    #[test]
    fn a_finished_session_without_markers_needs_no_warning() {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        assert_eq!(started(dir.path(), true).unfinished(false), None);
        assert!(started(dir.path(), false).unfinished(false).is_some());
        assert!(started(dir.path(), true).unfinished(true).is_some());
    }

    #[test]
    fn a_deleted_worktree_needs_no_warning() {
        let dir = tempdir().unwrap();
        let gone = dir.path().join("gone");
        assert_eq!(started(&gone, false).unfinished(false), None);
    }
}
