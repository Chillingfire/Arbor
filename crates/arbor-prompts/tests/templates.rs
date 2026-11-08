use std::fs;
use std::path::Path;

use arbor_config::ClarificationMode;
use arbor_core::{Plan, Priority, Request, Snippet, Task, WorkUnit, WorkUnitId};
use arbor_llm::{ContentPart, Message};
use arbor_prompts::{
    CheckProposalInputs, Conventions, PlannerInputs, Prompt, PromptError, Prompts,
    ScaffolderInputs, Template, TriedCheck, WriterInputs,
};
use tempfile::tempdir;

fn request(text: &str) -> Request {
    Request {
        text: text.to_owned(),
        issue: None,
    }
}

fn plan() -> Plan {
    Plan {
        goal: "Add a greeting".to_owned(),
        constraints: vec!["No new dependencies".to_owned()],
        tasks: vec![task()],
    }
}

fn task() -> Task {
    Task {
        title: "Add greet()".to_owned(),
        description: "A function that greets by name.".to_owned(),
    }
}

fn work_unit() -> WorkUnit {
    WorkUnit {
        id: WorkUnitId(7),
        file: "src/greet.rs".to_owned(),
        spec: "Return \"Hello, <name>!\".".to_owned(),
        context: vec![Snippet {
            path: "src/greet.rs".to_owned(),
            text: "pub fn greet(name: &str) -> String {".to_owned(),
            priority: Priority::Required,
        }],
        acceptance_criteria: vec!["greet(\"Ada\") is \"Hello, Ada!\"".to_owned()],
        test_filter: "greet".to_owned(),
    }
}

fn conventions(dir: &Path) -> Conventions {
    fs::write(dir.join("AGENTS.md"), "Use British spelling.\n").unwrap();
    Conventions::load(dir, &[]).unwrap()
}

fn planner(prompts: &Prompts, conventions: &Conventions, text: &str) -> Prompt {
    prompts
        .planner(
            conventions,
            &PlannerInputs {
                request: &request(text),
                clarification_mode: ClarificationMode::Ask,
            },
        )
        .unwrap()
}

fn scaffolder(prompts: &Prompts, conventions: &Conventions, mode: ClarificationMode) -> Prompt {
    prompts
        .scaffolder(
            conventions,
            &ScaffolderInputs {
                request: &request("Greet people"),
                plan: &plan(),
                task: &task(),
                clarification_mode: mode,
                cap_tokens: 8000,
            },
        )
        .unwrap()
}

fn writer(prompts: &Prompts) -> Prompt {
    prompts
        .writer(&WriterInputs {
            work_unit: &work_unit(),
        })
        .unwrap()
}

fn check_proposal(prompts: &Prompts, conventions: &Conventions, tried: &[TriedCheck]) -> Prompt {
    prompts
        .check_proposal(
            conventions,
            &CheckProposalInputs {
                detection: "Detected a Rust project from Cargo.toml.",
                tried_checks: tried,
            },
        )
        .unwrap()
}

fn override_template(root: &Path, template: &str, text: &str) {
    let dir = root.join(".arbor").join("prompts");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{template}.md")), text).unwrap();
}

#[test]
fn every_builtin_template_has_the_four_sections() {
    let dir = tempdir().unwrap();
    let prompts = Prompts::builtin();
    let conventions = conventions(dir.path());
    for prompt in [
        planner(&prompts, &conventions, "Greet people"),
        scaffolder(&prompts, &conventions, ClarificationMode::Ask),
        writer(&prompts),
        check_proposal(&prompts, &conventions, &[]),
    ] {
        let sections: Vec<&str> = prompt
            .instructions
            .lines()
            .filter(|line| line.starts_with("# "))
            .collect();
        assert_eq!(
            sections,
            ["# Role", "# Inputs", "# Output contract", "# Rules"],
            "{:?}",
            prompt.template
        );
    }
}

