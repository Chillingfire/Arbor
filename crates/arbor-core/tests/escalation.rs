mod common;

use arbor_core::{
    ApplyError, Effect, Event, FailedAttempt, MAX_REVISIONS, Phase, Revision, RevisionCause,
    Session, WorkUnitFailure, WorkUnitId,
};
use common::{plan, request, work_unit};

fn apply_all(session: &mut Session, events: &[Event]) -> Vec<Effect> {
    events
        .iter()
        .flat_map(|event| session.apply(event).unwrap())
        .collect()
}

fn write_and_commit(id: u32, commit: &str) -> [Event; 2] {
    [
        Event::WorkUnitPassed { id: WorkUnitId(id) },
        Event::WorkUnitCommitted {
            id: WorkUnitId(id),
            commit: commit.into(),
        },
    ]
}

/// A one-Task Session with WU-1, WU-2 and WU-3, whose Writer is about to fill WU-1.
fn writing() -> Session {
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
                work_units: vec![work_unit(1), work_unit(2), work_unit(3)],
            },
            Event::ScaffoldChecksPassed,
            Event::ScaffoldApproved,
            Event::SkeletonCommitted { commit: None },
        ],
    );
    session
}

fn retries_used_up(id: u32) -> WorkUnitFailure {
    WorkUnitFailure {
        id: WorkUnitId(id),
        attempts: (1..=3)
            .map(|attempt| FailedAttempt {
                reply: format!("```rust\nattempt {attempt}\n```"),
                error: "error[E0308]: mismatched types".into(),
            })
            .collect(),
        missing_response: None,
    }
}

fn missing_response(id: u32) -> WorkUnitFailure {
    WorkUnitFailure {
        id: WorkUnitId(id),
        attempts: vec![],
        missing_response: Some(vec!["the Config type".into()]),
    }
}

fn failed(failure: &WorkUnitFailure) -> Event {
    Event::WorkUnitFailed {
        failure: failure.clone(),
    }
}

fn revised(ids: &[u32]) -> Revision {
    Revision {
        diagnosis: "The Context Pack lacked the Config type".into(),
        work_units: ids.iter().map(|&id| work_unit(id)).collect(),
        shared_change: None,
    }
}

fn revision(ids: &[u32]) -> Event {
    Event::RevisionSubmitted {
        revision: revised(ids),
    }
}

/// The Revision is accepted and its edits committed.
fn accepted(ids: &[u32]) -> [Event; 2] {
    [revision(ids), Event::RevisionCommitted { commit: None }]
}

fn rewording_rejected(ids: &[u32]) -> Event {
    Event::RevisionRejected {
        revision: revised(ids),
        reason: "the Revision only rewords the spec".into(),
    }
}

#[test]
fn a_work_unit_whose_writer_used_up_its_retries_goes_to_a_revision() {
    let mut session = writing();
    let failure = retries_used_up(1);
    let effects = session.apply(&failed(&failure)).unwrap();
    assert_eq!(
        effects,
        [Effect::RunRevision {
            task: 0,
            cause: RevisionCause::WorkUnitFailed(failure),
        }]
    );
}

#[test]
fn a_missing_response_goes_straight_to_a_revision_and_is_allowed_once_per_work_unit() {
    let mut session = writing();
    assert!(session.may_send_missing_response(WorkUnitId(1)));
    let effects = session.apply(&failed(&missing_response(1))).unwrap();
    assert!(matches!(effects[..], [Effect::RunRevision { .. }]));
    assert!(!session.may_send_missing_response(WorkUnitId(1)));
    assert!(session.may_send_missing_response(WorkUnitId(2)));
}

#[test]
fn a_failure_for_another_work_unit_is_refused() {
    let mut session = writing();
    let before = session.clone();
    assert_eq!(
        session.apply(&failed(&retries_used_up(2))),
        Err(ApplyError::WrongWorkUnit {
            expected: WorkUnitId(1),
            got: WorkUnitId(2),
        })
    );
    assert_eq!(session, before);
}

