use std::fmt;
use std::fs;
use std::io::{self, Read};

use arbor_core::Request;

#[derive(Debug)]
pub enum HandoverError {
    Read { path: String, source: io::Error },
    Empty { path: String },
}

impl fmt::Display for HandoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "could not read the handover file {path}: {source}")
            }
            Self::Empty { path } => write!(f, "the handover file {path} is empty"),
        }
    }
}

/// Reads a Request from a file, or from stdin when `path` is `-`.
pub fn read(path: &str) -> Result<Request, HandoverError> {
    let text = if path == "-" {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text).map(|_| text)
    } else {
        fs::read_to_string(path)
    }
    .map_err(|source| HandoverError::Read {
        path: path.to_owned(),
        source,
    })?;
    request(path, text)
}

fn request(path: &str, text: String) -> Result<Request, HandoverError> {
    if text.trim().is_empty() {
        return Err(HandoverError::Empty {
            path: path.to_owned(),
        });
    }
    Ok(Request { text, issue: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_text_becomes_the_request() {
        let request = request("next.md", "# Add a flag\n\nDetails.\n".into()).unwrap();
        assert_eq!(request.text, "# Add a flag\n\nDetails.\n");
        assert_eq!(request.issue, None);
    }

    #[test]
    fn a_blank_handover_is_refused() {
        let err = request("next.md", " \n\t\n".into()).unwrap_err();
        assert_eq!(err.to_string(), "the handover file next.md is empty");
    }
}
