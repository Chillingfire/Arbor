mod common;

use arbor_core::{ApplyError, Effect, Event, Phase, Session};
use common::{plan, request};

fn at_plan_checkpoint() -> Session {
    let mut session = Session::new();
    session
        .apply(&Event::SessionStarted { request: request() })
        .unwrap();
    session
        .apply(&Event::PlanSubmitted { plan: plan(&["A"]) })
        .unwrap();
    session
}

#[test]
fn an_edited_plan_replaces_the_submitted_one_and_its_checkpoint_shows_again() {
    let mut session = at_plan_checkpoint();
    let edited = plan(&["A", "B"]);

    let effects = session
        .apply(&Event::PlanEdited {
            plan: edited.clone(),
        })
        .unwrap();

    assert_eq!(effects, [Effect::ShowPlanCheckpoint]);
    assert_eq!(session.plan(), Some(&edited));
    let effects = session
        .apply(&Event::PlanApproved {
            task_order: vec![1, 0],
        })
        .unwrap();
    assert_eq!(
        effects,
        [Effect::RunScaffolder {
            task: 0,
            feedback: None,
        }]
    );
    assert_eq!(session.plan().unwrap().tasks[0].title, "B");
}

#[test]
fn a_plan_can_only_be_edited_at_its_checkpoint() {
    let mut session = at_plan_checkpoint();
    session
        .apply(&Event::PlanApproved {
            task_order: vec![0],
        })
        .unwrap();
    let refused = session.apply(&Event::PlanEdited { plan: plan(&["B"]) });
    assert!(matches!(refused, Err(ApplyError::Unexpected { .. })));
    assert!(matches!(session.phase(), Phase::Scaffolding { .. }));
}
