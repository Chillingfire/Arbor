mod common;

use arbor_core::{ApplyError, Effect, Event, Session, Tier, Usage, Usd};
use common::{full_session_events, request};

fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64, cost: Usd) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        cost,
    }
}

fn recorded(tier: Tier, usage: Usage) -> Event {
    Event::UsageRecorded { tier, usage }
}

fn started() -> Session {
    let mut session = Session::new();
    session
        .apply(&Event::SessionStarted { request: request() })
        .unwrap();
    session
}

#[test]
fn usd_converts_from_dollars_to_whole_nano_dollars() {
    assert_eq!(Usd::from_dollars(0.0123).nanos(), 12_300_000);
    assert_eq!(Usd::from_dollars(0.000_000_000_6).nanos(), 1);
    assert_eq!(Usd::from_dollars(1.5).dollars(), 1.5);
    assert_eq!(Usd::from_dollars(-1.0), Usd::ZERO);
    assert_eq!(Usd::from_dollars(f64::NAN), Usd::ZERO);
}

#[test]
fn usage_is_totalled_per_tier_including_cache_tokens() {
    let mut session = started();
    let effects = session
        .apply(&recorded(
            Tier::Planner,
            usage(1000, 200, 800, 100, Usd::from_nanos(5_000)),
        ))
        .unwrap();
    // Recording usage doesn't move the Session on: the Planner is still running.
    assert_eq!(effects, [Effect::RunPlanner { feedback: None }]);
    session
        .apply(&recorded(
            Tier::Planner,
            usage(1500, 300, 1000, 0, Usd::from_nanos(7_000)),
        ))
        .unwrap();
    session
        .apply(&recorded(
            Tier::Writer,
            usage(400, 50, 0, 0, Usd::from_nanos(100)),
        ))
        .unwrap();

    let totals = session.usage();
    assert_eq!(
        *totals.tier(Tier::Planner),
        usage(2500, 500, 1800, 100, Usd::from_nanos(12_000))
    );
    assert_eq!(*totals.tier(Tier::Scaffolder), Usage::default());
    assert_eq!(
        *totals.tier(Tier::Writer),
        usage(400, 50, 0, 0, Usd::from_nanos(100))
    );
    assert_eq!(
        totals.total(),
        usage(2900, 550, 1800, 100, Usd::from_nanos(12_100))
    );
}

#[test]
fn usage_before_the_session_starts_or_after_it_ends_is_refused() {
    let event = recorded(Tier::Writer, usage(1, 1, 0, 0, Usd::from_nanos(1)));

    let mut session = Session::new();
    assert!(matches!(
        session.apply(&event),
        Err(ApplyError::Unexpected { .. })
    ));

    let mut session = Session::replay(&full_session_events()).unwrap();
    let before = session.clone();
    assert!(matches!(
        session.apply(&event),
        Err(ApplyError::Unexpected { .. })
    ));
    assert_eq!(session, before);
}

#[test]
fn replay_rebuilds_the_totals_including_calls_redone_after_a_crash() {
    let events = [
        Event::SessionStarted { request: request() },
        recorded(Tier::Planner, usage(100, 10, 0, 0, Usd::from_nanos(30))),
        // The process crashed and the Planner ran again on resume: both calls were paid for.
        recorded(Tier::Planner, usage(100, 10, 90, 0, Usd::from_nanos(20))),
    ];
    let session = Session::replay(&events).unwrap();
    assert_eq!(
        session.usage().total(),
        usage(200, 20, 90, 0, Usd::from_nanos(50))
    );
    assert_eq!(session.pending(), [Effect::RunPlanner { feedback: None }]);
}

#[test]
fn usage_events_round_trip_through_json() {
    let event = recorded(
        Tier::Scaffolder,
        usage(10, 20, 3, 4, Usd::from_dollars(0.25)),
    );
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "type": "usage_recorded",
            "tier": "scaffolder",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 20,
                "cache_read_tokens": 3,
                "cache_write_tokens": 4,
                "cost_nano_usd": 250_000_000u64
            }
        })
    );
    assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);
}
