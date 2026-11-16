//! The Failure Brief a stronger model gets when the Human Operator retries a failed Work Unit with
//! it, and the hint text the Human Operator can send any Tier from the Escalation Checkpoint
//! (grilling log, Round 3 decision 2).

use arbor_core::{RevisionRound, WorkUnitFailure};
use arbor_prompts::estimate_tokens;

/// The Failure Brief's own allowance, on top of the Writer Cap (grilling log, Round 4 decision 9).
pub const FAILURE_BRIEF_TOKENS: u32 = 2_000;

const TRIMMED: &str = "\n[… trimmed to fit the Failure Brief]";

/// Sums up a Work Unit's failures for a stronger model, mechanically with no model call: the latest
/// error, the Scaffolder's diagnoses, and what a Missing Response said was lacking. Stays within
/// [`FAILURE_BRIEF_TOKENS`] by trimming the error, which is the longest part.
pub fn failure_brief(failure: &WorkUnitFailure, revisions: &[RevisionRound]) -> String {
    let mut sections = vec![format!(
        "# Failure Brief\n\nEarlier Writers failed {} even after the Scaffolder revised it. Use \
         this to avoid their mistakes; the output contract and the rules above still apply.",
        failure.id
    )];
    if let Some(broken) = &failure.broken_region {
        sections.push(format!("## Broken Region\n\n{}", broken.problem));
    }
    if let Some(missing_items) = &failure.missing_response {
        sections.push(format!(
            "## What a Writer said its Context Pack lacked\n\n{}",
            bullets(missing_items)
        ));
    }
    let diagnoses: Vec<String> = revisions
        .iter()
        .filter(|round| revised_unit(round, failure))
        .map(|round| round.revision.diagnosis.clone())
        .collect();
    if !diagnoses.is_empty() {
        sections.push(format!(
            "## The Scaffolder's diagnoses, oldest first\n\n{}",
            bullets(&diagnoses)
        ));
    }
    let Some(latest) = failure.attempts.last() else {
        return within_allowance(sections.join("\n\n"));
    };
    let error_section = |error: &str| format!("## The latest error\n\n```\n{error}\n```");
    let without_error = sections.join("\n\n");
    let overhead = without_error.len() + "\n\n".len() + error_section("").len();
    let room = allowance_chars().saturating_sub(overhead);
    sections.push(error_section(&trimmed(latest.error.trim(), room)));
    within_allowance(sections.join("\n\n"))
}

/// The Human Operator's hint, as added to a Tier's inputs.
pub fn hint_input(hint: &str) -> String {
    format!(
        "# The Human Operator's hint\n\nThe Human Operator looked at the failure and sent this \
         hint. Follow it.\n\n{hint}"
    )
}

/// Whether the round revised the failed Work Unit, or answered its failure.
fn revised_unit(round: &RevisionRound, failure: &WorkUnitFailure) -> bool {
    round.cause.work_unit() == Some(failure.id)
        || round
            .revision
            .work_units
            .iter()
            .any(|unit| unit.id == failure.id)
}

fn bullets(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| format!("- {}", line.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// [`estimate_tokens`] counts 2 tokens per 5 bytes, so the allowance holds this many bytes.
fn allowance_chars() -> usize {
    usize::try_from(FAILURE_BRIEF_TOKENS).unwrap_or(usize::MAX) * 5 / 2
}

fn within_allowance(brief: String) -> String {
    if estimate_tokens(&brief) <= FAILURE_BRIEF_TOKENS {
        return brief;
    }
    trimmed(&brief, allowance_chars())
}

/// `text` cut to at most `room` bytes at a character boundary, saying it was cut.
fn trimmed(text: &str, room: usize) -> String {
    if text.len() <= room {
        return text.to_owned();
    }
    let mut end = room.saturating_sub(TRIMMED.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRIMMED}", &text[..end])
}
