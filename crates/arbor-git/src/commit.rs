use std::fmt;

use crate::command::GitError;

/// An ordinary commit message: one short sentence, optionally followed by a brief
/// description. Nothing is ever added to it, so commits carry no mention of Arbor, Work Units,
/// or models, and no trailers (ADR-0009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitMessage {
    subject: String,
    description: Option<String>,
}

impl CommitMessage {
    /// Refuses a subject that is empty or longer than one line.
    pub fn new(subject: &str) -> Result<Self, GitError> {
        let subject = subject.trim();
        if subject.is_empty() {
            return Err(GitError::Refused(
                "a commit subject can't be empty".to_owned(),
            ));
        }
        if subject.contains(['\n', '\r']) {
            return Err(GitError::Refused(
                "a commit subject must be a single line".to_owned(),
            ));
        }
        Ok(Self {
            subject: subject.to_owned(),
            description: None,
        })
    }

    /// Adds a brief description below the subject. A blank one is ignored.
    pub fn with_description(mut self, description: &str) -> Self {
        let description = description.trim();
        self.description = (!description.is_empty()).then(|| description.to_owned());
        self
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// The full message as git records it.
    pub fn text(&self) -> String {
        match &self.description {
            Some(description) => format!("{}\n\n{description}", self.subject),
            None => self.subject.clone(),
        }
    }
}

/// A commit's full hash, as recorded in the Session event log for Rewind and Replan.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitId(String);

impl CommitId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A hash read back from the event log. It is checked when used, not here.
impl From<String> for CommitId {
    fn from(hash: String) -> Self {
        Self(hash)
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
