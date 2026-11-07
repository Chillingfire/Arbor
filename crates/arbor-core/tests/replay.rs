mod common;

use arbor_core::{ApplyError, Effect, Event, Session, WorkUnitId};
use common::full_session_events;

#[test]
fn replay_rebuilds_the_live_state() {
    let events = full_session_events();
    let mut live = Session::new();
    for event in &events {
        live.apply(event).unwrap();
    }
    assert_eq!(Session::replay(&events).unwrap(), live);
    assert!(live.is_done());
}

#[test]
fn resuming_mid_session_redoes_the_in_flight_work() {
    let events = full_session_events();
    // The log ends right after WU-1 was committed, while the Writer for WU-2 was running.
    let resumed = Session::replay(&events[..10]).unwrap();
    assert_eq!(resumed.pending(), [Effect::RunWriter { id: WorkUnitId(2) }]);
}

#[test]
fn replaying_an_invalid_log_fails() {
    let events = [Event::ScaffoldApproved];
    assert!(matches!(
        Session::replay(&events),
        Err(ApplyError::Unexpected { .. })
    ));
}
