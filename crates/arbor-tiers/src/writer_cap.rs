//! The Writer Cap arithmetic: how many tokens a Work Unit's Writer input is estimated to take.

use arbor_core::{WorkUnit, WorkUnitId};
use arbor_prompts::estimate_tokens;

/// Added to the estimated Writer input because the token estimate is rough.
pub const WRITER_CAP_MARGIN_PERCENT: u64 = 10;

pub fn writer_input_tokens(unit: &WorkUnit) -> u32 {
    let criteria = unit
        .acceptance_criteria
        .iter()
        .map(|criterion| estimate_tokens(criterion));
    let snippets = unit
        .context
        .iter()
        .map(|snippet| estimate_tokens(&snippet.path) + estimate_tokens(&snippet.text));
    let raw: u64 = std::iter::once(estimate_tokens(&unit.spec))
        .chain(criteria)
        .chain(snippets)
        .map(u64::from)
        .sum();
    with_margin(raw)
}

pub(crate) fn with_margin(raw_tokens: u64) -> u32 {
    let tokens = (raw_tokens * (100 + WRITER_CAP_MARGIN_PERCENT)).div_ceil(100);
    u32::try_from(tokens).unwrap_or(u32::MAX)
}

/// `None` if every Work Unit fits the Writer Cap.
pub fn writer_cap_report(units: &[WorkUnit], cap_tokens: u32) -> Option<String> {
    let sizes: Vec<(WorkUnitId, u32)> = units
        .iter()
        .map(|unit| (unit.id, writer_input_tokens(unit)))
        .collect();
    if sizes.iter().all(|&(_, tokens)| tokens <= cap_tokens) {
        return None;
    }
    let cap = thousands(cap_tokens);
    let lines: Vec<String> = sizes
        .into_iter()
        .map(|(id, tokens)| {
            let over = if tokens > cap_tokens {
                " — over, trim or split"
            } else {
                ""
            };
            format!("- {id}: {} / {cap}{over}", thousands(tokens))
        })
        .collect();
    Some(format!(
        "Estimated Writer input per Work Unit against the Writer Cap, in tokens \
         ({WRITER_CAP_MARGIN_PERCENT}% margin included):\n{}",
        lines.join("\n")
    ))
}

fn thousands(number: u32) -> String {
    let digits = number.to_string();
    let mut grouped = String::new();
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}
