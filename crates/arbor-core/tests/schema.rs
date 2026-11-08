use arbor_core::{Plan, WorkUnit};
use serde_json::{Value, json};

fn schema_of<T: schemars::JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap()
}

fn required(schema: &Value) -> Vec<&str> {
    let mut fields: Vec<_> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    fields.sort_unstable();
    fields
}

#[test]
fn plan_schema_requires_goal_constraints_and_tasks() {
    let schema = schema_of::<Plan>();
    assert_eq!(required(&schema), ["constraints", "goal", "tasks"]);
    assert_eq!(required(&schema["$defs"]["Task"]), ["description", "title"]);
}

#[test]
fn work_unit_schema_requires_its_spec_context_and_test_filter() {
    let schema = schema_of::<WorkUnit>();
    assert_eq!(
        required(&schema),
        [
            "acceptance_criteria",
            "context",
            "file",
            "id",
            "spec",
            "test_filter"
        ]
    );
    assert_eq!(schema["$defs"]["WorkUnitId"]["type"], "integer");
    assert_eq!(
        required(&schema["$defs"]["Snippet"]),
        ["path", "priority", "text"]
    );
    assert_eq!(
        schema["$defs"]["Priority"]["enum"],
        json!(["required", "helpful"])
    );
}

#[test]
fn plan_schema_describes_its_fields_for_the_planner() {
    let schema = schema_of::<Plan>();
    for field in ["goal", "constraints", "tasks"] {
        assert!(
            schema["properties"][field]["description"].is_string(),
            "{field} has no description"
        );
    }
    let task = &schema["$defs"]["Task"]["properties"];
    assert!(task["description"]["description"].is_string());
}
