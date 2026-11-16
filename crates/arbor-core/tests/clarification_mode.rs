mod common;

use arbor_core::{ClarificationMode, Event, Phase, Session};
use common::{plan, request};

#[test]
fn a_switched_clarification_mode_is_kept_and_changes_nothing_else() {
    let mut session = Session::new();
    session
        .apply(&Event::SessionStarted { request: request() })
        .unwrap();
    session
        .apply(&Event::PlanSubmitted { plan: plan(&["A"]) })
        .unwrap();
    assert_eq!(session.clarification_mode(), None);

    let switched = Event::ClarificationModeSet {
        mode: ClarificationMode::Assume,
    };
    let effects = session.apply(&switched).unwrap();

    assert_eq!(effects, session.pending());
    assert_eq!(*session.phase(), Phase::PlanCheckpoint);
    assert_eq!(
        session.clarification_mode(),
        Some(ClarificationMode::Assume)
    );
    let json = serde_json::to_string(&switched).unwrap();
    assert_eq!(json, r#"{"type":"clarification_mode_set","mode":"assume"}"#);
    assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), switched);
}

#[test]
fn the_clarification_mode_cant_be_switched_before_the_session_starts() {
    let mut session = Session::new();
    let switched = Event::ClarificationModeSet {
        mode: ClarificationMode::Ask,
    };
    assert!(session.apply(&switched).is_err());
}
