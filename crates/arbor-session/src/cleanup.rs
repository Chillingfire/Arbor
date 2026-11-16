//! Startup pruning by retention, and the Human Operator's Delete. Pruning always keeps the branch;
//! Delete is the only path that removes one.

use std::fs;
use std::time::{Duration, SystemTime};

use arbor_git::{Repo, TreeSource, Worktree};
use arbor_tiers::scan_markers;

use crate::store::{
    RunLock, SessionFolder, SessionSummary, StoreError, io_error, retention, session_folders,
    summarize,
};

/// The Marker lines at the tip of `branch`, outside the paths its Marker Allowlist names, or
/// `None` when the branch is gone or can't be scanned.
pub fn branch_markers(repo: &Repo, branch: &str) -> Option<usize> {
    if !repo.branch_exists(branch).ok()? {
        return None;
    }
    let tip = TreeSource::Revision(format!("refs/heads/{branch}"));
    scan_markers(repo, &tip)
        .ok()
        .map(|marker_lines| marker_lines.len())
}

/// Every Session under the project, newest first, as the Session browser shows them.
pub fn list_sessions(
    repo: &Repo,
    retention_days: u32,
    now: SystemTime,
) -> Result<Vec<SessionSummary>, StoreError> {
    let markers_at_tip = |branch: &str| branch_markers(repo, branch);
    Ok(session_folders(repo.root())?
        .into_iter()
        .map(|folder| summarize(folder, now, retention_days, markers_at_tip))
        .collect())
}

/// A Session startup pruning removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunedSession {
    pub id: String,
    /// The branch, which is kept.
    pub branch: String,
    pub idle: Duration,
    /// Marker lines the kept branch still holds, or `None` when unknown.
    pub markers_at_tip: Option<usize>,
}

/// What startup pruning did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PruneReport {
    pub retention_days: u32,
    pub pruned: Vec<PrunedSession>,
    /// Sessions past retention whose worktree has uncommitted changes, so they were kept.
    pub dirty: Vec<String>,
    /// Sessions that couldn't be pruned, and why.
    pub failed: Vec<(String, String)>,
}

impl PruneReport {
    /// The one-line notice of what was pruned, naming the kept branches that still hold Markers.
    pub fn notice(&self) -> Option<String> {
        if self.pruned.is_empty() {
            return None;
        }
        let ids: Vec<&str> = self
            .pruned
            .iter()
            .map(|pruned| pruned.id.as_str())
            .collect();
        let mut notice = format!(
            "Pruned {} idle over {} days (branches kept): {}.",
            counted(self.pruned.len(), "Session"),
            self.retention_days,
            ids.join(", ")
        );
        let with_markers: Vec<String> = self
            .pruned
            .iter()
            .filter(|pruned| pruned.markers_at_tip.is_some_and(|markers| markers > 0))
            .map(|pruned| format!("{} ({})", pruned.branch, days_old(pruned.idle)))
            .collect();
        if !with_markers.is_empty() {
            notice.push_str(&format!(
                " Kept branches still holding Markers: {}.",
                with_markers.join(", ")
            ));
        }
        Some(notice)
    }

    /// One warning per Session that was due but kept.
    pub fn warnings(&self) -> Vec<String> {
        let dirty = self
            .dirty
            .iter()
            .map(|id| format!("Not pruning Session {id}: its worktree has uncommitted changes."));
        let failed = self
            .failed
            .iter()
            .map(|(id, reason)| format!("Could not prune Session {id}: {reason}"));
        dirty.chain(failed).collect()
    }
}

/// Removes the folder and worktree of each Session whose last event is older than
/// `retention_days`, keeping its branch. A running Session, or one whose worktree has uncommitted
/// changes, is skipped. `retention_days` of 0 prunes nothing.
pub fn prune_sessions(repo: &Repo, retention_days: u32, now: SystemTime) -> PruneReport {
    let mut report = PruneReport {
        retention_days,
        ..PruneReport::default()
    };
    let Some(kept_for) = retention(retention_days) else {
        return report;
    };
    let folders = match session_folders(repo.root()) {
        Ok(folders) => folders,
        Err(err) => {
            report.failed.push(("(all)".to_owned(), err.to_string()));
            return report;
        }
    };
    for folder in folders {
        let idle = folder
            .last_event_at()
            .ok()
            .and_then(|last_event_at| now.duration_since(last_event_at).ok())
            .unwrap_or_default();
        if idle <= kept_for {
            continue;
        }
        let Ok(lock) = folder.lock_run() else {
            continue;
        };
        let id = folder.id();
        match prune_one(repo, &folder, lock, idle) {
            Ok(Some(pruned)) => report.pruned.push(pruned),
            Ok(None) => report.dirty.push(id),
            Err(err) => report.failed.push((id, err.to_string())),
        }
    }
    report
}

/// `None` when the worktree has uncommitted changes and is kept. The lock is let go just before
/// the folder goes, since Windows can't remove a folder with an open file in it.
fn prune_one(
    repo: &Repo,
    folder: &SessionFolder,
    lock: RunLock,
    idle: Duration,
) -> Result<Option<PrunedSession>, StoreError> {
    let worktree_path = folder.worktree_path();
    if let Ok(worktree) = Worktree::open(&worktree_path)
        && worktree.has_uncommitted_changes()?
    {
        return Ok(None);
    }
    let branch = folder.branch();
    let markers_at_tip = branch_markers(repo, &branch);
    drop(lock);
    remove_folder(repo, folder)?;
    Ok(Some(PrunedSession {
        id: folder.id(),
        branch,
        idle,
        markers_at_tip,
    }))
}

/// Deletes the Session for good: its worktree, its folder, and its branch. The only path that
/// deletes a branch, so ask the Human Operator first, naming the branch.
pub fn delete_session(repo: &Repo, folder: &SessionFolder) -> Result<(), StoreError> {
    let lock = folder.lock_run()?;
    let branch = folder.branch();
    drop(lock);
    remove_folder(repo, folder)?;
    if repo.branch_exists(&branch)? {
        repo.delete_session_branch(&branch)?;
    }
    Ok(())
}

fn remove_folder(repo: &Repo, folder: &SessionFolder) -> Result<(), StoreError> {
    repo.remove_worktree(&folder.worktree_path())?;
    fs::remove_dir_all(folder.dir()).map_err(io_error(folder.dir()))
}

fn days_old(idle: Duration) -> String {
    let days = idle.as_secs() / (24 * 60 * 60);
    format!("{days} day{} old", if days == 1 { "" } else { "s" })
}

fn counted(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}
