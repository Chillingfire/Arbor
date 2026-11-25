//! The Escalation modal: the Human Operator's choices once the Scaffolder's Revisions couldn't make
//! a Task pass (grilling log, Round 3 decision 2).

use arbor_core::{RevisionCause, Tier};
use arbor_session::{Checkpoint, Decision, StrongerWriterOffer};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::Line;

use crate::modal::{
    CheckpointModal, ModalOutcome, TYPING_HINTS, TextEdit, edit_text, labelled, output_tail,
    typed_lines,
};
use crate::ui::{ACCENT, MUTED, YELLOW};

/// A hint being typed for a Tier to run again with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HintDraft {
    pub tier: Tier,
    pub hint: String,
}

/// What the Escalation offers, read from its Checkpoint.
struct Choices<'a> {
    task: usize,
    cause: &'a RevisionCause,
    may_replan: bool,
    failed_fix: Option<&'a str>,
    later_tasks: &'a [String],
    stronger_writer: Option<&'a StrongerWriterOffer>,
}

impl<'a> Choices<'a> {
    fn of(checkpoint: &'a Checkpoint) -> Option<Self> {
        let Checkpoint::Escalation {
            task,
            cause,
            may_replan,
            failed_fix,
            later_tasks,
            stronger_writer,
        } = checkpoint
        else {
            return None;
        };
        Some(Self {
            task: *task,
            cause,
            may_replan: *may_replan,
            failed_fix: failed_fix.as_deref(),
            later_tasks,
            stronger_writer: stronger_writer.as_ref(),
        })
    }

    /// The Tiers a hint can go to: the Writer only for a failed Work Unit, and the Planner only
    /// while the Task has its Replan.
    fn hint_tiers(&self) -> Vec<Tier> {
        let writer = self.cause.work_unit().map(|_| Tier::Writer);
        let planner = self.may_replan.then_some(Tier::Planner);
        writer
            .into_iter()
            .chain([Tier::Scaffolder])
            .chain(planner)
            .collect()
    }
}

impl CheckpointModal {
    pub(crate) fn escalation_key(&mut self, key: KeyEvent) -> Option<ModalOutcome> {
        let choices = Choices::of(&self.checkpoint)?;
        let hint_tiers = choices.hint_tiers();
        let (may_replan, has_stronger_writer) =
            (choices.may_replan, choices.stronger_writer.is_some());
        if self.hint_draft.is_some() && key.code == KeyCode::Esc {
            self.hint_draft = None;
            return None;
        }
        if let Some(draft) = &mut self.hint_draft {
            return hint_key(draft, key, &hint_tiers);
        }
        if let Some(confirmed) = self.confirming.take() {
            let confirms = matches!(key.code, KeyCode::Enter | KeyCode::Char('y'));
            return confirms.then_some(ModalOutcome::Decided(confirmed));
        }
        let decision = match key.code {
            KeyCode::Esc => return Some(ModalOutcome::Hide),
            KeyCode::Char('f') => Decision::Fixed,
            KeyCode::Char('p') if may_replan => Decision::Replan,
            KeyCode::Char('m') if has_stronger_writer => Decision::RetryWithStrongerWriter,
            KeyCode::Char('s') => Decision::Stop,
            KeyCode::Char('h') => {
                self.hint_draft = Some(HintDraft {
                    tier: hint_tiers[0],
                    hint: String::new(),
                });
                return None;
            }
            KeyCode::Char('k') => {
                self.confirming = Some(Decision::Skip);
                return None;
            }
            KeyCode::Char('x') => {
                self.confirming = Some(Decision::Abort);
                return None;
            }
            _ => return None,
        };
        Some(ModalOutcome::Decided(decision))
    }
}

/// Tab picks the next Tier; Enter sends the hint.
fn hint_key(draft: &mut HintDraft, key: KeyEvent, hint_tiers: &[Tier]) -> Option<ModalOutcome> {
    if key.code == KeyCode::Tab {
        let current = hint_tiers.iter().position(|&tier| tier == draft.tier);
        let next = current.map_or(0, |position| (position + 1) % hint_tiers.len());
        draft.tier = hint_tiers[next];
        return None;
    }
    match edit_text(&mut draft.hint, key) {
        TextEdit::Send => Some(ModalOutcome::Decided(Decision::RetryWithHint {
            tier: draft.tier,
            hint: std::mem::take(&mut draft.hint),
        })),
        TextEdit::Leave | TextEdit::Typed => None,
    }
}

pub(crate) fn escalation_hints(
    checkpoint_modal: &CheckpointModal,
) -> Vec<(&'static str, &'static str)> {
    if checkpoint_modal.hint_draft.is_some() {
        let mut hints = vec![("Tab", "Tier")];
        hints.extend_from_slice(TYPING_HINTS);
        return hints;
    }
    if checkpoint_modal.confirming.is_some() {
        return vec![("Enter", "confirm"), ("any key", "cancel")];
    }
    let Some(choices) = Choices::of(&checkpoint_modal.checkpoint) else {
        return Vec::new();
    };
    let mut hints = vec![("f", "I fixed it"), ("h", "hint")];
    if choices.stronger_writer.is_some() {
        hints.push(("m", "stronger model"));
    }
    if choices.may_replan {
        hints.push(("p", "replan"));
    }
    hints.extend([
        ("k", "skip"),
        ("x", "abort"),
        ("s", "stop"),
        ("Esc", "look around"),
    ]);
    hints
}

