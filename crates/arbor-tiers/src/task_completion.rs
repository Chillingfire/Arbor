//! What runs once a Task's Work Units have all passed: the full Unit Check (ADR-0003), and once
//! every Task is done, the check for leftover Markers (ADR-0002).

use std::path::Path;

use arbor_config::Checks;
use arbor_core::{Event, LeftoverMarker, SessionCommit, WorkUnitId};
use arbor_git::{CommitId, CommitMessage, GitError, Mainline, Worktree};
use arbor_tools::{CheckName, run_check};

use crate::markers::{Markers, is_well_formed_marker, marker_lines_of, strip};
use crate::unified_diff::{HunkLineKind, hunk_lines, new_file_path, parse_hunk};

pub const NO_UNIT_CHECK: &str = "No unit check is configured in arbor.toml, so Arbor can't run \
                                 the Task's tests. Skipping it.";

/// Runs `checks.unit` and returns `TaskChecksPassed` or `TaskChecksFailed`. With no unit check
/// configured the Task passes, after [`task_check_warning`]. `Err` means the Check couldn't run.
pub async fn run_task_check(worktree: &Path, checks: &Checks) -> Result<Event, String> {
    if checks.unit.is_none() {
        return Ok(Event::TaskChecksPassed);
    }
    let check_output = run_check(worktree, checks, CheckName::Unit, None).await?;
    Ok(if check_output.is_error {
        Event::TaskChecksFailed {
            output: check_output.text,
        }
    } else {
        Event::TaskChecksPassed
    })
}

pub fn task_check_warning(checks: &Checks) -> Option<&'static str> {
    checks.unit.is_none().then_some(NO_UNIT_CHECK)
}

/// Everything the Task changed: from the parent of its first commit, or from `HEAD` before it
/// has any, to the files in the worktree. Takes `Session::task_commits`.
pub fn task_diff(worktree: &Worktree, task_commits: &[SessionCommit]) -> Result<String, GitError> {
    worktree.diff_from(&task_start(worktree, task_commits)?)
}

/// Resets the worktree to where the Task started, after its Replan is approved, and returns
/// `TaskReset`. Takes `Session::task_commits`.
pub fn reset_task(worktree: &Worktree, task_commits: &[SessionCommit]) -> Result<Event, GitError> {
    worktree.reset_to(&task_start(worktree, task_commits)?)?;
    Ok(Event::TaskReset)
}

fn task_start(worktree: &Worktree, task_commits: &[SessionCommit]) -> Result<CommitId, GitError> {
    match task_commits.first() {
        Some(first) => worktree.parent_of(&CommitId::from(first.hash.clone())),
        None => worktree.head(),
    }
}

/// Default subjects for [`commit_operator_fix`]. Like every commit, they don't mention Arbor
/// (ADR-0009).
pub const TASK_FIX_SUBJECT: &str = "Fix the failing tests.";
pub const MARKER_FIX_SUBJECT: &str = "Remove leftover placeholder comments.";

/// Commits what the Human Operator changed by hand at a Checkpoint, with their git identity, and
/// returns `OperatorFixed`. Committing first keeps the fix out of the next Work Unit's commit and
/// lets Rewind see it.
pub fn commit_operator_fix(
    worktree: &Worktree,
    message: &CommitMessage,
) -> Result<Event, GitError> {
    let commit = worktree.commit(message)?;
    Ok(Event::OperatorFixed {
        commit: commit.map(|hash| hash.as_str().to_owned()),
    })
}

/// Removes the Region Markers of `id` from `file`, if the Human Operator's fix left them there.
/// Fails without committing anything if a Marker for `id` would still be left, e.g. a second
/// Region the fix added.
pub fn strip_work_unit_markers(worktree: &Path, file: &str, id: WorkUnitId) -> Result<(), String> {
    let path = worktree.join(file);
    let source = std::fs::read_to_string(&path).map_err(|error| format!("{file}: {error}"))?;
    let stripped = match Markers::parse(&source).region(id) {
        Some(region) => strip(&source, region),
        None => source,
    };
    let lines_left = marker_lines_of(&stripped, id);
    if !lines_left.is_empty() {
        let lines: Vec<String> = lines_left
            .iter()
            .map(|line| (line + 1).to_string())
            .collect();
        return Err(format!(
            "{file} still holds a Marker for {id} on line {}; remove it and fix again",
            lines.join(", ")
        ));
    }
    std::fs::write(&path, stripped).map_err(|error| format!("{file}: {error}"))
}

/// Runs the Checks a fix by hand must pass at an Escalation Checkpoint, and returns the output of
/// the first that fails. With a `test_filter`, the fix is of one Work Unit, so it passes the same
/// Checks a Writer's code does: later Work Units' Placeholders would fail the full Unit Check.
/// Without one, the fix is of a failed Task, so it passes `checks.unit`.
pub async fn check_operator_fix(
    worktree: &Path,
    checks: &Checks,
    test_filter: Option<&str>,
) -> Result<Option<String>, String> {
    let runs = match test_filter {
        Some(filter) => vec![
            (CheckName::Scaffold, checks.scaffold.is_some(), None),
            (
                CheckName::UnitFiltered,
                checks.unit_filtered.is_some(),
                Some(filter),
            ),
        ],
        None => vec![(CheckName::Unit, checks.unit.is_some(), None)],
    };
    for (check, configured, filter) in runs {
        if !configured {
            continue;
        }
        let check_output = run_check(worktree, checks, check, filter).await?;
        if check_output.is_error {
            return Ok(Some(check_output.text));
        }
    }
    Ok(None)
}

/// Looks for Markers in what the Session branch has committed and returns `MarkersChecked`.
/// Uncommitted changes don't count, since a push or merge would leave them behind.
pub fn check_markers(worktree: &Worktree, mainline: &Mainline) -> Result<Event, GitError> {
    let branch_diff = worktree.committed_diff_from(&worktree.fork_point(mainline)?)?;
    Ok(Event::MarkersChecked {
        remaining: leftover_markers(&branch_diff),
    })
}

/// The lines a unified diff adds that hold a well-formed Marker, with their line numbers in the new
/// file. A line that only mentions `ARBOR:`, e.g. in docs or a constant, is not one.
pub fn leftover_markers(diff: &str) -> Vec<LeftoverMarker> {
    let mut leftovers = Vec::new();
    let mut file: Option<String> = None;
    let mut lines = diff.lines();
    while let Some(line) = lines.next() {
        if let Some(path) = line.strip_prefix("+++ ") {
            file = new_file_path(path);
            continue;
        }
        let (Some(file), Some(hunk)) = (&file, line.strip_prefix("@@ ").and_then(parse_hunk))
        else {
            continue;
        };
        for hunk_line in hunk_lines(&mut lines, hunk) {
            if let HunkLineKind::Added(added_line) = hunk_line.kind
                && is_well_formed_marker(added_line)
            {
                leftovers.push(LeftoverMarker {
                    file: file.clone(),
                    line: hunk.new_start + hunk_line.new_offset,
                    text: added_line.trim().to_owned(),
                });
            }
        }
    }
    leftovers
}
