mod common;

use std::path::Path;

use arbor_config::Checks;
use arbor_tools::{Tools, Toolset};
use common::{call, err, ok, write};
use serde_json::json;
use tempfile::tempdir;

/// Run by a check as a child process: prints the arguments it was given, one per line.
#[test]
fn print_args() {
    for arg in std::env::args().skip(1) {
        println!("ARG<{arg}>");
    }
}

/// Run by a check as a child process: prints far more output than a check returns.
#[test]
fn print_many_lines() {
    for n in 1..=1000 {
        println!("output line {n}");
    }
}

/// A command that runs one of the helper tests above in this test binary.
fn helper(test: &str) -> String {
    let exe = std::env::current_exe().unwrap();
    let exe = shell_words::quote(exe.to_str().unwrap()).into_owned();
    format!("{exe} {test} --exact --nocapture --test-threads=1")
}

fn tools(dir: &Path, checks: Checks) -> Tools {
    Tools::new(dir, Toolset::Scaffolder, checks)
}

#[tokio::test]
async fn runs_a_configured_check() {
    let dir = tempdir().unwrap();
    let checks = Checks {
        unit: Some("cargo --version".into()),
        ..Checks::default()
    };
    let text = ok(
        &tools(dir.path(), checks),
        "run_checks",
        json!({"check": "unit"}),
    )
    .await;
    assert!(text.contains("exit code 0"), "{text}");
    assert!(text.contains("cargo "), "{text}");
}

#[tokio::test]
async fn a_failing_check_is_an_error_with_its_output() {
    let dir = tempdir().unwrap();
    let checks = Checks {
        scaffold: Some("cargo no-such-subcommand-for-arbor".into()),
        ..Checks::default()
    };
    let text = err(
        &tools(dir.path(), checks),
        "run_checks",
        json!({"check": "scaffold"}),
    )
    .await;
    assert!(text.contains("exit code"), "{text}");
    assert!(text.contains("no-such-subcommand-for-arbor"), "{text}");
}

#[tokio::test]
async fn unconfigured_checks_and_missing_programs_are_errors() {
    let dir = tempdir().unwrap();
    let checks = Checks {
        unit: Some("no-such-program-for-arbor --flag".into()),
        ..Checks::default()
    };
    let tools = tools(dir.path(), checks);
    let message = err(&tools, "run_checks", json!({"check": "scaffold"})).await;
    assert!(message.contains("arbor.toml"), "{message}");
    let message = err(&tools, "run_checks", json!({"check": "unit"})).await;
    assert!(message.contains("no-such-program-for-arbor"), "{message}");
    err(&tools, "run_checks", json!({"check": "shell"})).await;
}

#[tokio::test]
async fn the_filter_is_one_argument_and_never_shell_syntax() {
    let dir = tempdir().unwrap();
    let checks = Checks {
        unit_filtered: Some(format!("{} {{filter}}", helper("print_args"))),
        ..Checks::default()
    };
    let tools = tools(dir.path(), checks);
    let filter = "x --version; echo pwned && touch pwned.txt";
    let args = json!({"check": "unit_filtered", "filter": filter});
    let text = ok(&tools, "run_checks", args).await;
    assert!(text.contains(&format!("ARG<{filter}>")), "{text}");
    assert!(!text.lines().any(|line| line.trim() == "pwned"), "{text}");
    assert!(!dir.path().join("pwned.txt").exists());
}

#[tokio::test]
async fn filters_are_required_and_may_not_be_options() {
    let dir = tempdir().unwrap();
    let checks = Checks {
        unit_filtered: Some(format!("{} {{filter}}", helper("print_args"))),
        ..Checks::default()
    };
    let tools = tools(dir.path(), checks);
    err(&tools, "run_checks", json!({"check": "unit_filtered"})).await;
    let args = json!({"check": "unit_filtered", "filter": "--config=evil"});
    let message = err(&tools, "run_checks", args).await;
    assert!(message.contains('-'), "{message}");
}

#[tokio::test]
async fn long_output_is_trimmed_in_the_middle() {
    let dir = tempdir().unwrap();
    let checks = Checks {
        unit: Some(helper("print_many_lines")),
        ..Checks::default()
    };
    let text = ok(
        &tools(dir.path(), checks),
        "run_checks",
        json!({"check": "unit"}),
    )
    .await;
    assert!(text.contains("output line 1\n"), "the start is kept");
    assert!(text.contains("output line 1000"), "the end is kept");
    assert!(!text.contains("output line 500\n"));
    assert!(text.contains("lines omitted"), "{text}");
    assert!(text.lines().count() < 220, "{}", text.lines().count());
}

#[tokio::test]
async fn checks_are_fixed_when_the_tools_are_made() {
    let dir = tempdir().unwrap();
    let checks = Checks {
        unit: Some("cargo --version".into()),
        ..Checks::default()
    };
    let tools = tools(dir.path(), checks);
    write(
        dir.path(),
        "arbor.toml",
        "[checks]\nunit = \"no-such-program-for-arbor\"\n",
    );
    ok(&tools, "run_checks", json!({"check": "unit"})).await;
}

#[tokio::test]
async fn the_planner_cannot_run_checks() {
    let dir = tempdir().unwrap();
    let checks = Checks {
        unit: Some("cargo --version".into()),
        ..Checks::default()
    };
    let tools = Tools::new(dir.path(), Toolset::Planner, checks);
    let output = call(&tools, "run_checks", json!({"check": "unit"})).await;
    assert!(output.is_error);
    assert!(output.text.contains("Planner"), "{}", output.text);
}