/// The lines of the Escalation modal, and the line to keep in view while typing a hint.
pub(crate) fn escalation_lines(
    checkpoint_modal: &CheckpointModal,
) -> (Vec<Line<'static>>, Option<usize>) {
    let Some(choices) = Choices::of(&checkpoint_modal.checkpoint) else {
        return (Vec::new(), None);
    };
    let mut lines = failure_lines(choices.cause);
    if let Some(check_output) = choices.failed_fix {
        lines.push(Line::default());
        lines.push(Line::styled(
            "Your fix didn't pass the Checks:",
            Style::new().fg(YELLOW),
        ));
        lines.extend(output_tail(check_output));
    }
    lines.push(Line::default());
    if let Some(draft) = &checkpoint_modal.hint_draft {
        lines.push(Line::styled(
            format!(
                "Hint for the {:?}, which runs again with it. Tab picks another Tier.",
                draft.tier
            ),
            Style::new().fg(MUTED),
        ));
        lines.extend(typed_lines(&draft.hint));
        let focus = lines.len() - 1;
        return (lines, Some(focus));
    }
    if let Some(confirming) = &checkpoint_modal.confirming {
        lines.extend(confirmation_lines(confirming, &choices));
        let focus = lines.len() - 1;
        return (lines, Some(focus));
    }
    lines.extend(option_lines(&choices));
    (lines, None)
}

fn failure_lines(cause: &RevisionCause) -> Vec<Line<'static>> {
    match cause {
        RevisionCause::TaskChecksFailed { output } => {
            let mut lines = vec![Line::raw(
                "The Task's Unit Check still fails after the Scaffolder's Revisions.",
            )];
            lines.extend(output_tail(output));
            lines
        }
        RevisionCause::WorkUnitFailed(failure) => {
            let mut lines = vec![Line::raw(format!(
                "The Writer still can't make {} pass after the Scaffolder's Revisions.",
                failure.id
            ))];
            for missing in failure.missing_response.iter().flatten() {
                lines.push(labelled("Missing", missing));
            }
            if let Some(last_attempt) = failure.attempts.last() {
                lines.extend(output_tail(&last_attempt.error));
            }
            lines
        }
    }
}

fn option_lines(choices: &Choices) -> Vec<Line<'static>> {
    let mut options = vec![
        "f  Fix it yourself in the worktree; Arbor runs the Checks and commits the fix.".to_owned(),
        "h  Run a Tier again with your hint: the Writer, one more Revision, or the Planner."
            .to_owned(),
    ];
    if let Some(offer) = choices.stronger_writer {
        options.push(format!(
            "m  Write it again with {} in the Writer role, with a Failure Brief: about ${:.4} \
             per attempt, at most ${:.4} for {} attempts.",
            offer.model,
            offer.cost_per_attempt.dollars(),
            offer.cost_per_attempt.dollars() * offer.max_attempts as f64,
            offer.max_attempts
        ));
    }
    options.push(if choices.may_replan {
        "p  Send the Task to the Planner, which replaces it with revised Tasks.".to_owned()
    } else {
        "   The Task was replanned once already, so it can't go to the Planner again.".to_owned()
    });
    options.extend([
        "k  Skip the Task: reset its work and go on with the next one.".to_owned(),
        "x  Abort the Session for good, keeping the branch and worktree.".to_owned(),
        "s  Stop here, keeping the branch and worktree.".to_owned(),
    ]);
    options
        .into_iter()
        .map(|option| Line::styled(option, Style::new().fg(MUTED)))
        .collect()
}

fn confirmation_lines(confirming: &Decision, choices: &Choices) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if *confirming == Decision::Skip {
        lines.push(Line::styled(
            format!(
                "Skip Task {}? Its commits are reset and the Session goes on without it.",
                choices.task + 1
            ),
            Style::new().fg(YELLOW).bold(),
        ));
        if !choices.later_tasks.is_empty() {
            lines.push(Line::styled(
                "These later Tasks may depend on it:",
                Style::new().fg(YELLOW),
            ));
            for (offset, title) in choices.later_tasks.iter().enumerate() {
                lines.push(Line::styled(
                    format!("  {}. {title}", choices.task + 2 + offset),
                    Style::new().fg(ACCENT),
                ));
            }
        }
    } else {
        lines.push(Line::styled(
            "Abort the Session? It ends for good; the branch and worktree are kept.",
            Style::new().fg(YELLOW).bold(),
        ));
    }
    lines.push(Line::styled(
        "Enter to confirm, any other key to go back.",
        Style::new().fg(MUTED),
    ));
    lines
}
