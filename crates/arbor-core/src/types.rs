use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The change the Human Operator asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    pub text: String,
    /// The GitHub issue the Request was pulled from, if any.
    pub issue: Option<u64>,
}

/// The Planner's output for a Request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Plan {
    pub goal: String,
    pub constraints: Vec<String>,
    /// In the order they will run.
    pub tasks: Vec<Task>,
}

/// One step of a Plan that leaves the build green and its tests passing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Task {
    pub title: String,
    pub description: String,
}

/// Identifies a Work Unit within a Session. Shown and used in Markers as `WU-<n>`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct WorkUnitId(pub u32);

impl fmt::Display for WorkUnitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WU-{}", self.0)
    }
}

/// One Region in one file, filled by exactly one Writer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkUnit {
    pub id: WorkUnitId,
    /// Path of the file holding the Region, relative to the worktree root.
    pub file: String,
    pub spec: String,
    /// The Context Pack: everything the Writer is allowed to know.
    pub context: Vec<Snippet>,
    pub acceptance_criteria: Vec<String>,
    /// Substituted into `checks.unit_filtered` to run only this Work Unit's tests.
    pub test_filter: String,
}

/// One entry of a Context Pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Snippet {
    pub path: String,
    pub text: String,
    pub priority: Priority,
}

/// `Required` snippets are packed before `Helpful` ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Required,
    Helpful,
}
