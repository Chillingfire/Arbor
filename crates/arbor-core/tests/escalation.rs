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

fn revision(ids: &[u32]) -> Event {
    Event::RevisionSubmitted {
        revision: Revision {
            diagnosis: "The Context Pack lacked the Config type".into(),
            work_units: ids.iter().map(|&id| work_unit(id)).collect(),
        },
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
    let mut effects = session.apply(&revision(&[2, 4])).unwrap();
    effects.extend(apply_all(&mut session, &write_and_commit(2, "c2")));
    effects.extend(apply_all(&mut session, &write_and_commit(4, "c4")));
    effects.extend(apply_all(&mut session, &write_and_commit(3, "c3")));
    assert_eq!(
        effects,
        [
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
        session.apply(&revision(&[1])).unwrap();
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
    session.apply(&revision(&[1, 4])).unwrap();
    apply_all(&mut session, &write_and_commit(1, "c1"));
    session.apply(&failed(&retries_used_up(4))).unwrap();
    session.apply(&revision(&[4])).unwrap();
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
        session.apply(&revision(&[1])).unwrap();
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
        session.apply(&revision(&[1])).unwrap();
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
