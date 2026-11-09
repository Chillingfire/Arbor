//! A Planner Replan: one agentic run that reworks a failed Task from its failure chain (grilling
//! log, Round 3 decision 1).

use arbor_config::ClarificationMode;
use arbor_core::{Replan, Tier};
use arbor_llm::{ContentPart, Message, ModelInfo};
use arbor_prompts::Prompt;
use arbor_tools::{Tools, Toolset};

use crate::agent::{Agent, AgentSettings, Submission, UnsupportedModel};

pub const SUBMIT_REPLAN: &str = "submit_replan";

pub struct ReplanRun<'a> {
    /// The Planner's prompt rendered with its `replan` inputs and the same `clarification_mode`.
    pub prompt: &'a Prompt,
    pub model: ModelInfo,
    pub temperature: f64,
    pub clarification_mode: ClarificationMode,
    pub tools: Tools,
    /// The Human Operator's feedback on a rejected Replan.
    pub feedback: Option<&'a str>,
}

impl ReplanRun<'_> {
    /// # Panics
    ///
    /// If the prompt isn't the Planner's or the tools aren't read-only.
    pub fn start(self) -> Result<Agent<Replan>, UnsupportedModel> {
        assert_eq!(self.prompt.template, arbor_prompts::Template::Planner);
        assert_eq!(self.tools.toolset(), Toolset::Planner);
        let mut messages = self.prompt.messages();
        if let Some(feedback) = self.feedback {
            let Some(Message::User { content }) = messages.last_mut() else {
                unreachable!("a prompt ends with its inputs");
            };
            content.push(ContentPart::text(format!(
                "# Rejected Replan\n\nThe Human Operator rejected your Replan at the Checkpoint. \
                 Their feedback:\n\n{feedback}\n\nRevise it, then call `{SUBMIT_REPLAN}` again."
            )));
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

fn submission() -> Submission<Replan> {
    Submission::new(
        SUBMIT_REPLAN,
        "Submit the Replan: your diagnosis of why the Task failed, and the Task or Tasks that \
         replace it. Call it exactly once.",
        validate_replan,
    )
}

pub fn validate_replan(replan: &Replan) -> Result<(), String> {
    let mut problems = Vec::new();
    if replan.diagnosis.trim().is_empty() {
        problems.push("the diagnosis is empty; say why the Task failed".to_owned());
    }
    if replan.tasks.is_empty() {
        problems.push(
            "there are no Tasks; a Replan replaces the failed Task with at least one".to_owned(),
        );
    }
    for (index, task) in replan.tasks.iter().enumerate() {
        if task.title.trim().is_empty() {
            problems.push(format!("Task {} has no title", index + 1));
        }
        if task.description.trim().is_empty() {
            problems.push(format!("Task {} has no description", index + 1));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("the Replan is invalid: {}", problems.join("; ")))
    }
}
