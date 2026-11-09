mod common;

use arbor_core::{
    ApplyError, Effect, Event, MAX_REVISIONS, Phase, Replan, Revision, RevisionCause, Session,
    Task, WorkUnitId,
};
use common::{plan, request, work_unit};

fn apply_all(session: &mut Session, events: &[Event]) -> Vec<Effect> {
    events
        .iter()
        .flat_map(|event| session.apply(event).unwrap())
        .collect()
}

fn scaffold_and_write(first_id: u32, task_check_output: &str) -> Vec<Event> {
    vec![
        Event::WorkUnitsSubmitted {
            work_units: vec![work_unit(first_id)],
        },
        Event::ScaffoldChecksPassed,
        Event::ScaffoldApproved,
        Event::SkeletonCommitted { commit: None },
        Event::WorkUnitPassed {
            id: WorkUnitId(first_id),
        },
        Event::WorkUnitCommitted {
            id: WorkUnitId(first_id),
            commit: format!("c{first_id}"),
        },
        Event::TaskChecksFailed {
            output: task_check_output.into(),
        },
    ]
}

fn use_up_revisions(session: &mut Session, id: u32) {
    for round in 0..MAX_REVISIONS {
        apply_all(
            session,
            &[
                Event::RevisionSubmitted {
                    revision: Revision {
                        diagnosis: format!("Attempt {round}"),
                        work_units: vec![work_unit(id)],
                        shared_change: None,
                    },
                },
                Event::WorkUnitPassed { id: WorkUnitId(id) },
                Event::WorkUnitCommitted {
                    id: WorkUnitId(id),
                    commit: format!("r{round}"),
                },
                Event::TaskChecksFailed {
                    output: "still failing".into(),
                },
            ],
        );
    }
}

/// Tasks A and B approved; A's WU-1 fails the Task Check through both Revisions, so the
/// escalation Checkpoint is open.
fn escalated() -> Session {
    let mut session = Session::new();
    apply_all(
        &mut session,
        &[
            Event::SessionStarted { request: request() },
            Event::PlanSubmitted {
                plan: plan(&["A", "B"]),
            },
            Event::PlanApproved {
                task_order: vec![0, 1],
            },
        ],
    );
    apply_all(&mut session, &scaffold_and_write(1, "first failure"));
    use_up_revisions(&mut session, 1);
    assert!(matches!(
        session.phase(),
        Phase::EscalationCheckpoint { task: 0, .. }
    ));
    session
}

fn still_failing() -> RevisionCause {
    RevisionCause::TaskChecksFailed {
        output: "still failing".into(),
    }
}

fn task(title: &str) -> Task {
    Task {
        title: title.into(),
        description: format!("Do {title}"),
    }
}

fn replan(titles: &[&str]) -> Replan {
    Replan {
        diagnosis: "A was two changes in one".into(),
        tasks: titles.iter().map(|title| task(title)).collect(),
    }
}

#[test]
fn the_human_operator_sends_the_failure_to_the_planner_with_its_chain() {
    let mut session = escalated();
    assert!(session.may_replan(0));
    let effects = session.apply(&Event::ReplanRequested).unwrap();
    assert_eq!(
        effects,
        [Effect::RunReplan {
            task: 0,
            cause: still_failing(),
            feedback: None,
        }]
    );
    assert_eq!(
        session.revisions().len(),
        MAX_REVISIONS,
        "the Planner sees every Revision"
    );
}

#[test]
fn the_replan_goes_through_the_plan_checkpoint() {
    let mut session = escalated();
    session.apply(&Event::ReplanRequested).unwrap();
    let effects = session
        .apply(&Event::ReplanSubmitted {
            replan: replan(&["A1", "A2"]),
        })
        .unwrap();
    assert_eq!(
        effects,
        [Effect::ShowReplanCheckpoint {
            task: 0,
            replan: replan(&["A1", "A2"]),
        }]
    );

    let effects = session
        .apply(&Event::CheckpointRejected {
            feedback: "Keep it one Task".into(),
        })
        .unwrap();
    assert_eq!(
        effects,
        [Effect::RunReplan {
            task: 0,
            cause: still_failing(),
            feedback: Some("Keep it one Task".into()),
        }]
    );
}

#[test]
fn an_approved_replan_replaces_the_task_and_resets_the_worktree_to_its_start() {
    let mut session = escalated();
    apply_all(
        &mut session,
        &[
            Event::ReplanRequested,
            Event::ReplanSubmitted {
                replan: replan(&["A1", "A2", "A3"]),
            },
        ],
    );
    let effects = session
        .apply(&Event::PlanApproved {
            task_order: vec![1, 0],
        })
        .unwrap();
    assert_eq!(effects, [Effect::ResetTask { task: 0 }]);
    let titles: Vec<_> = session
        .plan()
        .unwrap()
        .tasks
        .iter()
        .map(|task| task.title.as_str())
        .collect();
    assert_eq!(titles, ["A2", "A1", "B"]);
    assert_eq!(session.task_commits().len(), 3, "kept until the reset");

    let effects = session.apply(&Event::TaskReset).unwrap();
    assert_eq!(
        effects,
        [Effect::RunScaffolder {
            task: 0,
            feedback: None,
        }]
    );
    assert!(session.task_commits().is_empty());
    assert!(session.commits().is_empty());
    assert!(session.revisions().is_empty());
    assert!(session.work_units().is_empty());
    assert_eq!(
        session.next_work_unit_id(),
        WorkUnitId(2),
        "IDs are never reused"
    );
}

#[test]
fn a_replan_with_an_invalid_task_order_is_refused() {
    let mut session = escalated();
    apply_all(
        &mut session,
        &[
            Event::ReplanRequested,
            Event::ReplanSubmitted {
                replan: replan(&["A1"]),
            },
        ],
    );
    let before = session.clone();
    assert_eq!(
        session.apply(&Event::PlanApproved {
            task_order: vec![1],
        }),
        Err(ApplyError::InvalidTaskOrder {
            task_order: vec![1],
        })
    );
    assert_eq!(session, before);
}

#[test]
fn a_task_from_a_replan_cannot_be_replanned_again() {
    let mut session = escalated();
    apply_all(
        &mut session,
        &[
            Event::ReplanRequested,
            Event::ReplanSubmitted {
                replan: replan(&["A1"]),
            },
            Event::PlanApproved {
                task_order: vec![0],
            },
            Event::TaskReset,
        ],
    );
    apply_all(&mut session, &scaffold_and_write(2, "A1 failed"));
    use_up_revisions(&mut session, 2);
    assert!(!session.may_replan(0));
    let before = session.clone();
    assert_eq!(
        session.apply(&Event::ReplanRequested),
        Err(ApplyError::ReplanUsed { task: 0 })
    );
    assert_eq!(session, before);
}

#[test]
fn the_next_task_keeps_its_own_replan() {
    let mut session = escalated();
    apply_all(
        &mut session,
        &[
            Event::ReplanRequested,
            Event::ReplanSubmitted {
                replan: replan(&["A1"]),
            },
            Event::PlanApproved {
                task_order: vec![0],
            },
            Event::TaskReset,
        ],
    );
    assert!(session.may_replan(1));
}

#[test]
fn replan_events_round_trip_through_json() {
    for event in [
        Event::ReplanRequested,
        Event::ReplanSubmitted {
            replan: replan(&["A1"]),
        },
        Event::TaskReset,
    ] {
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
    }
    assert_eq!(
        serde_json::to_string(&Event::ReplanRequested).unwrap(),
        r#"{"type":"replan_requested"}"#
    );
}
