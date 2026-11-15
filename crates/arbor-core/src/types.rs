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
    /// What the Request should achieve, in a sentence or two.
    pub goal: String,
    /// Limits every Task must respect. Assumptions made in Clarification Mode Assume are recorded
    /// here, each starting with `Assumption:`.
    pub constraints: Vec<String>,
    /// In the order they will run.
    pub tasks: Vec<Task>,
}

pub const ASSUMPTION_PREFIX: &str = "Assumption:";

impl Plan {
    pub fn assumptions(&self) -> impl Iterator<Item = &str> {
        self.constraints
            .iter()
            .map(String::as_str)
            .filter(|constraint| is_assumption(constraint))
    }
}

pub fn is_assumption(constraint: &str) -> bool {
    constraint
        .trim_start()
        .get(..ASSUMPTION_PREFIX.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(ASSUMPTION_PREFIX))
}

/// The assumptions recorded in free text such as a Work Unit's spec: each runs from an
/// `Assumption:` to the end of its line or the next `Assumption:`.
pub fn assumptions_in(spec: &str) -> Vec<&str> {
    let lowered = spec.to_ascii_lowercase();
    let prefix = ASSUMPTION_PREFIX.to_ascii_lowercase();
    let starts: Vec<usize> = lowered
        .match_indices(&prefix)
        .map(|(start, _)| start)
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            let next = starts.get(index + 1).copied().unwrap_or(spec.len());
            let line_end = spec[start..]
                .find('\n')
                .map_or(spec.len(), |end| start + end);
            spec[start..next.min(line_end)].trim()
        })
        .collect()
}

/// One step of a Plan that leaves the build green and its tests passing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Task {
    /// A short name for the Task.
    pub title: String,
    /// What the Task changes and why, naming the files and modules it touches. Never code.
    pub description: String,
}

/// The Planner's rework of a failed Task, submitted with `submit_replan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Replan {
    /// Why the Task failed, judging by its failure chain, and what the revised Tasks do
    /// differently.
    pub diagnosis: String,
    /// The Tasks that replace the failed one, in the order they will run. One or more.
    pub tasks: Vec<Task>,
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

/// A commit Arbor made on the Session branch. Rewind and the Task diff start from these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCommit {
    pub hash: String,
    pub origin: CommitOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitOrigin {
    Skeleton,
    Revision,
    WorkUnit(WorkUnitId),
    /// The Human Operator's fix at a Checkpoint.
    OperatorFix,
}

/// A line the Session branch adds that still holds `ARBOR:`, found by the end-of-session Marker
/// check (ADR-0002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftoverMarker {
    pub file: String,
    /// 1-based, in the file as it is now.
    pub line: usize,
    pub text: String,
}

/// One of the three LLM roles a Session splits work across (ADR-0001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Planner,
    Scaffolder,
    Writer,
}
