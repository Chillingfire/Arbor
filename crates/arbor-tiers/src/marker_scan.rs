//! The Marker scan Arbor runs at startup, and `arbor check-markers` runs for git hooks and CI:
//! every line of a tree that holds a Marker, well-formed or not, outside the paths the Marker
//! Allowlist of a trusted commit names (ADR-0002).

use std::fmt;

use arbor_git::{GitError, LineMatch, Repo, TreeSource};

use crate::marker_allowlist::{AllowlistError, MARKER_ALLOWLIST_FILE, MarkerAllowlist};
use crate::markers::{BEGIN, END, is_begin_marker, is_marker_line};

#[derive(Debug)]
pub enum ScanError {
    Git(GitError),
    Allowlist(AllowlistError),
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(err) => err.fmt(f),
            Self::Allowlist(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for ScanError {}

impl From<GitError> for ScanError {
    fn from(err: GitError) -> Self {
        Self::Git(err)
    }
}

/// A line holding a Marker, as `arbor check-markers` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerLine {
    pub file: String,
    /// 1-based.
    pub line: usize,
    /// The line with surrounding whitespace trimmed.
    pub text: String,
}

impl fmt::Display for MarkerLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.line, self.text)
    }
}

impl MarkerLine {
    /// Whether the line opens a Region, so a Placeholder still follows it.
    pub fn opens_region(&self) -> bool {
        is_begin_marker(&self.text)
    }
}

/// Every Marker line in the tracked text files of `source`, in file order, except in the paths
/// the Marker Allowlist at `allowlist_commit` names. With no commit, every path is checked.
///
/// The list comes from a commit the caller trusts, never from `source` itself, so a change can't
/// allowlist its own Markers.
pub fn scan_markers(
    repo: &Repo,
    source: &TreeSource,
    allowlist_commit: Option<&str>,
) -> Result<Vec<MarkerLine>, ScanError> {
    let allowlist = match allowlist_commit {
        Some(commit) => allowlist_at(repo, commit)?,
        None => MarkerAllowlist::default(),
    };
    // git narrows the search to likely lines; `is_marker_line` decides, so both agree on what a
    // Marker is.
    let pattern = format!("({BEGIN}|{END})[[:space:]]");
    let candidates = repo.grep(&pattern, source)?;
    Ok(candidates
        .into_iter()
        .filter(|candidate| is_marker_line(&candidate.text) && !allowlist.allows(&candidate.file))
        .map(marker_line)
        .collect())
}

/// The Marker Allowlist committed at `commit`; empty when it has none.
pub fn allowlist_at(repo: &Repo, commit: &str) -> Result<MarkerAllowlist, ScanError> {
    let committed = TreeSource::Revision(commit.to_owned());
    match repo.read_file(MARKER_ALLOWLIST_FILE, &committed)? {
        Some(allowlist_text) => {
            MarkerAllowlist::parse(&allowlist_text).map_err(ScanError::Allowlist)
        }
        None => Ok(MarkerAllowlist::default()),
    }
}

fn marker_line(line_match: LineMatch) -> MarkerLine {
    MarkerLine {
        file: line_match.file,
        line: line_match.line,
        text: line_match.text.trim().to_owned(),
    }
}
