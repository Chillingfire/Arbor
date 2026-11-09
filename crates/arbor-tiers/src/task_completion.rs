//! What runs once a Task's Work Units have all passed: the full Unit Check (ADR-0003), and once
//! every Task is done, the check for leftover Markers (ADR-0002).

use std::path::Path;

use arbor_config::Checks;
use arbor_core::{Event, LeftoverMarker, SessionCommit};
use arbor_git::{CommitId, CommitMessage, GitError, Mainline, Worktree};
use arbor_tools::{CheckName, run_check};

use crate::markers::is_well_formed_marker;

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
        let added = hunk_added_lines(&mut lines, hunk);
        leftovers.extend(added.filter(|(_, text)| is_well_formed_marker(text)).map(
            |(number, text)| LeftoverMarker {
                file: file.clone(),
                line: number,
                text: text.trim().to_owned(),
            },
        ));
    }
    leftovers
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Hunk {
    pub(crate) old_lines: usize,
    pub(crate) new_start: usize,
    pub(crate) new_lines: usize,
}

/// Parses the rest of `@@ -1,3 +1,4 @@`, where a missing count means 1.
pub(crate) fn parse_hunk(header: &str) -> Option<Hunk> {
    let mut ranges = header.split_whitespace();
    let (_, old_lines) = parse_range(ranges.next()?.strip_prefix('-')?)?;
    let (new_start, new_lines) = parse_range(ranges.next()?.strip_prefix('+')?)?;
    Some(Hunk {
        old_lines,
        new_start,
        new_lines,
    })
}

fn parse_range(range: &str) -> Option<(usize, usize)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

/// Consumes exactly the hunk's lines, so an added line that starts with `++` is never taken for
/// a file header.
fn hunk_added_lines<'a>(
    lines: &mut std::str::Lines<'a>,
    hunk: Hunk,
) -> impl Iterator<Item = (usize, &'a str)> {
    let mut added = Vec::new();
    let (mut old_left, mut new_left) = (hunk.old_lines, hunk.new_lines);
    let mut number = hunk.new_start;
    while old_left + new_left > 0 {
        let Some(line) = lines.next() else { break };
        match line.as_bytes().first() {
            Some(b'+') => {
                added.push((number, &line[1..]));
                number += 1;
                new_left = new_left.saturating_sub(1);
            }
            Some(b'-') => old_left = old_left.saturating_sub(1),
            Some(b'\\') => {}
            _ => {
                number += 1;
                old_left = old_left.saturating_sub(1);
                new_left = new_left.saturating_sub(1);
            }
        }
    }
    added.into_iter()
}

/// `b/src/lib.rs` gives `src/lib.rs`; a deleted file (`/dev/null`) gives `None`.
pub(crate) fn new_file_path(header: &str) -> Option<String> {
    let path = header.trim_end().trim_matches('"');
    path.strip_prefix("b/").map(str::to_owned)
}