#[test]
fn messages_put_static_parts_first_with_a_breakpoint_after_the_conventions() {
    let dir = tempdir().unwrap();
    let prompt = planner(
        &Prompts::builtin(),
        &conventions(dir.path()),
        "Greet people",
    );

    assert_eq!(
        prompt.messages(),
        [
            Message::System {
                content: vec![
                    ContentPart::text(prompt.instructions.clone()),
                    ContentPart::cached(prompt.conventions.clone().unwrap()),
                ],
            },
            Message::User {
                content: vec![ContentPart::text(prompt.inputs.clone())],
            },
        ]
    );
    assert!(prompt.conventions.unwrap().contains("British spelling"));
    assert!(prompt.inputs.contains("Greet people"));
}

#[test]
fn without_conventions_the_breakpoint_goes_after_the_instructions() {
    let dir = tempdir().unwrap();
    let empty = Conventions::load(dir.path(), &[]).unwrap();
    let prompt = planner(&Prompts::builtin(), &empty, "Greet people");
    assert_eq!(prompt.conventions, None);
    assert_eq!(
        prompt.messages()[0],
        Message::System {
            content: vec![ContentPart::cached(prompt.instructions.clone())],
        }
    );
}

#[test]
fn the_writer_gets_no_conventions_and_sees_its_work_unit() {
    let prompt = writer(&Prompts::builtin());
    assert_eq!(prompt.template, Template::Writer);
    assert_eq!(prompt.conventions, None);
    assert!(prompt.inputs.contains("WU-7"));
    assert!(prompt.inputs.contains("src/greet.rs"));
    assert!(
        prompt
            .inputs
            .contains("pub fn greet(name: &str) -> String {")
    );
    assert!(prompt.inputs.contains("priority=\"required\""));
    assert!(prompt.inputs.contains("greet(\"Ada\") is \"Hello, Ada!\""));
}

#[test]
fn instructions_do_not_change_with_session_inputs() {
    let dir = tempdir().unwrap();
    let prompts = Prompts::builtin();
    let conventions = conventions(dir.path());
    let first = planner(&prompts, &conventions, "Greet people");
    let second = planner(&prompts, &conventions, "Say goodbye");
    assert_eq!(first.instructions, second.instructions);
    assert_eq!(first.conventions, second.conventions);
    assert_ne!(first.inputs, second.inputs);
    assert!(!first.instructions.contains("Greet people"));
}

#[test]
fn the_scaffolder_is_told_the_writer_cap_and_the_clarification_mode() {
    let dir = tempdir().unwrap();
    let prompts = Prompts::builtin();
    let conventions = conventions(dir.path());
    let ask = scaffolder(&prompts, &conventions, ClarificationMode::Ask);
    let assume = scaffolder(&prompts, &conventions, ClarificationMode::Assume);
    assert!(ask.instructions.contains("8000 tokens"));
    assert!(ask.instructions.contains("Clarification Mode is Ask"));
    assert!(assume.instructions.contains("Clarification Mode is Assume"));
    assert!(ask.inputs.contains("No new dependencies"));
    assert!(ask.inputs.contains("A function that greets by name."));
}

#[test]
fn text_joins_the_parts_in_prompt_order_and_is_saved_per_tier() {
    let dir = tempdir().unwrap();
    let prompt = planner(
        &Prompts::builtin(),
        &conventions(dir.path()),
        "Greet people",
    );
    let text = prompt.text();
    let instructions = text.find("# Role").unwrap();
    let conventions = text.find("British spelling").unwrap();
    let inputs = text.find("Greet people").unwrap();
    assert!(instructions < conventions && conventions < inputs);
    assert!(!text.contains('\r'));

    let session = dir.path().join(".arbor").join("sessions").join("s1");
    let path = prompt.save(&session).unwrap();
    assert_eq!(path, session.join("prompts").join("planner.md"));
    assert_eq!(fs::read_to_string(path).unwrap(), text);
}

