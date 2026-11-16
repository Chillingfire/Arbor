use arbor_core::{
    FailedAttempt, Priority, Revision, RevisionCause, RevisionRound, Snippet, WorkUnit,
    WorkUnitFailure, WorkUnitId,
};
use arbor_prompts::estimate_tokens;
use arbor_tiers::failure_brief;

const ALLOWANCE_TOKENS: u32 = 2_000;

fn work_unit(id: u32) -> WorkUnit {
    WorkUnit {
        id: WorkUnitId(id),
        file: "src/lib.rs".into(),
        spec: "Return the longest word".into(),
        context: vec![Snippet {
            path: "src/lib.rs".into(),
            text: "pub fn longest_word(text: &str) -> Option<&str>".into(),
            priority: Priority::Required,
        }],
        acceptance_criteria: Vec::new(),
        test_filter: "longest".into(),
    }
}

fn failure(id: u32, error: &str) -> WorkUnitFailure {
    WorkUnitFailure {
        id: WorkUnitId(id),
        attempts: vec![
            FailedAttempt {
                reply: "first".into(),
                error: "an older error".into(),
            },
            FailedAttempt {
                reply: "second".into(),
                error: error.into(),
            },
        ],
        missing_response: Some(vec!["the Words type".into()]),
        broken_region: None,
    }
}

fn round(id: u32, diagnosis: &str) -> RevisionRound {
    RevisionRound {
        cause: RevisionCause::WorkUnitFailed(failure(id, "error")),
        revision: Revision {
            diagnosis: diagnosis.into(),
            work_units: vec![work_unit(id)],
            shared_change: None,
        },
        rejection: None,
    }
}

#[test]
fn the_brief_holds_the_latest_error_the_diagnoses_and_what_was_missing() {
    let brief = failure_brief(
        &failure(1, "assertion failed: ties keep the first word"),
        &[
            round(1, "The spec left out ties"),
            round(2, "Another Work Unit's diagnosis"),
            round(1, "The Context Pack lacked split_whitespace"),
        ],
        ALLOWANCE_TOKENS,
    );

    assert!(brief.starts_with("# Failure Brief"), "{brief}");
    assert!(brief.contains("ties keep the first word"), "{brief}");
    assert!(!brief.contains("an older error"), "{brief}");
    assert!(
        brief.contains("- The spec left out ties\n- The Context Pack lacked split_whitespace"),
        "{brief}"
    );
    assert!(!brief.contains("Another Work Unit"), "{brief}");
    assert!(brief.contains("- the Words type"), "{brief}");
}

#[test]
fn a_long_error_is_trimmed_so_the_brief_fits_its_allowance() {
    let long_error = "error[E0308]: mismatched types\n".repeat(1_000);
    let brief = failure_brief(
        &failure(1, &long_error),
        &[round(1, "The spec left out ties")],
        ALLOWANCE_TOKENS,
    );

    assert!(estimate_tokens(&brief) <= ALLOWANCE_TOKENS);
    assert!(
        brief.contains("trimmed to fit the Failure Brief"),
        "{brief}"
    );
    assert!(brief.contains("- The spec left out ties"), "{brief}");
    assert!(brief.trim_end().ends_with("```"), "{brief}");
}
