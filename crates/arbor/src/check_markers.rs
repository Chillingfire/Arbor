//! `arbor check-markers`: prints every Marker line in a tree as `file:line: text` and exits 1 if
//! there are any, so git hooks and CI can refuse them (ADR-0002).

use std::env;
use std::process::ExitCode;

use arbor_git::Repo;
use arbor_tiers::scan_markers;

use crate::cli::CheckMarkers;

/// Exit status when the check couldn't run. A hook treats it as a refusal too.
const COULD_NOT_CHECK: u8 = 2;

pub fn run(check: &CheckMarkers) -> ExitCode {
    let repo = match env::current_dir().map(Repo::open) {
        Ok(Ok(repo)) => repo,
        Ok(Err(err)) => return could_not_check(&format!("{err}; run it inside a git repository")),
        Err(err) => {
            return could_not_check(&format!("could not read the current directory: {err}"));
        }
    };
    if check.allow_session_worktree && repo.is_session_worktree() {
        return ExitCode::SUCCESS;
    }
    let marker_lines = match scan_markers(&repo, &check.source) {
        Ok(marker_lines) => marker_lines,
        Err(err) => return could_not_check(&err.to_string()),
    };
    if marker_lines.is_empty() {
        return ExitCode::SUCCESS;
    }
    for marker_line in &marker_lines {
        println!("{marker_line}");
    }
    eprintln!(
        "arbor: found {} Marker line{}. Markers belong only to an unfinished Session's branch; \
         remove them before this reaches the mainline.",
        marker_lines.len(),
        if marker_lines.len() == 1 { "" } else { "s" }
    );
    ExitCode::FAILURE
}

fn could_not_check(message: &str) -> ExitCode {
    eprintln!("arbor: could not check for Markers: {message}");
    ExitCode::from(COULD_NOT_CHECK)
}