#[test]
fn a_project_override_replaces_the_builtin_template() {
    let dir = tempdir().unwrap();
    override_template(
        dir.path(),
        "planner",
        "{% block instructions %}\r\nPlan carefully ({{ clarification_mode }}).\r\n{% endblock %}\r\n\
         {% block inputs %}\r\nAsked: {{ request.text }}\r\n{% endblock %}\r\n",
    );
    let prompts = Prompts::load(dir.path()).unwrap();
    let empty = Conventions::default();
    let prompt = planner(&prompts, &empty, "Greet people");
    assert_eq!(prompt.instructions, "Plan carefully (ask).\n");
    assert_eq!(prompt.inputs, "Asked: Greet people\n");
    // Templates without an override keep the built-in one.
    assert!(writer(&prompts).instructions.starts_with("# Role"));
}

#[test]
fn an_unknown_variable_fails_at_load_even_in_an_untaken_branch() {
    let dir = tempdir().unwrap();
    override_template(
        dir.path(),
        "writer",
        "{% block instructions %}{% if false %}{{ cap_tokens }}{% endif %}{% endblock %}\
         {% block inputs %}{{ work_unit.spec }}{% endblock %}",
    );
    let err = Prompts::load(dir.path()).unwrap_err();
    match &err {
        PromptError::UnknownVariables {
            template,
            path,
            variables,
        } => {
            assert_eq!(*template, Template::Writer);
            assert!(path.ends_with("writer.md"), "{path:?}");
            assert_eq!(variables, &["cap_tokens"]);
        }
        other => panic!("unexpected error: {other:?}"),
    }
    let message = err.to_string();
    assert!(
        message.contains("cap_tokens") && message.contains("writer.md"),
        "{message}"
    );
}

#[test]
fn session_inputs_in_the_instructions_fail_at_load() {
    let dir = tempdir().unwrap();
    override_template(
        dir.path(),
        "planner",
        "{% block instructions %}Plan {{ request.text }}{% endblock %}\
         {% block inputs %}{{ request.text }}{% endblock %}",
    );
    let err = Prompts::load(dir.path()).unwrap_err();
    assert!(
        matches!(
            err,
            PromptError::NotStatic {
                template: Template::Planner,
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn broken_templates_fail_at_load() {
    for text in [
        "{% block instructions %}unclosed",
        "{% block instructions %}Only instructions{% endblock %}",
        "{% block instructions %}{{ request.text | nosuchfilter }}{% endblock %}\
         {% block inputs %}{% endblock %}",
    ] {
        let dir = tempdir().unwrap();
        override_template(dir.path(), "planner", text);
        let err = Prompts::load(dir.path()).unwrap_err();
        assert!(
            matches!(
                err,
                PromptError::Invalid {
                    template: Template::Planner,
                    ..
                }
            ),
            "{text}: {err:?}"
        );
        assert!(err.to_string().contains("planner.md"), "{err}");
    }
}

#[test]
fn the_check_proposal_sees_the_conventions_and_each_failed_command_with_its_output() {
    let dir = tempdir().unwrap();
    let tried = [TriedCheck {
        check: "scaffold".to_owned(),
        command: "cargo check --all-targets".to_owned(),
        output: "error: could not find `Cargo.toml`".to_owned(),
    }];
    let prompt = check_proposal(&Prompts::builtin(), &conventions(dir.path()), &tried);

    assert_eq!(prompt.template, Template::CheckProposal);
    assert!(prompt.conventions.unwrap().contains("British spelling"));
    assert!(prompt.instructions.contains("submit_checks"));
    assert!(
        prompt
            .inputs
            .contains("Detected a Rust project from Cargo.toml.")
    );
    assert!(
        prompt
            .inputs
            .contains("## scaffold: `cargo check --all-targets`")
    );
    assert!(prompt.inputs.contains("could not find `Cargo.toml`"));
}

#[test]
fn without_failed_commands_the_check_proposal_shows_only_the_detection() {
    let prompt = check_proposal(&Prompts::builtin(), &Conventions::default(), &[]);
    assert_eq!(
        prompt.inputs,
        "# Detection

Detected a Rust project from Cargo.toml.
"
    );
}
