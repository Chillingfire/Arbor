//! The Writer's output contract (Round 5 decision 5): exactly one fenced block and nothing else.
//!
//! - Code, fenced with the file's language, e.g. ```` ```rust ````: the lines just above the
//!   BEGIN Marker, then the code for the Region, then the lines just below the END Marker. The
//!   context lines are the [`CONTEXT_LINES`] lines just outside the Marker lines, never the Markers
//!   themselves, and fewer only where the file has fewer (see [`Region::context_before`]).
//! - A Missing Response, fenced as ```` ```missing ````, with one needed item per line.
//!
//! Anything else is a [`ReplyError`], which costs the Writer one of its attempts. Parsing is pure:
//! [`parse_reply`] reads the reply, then [`CodeReply::fit`] checks its context lines against the
//! file and returns the code to splice.
//!
//! [`Region::context_before`]: crate::markers::Region::context_before

use std::fmt;

use crate::markers::{CONTEXT_LINES, END, is_marker_line};

/// The fence info string of a Missing Response.
pub const MISSING: &str = "missing";

/// A Writer reply that follows the output contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriterReply {
    Code(CodeReply),
    /// A Missing Response: what the Context Pack lacked, one item per entry.
    Missing(Vec<String>),
}

/// A fenced code block, context lines included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeReply {
    /// The fence's language, e.g. `rust`. Empty for a bare fence, which is accepted.
    pub language: String,
    /// Every line inside the fence.
    pub lines: Vec<String>,
}

/// Which side of the code a context line is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Before,
    After,
}

/// Why a reply was rejected. Its message goes back to the Writer, so it says what to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyError {
    /// The reply was empty.
    Empty,
    /// The provider stopped the reply at its token limit.
    CutOff,
    /// No fenced block.
    NoBlock,
    /// A fence that is never closed.
    Unclosed,
    /// More than one fenced block.
    SeveralBlocks(usize),
    /// Text outside the fenced block.
    TextOutside,
    /// A `missing` block that lists nothing.
    EmptyMissing,
    /// Too few lines to hold the context lines.
    TooShort { needed: usize, found: usize },
    /// Context lines that don't match the file.
    ContextMismatch {
        side: Side,
        expected: Vec<String>,
        found: Vec<String>,
    },
    /// The code holds a Marker line, which only Arbor may write.
    MarkerInCode { line: String },
}

impl fmt::Display for ReplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the reply was empty"),
            Self::CutOff => f.write_str(
                "the reply was cut off at the token limit before the fenced block was finished",
            ),
            Self::NoBlock => f.write_str("the reply has no fenced block"),
            Self::Unclosed => f.write_str("the fenced block is never closed with ```"),
            Self::SeveralBlocks(count) => {
                write!(f, "the reply has {count} fenced blocks; send exactly one")
            }
            Self::TextOutside => f.write_str(
                "the reply has text outside the fenced block; send only the block, with no \
                 explanation before or after it",
            ),
            Self::EmptyMissing => f.write_str(
                "the `missing` block lists nothing; name each missing item on its own line",
            ),
            Self::TooShort { needed, found } => write!(
                f,
                "the code block has {found} lines, but the context lines alone take {needed}; \
                 start with the lines just above the BEGIN Marker and end with the lines just \
                 below the END Marker"
            ),
            Self::ContextMismatch {
                side,
                expected,
                found,
            } => {
                let (count, place) = match side {
                    Side::Before => ("first", "just above the BEGIN Marker"),
                    Side::After => ("last", "just below the END Marker"),
                };
                write!(
                    f,
                    "the {count} {} lines of the code block must be the lines {place}, \
                     unchanged and without the Marker line.\nExpected:\n{}\nFound:\n{}",
                    expected.len(),
                    quoted(expected),
                    quoted(found)
                )
            }
            Self::MarkerInCode { line } => write!(
                f,
                "the code holds a Marker line ({line:?}); leave out the BEGIN and {END} Marker \
                 lines, Arbor keeps them"
            ),
        }
    }
}

impl std::error::Error for ReplyError {}

