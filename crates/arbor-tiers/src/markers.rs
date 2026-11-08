//! Markers: the comments that bound each Region (ADR-0002).
//!
//! Only the `ARBOR:BEGIN WU-<n>` / `ARBOR:END WU-<n>` text is matched, so any comment syntax works.
//! Line numbers are 0-based indices into [`str::lines`]; error messages show them 1-based.

use std::fmt;
use std::ops::Range;

use arbor_core::WorkUnitId;

pub const BEGIN: &str = "ARBOR:BEGIN";
pub const END: &str = "ARBOR:END";

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
