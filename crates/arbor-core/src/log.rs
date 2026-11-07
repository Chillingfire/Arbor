//! The Session's `events.jsonl`: one JSON event per line, appended and flushed before the event is
//! shown (ADR-0004). The caller supplies the reader or writer, so the core still does no I/O of its
//! own (ADR-0005).

use std::fmt;
use std::io::{self, BufRead, Write};

use crate::event::Event;

pub struct EventLog<W> {
    out: W,
}

impl<W: Write> EventLog<W> {
    pub fn new(out: W) -> Self {
        Self { out }
    }

    /// Writes the event as one line and flushes it.
    pub fn append(&mut self, event: &Event) -> io::Result<()> {
        let mut line = serde_json::to_vec(event)?;
        line.push(b'\n');
        self.out.write_all(&line)?;
        self.out.flush()
    }

    pub fn get_ref(&self) -> &W {
        &self.out
    }
}

#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    /// A complete line that isn't a valid event. `line` is 1-based.
    Corrupt {
        line: usize,
        source: serde_json::Error,
    },
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "reading the event log: {err}"),
            Self::Corrupt { line, source } => {
                write!(f, "event log line {line} is corrupt: {source}")
            }
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Corrupt { source, .. } => Some(source),
        }
    }
}

/// Reads every event in the log. A final line without a newline is a write torn by a crash and is
/// dropped, since its event was never shown.
pub fn read_events(mut input: impl BufRead) -> Result<Vec<Event>, ReadError> {
    let mut events = Vec::new();
    let mut buf = String::new();
    for line in 1.. {
        buf.clear();
        if input.read_line(&mut buf).map_err(ReadError::Io)? == 0 {
            break;
        }
        let Some(json) = buf.strip_suffix('\n') else {
            break;
        };
        let event =
            serde_json::from_str(json).map_err(|source| ReadError::Corrupt { line, source })?;
        events.push(event);
    }
    Ok(events)
}
