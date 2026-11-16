//! The Marker scan Arbor runs at startup and at Session end, and `arbor check-markers` runs for
//! git hooks and CI: every line of a tree that holds a Marker, well-formed or not (ADR-0002).

use std::fmt;

use arbor_git::{GitError, LineMatch, Repo, TreeSource};

use crate::markers::{BEGIN, END, is_begin_marker, is_marker_line};

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

/// Every Marker line in the tracked text files of `source`, in file order.
pub fn scan_markers(repo: &Repo, source: &TreeSource) -> Result<Vec<MarkerLine>, GitError> {
    // git narrows the search to likely lines; `is_marker_line` decides, so both agree on what a
    // Marker is.
    let pattern = format!("({BEGIN}|{END})[[:space:]]");
    let candidates = repo.grep(&pattern, source)?;
    Ok(candidates
        .into_iter()
        .filter(|candidate| is_marker_line(&candidate.text))
        .map(marker_line)
        .collect())
}

fn marker_line(line_match: LineMatch) -> MarkerLine {
    MarkerLine {
        file: line_match.file,
        line: line_match.line,
        text: line_match.text.trim().to_owned(),
    }
}
