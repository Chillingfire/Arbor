use arbor_core::Plan;

fn plan_with(constraints: &[&str]) -> Plan {
    Plan {
        goal: "Add a flag".into(),
        constraints: constraints.iter().map(|&c| c.to_owned()).collect(),
        tasks: vec![],
    }
}

#[test]
fn assumptions_are_the_constraints_marked_as_such() {
    let plan = plan_with(&[
        "Keep the public API stable",
        "Assumption: the flag defaults to off",
        "  assumption: only Linux matters",
        "ASSUMPTION:no config file",
        "Assumptions are listed below",
        "An Assumption: in the middle doesn't count",
    ]);
    assert_eq!(
        plan.assumptions().collect::<Vec<_>>(),
        [
            "Assumption: the flag defaults to off",
            "  assumption: only Linux matters",
            "ASSUMPTION:no config file",
        ]
    );
}

#[test]
fn a_plan_without_assumptions_has_none() {
    assert_eq!(
        plan_with(&["Keep it small", "Ass"]).assumptions().count(),
        0
    );
}