#[test]
fn after_a_work_unit_revision_the_writers_go_on_with_the_rest_of_the_task() {
    let mut session = writing();
    apply_all(&mut session, &write_and_commit(1, "c1"));
    session.apply(&failed(&retries_used_up(2))).unwrap();
    let mut effects = apply_all(&mut session, &accepted(&[2, 4]));
    effects.extend(apply_all(&mut session, &write_and_commit(2, "c2")));
    effects.extend(apply_all(&mut session, &write_and_commit(4, "c4")));
    effects.extend(apply_all(&mut session, &write_and_commit(3, "c3")));
    assert_eq!(
        effects,
        [
            Effect::CommitRevision { task: 0 },
            Effect::RunWriter { id: WorkUnitId(2) },
            Effect::CommitWorkUnit { id: WorkUnitId(2) },
            Effect::RunWriter { id: WorkUnitId(4) },
            Effect::CommitWorkUnit { id: WorkUnitId(4) },
            Effect::RunWriter { id: WorkUnitId(3) },
            Effect::CommitWorkUnit { id: WorkUnitId(3) },
            Effect::RunTaskChecks { task: 0 },
        ]
    );
}

#[test]
fn a_work_unit_revision_may_revise_only_the_failed_work_unit() {
    let mut session = writing();
    session.apply(&failed(&retries_used_up(1))).unwrap();
    let before = session.clone();
    assert_eq!(
        session.apply(&revision(&[1, 2])),
        Err(ApplyError::OutsideRevisionScope { id: WorkUnitId(2) })
    );
    assert_eq!(session, before);
}

#[test]
fn after_two_revisions_of_a_work_unit_the_human_operator_decides() {
    let mut session = writing();
    for _ in 0..MAX_REVISIONS {
        session.apply(&failed(&retries_used_up(1))).unwrap();
        apply_all(&mut session, &accepted(&[1]));
    }
    let failure = retries_used_up(1);
    let effects = session.apply(&failed(&failure)).unwrap();
    assert_eq!(
        effects,
        [Effect::ShowEscalationCheckpoint {
            task: 0,
            cause: RevisionCause::WorkUnitFailed(failure),
        }]
    );
}

#[test]
fn work_units_split_off_by_a_revision_share_its_revision_count() {
    let mut session = writing();
    session.apply(&failed(&retries_used_up(1))).unwrap();
    apply_all(&mut session, &accepted(&[1, 4]));
    apply_all(&mut session, &write_and_commit(1, "c1"));
    session.apply(&failed(&retries_used_up(4))).unwrap();
    apply_all(&mut session, &accepted(&[4]));
    let effects = session.apply(&failed(&retries_used_up(4))).unwrap();
    assert!(
        matches!(effects[..], [Effect::ShowEscalationCheckpoint { .. }]),
        "{effects:?}"
    );
}

#[test]
fn each_work_unit_gets_its_own_revisions() {
    let mut session = writing();
    for _ in 0..MAX_REVISIONS {
        session.apply(&failed(&retries_used_up(1))).unwrap();
        apply_all(&mut session, &accepted(&[1]));
    }
    apply_all(&mut session, &write_and_commit(1, "c1"));
    let effects = session.apply(&failed(&retries_used_up(2))).unwrap();
    assert!(matches!(effects[..], [Effect::RunRevision { .. }]));
}

#[test]
fn an_operator_fix_of_a_failed_work_unit_goes_on_with_the_next_one() {
    let mut session = writing();
    for _ in 0..MAX_REVISIONS {
        session.apply(&failed(&retries_used_up(1))).unwrap();
        apply_all(&mut session, &accepted(&[1]));
    }
    session.apply(&failed(&retries_used_up(1))).unwrap();
    assert!(matches!(
        session.phase(),
        Phase::EscalationCheckpoint { task: 0, .. }
    ));
    let effects = session
        .apply(&Event::OperatorFixed {
            commit: Some("fix".into()),
        })
        .unwrap();
    assert_eq!(effects, [Effect::RunWriter { id: WorkUnitId(2) }]);
}

