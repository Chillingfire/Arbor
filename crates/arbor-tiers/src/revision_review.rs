//! Whether a submitted Revision makes a real change or only rewords (grilling log, Round 2
//! decision 4). A rejected Revision still counts toward the Revision limit.

use std::fs;
use std::path::Path;

use arbor_core::{Event, Revision, WorkUnit};
use arbor_git::{GitError, Repo, Worktree};

use crate::markers::{Markers, is_marker_line};
use crate::task_completion::{new_file_path, parse_hunk};

pub const REWORDING_ONLY: &str = "the Revision changed no Context Pack, acceptance criteria, test \
                                  filter, file or Work Unit split, and nothing in the Skeleton or \
                                  tests outside the Regions, so it only rewords the spec";

/// Returns `RevisionSubmitted`, or `RevisionRejected` for a Revision that only rewords.
/// `task_work_units` are the Task's Work Units before the Revision, i.e. `Session::work_units`, so
/// a Revision that repeats the previous one is rejected too.
pub fn review_revision(
    worktree: &Path,
    task_work_units: &[WorkUnit],
    revision: Revision,
) -> Result<Event, GitError> {
    if revision.shared_change.is_some()
        || changes_work_units(task_work_units, &revision)
        || worktree_changed_outside_regions(worktree)?
    {
        return Ok(Event::RevisionSubmitted { revision });
    }
    Ok(Event::RevisionRejected {
        revision,
        reason: REWORDING_ONLY.to_owned(),
    })
}

/// A spec change alone doesn't count: Arbor can't tell a tighter spec from a reworded one.
pub fn changes_work_units(task_work_units: &[WorkUnit], revision: &Revision) -> bool {
    revision.work_units.iter().any(|revised| {
        match task_work_units.iter().find(|unit| unit.id == revised.id) {
            None => true,
            Some(before) => {
                before.file != revised.file
                    || before.context != revised.context
                    || before.acceptance_criteria != revised.acceptance_criteria
                    || before.test_filter != revised.test_filter
            }
        }
    })
}

fn worktree_changed_outside_regions(path: &Path) -> Result<bool, GitError> {
    let worktree = Worktree::open(path)?;
    let diff = worktree.diff_from(&worktree.head()?)?;
    let read_current = |file: &str| fs::read_to_string(worktree.path().join(file)).ok();
    if changes_outside_regions(&diff, read_current) {
        return Ok(true);
    }
    let changed_paths = Repo::open(path)?.uncommitted_changes()?;
    let untracked = changed_paths
        .iter()
        .any(|changed| !diff.contains(&format!("+++ b/{changed}")));
    Ok(untracked)
}

/// Whether a diff to the worktree changes a line outside the Regions of the files as they are
/// now. Marking a Region again and putting its Placeholder back don't count, since every
/// Revision of a written Work Unit has to.
pub fn changes_outside_regions(diff: &str, read_current: impl Fn(&str) -> Option<String>) -> bool {
    let mut regions: Option<Vec<(usize, usize)>> = None;
    let mut lines = diff.lines();
    while let Some(line) = lines.next() {
        if let Some(path) = line.strip_prefix("+++ ") {
            regions = new_file_path(path)
                .and_then(|file| read_current(&file))
                .map(|file_text| region_spans(&file_text));
            continue;
        }
        let Some(hunk) = line.strip_prefix("@@ ").and_then(parse_hunk) else {
            continue;
        };
        let Some(spans) = &regions else {
            return true;
        };
        let inside = |index: usize| {
            spans
                .iter()
                .any(|&(begin, end)| (begin..=end).contains(&index))
        };
        let (mut old_left, mut new_left) = (hunk.old_lines, hunk.new_lines);
        let mut index = hunk.new_start.saturating_sub(1);
        while old_left + new_left > 0 {
            let Some(hunk_line) = lines.next() else { break };
            match hunk_line.as_bytes().first() {
                Some(b'+') => {
                    if !inside(index) && !is_marker_line(&hunk_line[1..]) {
                        return true;
                    }
                    index += 1;
                    new_left = new_left.saturating_sub(1);
                }
                Some(b'-') => {
                    if !inside(index) {
                        return true;
                    }
                    old_left = old_left.saturating_sub(1);
                }
                Some(b'\\') => {}
                _ => {
                    index += 1;
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                }
            }
        }
    }
    false
}

/// 0-based line indices of each Region's BEGIN and END Markers.
fn region_spans(source: &str) -> Vec<(usize, usize)> {
    Markers::parse(source)
        .regions()
        .iter()
        .map(|region| (region.begin, region.end))
        .collect()
}
