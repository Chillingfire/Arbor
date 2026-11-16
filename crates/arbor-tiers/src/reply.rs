//! The Writer's output contract (Round 5 decision 5): exactly one fenced block and nothing else,
//! either code wrapped in its context lines or a ```` ```missing ```` block.

use std::fmt;

use crate::markers::{CONTEXT_LINES, END, is_marker_line};

pub const MISSING: &str = "missing";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriterReply {
    Code(CodeReply),
    Missing(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeReply {
    /// Empty for a bare fence.
    pub language: String,
    /// Context lines included.
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Before,
    After,
}

/// Its message goes back to the Writer, so it says what to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyError {
    Empty,
    CutOff,
    NoBlock,
    Unclosed,
    SeveralBlocks(usize),
    TextOutside,
    EmptyMissing,
    /// A Missing Response is allowed once per Work Unit, and this one's was already sent.
    MissingResponseUsed,
    TooShort {
        needed: usize,
        found: usize,
    },
    ContextMismatch {
        side: Side,
        expected: Vec<String>,
        found: Vec<String>,
    },
    MarkerInCode {
        line: String,
    },
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
            Self::MissingResponseUsed => f.write_str(
                "this Work Unit already sent its one `missing` block, and its Context Pack was \
                 revised since; reply with code this time",
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

/// A fence closes with at least as many backticks as opened it, so a longer fence can hold code
/// that itself contains ```` ``` ````.
pub fn parse_reply(reply_text: &str) -> Result<WriterReply, ReplyError> {
    if reply_text.trim().is_empty() {
        return Err(ReplyError::Empty);
    }
    let lines: Vec<&str> = reply_text.lines().collect();
    let mut blocks = Vec::new();
    let mut outside = false;
    let mut line_index = 0;
    while line_index < lines.len() {
        let Some((ticks, info_string)) = opening_fence(lines[line_index]) else {
            outside |= !lines[line_index].trim().is_empty();
            line_index += 1;
            continue;
        };
        let close = (line_index + 1..lines.len())
            .find(|&close_index| closes(lines[close_index], ticks))
            .ok_or(ReplyError::Unclosed)?;
        blocks.push((info_string, &lines[line_index + 1..close]));
        line_index = close + 1;
    }
    let (info_string, body) = match blocks.as_slice() {
        [] => return Err(ReplyError::NoBlock),
        [block] => *block,
        _ => return Err(ReplyError::SeveralBlocks(blocks.len())),
    };
    if outside {
        return Err(ReplyError::TextOutside);
    }
    let language = info_string.split_whitespace().next().unwrap_or_default();
    if language.eq_ignore_ascii_case(MISSING) {
        let missing_items = missing_items(body);
        if missing_items.is_empty() {
            return Err(ReplyError::EmptyMissing);
        }
        return Ok(WriterReply::Missing(missing_items));
    }
    Ok(WriterReply::Code(CodeReply {
        language: language.to_owned(),
        lines: body.iter().map(|&line| line.to_owned()).collect(),
    }))
}

/// One item per line, without its list bullet.
fn missing_items(body: &[&str]) -> Vec<String> {
    body.iter()
        .map(|line| {
            let line = line.trim();
            line.strip_prefix(['-', '*'])
                .filter(|rest| rest.is_empty() || rest.starts_with(' '))
                .unwrap_or(line)
                .trim()
                .to_owned()
        })
        .filter(|missing_item| !missing_item.is_empty())
        .collect()
}

impl CodeReply {
    /// Returns the code between the context lines. Trailing whitespace is ignored when matching
    /// them, so a stray `\r` doesn't cost a retry; indentation must match exactly.
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

fn opening_fence(line: &str) -> Option<(usize, &str)> {
    let line = unindent(line)?;
    let ticks = line.len() - line.trim_start_matches('`').len();
    let info_string = &line[ticks..];
    (ticks >= 3 && !info_string.contains('`')).then_some((ticks, info_string.trim()))
}

fn closes(line: &str, ticks: usize) -> bool {
    let Some(line) = unindent(line) else {
        return false;
    };
    let line = line.trim_end();
    line.len() >= ticks && line.bytes().all(|byte| byte == b'`')
}

/// As in Markdown, a fence may be indented by at most 3 spaces, so an indented ```` ``` ````
/// inside the code, e.g. in a Python docstring, doesn't end the block.
fn unindent(line: &str) -> Option<&str> {
    let spaces = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[spaces..];
    (spaces <= 3 && !rest.starts_with('\t')).then_some(rest)
}

/// Fenced so blank lines and indentation stay visible.
fn quoted(lines: &[String]) -> String {
    let mut fenced = String::from("```\n");
    for line in lines {
        fenced.push_str(line);
        fenced.push('\n');
    }
    fenced.push_str("```");
    fenced
}