#[test]
fn work_unit_failed_round_trips_through_json() {
    let event = failed(&missing_response(3));
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.starts_with(r#"{"type":"work_unit_failed""#), "{json}");
    assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);

    let cause = RevisionCause::WorkUnitFailed(retries_used_up(2));
    let json = serde_json::to_string(&cause).unwrap();
    assert!(
        json.starts_with(r#"{"kind":"work_unit_failed","id":2"#),
        "{json}"
    );
    assert_eq!(serde_json::from_str::<RevisionCause>(&json).unwrap(), cause);
}

#[test]
fn a_rejected_revision_counts_toward_the_limit_and_the_scaffolder_revises_again() {
    let mut session = writing();
    let failure = retries_used_up(1);
    session.apply(&failed(&failure)).unwrap();
    let effects = session.apply(&rewording_rejected(&[1])).unwrap();
    assert_eq!(effects, [Effect::DiscardRevision { task: 0 }]);
    let effects = session.apply(&Event::RevisionDiscarded).unwrap();
    assert_eq!(
        effects,
        [Effect::RunRevision {
            task: 0,
            cause: RevisionCause::WorkUnitFailed(failure.clone()),
        }]
    );
    assert_eq!(
        session.revisions()[0].rejection.as_deref(),
        Some("the Revision only rewords the spec")
    );
    assert_eq!(session.work_units()[0], work_unit(1));

    session.apply(&rewording_rejected(&[1])).unwrap();
    let effects = session.apply(&Event::RevisionDiscarded).unwrap();
    assert_eq!(
        effects,
        [Effect::ShowEscalationCheckpoint {
            task: 0,
            cause: RevisionCause::WorkUnitFailed(failure),
        }]
    );
}

#[test]
fn a_shared_signature_change_goes_straight_to_the_escalation_checkpoint() {
    let mut session = writing();
    let failure = retries_used_up(1);
    session.apply(&failed(&failure)).unwrap();
    let shared = Revision {
        diagnosis: "parse() must return a Result, and WU-2 and WU-3 call it".into(),
        work_units: vec![],
        shared_change: Some("Change parse() to return Result<Config, ParseError>".into()),
    };
    let effects = session
        .apply(&Event::RevisionSubmitted {
            revision: shared.clone(),
        })
        .unwrap();
    assert_eq!(
        effects,
        [Effect::DiscardRevision { task: 0 }],
        "the Revision's edits are undone before the Human Operator sees the worktree"
    );
    let effects = session.apply(&Event::RevisionDiscarded).unwrap();
    assert_eq!(
        effects,
        [Effect::ShowEscalationCheckpoint {
            task: 0,
            cause: RevisionCause::WorkUnitFailed(failure),
        }]
    );
    assert_eq!(session.revisions()[0].revision, shared);
}

fn with_revision_checkpoints() -> Session {
    let mut session = writing();
    session
        .apply(&Event::RevisionCheckpointsSet { enabled: true })
        .unwrap();
    session.apply(&failed(&retries_used_up(1))).unwrap();
    session
}

#[test]
fn with_revision_checkpoints_on_the_human_operator_approves_each_revision() {
    let mut session = with_revision_checkpoints();
    let effects = session.apply(&revision(&[1])).unwrap();
    assert_eq!(
        effects,
        [Effect::ShowRevisionCheckpoint {
            task: 0,
            revision: revised(&[1]),
        }]
    );
    assert!(session.revisions().is_empty());

    let effects = session.apply(&Event::RevisionApproved).unwrap();
    assert_eq!(effects, [Effect::CommitRevision { task: 0 }]);
    let effects = session
        .apply(&Event::RevisionCommitted { commit: None })
        .unwrap();
    assert_eq!(effects, [Effect::RunWriter { id: WorkUnitId(1) }]);
    assert_eq!(session.revisions().len(), 1);
}

#[test]
fn a_revision_rejected_at_its_checkpoint_counts_and_carries_the_feedback() {
    let mut session = with_revision_checkpoints();
    session.apply(&revision(&[1])).unwrap();
    let effects = session
        .apply(&Event::CheckpointRejected {
            feedback: "Give it the Config type instead".into(),
        })
        .unwrap();
    assert_eq!(effects, [Effect::DiscardRevision { task: 0 }]);
    let effects = session.apply(&Event::RevisionDiscarded).unwrap();
    assert!(matches!(effects[..], [Effect::RunRevision { task: 0, .. }]));
    assert_eq!(
        session.revisions()[0].rejection.as_deref(),
        Some("Give it the Config type instead")
    );
}

#[test]
fn revision_checkpoint_events_round_trip_through_json() {
    for (event, expected) in [
        (
            Event::RevisionCheckpointsSet { enabled: true },
            r#"{"type":"revision_checkpoints_set","enabled":true}"#,
        ),
        (Event::RevisionApproved, r#"{"type":"revision_approved"}"#),
        (
            Event::RevisionCommitted {
                commit: Some("r1".into()),
            },
            r#"{"type":"revision_committed","commit":"r1"}"#,
        ),
        (Event::RevisionDiscarded, r#"{"type":"revision_discarded"}"#),
    ] {
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, expected);
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
    }
    let rejected = rewording_rejected(&[1]);
    let json = serde_json::to_string(&rejected).unwrap();
    assert!(!json.contains("shared_change"), "{json}");
    assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), rejected);
}
