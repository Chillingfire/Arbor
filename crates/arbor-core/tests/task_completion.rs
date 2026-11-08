mod common;

use arbor_core::{
    ApplyError, Effect, Event, MAX_REVISIONS, Phase, Revision, RevisionCause, Session, WorkUnitId,
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

/// A Session whose only Task has written WU-1 and WU-2 and is running its full Unit Check.
fn at_task_checks(task_titles: &[&str]) -> Session {
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
                work_units: vec![work_unit(1), work_unit(2)],
            },
            Event::ScaffoldChecksPassed,
            Event::ScaffoldApproved,
        ],
    );
    apply_all(&mut session, &write_and_commit(1, "c1"));
    apply_all(&mut session, &write_and_commit(2, "c2"));
    assert_eq!(session.pending(), [Effect::RunTaskChecks { task: 0 }]);
    session
}

fn failed(output: &str) -> Event {
    Event::TaskChecksFailed {
        output: output.into(),
    }
}

fn cause(output: &str) -> RevisionCause {
    RevisionCause::TaskChecksFailed {
        output: output.into(),
    }
}

fn revision(diagnosis: &str, ids: &[u32]) -> Event {
    Event::RevisionSubmitted {
        revision: Revision {
            diagnosis: diagnosis.into(),
            work_units: ids.iter().map(|&id| work_unit(id)).collect(),
        },
    }
}

#[test]
fn a_failed_task_check_sends_its_output_to_a_revision() {
    let mut session = at_task_checks(&["A"]);
    let effects = session.apply(&failed("test wu_2 failed")).unwrap();
    assert_eq!(
        effects,
        [Effect::RunRevision {
            task: 0,
            cause: cause("test wu_2 failed"),
        }]
    );
}

#[test]
fn only_the_revised_work_units_are_written_again_then_the_task_is_checked() {
    let mut session = at_task_checks(&["A"]);
    session.apply(&failed("wu_2 failed")).unwrap();
    let mut effects = session
        .apply(&revision("WU-2 lacked the parser", &[2]))
        .unwrap();
    effects.extend(apply_all(&mut session, &write_and_commit(2, "c3")));
    assert_eq!(
        effects,
        [
            Effect::RunWriter { id: WorkUnitId(2) },
            Effect::CommitWorkUnit { id: WorkUnitId(2) },
            Effect::RunTaskChecks { task: 0 },
        ]
    );
    let task_commits: Vec<_> = session
        .task_commits()
        .iter()
        .map(|(_, commit)| commit.as_str())
        .collect();
    assert_eq!(task_commits, ["c1", "c2", "c3"]);
    assert_eq!(session.revisions().len(), 1);
    assert_eq!(session.revisions()[0].cause, cause("wu_2 failed"));
}

#[test]
fn a_revision_replaces_work_units_by_id_and_adds_new_ones() {
    let mut session = at_task_checks(&["A"]);
    session.apply(&failed("wu_2 failed")).unwrap();
    let mut revised = work_unit(2);
    revised.spec = "Parse first, then fill".into();
    let event = Event::RevisionSubmitted {
        revision: Revision {
            diagnosis: "WU-2 was too big; split off WU-3".into(),
            work_units: vec![revised.clone(), work_unit(3)],
        },
    };
    let effects = session.apply(&event).unwrap();
    assert_eq!(effects, [Effect::RunWriter { id: WorkUnitId(2) }]);
    assert_eq!(session.work_units(), [work_unit(1), revised, work_unit(3)]);
    assert_eq!(session.next_work_unit_id(), WorkUnitId(4));
}

#[test]
fn a_revision_may_not_reuse_an_id_from_an_earlier_task() {
    let mut session = at_task_checks(&["A", "B"]);
    apply_all(
        &mut session,
        &[
            Event::TaskChecksPassed,
            Event::WorkUnitsSubmitted {
                work_units: vec![work_unit(3)],
            },
            Event::ScaffoldChecksPassed,
            Event::ScaffoldApproved,
        ],
    );
    apply_all(&mut session, &write_and_commit(3, "c3"));
    session.apply(&failed("wu_3 failed")).unwrap();
    let before = session.clone();
    assert_eq!(
        session.apply(&revision("Split", &[3, 1])),
        Err(ApplyError::WorkUnitIdTaken { id: WorkUnitId(1) })
    );
    assert_eq!(
        session.apply(&revision("Twice", &[3, 3])),
        Err(ApplyError::DuplicateWorkUnit { id: WorkUnitId(3) })
    );
    assert_eq!(session, before);
}

#[test]
fn after_the_last_revision_fails_the_human_operator_decides() {
    let mut session = at_task_checks(&["A"]);
    for round in 0..MAX_REVISIONS {
        session.apply(&failed("still failing")).unwrap();
        session.apply(&revision("Try again", &[2])).unwrap();
        apply_all(&mut session, &write_and_commit(2, &format!("r{round}")));
    }
    let effects = session.apply(&failed("still failing")).unwrap();
    assert_eq!(
        effects,
        [Effect::ShowEscalationCheckpoint {
            task: 0,
            cause: cause("still failing"),
        }]
    );
    assert_eq!(session.revisions().len(), MAX_REVISIONS);
}

#[test]
fn an_operator_fix_at_the_escalation_checkpoint_reruns_the_task_check() {
    let mut session = at_task_checks(&["A"]);
    for _ in 0..MAX_REVISIONS {
        session.apply(&failed("still failing")).unwrap();
        session.apply(&revision("Try again", &[2])).unwrap();
        apply_all(&mut session, &write_and_commit(2, "r"));
    }
    session.apply(&failed("still failing")).unwrap();
    let effects = session.apply(&Event::OperatorFixed).unwrap();
    assert_eq!(effects, [Effect::RunTaskChecks { task: 0 }]);

    let effects = session.apply(&failed("broken again")).unwrap();
    assert!(
        matches!(effects[..], [Effect::ShowEscalationCheckpoint { .. }]),
        "a fix by hand doesn't give the Scaffolder more Revisions"
    );
}

#[test]
fn a_passing_task_check_starts_the_next_task_with_no_revisions() {
    let mut session = at_task_checks(&["A", "B"]);
    session.apply(&failed("wu_2 failed")).unwrap();
    session.apply(&revision("Fix WU-2", &[2])).unwrap();
    apply_all(&mut session, &write_and_commit(2, "c3"));
    let effects = session.apply(&Event::TaskChecksPassed).unwrap();
    assert_eq!(
        effects,
        [Effect::RunScaffolder {
            task: 1,
            feedback: None
        }]
    );
    assert!(session.revisions().is_empty());
    assert!(session.task_commits().is_empty());
}

#[test]
fn resuming_during_a_revision_reruns_it() {
    let mut session = at_task_checks(&["A"]);
    session.apply(&failed("wu_2 failed")).unwrap();
    assert!(matches!(session.phase(), Phase::Revising { task: 0, .. }));
    assert_eq!(
        session.pending(),
        [Effect::RunRevision {
            task: 0,
            cause: cause("wu_2 failed"),
        }]
    );
}

#[test]
fn revision_events_round_trip_through_json() {
    let event = revision("Fix WU-2", &[2]);
    let json = serde_json::to_string(&event).unwrap();
    assert!(
        json.starts_with(r#"{"type":"revision_submitted""#),
        "{json}"
    );
    assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
}
