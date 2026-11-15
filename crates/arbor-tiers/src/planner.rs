use arbor_config::ClarificationMode;
use arbor_core::{Plan, Task, Tier};
use arbor_llm::ModelInfo;
use arbor_prompts::Prompt;
use arbor_tools::{Tools, Toolset};

use crate::agent::{
    Agent, AgentSettings, Submission, UnsupportedModel, append_input, reject_if_any,
};

pub const SUBMIT_PLAN: &str = "submit_plan";

pub struct PlannerRun<'a> {
    /// Rendered with the same `clarification_mode`.
    pub prompt: &'a Prompt,
    pub model: ModelInfo,
    pub temperature: f64,
    pub clarification_mode: ClarificationMode,
    pub tools: Tools,
    pub rejected: Option<Rejection<'a>>,
}

#[derive(Debug, Clone, Copy)]
pub struct Rejection<'a> {
    pub plan: &'a Plan,
    pub feedback: &'a str,
}

impl PlannerRun<'_> {
    /// # Panics
    ///
    /// If the prompt isn't the Planner's or the tools aren't read-only.
    pub fn start(self) -> Result<Agent<Plan>, UnsupportedModel> {
        assert_eq!(self.prompt.template, arbor_prompts::Template::Planner);
        assert_eq!(self.tools.toolset(), Toolset::Planner);
        let mut messages = self.prompt.messages();
        if let Some(rejection) = self.rejected {
            append_input(&mut messages, rejection_text(rejection));
        }
        let settings = AgentSettings {
            tier: Tier::Planner,
            model: self.model,
            temperature: self.temperature,
            clarification_mode: self.clarification_mode,
        };
        Agent::new(settings, submission(), self.tools, messages)
    }
}

fn submission() -> Submission<Plan> {
    Submission::new(
        SUBMIT_PLAN,
        "Submit the Plan for the Request: its goal, constraints, and ordered Tasks. Call it \
         exactly once, when the Plan is complete.",
        validate_plan,
    )
}

pub fn validate_plan(plan: &Plan) -> Result<(), String> {
    let mut problems = Vec::new();
    if plan.goal.trim().is_empty() {
        problems.push("the goal is empty".to_owned());
    }
    if plan.tasks.is_empty() {
        problems.push("there are no Tasks; a Plan needs at least one".to_owned());
    }
    problems.extend(task_problems(&plan.tasks));
    reject_if_any("the Plan is invalid", problems)
}

pub(crate) fn task_problems(tasks: &[Task]) -> Vec<String> {
    let mut problems = Vec::new();
    for (task_number, task) in (1..).zip(tasks) {
        if task.title.trim().is_empty() {
            problems.push(format!("Task {task_number} has no title"));
        }
        if task.description.trim().is_empty() {
            problems.push(format!("Task {task_number} has no description"));
        }
    }
    problems
}

fn rejection_text(rejection: Rejection) -> String {
    let plan = serde_json::to_string_pretty(rejection.plan).expect("a Plan serializes to JSON");
    format!(
        "# Rejected Plan\n\nThe Human Operator rejected this Plan at the Checkpoint:\n\n```json\n\
         {plan}\n```\n\nTheir feedback:\n\n{}\n\nRevise the Plan to address the feedback, then \
         call `{SUBMIT_PLAN}` again.",
        rejection.feedback
    )
}
