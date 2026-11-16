//! The Human Operator's choices at the Escalation Checkpoint beyond a fix by hand and a Replan
//! (grilling log, Round 3 decision 2).

mod common;

use arbor_core::{
    ApplyError, Effect, Event, FailedAttempt, MAX_REVISIONS, Phase, Revision, RevisionCause,
    Session, Tier, WorkUnitFailure, WorkUnitId,
};
use common::{plan, request, work_unit};

fn apply_all(session: &mut Session, events: &[Event]) -> Vec<Effect> {
    events
        .iter()
        .flat_map(|event| session.apply(event).unwrap())
        .collect()
}

fn write_and_commit(id: u32) -> [Event; 2] {
    [
        Event::WorkUnitPassed {
            id: WorkUnitId(id),
            model: None,
        },
        Event::WorkUnitCommitted {
            id: WorkUnitId(id),
            commit: format!("c{id}"),
        },
    ]
}

fn retries_used_up(id: u32) -> WorkUnitFailure {
    WorkUnitFailure {
        id: WorkUnitId(id),
        attempts: vec![FailedAttempt {
            reply: "```rust\ntodo!()\n```".into(),
            error: "error[E0308]: mismatched types".into(),
        }],
        missing_response: None,
        broken_region: None,
    }
}

fn accepted_revision(id: u32) -> [Event; 2] {
    [
        Event::RevisionSubmitted {
            revision: Revision {
                diagnosis: "The spec left out ties".into(),
                work_units: vec![work_unit(id)],
                shared_change: None,
            },
        },
        Event::RevisionCommitted { commit: None },
    ]
}

/// A Session with Tasks `task_titles` whose first Task has WU-1, WU-2 and WU-3, waiting at the
/// Escalation Checkpoint after WU-1 used up its Revisions.
fn escalated(task_titles: &[&str]) -> Session {
    let mut session = Session::new();
    apply_all(
        &mut session,
        &[
            Event::SessionStarted { request: request() },
            Event::PlanSubmitted {
                plan: plan(task_titles),
            },
            Event::PlanApproved {
                task_order: (0..task_titles.len()).collect(),
            },
            Event::WorkUnitsSubmitted {
                work_units: vec![work_unit(1), work_unit(2), work_unit(3)],
            },
            Event::ScaffoldChecksPassed,
            Event::ScaffoldApproved,
            Event::SkeletonCommitted {
                commit: Some("skeleton".into()),
            },
        ],
    );
    for _ in 0..MAX_REVISIONS {
        let failed = Event::WorkUnitFailed {
            failure: retries_used_up(1),
        };
        session.apply(&failed).unwrap();
        apply_all(&mut session, &accepted_revision(1));
    }
    let failed = Event::WorkUnitFailed {
        failure: retries_used_up(1),
    };
    session.apply(&failed).unwrap();
    assert!(matches!(
        session.phase(),
        Phase::EscalationCheckpoint { task: 0, .. }
    ));
    session
}

/// A one-Task Session at the Escalation Checkpoint after its Task Check kept failing.
fn escalated_task_check() -> Session {
    let mut session = Session::new();
    apply_all(
        &mut session,
        &[
            Event::SessionStarted { request: request() },
            Event::PlanSubmitted { plan: plan(&["A"]) },
            Event::PlanApproved {
                task_order: vec![0],
            },
            Event::WorkUnitsSubmitted {
                work_units: vec![work_unit(1)],
            },
            Event::ScaffoldChecksPassed,
            Event::ScaffoldApproved,
            Event::SkeletonCommitted { commit: None },
        ],
    );
    apply_all(&mut session, &write_and_commit(1));
    let task_check_failed = Event::TaskChecksFailed {
        output: "test failed".into(),
    };
    for _ in 0..MAX_REVISIONS {
        session.apply(&task_check_failed).unwrap();
        apply_all(&mut session, &accepted_revision(1));
        apply_all(&mut session, &write_and_commit(1));
    }
    session.apply(&task_check_failed).unwrap();
    assert!(matches!(
        session.phase(),
        Phase::EscalationCheckpoint { task: 0, .. }
    ));
    session
}

fn hinted(tier: Tier) -> Event {
    Event::RetryWithHint {
        tier,
        hint: "Keep the first of tied words".into(),
    }
}

#[test]
fn a_writer_hint_writes_the_failed_work_unit_again_then_the_rest() {
    let mut session = escalated(&["A"]);
    let effects = session.apply(&hinted(Tier::Writer)).unwrap();
    assert_eq!(effects, [Effect::RunWriter { id: WorkUnitId(1) }]);
    assert_eq!(
        session.hint_for(Tier::Writer, Some(WorkUnitId(1))),
        Some("Keep the first of tied words")
    );
    assert_eq!(session.hint_for(Tier::Writer, Some(WorkUnitId(2))), None);
    assert_eq!(session.hint_for(Tier::Scaffolder, None), None);

    let effects = apply_all(&mut session, &write_and_commit(1));
    assert_eq!(
        effects,
        [
            Effect::CommitWorkUnit { id: WorkUnitId(1) },
            Effect::RunWriter { id: WorkUnitId(2) },
        ]
    );
}

#[test]
fn a_scaffolder_hint_runs_one_more_revision_of_the_same_failure() {
    let mut session = escalated(&["A"]);
    let effects = session.apply(&hinted(Tier::Scaffolder)).unwrap();
    assert_eq!(
        effects,
        [Effect::RunRevision {
            task: 0,
            cause: RevisionCause::WorkUnitFailed(retries_used_up(1)),
        }]
    );
    assert_eq!(
        session.hint_for(Tier::Scaffolder, None),
        Some("Keep the first of tied words")
    );
}

