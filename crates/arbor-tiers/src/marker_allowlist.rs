//! The Marker allowlist: a committed file of gitignore-style patterns naming the paths where
//! Markers are expected, e.g. tests and docs about Markers. It narrows where the Marker scan
//! looks; it can't turn the scan off, so a pattern that would allow every path is refused.

use std::fmt;

/// The allowlist's file, at the repository's top level.
pub const MARKER_ALLOWLIST_FILE: &str = ".arbor-markers-allowed";

/// A line of the allowlist that Arbor won't use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowlistError {
    /// 1-based.
    pub line: usize,
    pub pattern: String,
    pub reason: &'static str,
}

impl fmt::Display for AllowlistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{MARKER_ALLOWLIST_FILE} line {}: `{}` {}",
            self.line, self.pattern, self.reason
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MarkerAllowlist {
    patterns: Vec<Pattern>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    text: String,
    segments: Vec<String>,
    /// Matched at the top level only, not at any depth.
    anchored: bool,
    /// Ends in `/`, so it matches only a directory, and with it everything inside.
    directory_only: bool,
}

impl MarkerAllowlist {
    /// Parses the allowlist's text. Blank lines and lines starting with `#` are skipped.
    pub fn parse(allowlist_text: &str) -> Result<Self, AllowlistError> {
        let mut patterns = Vec::new();
        for (line_index, raw_line) in allowlist_text.lines().enumerate() {
            let pattern_text = raw_line.trim();
            if pattern_text.is_empty() || pattern_text.starts_with('#') {
                continue;
            }
            let refuse = |reason| AllowlistError {
                line: line_index + 1,
                pattern: pattern_text.to_owned(),
                reason,
            };
            if pattern_text.starts_with('!') {
                return Err(refuse(
                    "negates a pattern, which the allowlist doesn't support",
                ));
            }
            if !pattern_text
                .chars()
                .any(|character| !"*?/".contains(character))
            {
                return Err(refuse(
                    "would allow Markers everywhere; name the paths where they are expected",
                ));
            }
            patterns.push(Pattern::parse(pattern_text));
        }
        Ok(Self { patterns })
    }

    /// Whether Markers are expected in `file`, a path relative to the top level with `/`
    /// separators.
    pub fn allows(&self, file: &str) -> bool {
        let path_segments: Vec<&str> = file.split('/').collect();
        self.patterns
            .iter()
            .any(|pattern| pattern.matches(&path_segments))
    }

    /// The patterns that each allow more than half of `tracked_files`, such as `*.rs` or `src/`,
    /// so the Marker check skips most of the code. Too few files to judge gives none.
    pub fn broad_patterns(&self, tracked_files: &[String]) -> Vec<BroadPattern> {
        if tracked_files.len() < MIN_FILES_TO_JUDGE_BREADTH {
            return Vec::new();
        }
        let split_files: Vec<Vec<&str>> = tracked_files
            .iter()
            .map(|file| file.split('/').collect())
            .collect();
        self.patterns
            .iter()
            .filter_map(|pattern| {
                let allowed_files = split_files
                    .iter()
                    .filter(|path_segments| pattern.matches(path_segments))
                    .count();
                (allowed_files * 2 > tracked_files.len()).then(|| BroadPattern {
                    pattern: pattern.text.clone(),
                    allowed_files,
                    tracked_files: tracked_files.len(),
                })
            })
            .collect()
    }
}

/// A repository with fewer tracked files than this is too small for a share of them to say a
/// pattern is broad.
const MIN_FILES_TO_JUDGE_BREADTH: usize = 10;

/// An allowlist pattern that allows most of the tracked files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BroadPattern {
    pub pattern: String,
    pub allowed_files: usize,
    pub tracked_files: usize,
}

impl fmt::Display for BroadPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{MARKER_ALLOWLIST_FILE}: `{}` allows {} of the {} tracked files, so the Marker check \
             skips them all; name only the paths where Markers are expected",
            self.pattern, self.allowed_files, self.tracked_files
        )
    }
}

impl Pattern {
    fn parse(pattern_text: &str) -> Self {
        let directory_only = pattern_text.ends_with('/');
        let trimmed = pattern_text.trim_end_matches('/');
        let anchored = trimmed.contains('/');
        let segments = trimmed
            .trim_start_matches('/')
            .split('/')
            .map(str::to_owned)
            .collect();
        Self {
            text: pattern_text.to_owned(),
            segments,
            anchored,
            directory_only,
        }
    }

    /// Like gitignore, a pattern matching a directory covers everything inside it.
    fn matches(&self, path_segments: &[&str]) -> bool {
        let last_prefix = if self.directory_only {
            path_segments.len().saturating_sub(1)
        } else {
            path_segments.len()
        };
        let pattern_segments: Vec<&str> = self.segments.iter().map(String::as_str).collect();
        (1..=last_prefix).any(|prefix_length| {
            let prefix = &path_segments[..prefix_length];
            if self.anchored {
                segments_match(&pattern_segments, prefix)
            } else {
                prefix
                    .last()
                    .is_some_and(|name| segment_matches(pattern_segments[0], name))
            }
        })
    }
}

/// `**` matches any number of whole path segments, including none.
fn segments_match(pattern_segments: &[&str], path_segments: &[&str]) -> bool {
    match pattern_segments.split_first() {
        None => path_segments.is_empty(),
        Some((&"**", rest)) => {
            (0..=path_segments.len()).any(|skipped| segments_match(rest, &path_segments[skipped..]))
        }
        Some((first, rest)) => path_segments
            .split_first()
            .is_some_and(|(name, path_rest)| {
                segment_matches(first, name) && segments_match(rest, path_rest)
            }),
    }
}

/// `*` matches any run of characters and `?` any one character, within one segment.
fn segment_matches(pattern: &str, name: &str) -> bool {
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let name_chars: Vec<char> = name.chars().collect();
    wildcard_match(&pattern_chars, &name_chars)
}

fn wildcard_match(pattern: &[char], name: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => (0..=name.len()).any(|skipped| wildcard_match(rest, &name[skipped..])),
        Some((&'?', rest)) => !name.is_empty() && wildcard_match(rest, &name[1..]),
        Some((literal, rest)) => name.first() == Some(literal) && wildcard_match(rest, &name[1..]),
    }
}
