//! The agentic Tiers: the Planner, and later the Scaffolder.
//!
//! An [`Agent`] runs one model call per [`Agent::turn`], runs the tools it asks for, and ends when
//! the model calls its submit tool with arguments that parse and validate, or escalates after
//! [`MAX_SUBMIT_FAILURES`] failed submissions.

mod agent;

pub use agent::{
    Agent, AgentSettings, MAX_SUBMIT_FAILURES, Outcome, SUBMIT_NOW_PERCENT, Submission, Turn,
    TurnError, UnsupportedModel, core_usage, require_tool_calling,
};
