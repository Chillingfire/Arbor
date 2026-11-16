mod common;

use arbor_core::{ApplyError, CommitOrigin, Effect, Event, Session, SessionCommit, WorkUnitId};
use common::{plan, request, work_unit};

fn apply_all(session: &mut Session, events: &[Event]) -> Vec<Effect> {
    events
        .iter()
        .flat_map(|event| session.apply(event).unwrap())
        .collect()
}

#[test]
fn starting_a_session_runs_the_planner() {
    let mut session = Session::new();
    let effects = session
        .apply(&Event::SessionStarted { request: request() })
        .unwrap();
    assert_eq!(effects, [Effect::RunPlanner { feedback: None }]);
}

#[test]
fn happy_path_runs_plan_scaffold_write_to_done() {
    let mut session = Session::new();
    let effects = apply_all(
        &mut session,
        &[
            Event::SessionStarted { request: request() },
            Event::PlanSubmitted {
                plan: plan(&["Add flag"]),
            },
            Event::PlanApproved {
                task_order: vec![0],
            },
            Event::WorkUnitsSubmitted {
                work_units: vec![work_unit(1)],
            },
            Event::ScaffoldChecksPassed,
            Event::ScaffoldApproved,
            Event::SkeletonCommitted { commit: None },
            Event::WorkUnitPassed {
                id: WorkUnitId(1),
                model: None,
            },
            Event::WorkUnitCommitted {
                id: WorkUnitId(1),
                commit: "abc123".into(),
            },
            Event::TaskChecksPassed,
            Event::MarkersChecked { remaining: vec![] },
        ],
    );
    assert_eq!(
        effects,
        [
            Effect::RunPlanner { feedback: None },
            Effect::ShowPlanCheckpoint,
            Effect::RunScaffolder {
                task: 0,
                feedback: None
            },
            Effect::RunScaffoldChecks { task: 0 },
            Effect::ShowScaffoldCheckpoint { task: 0 },
            Effect::CommitSkeleton { task: 0 },
            Effect::RunWriter { id: WorkUnitId(1) },
            Effect::CommitWorkUnit { id: WorkUnitId(1) },
            Effect::RunTaskChecks { task: 0 },
            Effect::CheckMarkers,
            Effect::EndSession,
        ]
    );
    assert!(session.is_done());
}

/// A Session at its Plan Checkpoint, for a Plan with the given Tasks.
fn at_plan_checkpoint(task_titles: &[&str]) -> Session {
    let mut session = Session::new();
    apply_all(
        &mut session,
        &[
            Event::SessionStarted { request: request() },
            Event::PlanSubmitted {
                plan: plan(task_titles),
            },
        ],
    );
    session
}

#[test]
fn rejecting_the_plan_reruns_the_planner_with_feedback() {
    let mut session = at_plan_checkpoint(&["Add flag"]);
    let effects = session
        .apply(&Event::CheckpointRejected {
            feedback: "Also update the README".into(),
        })
        .unwrap();
    assert_eq!(
        effects,
        [Effect::RunPlanner {
            feedback: Some("Also update the README".into())
        }]
    );
}

#[test]
fn rejecting_the_scaffold_reruns_the_scaffolder_with_feedback() {
    let mut session = at_plan_checkpoint(&["Add flag"]);
    apply_all(
        &mut session,
        &[
            Event::PlanApproved {
                task_order: vec![0],
            },
            Event::WorkUnitsSubmitted {
                work_units: vec![work_unit(1)],
            },
            Event::ScaffoldChecksPassed,
        ],
    );
    let effects = session
        .apply(&Event::CheckpointRejected {
            feedback: "Split WU-1".into(),
        })
        .unwrap();
    assert_eq!(
        effects,
        [Effect::RunScaffolder {
            task: 0,
            feedback: Some("Split WU-1".into())
        }]
    );
}

#[test]
fn approved_task_order_drops_and_reorders_tasks() {
    let mut session = at_plan_checkpoint(&["A", "B", "C"]);
    session
        .apply(&Event::PlanApproved {
            task_order: vec![2, 0],
        })
        .unwrap();
    let titles: Vec<_> = session
        .plan()
        .unwrap()
        .tasks
        .iter()
        .map(|task| task.title.as_str())
        .collect();
    assert_eq!(titles, ["C", "A"]);
}

