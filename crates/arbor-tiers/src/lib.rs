//! The agentic Tiers: the Planner and the Scaffolder.
//!
//! [`PlannerRun`] starts the Planner, which turns a Request into a Plan via `submit_plan`.
//! [`ScaffolderRun`] starts the Scaffolder, which turns a Task into a Skeleton, tests and Work
//! Units via `submit_work_units`, checked by [`ScaffoldCheck`]. [`markers`] finds the Regions.
//!
//! An [`Agent`] runs one model call per [`Agent::turn`], runs the tools it asks for, and ends when
//! the model calls its submit tool with arguments that parse and validate, or escalates after
//! [`MAX_SUBMIT_FAILURES`] failed submissions.

mod agent;
pub mod markers;
mod planner;
pub mod reply;
mod scaffold_check;
mod scaffolder;

pub use agent::{
    Agent, AgentSettings, MAX_SUBMIT_FAILURES, Outcome, SUBMIT_NOW_PERCENT, Submission, Turn,
    TurnError, UnsupportedModel, Validate, core_usage, require_tool_calling,
};
pub use planner::{PlannerRun, Rejection, SUBMIT_PLAN, validate_plan};
pub use scaffold_check::{
    NO_SCAFFOLD_CHECK, ScaffoldCheck, WRITER_CAP_MARGIN_PERCENT, field_problems, marker_problems,
    writer_cap_report, writer_input_tokens,
};
pub use scaffolder::{SUBMIT_WORK_UNITS, ScaffolderRun, WorkUnits};
