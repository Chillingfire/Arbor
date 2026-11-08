use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use arbor_llm::{ContentPart, Message};

use crate::Template;

/// A rendered prompt for one call, split into its parts in prompt order: static parts first, so
/// the provider can cache them.
///
/// 1. `instructions`: the rendered template (Role, Inputs, Output contract, Rules). The same for
///    every call of this template while the settings it uses don't change.
/// 2. `conventions`: the project's Conventions. The same for a whole Session. Always `None` for
///    the Writer.
/// 3. `inputs`: the Session inputs for this call, e.g. the Request or the Work Unit.
/// 4. The transcript, which the caller appends after [`Prompt::messages`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub template: Template,
    pub instructions: String,
    pub conventions: Option<String>,
    pub inputs: String,
}

impl Prompt {
    /// The opening messages: a system message holding the static parts, with a cache breakpoint
    /// on the last of them, then a user message holding the inputs.
    ///
    /// The caller places the transcript's breakpoint with `ChatRequest::cache_transcript`.
    pub fn messages(&self) -> Vec<Message> {
        let system = match &self.conventions {
            Some(conventions) => vec![
                ContentPart::text(self.instructions.clone()),
                ContentPart::cached(conventions.clone()),
            ],
            None => vec![ContentPart::cached(self.instructions.clone())],
        };
        vec![
            Message::System { content: system },
            Message::User {
                content: vec![ContentPart::text(self.inputs.clone())],
            },
        ]
    }

    /// The whole prompt as plain text, parts separated by a blank line.
    pub fn text(&self) -> String {
        [
            Some(self.instructions.as_str()),
            self.conventions.as_deref(),
            Some(self.inputs.as_str()),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
    }

    /// Writes [`Prompt::text`] to `<session_dir>/prompts/<template>.md`, replacing that template's
    /// previous prompt, and returns the path.
    pub fn save(&self, session_dir: &Path) -> io::Result<PathBuf> {
        let dir = session_dir.join("prompts");
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.md", self.template.name()));
        fs::write(&path, self.text())?;
        Ok(path)
    }
}
