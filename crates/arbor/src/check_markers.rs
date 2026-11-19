//! `arbor check-markers`: prints every Marker line in a tree as `file:line: text` and exits 1 if
//! there are any, so git hooks and CI can refuse them (ADR-0002).

use std::env;
use std::path::Path;
use std::process::ExitCode;

use arbor_config::{Config, Platform, user_config_path};
use arbor_git::{Repo, TreeSource};
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
    let allowlist_commit = match &check.allowlist_rev {
        Some(allowlist_rev) => Some(allowlist_rev.clone()),
        None => trusted_allowlist_commit(
            &repo,
            &check.source,
            configured_mainline(repo.root()).as_deref(),
        ),
    };
    if allowlist_commit.is_none() && matches!(check.source, TreeSource::Revision(_)) {
        eprintln!(
            "arbor: no mainline was found to read the Marker Allowlist from, so every path is \
             checked; name a commit with --allowlist-rev"
        );
    }
    let marker_lines = match scan_markers(&repo, &check.source, allowlist_commit.as_deref()) {
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
        "arbor: found {} Marker line{}. Markers must not reach the mainline or a remote; remove \
         them, or finish the Session that left them.",
        marker_lines.len(),
        if marker_lines.len() == 1 { "" } else { "s" }
    );
    ExitCode::FAILURE
}

/// The commit whose Marker Allowlist a scan of `source` uses when none is named, so a change
/// never allowlists its own Markers. A commit is checked against `HEAD`, the commit it builds on.
/// A revision, e.g. a push, may hold any number of commits, so it is checked against the mainline
/// tip, which only reviewed changes reach. `None` when there is no such commit.
fn trusted_allowlist_commit(
    repo: &Repo,
    source: &TreeSource,
    chosen_mainline: Option<&str>,
) -> Option<String> {
    match source {
        TreeSource::WorkingTree | TreeSource::Staged => repo.head_commit(),
        TreeSource::Revision(_) => repo
            .mainline(chosen_mainline)
            .ok()
            .map(|mainline| mainline.start_point()),
    }
}

/// The mainline set in `arbor.toml`, if any. A config that won't load leaves Arbor's guess.
fn configured_mainline(root: &Path) -> Option<String> {
    let user_config = user_config_path(Platform::current(), |name| env::var(name).ok());
    Config::load(user_config.as_deref(), root)
        .ok()
        .and_then(|config| config.mainline)
}

fn could_not_check(message: &str) -> ExitCode {
    eprintln!("arbor: could not check for Markers: {message}");
    ExitCode::from(COULD_NOT_CHECK)
}
