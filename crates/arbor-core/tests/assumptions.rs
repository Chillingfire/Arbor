use arbor_core::{Plan, assumptions_in};

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

#[test]
fn a_specs_assumptions_run_to_the_end_of_their_line_or_the_next_one() {
    let spec = "Return the longest word. Assumption: ties keep the first. assumption: ASCII only\n\
                Split on whitespace.\nAssumption: empty text has no words";
    assert_eq!(
        assumptions_in(spec),
        [
            "Assumption: ties keep the first.",
            "assumption: ASCII only",
            "Assumption: empty text has no words",
        ]
    );
    assert!(assumptions_in("Return the longest word.").is_empty());
}
