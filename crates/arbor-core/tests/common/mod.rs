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
