//! Agent tools for the Planner and Scaffolder, confined to the Session worktree.
//!
//! There is deliberately no shell tool (ADR-0008).

mod read;
mod worktree;

use std::path::PathBuf;

use arbor_config::Checks;
use arbor_llm::ToolDef;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

use crate::worktree::Worktree;

/// Which Tier the tools serve. The Planner may only read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toolset {
    Planner,
    Scaffolder,
}

/// A tool's result, sent back to the model as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

/// The tools for one Tier in one Session worktree.
pub struct Tools {
    worktree: Worktree,
    toolset: Toolset,
    #[allow(dead_code)]
    checks: Checks,
}

impl Tools {
    /// `checks` are the commands loaded at Session start. Later edits to `arbor.toml` in the
    /// worktree don't change them.
    pub fn new(worktree: impl Into<PathBuf>, toolset: Toolset, checks: Checks) -> Self {
        Self {
            worktree: Worktree::new(worktree.into()),
            toolset,
            checks,
        }
    }

    pub fn toolset(&self) -> Toolset {
        self.toolset
    }

    /// The tools this Tier may call.
    pub fn definitions(&self) -> Vec<ToolDef> {
        vec![
            definition::<read::ReadFileArgs>(
                "read_file",
                "Read a text file with numbered lines, at most 400 lines at a time. Use offset \
                 and limit to page through longer files.",
            ),
            definition::<read::ListDirArgs>(
                "list_dir",
                "List a directory's entries. Directories end with /. Files excluded by \
                 .gitignore are left out.",
            ),
        ]
    }

    /// Runs a tool the model called. `arguments` is the JSON text the model wrote. Failures,
    /// including refusals, come back as error output for the model rather than as `Err`.
    pub async fn call(&self, name: &str, arguments: &str) -> ToolOutput {
        let result = match name {
            "read_file" => {
                parse(name, arguments).and_then(|args| read::read_file(&self.worktree, args))
            }
            "list_dir" => {
                parse(name, arguments).and_then(|args| read::list_dir(&self.worktree, args))
            }
            _ => Err(format!("unknown tool {name}")),
        };
        match result {
            Ok(text) => ToolOutput {
                text,
                is_error: false,
            },
            Err(text) => ToolOutput {
                text,
                is_error: true,
            },
        }
    }
}

fn definition<T: JsonSchema>(name: &str, description: &str) -> ToolDef {
    let mut parameters =
        serde_json::to_value(schemars::schema_for!(T)).expect("a JSON Schema serializes to JSON");
    if let Some(object) = parameters.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    ToolDef {
        name: name.to_owned(),
        description: description.to_owned(),
        parameters,
    }
}

fn parse<T: DeserializeOwned>(name: &str, arguments: &str) -> Result<T, String> {
    let arguments = if arguments.trim().is_empty() {
        "{}"
    } else {
        arguments
    };
    serde_json::from_str(arguments).map_err(|err| format!("invalid arguments for {name}: {err}"))
}
