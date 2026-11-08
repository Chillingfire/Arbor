//! Markers: the comments that bound each Region (ADR-0002).
//!
//! A Marker is a line holding `ARBOR:BEGIN WU-<n>` or `ARBOR:END WU-<n>`. Only that text is
//! matched, so the comment syntax around it doesn't matter: `// ARBOR:BEGIN WU-7`,
//! `# ARBOR:END WU-7` and `<!-- ARBOR:BEGIN WU-7 -->` are all Markers.
//!
//! This lives in `arbor-tiers` because the Tiers are its only users: the Scaffolder checks the
//! Markers it wrote, and the Writer locates a Region by its Work Unit's ID to splice code into it.
//! It does no I/O, so it can move to `arbor-core` if anything outside the Tiers needs it.
//!
//! Line numbers are 0-based indices into [`str::lines`], which match the indices of
//! `split_inclusive('\n')` when a caller needs to keep line endings. [`MarkerError`]'s messages
//! show them 1-based, as editors do.

use std::fmt;
use std::ops::Range;

use arbor_core::WorkUnitId;

/// Starts a Region.
pub const BEGIN: &str = "ARBOR:BEGIN";
/// Ends a Region.
pub const END: &str = "ARBOR:END";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    Begin,
    End,
}

/// One Marker line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marker {
    pub kind: MarkerKind,
    pub id: WorkUnitId,
    pub line: usize,
}

/// A Region: a BEGIN and END Marker pair for one Work Unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub id: WorkUnitId,
    /// The BEGIN Marker's line.
    pub begin: usize,
    /// The END Marker's line.
    pub end: usize,
}

impl Region {
    /// The lines between the Markers: the Placeholder, and later the Writer's code.
    pub fn body(&self) -> Range<usize> {
        self.begin + 1..self.end
    }
}

/// A problem with a file's Markers. Any of them fails the Scaffold Check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerError {
    /// A line holds `ARBOR:BEGIN` or `ARBOR:END` without a readable `WU-<n>` after it.
    Malformed { line: usize },
    /// A BEGIN Marker with no END after it.
    Unclosed { id: WorkUnitId, line: usize },
    /// An END Marker with no BEGIN before it.
    Unopened { id: WorkUnitId, line: usize },
    /// A Region opened for one Work Unit and closed for another.
    Mismatched {
        begin: WorkUnitId,
        begin_line: usize,
        end: WorkUnitId,
        end_line: usize,
    },
    /// A BEGIN Marker inside another Region. Regions can't nest.
    Nested {
        outer: WorkUnitId,
        outer_line: usize,
        inner: WorkUnitId,
        line: usize,
    },
    /// More than one Region for the same Work Unit, starting on these lines.
    Duplicated { id: WorkUnitId, lines: Vec<usize> },
    /// No Region for a Work Unit that should have one.
    Missing { id: WorkUnitId },
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
            Self::Missing { id } => write!(f, "{id} has no {BEGIN}/{END} Marker pair"),
        }
    }
}

/// Every Marker in one file, paired into Regions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Markers {
    regions: Vec<Region>,
    errors: Vec<MarkerError>,
}

impl Markers {
    /// Finds and pairs the Markers in a file's text.
    pub fn parse(text: &str) -> Self {
        let mut markers = Self::default();
        let mut open: Option<Marker> = None;
        for (line, content) in text.lines().enumerate() {
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
        markers
    }

    /// Every Region whose Markers pair up, in file order, duplicates included.
    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    /// The problems found while pairing, in file order except that duplicates come last.
    pub fn errors(&self) -> &[MarkerError] {
        &self.errors
    }

    /// The Work Unit's Region, if it has exactly one.
    pub fn region(&self, id: WorkUnitId) -> Option<&Region> {
        let mut found = self.regions.iter().filter(|region| region.id == id);
        match (found.next(), found.next()) {
            (Some(region), None) => Some(region),
            _ => None,
        }
    }

    /// Every problem with the file's Markers, given the Work Units it should hold a Region for.
    /// Regions for other Work Units are not an error here.
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
    /// Whether the error is about this Work Unit's Markers.
    fn concerns(&self, id: WorkUnitId) -> bool {
        match self {
            Self::Malformed { .. } => false,
            Self::Unclosed { id: other, .. }
            | Self::Unopened { id: other, .. }
            | Self::Duplicated { id: other, .. }
            | Self::Missing { id: other } => *other == id,
            Self::Mismatched { begin, end, .. } => *begin == id || *end == id,
            Self::Nested { outer, inner, .. } => *outer == id || *inner == id,
        }
    }
}

/// Reads a Marker from one line. `None` if the line holds no Marker.
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

/// Reads `WU-<n>` at the start of `text`, followed by the end of the line or a character that
/// can't be part of the ID, such as the space before `-->`.
fn parse_id(text: &str) -> Option<WorkUnitId> {
    let digits = text.strip_prefix("WU-")?;
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    let after = digits[end..].chars().next();
    if after.is_some_and(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    digits[..end].parse().ok().map(WorkUnitId)
}
