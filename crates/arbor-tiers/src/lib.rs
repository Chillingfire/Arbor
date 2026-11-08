//! The agentic Tiers: the Planner, and later the Scaffolder.
//!
//! [`PlannerRun`] starts the Planner, which turns a Request into a Plan via `submit_plan`.
//!
//! An [`Agent`] runs one model call per [`Agent::turn`], runs the tools it asks for, and ends when
//! the model calls its submit tool with arguments that parse and validate, or escalates after
//! [`MAX_SUBMIT_FAILURES`] failed submissions.

mod agent;
pub mod markers;
mod planner;

pub use agent::{
    Agent, AgentSettings, MAX_SUBMIT_FAILURES, Outcome, SUBMIT_NOW_PERCENT, Submission, Turn,
    TurnError, UnsupportedModel, Validate, core_usage, require_tool_calling,
};
pub use planner::{PlannerRun, Rejection, SUBMIT_PLAN, validate_plan};