#[test]
fn work_units_and_tasks_run_in_order() {
    let mut session = at_plan_checkpoint(&["A", "B"]);
    let effects = apply_all(
        &mut session,
        &[
            Event::PlanApproved {
                task_order: vec![0, 1],
            },
            Event::WorkUnitsSubmitted {
                work_units: vec![work_unit(1), work_unit(2)],
            },
            Event::ScaffoldChecksPassed,
            Event::ScaffoldApproved,
            Event::SkeletonCommitted { commit: None },
            Event::WorkUnitPassed {
                id: WorkUnitId(1),
                model: None,
            },
            Event::WorkUnitCommitted {
                id: WorkUnitId(1),
                commit: "c1".into(),
            },
            Event::WorkUnitPassed {
                id: WorkUnitId(2),
                model: None,
            },
            Event::WorkUnitCommitted {
                id: WorkUnitId(2),
                commit: "c2".into(),
            },
            Event::TaskChecksPassed,
            Event::WorkUnitsSubmitted {
                work_units: vec![work_unit(3)],
            },
            Event::ScaffoldChecksPassed,
            Event::ScaffoldApproved,
            Event::SkeletonCommitted { commit: None },
            Event::WorkUnitPassed {
                id: WorkUnitId(3),
                model: None,
            },
            Event::WorkUnitCommitted {
                id: WorkUnitId(3),
                commit: "c3".into(),
            },
            Event::TaskChecksPassed,
        ],
    );
    assert_eq!(
        effects,
        [
            Effect::RunScaffolder {
                task: 0,
                feedback: None
            },
            Effect::RunScaffoldChecks { task: 0 },
            Effect::ShowScaffoldCheckpoint { task: 0 },
            Effect::CommitSkeleton { task: 0 },
            Effect::RunWriter { id: WorkUnitId(1) },
            Effect::CommitWorkUnit { id: WorkUnitId(1) },
            Effect::RunWriter { id: WorkUnitId(2) },
            Effect::CommitWorkUnit { id: WorkUnitId(2) },
            Effect::RunTaskChecks { task: 0 },
            Effect::RunScaffolder {
                task: 1,
                feedback: None
            },
            Effect::RunScaffoldChecks { task: 1 },
            Effect::ShowScaffoldCheckpoint { task: 1 },
            Effect::CommitSkeleton { task: 1 },
            Effect::RunWriter { id: WorkUnitId(3) },
            Effect::CommitWorkUnit { id: WorkUnitId(3) },
            Effect::RunTaskChecks { task: 1 },
            Effect::CheckMarkers,
        ]
    );
    let commits: Vec<_> = session
        .commits()
        .iter()
        .map(|commit| commit.hash.as_str())
        .collect();
    assert_eq!(commits, ["c1", "c2", "c3"]);
}

#[test]
fn invalid_task_orders_are_refused_and_change_nothing() {
    for task_order in [vec![], vec![3], vec![0, 0]] {
        let mut session = at_plan_checkpoint(&["A", "B", "C"]);
        let before = session.clone();
        let apply_result = session.apply(&Event::PlanApproved {
            task_order: task_order.clone(),
        });
        assert_eq!(
            apply_result,
            Err(ApplyError::InvalidTaskOrder { task_order }),
        );
        assert_eq!(session, before);
    }
}

/// A Session at its Scaffold Checkpoint for Task 0, with the given Work Units.
fn at_scaffold_checkpoint(work_unit_ids: &[u32]) -> Session {
    let mut session = at_plan_checkpoint(&["A"]);
    apply_all(
        &mut session,
        &[
            Event::PlanApproved {
                task_order: vec![0],
            },
            Event::WorkUnitsSubmitted {
                work_units: work_unit_ids.iter().map(|&id| work_unit(id)).collect(),
            },
            Event::ScaffoldChecksPassed,
        ],
    );
    session
}

#[test]
fn events_out_of_order_are_refused_and_change_nothing() {
    let mut session = at_plan_checkpoint(&["A"]);
    let before = session.clone();
    let event = Event::WorkUnitCommitted {
        id: WorkUnitId(1),
        commit: "c1".into(),
    };
    assert!(matches!(
        session.apply(&event),
        Err(ApplyError::Unexpected { .. })
    ));
    assert_eq!(session, before);
}

#[test]
fn events_for_another_work_unit_are_refused() {
    let mut session = at_scaffold_checkpoint(&[1, 2]);
    session.apply(&Event::ScaffoldApproved).unwrap();
    session
        .apply(&Event::SkeletonCommitted { commit: None })
        .unwrap();
    let before = session.clone();
    assert_eq!(
        session.apply(&Event::WorkUnitPassed {
            id: WorkUnitId(2),
            model: None
        }),
        Err(ApplyError::WrongWorkUnit {
            expected: WorkUnitId(1),
            got: WorkUnitId(2)
        })
    );
    assert_eq!(session, before);

    session
        .apply(&Event::WorkUnitPassed {
            id: WorkUnitId(1),
            model: None,
        })
        .unwrap();
    assert_eq!(
        session.apply(&Event::WorkUnitCommitted {
            id: WorkUnitId(2),
            commit: "c2".into()
        }),
        Err(ApplyError::WrongWorkUnit {
            expected: WorkUnitId(1),
            got: WorkUnitId(2)
        })
    );
}

#[test]
fn duplicate_work_unit_ids_are_refused() {
    let mut session = at_plan_checkpoint(&["A"]);
    session
        .apply(&Event::PlanApproved {
            task_order: vec![0],
        })
        .unwrap();
    assert_eq!(
        session.apply(&Event::WorkUnitsSubmitted {
            work_units: vec![work_unit(1), work_unit(1)],
        }),
        Err(ApplyError::DuplicateWorkUnit { id: WorkUnitId(1) })
    );
}

#[test]
fn a_task_without_work_units_goes_straight_to_task_checks() {
    let mut session = at_scaffold_checkpoint(&[]);
    session.apply(&Event::ScaffoldApproved).unwrap();
    assert_eq!(
        session
            .apply(&Event::SkeletonCommitted { commit: None })
            .unwrap(),
        [Effect::RunTaskChecks { task: 0 }]
    );
}

#[test]
fn the_approved_skeleton_is_committed_before_any_writer_runs() {
    let mut session = at_scaffold_checkpoint(&[1]);
    assert_eq!(
        session.apply(&Event::ScaffoldApproved).unwrap(),
        [Effect::CommitSkeleton { task: 0 }]
    );
    assert_eq!(
        session
            .apply(&Event::SkeletonCommitted {
                commit: Some("s1".into())
            })
            .unwrap(),
        [Effect::RunWriter { id: WorkUnitId(1) }]
    );
    assert_eq!(
        session.task_commits(),
        [SessionCommit {
            hash: "s1".into(),
            origin: CommitOrigin::Skeleton
        }]
    );
}