#[test]
fn a_planner_hint_replans_the_task_and_uses_up_its_replan() {
    let mut session = escalated(&["A"]);
    let effects = session.apply(&hinted(Tier::Planner)).unwrap();
    assert_eq!(
        effects,
        [Effect::RunReplan {
            task: 0,
            cause: RevisionCause::WorkUnitFailed(retries_used_up(1)),
            feedback: None,
        }]
    );
    assert_eq!(
        session.hint_for(Tier::Planner, None),
        Some("Keep the first of tied words")
    );
}

#[test]
fn a_writer_retry_needs_a_failed_work_unit() {
    for refused in [
        hinted(Tier::Writer),
        Event::StrongerWriterRequested {
            model: "anthropic/claude-sonnet-5.5".into(),
        },
    ] {
        let mut session = escalated_task_check();
        let before = session.clone();
        assert_eq!(session.apply(&refused), Err(ApplyError::NoFailedWorkUnit));
        assert_eq!(session, before);
    }
}

#[test]
fn a_stronger_writer_retries_the_failed_work_unit_and_its_pass_is_tagged_with_the_model() {
    let mut session = escalated(&["A"]);
    let model = "anthropic/claude-sonnet-5.5";
    let effects = session
        .apply(&Event::StrongerWriterRequested {
            model: model.into(),
        })
        .unwrap();
    assert_eq!(effects, [Effect::RunWriter { id: WorkUnitId(1) }]);
    let stronger = session.stronger_writer(WorkUnitId(1)).unwrap();
    assert_eq!(stronger.model, model);
    assert_eq!(stronger.failure, retries_used_up(1));
    assert!(session.stronger_writer(WorkUnitId(2)).is_none());

    let passed = Event::WorkUnitPassed {
        id: WorkUnitId(1),
        model: Some(model.into()),
    };
    assert_eq!(
        session.apply(&passed).unwrap(),
        [Effect::CommitWorkUnit { id: WorkUnitId(1) }]
    );
    let json = serde_json::to_string(&passed).unwrap();
    assert_eq!(
        json,
        r#"{"type":"work_unit_passed","id":1,"model":"anthropic/claude-sonnet-5.5"}"#
    );
}

#[test]
fn a_later_escalation_choice_replaces_the_earlier_one() {
    let mut session = escalated(&["A"]);
    session
        .apply(&Event::StrongerWriterRequested {
            model: "anthropic/claude-sonnet-5.5".into(),
        })
        .unwrap();
    session
        .apply(&Event::WorkUnitFailed {
            failure: retries_used_up(1),
        })
        .unwrap();
    session.apply(&hinted(Tier::Writer)).unwrap();
    assert!(session.stronger_writer(WorkUnitId(1)).is_none());
    assert!(
        session
            .hint_for(Tier::Writer, Some(WorkUnitId(1)))
            .is_some()
    );
}

#[test]
fn a_skipped_task_is_reset_and_the_session_goes_on_with_the_next() {
    let mut session = escalated(&["A", "B"]);
    let effects = session.apply(&Event::TaskSkipped).unwrap();
    assert_eq!(effects, [Effect::ResetTask { task: 0 }]);
    let effects = session.apply(&Event::TaskReset).unwrap();
    assert_eq!(
        effects,
        [Effect::RunScaffolder {
            task: 1,
            feedback: None,
        }]
    );
    assert_eq!(session.skipped_tasks(), [0]);
    assert!(session.work_units().is_empty());
    assert!(session.task_commits().is_empty());
}

#[test]
fn skipping_the_last_task_goes_on_to_the_marker_check() {
    let mut session = escalated(&["A"]);
    apply_all(&mut session, &[Event::TaskSkipped]);
    let effects = session.apply(&Event::TaskReset).unwrap();
    assert_eq!(effects, [Effect::CheckMarkers]);
}

#[test]
fn an_aborted_session_ends_and_takes_no_more_events() {
    let mut session = escalated(&["A"]);
    let effects = session.apply(&Event::SessionAborted).unwrap();
    assert_eq!(effects, [Effect::EndSession]);
    assert_eq!(*session.phase(), Phase::Aborted);
    assert!(!session.is_done());
    let cap_removed = Event::BudgetCapSet { cap: None };
    assert!(matches!(
        session.apply(&cap_removed),
        Err(ApplyError::Unexpected { .. })
    ));
}

#[test]
fn escalation_choices_are_refused_outside_the_escalation_checkpoint() {
    let mut session = escalated(&["A"]);
    session.apply(&hinted(Tier::Writer)).unwrap();
    for refused in [
        hinted(Tier::Scaffolder),
        Event::TaskSkipped,
        Event::SessionAborted,
    ] {
        assert!(matches!(
            session.apply(&refused),
            Err(ApplyError::Unexpected { .. })
        ));
    }
}

#[test]
fn escalation_choices_round_trip_through_json() {
    for (event, expected) in [
        (
            hinted(Tier::Scaffolder),
            r#"{"type":"retry_with_hint","tier":"scaffolder","hint":"Keep the first of tied words"}"#,
        ),
        (
            Event::StrongerWriterRequested { model: "m".into() },
            r#"{"type":"stronger_writer_requested","model":"m"}"#,
        ),
        (Event::TaskSkipped, r#"{"type":"task_skipped"}"#),
        (Event::SessionAborted, r#"{"type":"session_aborted"}"#),
        (
            Event::WorkUnitPassed {
                id: WorkUnitId(2),
                model: None,
            },
            r#"{"type":"work_unit_passed","id":2}"#,
        ),
    ] {
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, expected);
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
    }
}
