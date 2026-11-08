mod common;

use arbor_core::{ApplyError, Effect, Event, Phase, Session, Tier, Usage, Usd};
use common::{full_session_events, plan, request};

fn spent(tier: Tier, nanos: u64) -> Event {
    Event::UsageRecorded {
        tier,
        usage: Usage {
            input_tokens: 100,
            output_tokens: 10,
            cost: Usd::from_nanos(nanos),
            ..Usage::default()
        },
    }
}

fn cap(nanos: u64) -> Event {
    Event::BudgetCapSet {
        cap: Some(Usd::from_nanos(nanos)),
    }
}

fn apply_all(session: &mut Session, events: &[Event]) -> Vec<Effect> {
    events
        .iter()
        .flat_map(|event| session.apply(event).unwrap())
        .collect()
}

/// A Session whose Planner is running under a Budget Cap of 100 nano-dollars.
fn planning_with_cap() -> Session {
    let mut session = Session::new();
    apply_all(
        &mut session,
        &[Event::SessionStarted { request: request() }, cap(100)],
    );
    session
}

/// A Session whose Planner reached the cap of 100 nano-dollars.
fn at_budget_checkpoint() -> Session {
    let mut session = planning_with_cap();
    session.apply(&spent(Tier::Planner, 100)).unwrap();
    session
}

#[test]
fn without_a_cap_spending_never_pauses() {
    let mut session = Session::new();
    session
        .apply(&Event::SessionStarted { request: request() })
        .unwrap();
    let effects = session.apply(&spent(Tier::Planner, u64::MAX)).unwrap();
    assert_eq!(effects, [Effect::RunPlanner { feedback: None }]);
    assert_eq!(session.budget_cap(), None);
    assert!(!session.is_at_budget_checkpoint());
}

#[test]
fn setting_a_cap_is_recorded_and_changes_nothing_else() {
    let mut session = Session::new();
    session
        .apply(&Event::SessionStarted { request: request() })
        .unwrap();
    let effects = session.apply(&cap(100)).unwrap();
    assert_eq!(effects, [Effect::RunPlanner { feedback: None }]);
    assert_eq!(session.budget_cap(), Some(Usd::from_nanos(100)));
}

#[test]
fn spending_below_the_cap_carries_on() {
    let mut session = planning_with_cap();
    let effects = session.apply(&spent(Tier::Planner, 99)).unwrap();
    assert_eq!(effects, [Effect::RunPlanner { feedback: None }]);
    assert!(!session.is_at_budget_checkpoint());
}

#[test]
fn reaching_the_cap_opens_a_budget_checkpoint_after_the_call() {
    let mut session = planning_with_cap();
    session.apply(&spent(Tier::Planner, 60)).unwrap();
    let effects = session.apply(&spent(Tier::Planner, 40)).unwrap();
    assert_eq!(effects, [Effect::ShowBudgetCheckpoint]);
    assert!(session.is_at_budget_checkpoint());
    assert_eq!(session.pending(), [Effect::ShowBudgetCheckpoint]);
    // The call that crossed the cap is counted in full.
    assert_eq!(session.usage().total().cost, Usd::from_nanos(100));
}

#[test]
fn the_outcome_of_the_call_that_reached_the_cap_still_lands() {
    let mut session = at_budget_checkpoint();
    // That last Planner call was the one that submitted the Plan.
    let effects = session
        .apply(&Event::PlanSubmitted { plan: plan(&["A"]) })
        .unwrap();
    assert_eq!(effects, [Effect::ShowBudgetCheckpoint]);
    assert_eq!(*session.phase(), Phase::PlanCheckpoint);
    assert!(session.plan().is_some());
}

#[test]
fn raising_the_cap_resumes_the_paused_work() {
    let mut session = at_budget_checkpoint();
    let effects = session.apply(&cap(200)).unwrap();
    assert_eq!(effects, [Effect::RunPlanner { feedback: None }]);
    assert!(!session.is_at_budget_checkpoint());

    // The raised cap is enforced in turn.
    let effects = session.apply(&spent(Tier::Planner, 100)).unwrap();
    assert_eq!(effects, [Effect::ShowBudgetCheckpoint]);
}

#[test]
fn removing_the_cap_resumes_the_paused_work() {
    let mut session = at_budget_checkpoint();
    session
        .apply(&Event::PlanSubmitted { plan: plan(&["A"]) })
        .unwrap();
    let effects = session.apply(&Event::BudgetCapSet { cap: None }).unwrap();
    assert_eq!(effects, [Effect::ShowPlanCheckpoint]);
    assert_eq!(session.budget_cap(), None);
}

#[test]
fn a_cap_that_is_already_reached_is_refused() {
    for nanos in [50, 100] {
        let mut session = at_budget_checkpoint();
        let before = session.clone();
        assert_eq!(
            session.apply(&cap(nanos)),
            Err(ApplyError::BudgetCapReached {
                cap: Usd::from_nanos(nanos),
                spent: Usd::from_nanos(100),
            })
        );
        assert_eq!(session, before);
    }
}

#[test]
fn stopping_at_the_budget_checkpoint_ends_the_session() {
    let mut session = at_budget_checkpoint();
    let effects = session.apply(&Event::SessionStopped).unwrap();
    assert_eq!(effects, [Effect::EndSession]);
    assert_eq!(*session.phase(), Phase::Stopped);
    assert!(!session.is_at_budget_checkpoint());
    assert!(!session.is_done());

    for event in [spent(Tier::Planner, 1), cap(500), Event::SessionStopped] {
        assert!(matches!(
            session.apply(&event),
            Err(ApplyError::Unexpected { .. })
        ));
    }
}

#[test]
fn stopping_is_refused_outside_a_budget_checkpoint() {
    let mut session = planning_with_cap();
    let before = session.clone();
    assert!(matches!(
        session.apply(&Event::SessionStopped),
        Err(ApplyError::Unexpected { .. })
    ));
    assert_eq!(session, before);
}

#[test]
fn a_session_that_finishes_after_reaching_the_cap_still_ends() {
    // The cap is reached by the last Writer call; its free checks and commit still complete.
    let mut events = full_session_events();
    events.insert(1, cap(100));
    let passed = events
        .iter()
        .rposition(|event| matches!(event, Event::WorkUnitPassed { .. }))
        .unwrap();
    events.insert(passed, spent(Tier::Writer, 100));
    let session = Session::replay(&events).unwrap();
    assert!(session.is_done());
    assert!(!session.is_at_budget_checkpoint());
    assert_eq!(session.pending(), [Effect::EndSession]);
}

#[test]
fn resuming_at_a_budget_checkpoint_shows_it_again() {
    let events = [
        Event::SessionStarted { request: request() },
        cap(100),
        spent(Tier::Planner, 100),
    ];
    let mut live = Session::new();
    apply_all(&mut live, &events);
    let resumed = Session::replay(&events).unwrap();
    assert_eq!(resumed, live);
    assert_eq!(resumed.pending(), [Effect::ShowBudgetCheckpoint]);
}

#[test]
fn budget_events_round_trip_through_json() {
    for event in [
        cap(100),
        Event::BudgetCapSet { cap: None },
        Event::SessionStopped,
    ] {
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
    }
    assert_eq!(
        serde_json::to_value(cap(100)).unwrap(),
        serde_json::json!({ "type": "budget_cap_set", "cap_nano_usd": 100 })
    );
}
