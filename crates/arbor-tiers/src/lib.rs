//! The agentic Tiers.

mod agent;
pub mod markers;
mod planner;
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
