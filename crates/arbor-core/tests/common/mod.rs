#![allow(dead_code)]

use arbor_core::{Plan, Priority, Request, Snippet, Task, WorkUnit, WorkUnitId};

pub fn request() -> Request {
    Request {
        text: "Add a --version flag".into(),
        issue: None,
    }
}

pub fn plan(task_titles: &[&str]) -> Plan {
    Plan {
        goal: "Print the version".into(),
        constraints: vec!["No new dependencies".into()],
        tasks: task_titles
            .iter()
            .map(|title| Task {
                title: (*title).into(),
                description: format!("Do {title}"),
            })
            .collect(),
    }
}

pub fn work_unit(id: u32) -> WorkUnit {
    WorkUnit {
        id: WorkUnitId(id),
        file: "src/main.rs".into(),
        spec: format!("Fill region {id}"),
        context: vec![Snippet {
            path: "src/lib.rs".into(),
            text: "pub fn version() -> &'static str;".into(),
            priority: Priority::Required,
        }],
        acceptance_criteria: vec!["Prints the version".into()],
        test_filter: format!("wu_{id}"),
    }
}

/// Every event of a one-Task, two-Work-Unit Session, from start to done.
pub fn full_session_events() -> Vec<arbor_core::Event> {
    use arbor_core::Event;
    vec![
        Event::SessionStarted { request: request() },
        Event::PlanSubmitted {
            plan: plan(&["Add flag"]),
        },
        Event::CheckpointRejected {
            feedback: "Mention the README".into(),
        },
        Event::PlanSubmitted {
            plan: plan(&["Add flag", "Update README"]),
        },
        Event::PlanApproved {
            task_order: vec![0],
        },
        Event::WorkUnitsSubmitted {
            work_units: vec![work_unit(1), work_unit(2)],
        },
        Event::ScaffoldChecksPassed,
        Event::ScaffoldApproved,
        Event::WorkUnitPassed { id: WorkUnitId(1) },
        Event::WorkUnitCommitted {
            id: WorkUnitId(1),
            commit: "c1".into(),
        },
        Event::WorkUnitPassed { id: WorkUnitId(2) },
        Event::WorkUnitCommitted {
            id: WorkUnitId(2),
            commit: "c2".into(),
        },
        Event::TaskChecksPassed,
    ]
}
