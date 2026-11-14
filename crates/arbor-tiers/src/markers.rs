//! Markers: the comments that bound each Region (ADR-0002).
//!
//! Only the `ARBOR:BEGIN WU-<n>` / `ARBOR:END WU-<n>` text is matched, so any comment syntax works.
//! Line numbers are 0-based indices into [`str::lines`]; error messages show them 1-based.

use std::fmt;
use std::ops::Range;

use arbor_core::WorkUnitId;

pub const BEGIN: &str = "ARBOR:BEGIN";
pub const END: &str = "ARBOR:END";

/// Lines of context a Writer echoes on each side of its code (ADR-0002).
pub const CONTEXT_LINES: usize = 2;

pub use arbor_prompts::COMMENT_LESS_EXTENSIONS;

/// Whether `file`'s format has no comments, judged by its extension.
pub fn is_comment_less(file: &str) -> bool {
    std::path::Path::new(file)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            COMMENT_LESS_EXTENSIONS
                .iter()
                .any(|comment_less| extension.eq_ignore_ascii_case(comment_less))
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    Begin,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marker {
    pub kind: MarkerKind,
    pub id: WorkUnitId,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub id: WorkUnitId,
    pub begin: usize,
    pub end: usize,
}

impl Region {
    pub fn body(&self) -> Range<usize> {
        self.begin + 1..self.end
    }

    pub fn context_before(&self) -> Range<usize> {
        self.begin.saturating_sub(CONTEXT_LINES)..self.begin
    }

    pub fn context_after(&self, line_count: usize) -> Range<usize> {
        let start = (self.end + 1).min(line_count);
        start..(self.end + 1 + CONTEXT_LINES).min(line_count)
    }
}

/// The new lines end like the BEGIN Marker's line, so a file with `\r\n` line endings keeps them.
///
/// # Panics
///
/// If `region` doesn't come from [`Markers::parse`] of this `source`.
pub fn splice(source: &str, region: &Region, code: &[&str]) -> String {
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let eol = if lines[region.begin].ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut spliced = String::with_capacity(source.len());
    spliced.extend(lines[..=region.begin].iter().copied());
    for line in code {
        spliced.push_str(line);
        spliced.push_str(eol);
    }
    spliced.extend(lines[region.end..].iter().copied());
    spliced
}

/// # Panics
///
/// If `region` doesn't come from [`Markers::parse`] of this `source`.
pub fn strip(source: &str, region: &Region) -> String {
    assert!(region.end < source.split_inclusive('\n').count());
    source
        .split_inclusive('\n')
        .enumerate()
        .filter(|&(line, _)| line != region.begin && line != region.end)
        .map(|(_, content)| content)
        .collect()
}

/// Malformed Markers count too.
pub fn is_marker_line(line: &str) -> bool {
    parse_line(line, 0).is_some()
}

/// Only a complete `ARBOR:BEGIN WU-<n>` or `ARBOR:END WU-<n>` counts.
pub fn is_well_formed_marker(line: &str) -> bool {
    matches!(parse_line(line, 0), Some(Ok(_)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerError {
    Malformed {
        line: usize,
    },
    Unclosed {
        id: WorkUnitId,
        line: usize,
    },
    Unopened {
        id: WorkUnitId,
        line: usize,
    },
    Mismatched {
        begin: WorkUnitId,
        begin_line: usize,
        end: WorkUnitId,
        end_line: usize,
    },
    Nested {
        outer: WorkUnitId,
        outer_line: usize,
        inner: WorkUnitId,
        line: usize,
    },
    Duplicated {
        id: WorkUnitId,
        lines: Vec<usize>,
    },
    /// The second Region's context lines hold the first one's END Marker, which the first Writer
    /// strips, so the second Writer's Context Pack no longer matches the file.
    TooClose {
        first: WorkUnitId,
        end_line: usize,
        second: WorkUnitId,
        begin_line: usize,
    },
    Missing {
        id: WorkUnitId,
    },
}

impl fmt::Display for MarkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { line } => write!(
                f,
                "line {}: a Marker needs a Work Unit ID, as in `{BEGIN} WU-7`",
                line + 1
            ),
            Self::Unclosed { id, line } => {
                write!(f, "{id}: {BEGIN} on line {} has no {END}", line + 1)
            }
            Self::Unopened { id, line } => {
                write!(f, "{id}: {END} on line {} has no {BEGIN}", line + 1)
            }
            Self::Mismatched {
                begin,
                begin_line,
                end,
                end_line,
            } => write!(
                f,
                "{BEGIN} {begin} on line {} is closed by {END} {end} on line {}",
                begin_line + 1,
                end_line + 1
            ),
            Self::Nested {
                outer,
                outer_line,
                inner,
                line,
            } => write!(
                f,
                "{BEGIN} {inner} on line {} is inside {outer}, opened on line {}; Regions can't \
                 nest",
                line + 1,
                outer_line + 1
            ),
            Self::Duplicated { id, lines } => {
                let lines: Vec<String> = lines.iter().map(|line| (line + 1).to_string()).collect();
                write!(
                    f,
                    "{id} has more than one Region, on lines {}",
                    lines.join(", ")
                )
            }
            Self::TooClose {
                first,
                end_line,
                second,
                begin_line,
            } => write!(
                f,
                "{BEGIN} {second} on line {} is too close to {END} {first} on line {}; leave at \
                 least {CONTEXT_LINES} lines between Regions so neither one's context lines hold \
                 a Marker",
                begin_line + 1,
                end_line + 1
            ),
            Self::Missing { id } => write!(f, "{id} has no {BEGIN}/{END} Marker pair"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Markers {
    regions: Vec<Region>,
    errors: Vec<MarkerError>,
}

impl Markers {
    pub fn parse(source: &str) -> Self {
        let mut markers = Self::default();
        let mut open: Option<Marker> = None;
        for (line, content) in source.lines().enumerate() {
            let marker = match parse_line(content, line) {
                None => continue,
                Some(Err(error)) => {
                    markers.errors.push(error);
                    continue;
                }
                Some(Ok(marker)) => marker,
            };
            match (marker.kind, open) {
                (MarkerKind::Begin, outer) => {
                    if let Some(outer) = outer {
                        markers.errors.push(MarkerError::Nested {
                            outer: outer.id,
                            outer_line: outer.line,
                            inner: marker.id,
                            line,
                        });
                    }
                    open = Some(marker);
                }
                (MarkerKind::End, Some(begin)) if begin.id == marker.id => {
                    markers.regions.push(Region {
                        id: marker.id,
                        begin: begin.line,
                        end: line,
                    });
                    open = None;
                }
                (MarkerKind::End, Some(begin)) => {
                    markers.errors.push(MarkerError::Mismatched {
                        begin: begin.id,
                        begin_line: begin.line,
                        end: marker.id,
                        end_line: line,
                    });
                    open = None;
                }
                (MarkerKind::End, None) => markers.errors.push(MarkerError::Unopened {
                    id: marker.id,
                    line,
                }),
            }
        }
        if let Some(begin) = open {
            markers.errors.push(MarkerError::Unclosed {
                id: begin.id,
                line: begin.line,
            });
        }
        markers.find_duplicates();
        markers.find_close_regions();
        markers
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    pub fn errors(&self) -> &[MarkerError] {
        &self.errors
    }

    /// `None` unless the Work Unit has exactly one Region.
    pub fn region(&self, id: WorkUnitId) -> Option<&Region> {
        let mut found = self.regions.iter().filter(|region| region.id == id);
        match (found.next(), found.next()) {
            (Some(region), None) => Some(region),
            _ => None,
        }
    }

    /// Regions for Work Units not in `expected` are not an error here.
    pub fn check(&self, expected: &[WorkUnitId]) -> Vec<MarkerError> {
        let mut errors = self.errors.clone();
        for &id in expected {
            let mentioned = self.regions.iter().any(|region| region.id == id)
                || self.errors.iter().any(|error| error.concerns(id));
            if !mentioned {
                errors.push(MarkerError::Missing { id });
            }
        }
        errors
    }

    fn find_close_regions(&mut self) {
        for pair in self.regions.windows(2) {
            let [first, second] = pair else { continue };
            if first.id != second.id && second.begin - first.end - 1 < CONTEXT_LINES {
                self.errors.push(MarkerError::TooClose {
                    first: first.id,
                    end_line: first.end,
                    second: second.id,
                    begin_line: second.begin,
                });
            }
        }
    }

    fn find_duplicates(&mut self) {
        let mut ids: Vec<WorkUnitId> = self.regions.iter().map(|region| region.id).collect();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let lines: Vec<usize> = self
                .regions
                .iter()
                .filter(|region| region.id == id)
                .map(|region| region.begin)
                .collect();
            if lines.len() > 1 {
                self.errors.push(MarkerError::Duplicated { id, lines });
            }
        }
    }
}

impl MarkerError {
    fn concerns(&self, id: WorkUnitId) -> bool {
        match self {
            Self::Malformed { .. } => false,
            Self::Unclosed { id: other, .. }
            | Self::Unopened { id: other, .. }
            | Self::Duplicated { id: other, .. }
            | Self::Missing { id: other } => *other == id,
            Self::Mismatched { begin, end, .. } => *begin == id || *end == id,
            Self::Nested { outer, inner, .. } => *outer == id || *inner == id,
            Self::TooClose { first, second, .. } => *first == id || *second == id,
        }
    }
}

fn parse_line(content: &str, line: usize) -> Option<Result<Marker, MarkerError>> {
    let (kind, rest) = [(MarkerKind::Begin, BEGIN), (MarkerKind::End, END)]
        .into_iter()
        .find_map(|(kind, keyword)| {
            let start = content.find(keyword)?;
            let rest = &content[start + keyword.len()..];
            // `ARBOR:BEGINNING` and the like aren't Markers.
            rest.starts_with(char::is_whitespace)
                .then_some((kind, rest))
        })?;
    Some(
        parse_id(rest.trim_start())
            .map(|id| Marker { kind, id, line })
            .ok_or(MarkerError::Malformed { line }),
    )
}

fn parse_id(marker_id: &str) -> Option<WorkUnitId> {
    let digits = marker_id.strip_prefix("WU-")?;
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    let after = digits[end..].chars().next();
    if after.is_some_and(|character| character.is_alphanumeric() || character == '_') {
        return None;
    }
    digits[..end].parse().ok().map(WorkUnitId)
}
