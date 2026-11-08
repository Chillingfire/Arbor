use std::fmt;

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
