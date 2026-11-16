mod common;

use arbor_core::{
    ApplyError, Effect, Event, Phase, RewindPoint, RunOutcome, Session, Tier, Usage, Usd,
    WorkUnitId, rewind_points,
};
use common::{full_session_events, plan, work_unit};

fn usage(nanos: u64) -> Event {
    Event::UsageRecorded {
        tier: Tier::Planner,
        usage: Usage {
            cost: Usd::from_nanos(nanos),
            ..Usage::default()
        },
    }
}

fn cancelled() -> Event {
    Event::RunEnded {
        outcome: RunOutcome::Cancelled,
    }
}

#[test]
fn a_run_ending_leaves_the_session_where_it_waits() {
    let mut events = full_session_events()[..2].to_vec();
    events.push(cancelled());
    let session = Session::replay(&events).unwrap();
    assert_eq!(session.phase(), &Phase::PlanCheckpoint);
    assert_eq!(session.pending(), [Effect::ShowPlanCheckpoint]);
}

#[test]
fn a_run_can_end_at_the_budget_checkpoint() {
    let mut events = full_session_events()[..1].to_vec();
    events.push(Event::BudgetCapSet {
        cap: Some(Usd::from_nanos(10)),
    });
    events.push(usage(10));
    events.push(Event::RunEnded {
        outcome: RunOutcome::Failed {
            reason: "the model call failed".into(),
        },
    });
    let session = Session::replay(&events).unwrap();
    assert!(session.is_at_budget_checkpoint());
}

#[test]
fn only_replay_applies_a_rewind() {
    let mut session = Session::replay(&full_session_events()[..2]).unwrap();
    assert_eq!(
        session.apply(&Event::Rewound { event_count: 2 }),
        Err(ApplyError::RewindNeedsLog)
    );
}

#[test]
fn a_rewind_goes_back_to_the_checkpoint_and_keeps_what_was_spent() {
    let mut events = full_session_events()[..2].to_vec();
    events.push(usage(5));
    events.extend(full_session_events()[2..9].iter().cloned());
    events.push(usage(7));
    events.push(Event::Rewound { event_count: 2 });

    let session = Session::replay(&events).unwrap();

    assert_eq!(session.phase(), &Phase::PlanCheckpoint);
    assert_eq!(session.plan(), Some(&plan(&["Add flag"])));
    assert!(session.commits().is_empty());
    assert_eq!(session.usage().total().cost, Usd::from_nanos(12));
    assert_eq!(session.next_work_unit_id(), WorkUnitId(3));
}

#[test]
fn a_rewind_must_name_an_earlier_checkpoint() {
    let events = full_session_events();
    for event_count in [1, 6, events.len() + 1] {
        let mut rewound = events.clone();
        rewound.push(Event::Rewound { event_count });
        assert_eq!(
            Session::replay(&rewound),
            Err(ApplyError::InvalidRewind { event_count }),
            "{event_count}"
        );
    }
}

#[test]
fn the_session_goes_on_from_the_checkpoint_it_was_rewound_to() {
    let mut events = full_session_events();
    events.push(Event::Rewound { event_count: 4 });
    events.push(Event::PlanApproved {
        task_order: vec![1],
    });
    let session = Session::replay(&events).unwrap();
    assert_eq!(
        session.pending(),
        [Effect::RunScaffolder {
            task: 0,
            feedback: None
        }]
    );
    assert_eq!(session.plan().unwrap().tasks[0].title, "Update README");
}

#[test]
fn every_checkpoint_with_restorable_files_is_a_rewind_point() {
    let points = rewind_points(&full_session_events()).unwrap();
    let plan_checkpoint = |event_count| RewindPoint {
        event_count,
        phase: Phase::PlanCheckpoint,
        head: None,
        reviewed_edits: None,
    };
    assert_eq!(
        points,
        [
            plan_checkpoint(2),
            plan_checkpoint(4),
            RewindPoint {
                event_count: 7,
                phase: Phase::ScaffoldCheckpoint { task: 0 },
                head: None,
                reviewed_edits: None,
            },
        ]
    );
}

#[test]
fn a_scaffold_checkpoint_points_at_the_commit_that_took_its_edits() {
    let mut events = full_session_events()[..8].to_vec();
    events.push(Event::SkeletonCommitted {
        commit: Some("skeleton".into()),
    });
    let points = rewind_points(&events).unwrap();
    assert_eq!(points[2].reviewed_edits.as_deref(), Some("skeleton"));
}

#[test]
fn a_scaffold_checkpoint_whose_edits_were_never_committed_is_no_rewind_point() {
    let mut events = full_session_events()[..7].to_vec();
    let at_checkpoint = rewind_points(&events).unwrap();
    events.push(Event::CheckpointRejected {
        feedback: "Smaller units".into(),
    });
    events.push(Event::WorkUnitsSubmitted {
        work_units: vec![work_unit(3)],
    });
    let after_rejection = rewind_points(&events).unwrap();

    assert_eq!(at_checkpoint.len(), 2);
    assert_eq!(after_rejection.len(), 2);
}

#[test]
fn points_rewound_away_from_stay_listed_and_the_rewind_adds_none() {
    let mut events = full_session_events();
    events.push(Event::Rewound { event_count: 2 });
    let points = rewind_points(&events).unwrap();
    assert_eq!(points.len(), 3);
    assert_eq!(points[2].head, None);

    events.push(Event::CheckpointRejected {
        feedback: "Again".into(),
    });
    events.push(Event::PlanSubmitted {
        plan: plan(&["Other"]),
    });
    let points = rewind_points(&events).unwrap();
    assert_eq!(points.len(), 4);
    assert_eq!(points[3].event_count, events.len());
}