/// Reads a Writer's reply. It must be exactly one fenced block, surrounded by nothing but
/// whitespace.
///
/// A fence opens with a line of three or more backticks and an optional info string, and closes
/// with a line of only backticks, at least as many as opened it. Lines inside are kept as they
/// are, so a longer fence can hold code that itself contains ```` ``` ````.
pub fn parse_reply(text: &str) -> Result<WriterReply, ReplyError> {
    if text.trim().is_empty() {
        return Err(ReplyError::Empty);
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut outside = false;
    let mut i = 0;
    while i < lines.len() {
        let Some((ticks, info)) = opening_fence(lines[i]) else {
            outside |= !lines[i].trim().is_empty();
            i += 1;
            continue;
        };
        let close = (i + 1..lines.len())
            .find(|&j| closes(lines[j], ticks))
            .ok_or(ReplyError::Unclosed)?;
        blocks.push((info, &lines[i + 1..close]));
        i = close + 1;
    }
    let (info, body) = match blocks.as_slice() {
        [] => return Err(ReplyError::NoBlock),
        [block] => *block,
        _ => return Err(ReplyError::SeveralBlocks(blocks.len())),
    };
    if outside {
        return Err(ReplyError::TextOutside);
    }
    let language = info.split_whitespace().next().unwrap_or_default();
    if language.eq_ignore_ascii_case(MISSING) {
        let items: Vec<String> = body
            .iter()
            .map(|line| {
                // A list bullet is not part of the item.
                let line = line.trim();
                line.strip_prefix(['-', '*'])
                    .filter(|rest| rest.is_empty() || rest.starts_with(' '))
                    .unwrap_or(line)
                    .trim()
                    .to_owned()
            })
            .filter(|item| !item.is_empty())
            .collect();
        if items.is_empty() {
            return Err(ReplyError::EmptyMissing);
        }
        return Ok(WriterReply::Missing(items));
    }
    Ok(WriterReply::Code(CodeReply {
        language: language.to_owned(),
        lines: body.iter().map(|&line| line.to_owned()).collect(),
    }))
}

impl CodeReply {
    /// Checks the context lines against `before` and `after`, the file's lines just outside the
    /// Markers (at most [`CONTEXT_LINES`] each), and returns the code between them.
    ///
    /// Lines match when they are equal apart from trailing whitespace, so a stray `\r` or
    /// trailing space doesn't cost a retry; indentation must match exactly. The code may be
    /// empty, but may not contain a Marker line.
    pub fn fit<'a>(&'a self, before: &[&str], after: &[&str]) -> Result<Vec<&'a str>, ReplyError> {
        debug_assert!(before.len() <= CONTEXT_LINES && after.len() <= CONTEXT_LINES);
        let needed = before.len() + after.len();
        if self.lines.len() < needed {
            return Err(ReplyError::TooShort {
                needed,
                found: self.lines.len(),
            });
        }
        let (head, rest) = self.lines.split_at(before.len());
        let (code, tail) = rest.split_at(rest.len() - after.len());
        for (side, expected, found) in [(Side::Before, before, head), (Side::After, after, tail)] {
            let matches = expected
                .iter()
                .zip(found)
                .all(|(expected, found)| expected.trim_end() == found.trim_end());
            if !matches {
                return Err(ReplyError::ContextMismatch {
                    side,
                    expected: expected.iter().map(|&line| line.to_owned()).collect(),
                    found: found.to_vec(),
                });
            }
        }
        if let Some(line) = code.iter().find(|line| is_marker_line(line)) {
            return Err(ReplyError::MarkerInCode { line: line.clone() });
        }
        Ok(code.iter().map(String::as_str).collect())
    }
}

/// The backtick count and info string of a line that opens a fence.
fn opening_fence(line: &str) -> Option<(usize, &str)> {
    let line = line.trim_start();
    let ticks = line.len() - line.trim_start_matches('`').len();
    let info = &line[ticks..];
    (ticks >= 3 && !info.contains('`')).then_some((ticks, info.trim()))
}

/// Whether the line closes a fence opened with `ticks` backticks.
fn closes(line: &str, ticks: usize) -> bool {
    let line = line.trim();
    line.len() >= ticks && line.bytes().all(|b| b == b'`')
}

/// Lines in a block of their own, so blank lines and indentation stay visible.
fn quoted(lines: &[String]) -> String {
    let mut text = String::from("```\n");
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str("```");
    text
}
