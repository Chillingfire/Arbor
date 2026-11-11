//! `cargo xtask eval`: runs fixed Requests against small sample repos with real models, through
//! the same Autopilot the end-to-end tests use, and reports pass rate, escalations and cost per
//! Tier. Results are written as JSON for comparison across runs.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use arbor_config::{API_KEY_ENV, ClarificationMode, Config, Platform, resolve_api_key};
use arbor_core::{Event, Request, Session, Tier};
use arbor_git::{Repo, SessionBranch};
use arbor_llm::OpenRouterClient;
use arbor_session::{Autopilot, SessionEnd};
use serde::Serialize;

const WORDY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../crates/arbor/tests/fixtures/sample-repo"
);

struct EvalCase {
    name: &'static str,
    sample_repo: &'static str,
    request: &'static str,
}

const CASES: &[EvalCase] = &[
    EvalCase {
        name: "wordy-longest-word",
        sample_repo: WORDY,
        request: "Add a `longest_word` function to wordy that returns the longest word in a \
                  text, or `None` when the text has no words. When several words are equally \
                  long, return the first.",
    },
    EvalCase {
        name: "wordy-capitalize-words",
        sample_repo: WORDY,
        request: "Add a `capitalize_words` function to wordy that returns the text with the \
                  first letter of each word upper-cased, keeping the whitespace between words \
                  exactly as it was.",
    },
    EvalCase {
        name: "wordy-unique-words",
        sample_repo: WORDY,
        request: "Add a `unique_words` function to wordy that returns the distinct words of a \
                  text in the order they first appear, comparing words case-insensitively and \
                  keeping each word's first spelling.",
    },
];

#[derive(Serialize)]
struct EvalReport {
    started_unix_secs: u64,
    models: TierModels,
    cases_run: usize,
    cases_passed: usize,
    pass_rate: f64,
    escalations: Escalations,
    cost: TierCosts,
    cases: Vec<CaseReport>,
}

#[derive(Serialize)]
struct TierModels {
    planner: String,
    scaffolder: String,
    writer: String,
}

#[derive(Serialize)]
struct CaseReport {
    name: String,
    /// The Session finished and the case's held-out acceptance tests passed.
    passed: bool,
    /// How the Session ended: `finished`, or why it stopped.
    end: String,
    /// The held-out acceptance tests: `passed`, `failed: <output>`, or why they didn't run.
    acceptance: String,
    escalations: Escalations,
    cost: TierCosts,
}

#[derive(Serialize, Default, Clone, Copy)]
struct Escalations {
    /// Writer attempts after the first in each Writer run. A run that ends in a failed Work Unit
    /// or a Missing Response counts only its attempts after the first.
    writer_retries: usize,
    /// Work Units whose Writer gave up, handing them on to a Revision.
    work_units_failed: usize,
    task_checks_failed: usize,
    revisions: usize,
    revisions_rejected: usize,
    replans: usize,
    /// Sessions that stopped for the Human Operator.
    operator_stops: usize,
}

impl Escalations {
    fn add(&mut self, other: Escalations) {
        self.writer_retries += other.writer_retries;
        self.work_units_failed += other.work_units_failed;
        self.task_checks_failed += other.task_checks_failed;
        self.revisions += other.revisions;
        self.revisions_rejected += other.revisions_rejected;
        self.replans += other.replans;
        self.operator_stops += other.operator_stops;
    }
}

#[derive(Serialize, Default, Clone, Copy)]
struct TierCosts {
    planner: TierCost,
    scaffolder: TierCost,
    writer: TierCost,
    total_usd: f64,
}

#[derive(Serialize, Default, Clone, Copy)]
struct TierCost {
    usd: f64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
}

impl TierCosts {
    fn of(session: &Session) -> Self {
        let tier_cost = |tier| {
            let usage = session.usage().tier(tier);
            TierCost {
                usd: usage.cost.dollars(),
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_tokens,
            }
        };
        Self {
            planner: tier_cost(Tier::Planner),
            scaffolder: tier_cost(Tier::Scaffolder),
            writer: tier_cost(Tier::Writer),
            total_usd: session.usage().total().cost.dollars(),
        }
    }

    fn add(&mut self, other: TierCosts) {
        for (sum, part) in [
            (&mut self.planner, other.planner),
            (&mut self.scaffolder, other.scaffolder),
            (&mut self.writer, other.writer),
        ] {
            sum.usd += part.usd;
            sum.input_tokens += part.input_tokens;
            sum.output_tokens += part.output_tokens;
            sum.cache_read_tokens += part.cache_read_tokens;
        }
        self.total_usd += other.total_usd;
    }
}

struct EvalOptions {
    case_filter: Option<String>,
    out: Option<PathBuf>,
}

fn parse_options(eval_args: &[String]) -> Result<EvalOptions, String> {
    let mut options = EvalOptions {
        case_filter: None,
        out: None,
    };
    let mut remaining_args = eval_args.iter();
    while let Some(option) = remaining_args.next() {
        let mut option_value = || {
            remaining_args
                .next()
                .cloned()
                .ok_or_else(|| format!("{option} needs a value"))
        };
        match option.as_str() {
            "--case" => options.case_filter = Some(option_value()?),
            "--out" => options.out = Some(PathBuf::from(option_value()?)),
            unknown => return Err(format!("unknown argument `{unknown}`")),
        }
    }
    Ok(options)
}

pub fn eval(eval_args: &[String]) -> ExitCode {
    let options = match parse_options(eval_args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\nusage: cargo xtask eval [--case <name>] [--out <file.json>]");
            return ExitCode::FAILURE;
        }
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a Tokio runtime starts");
    match runtime.block_on(run_cases(&options)) {
        Ok(report_path) => {
            println!("Results written to {}", report_path.display());
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("eval failed: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run_cases(options: &EvalOptions) -> Result<PathBuf, String> {
    let cases: Vec<&EvalCase> = CASES
        .iter()
        .filter(|case| {
            options
                .case_filter
                .as_deref()
                .is_none_or(|name| name == case.name)
        })
        .collect();
    if cases.is_empty() {
        let names: Vec<&str> = CASES.iter().map(|case| case.name).collect();
        return Err(format!(
            "no case matches; the cases are {}",
            names.join(", ")
        ));
    }
    let user_config =
        arbor_config::user_config_path(Platform::current(), |name| env::var(name).ok());
    let user_text = user_config
        .as_deref()
        .and_then(|path| fs::read_to_string(path).ok());
    let api_key = resolve_api_key(env::var(API_KEY_ENV).ok().as_deref(), user_text.as_deref())
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("set {API_KEY_ENV} or api_key in the user config"))?;
    let client = OpenRouterClient::new(api_key.expose());

    let started_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs());
    let mut case_reports = Vec::new();
    let mut models = None;
    for case in cases {
        println!("== {}", case.name);
        let (case_report, config) = run_case(case, user_config.as_deref(), &client).await?;
        println!(
            "   {} ({}; acceptance tests {}), ${:.4}",
            if case_report.passed { "PASS" } else { "FAIL" },
            case_report.end,
            case_report.acceptance,
            case_report.cost.total_usd
        );
        models.get_or_insert(TierModels {
            planner: config.roles.planner.model,
            scaffolder: config.roles.scaffolder.model,
            writer: config.roles.writer.model,
        });
        case_reports.push(case_report);
    }

    let mut escalations = Escalations::default();
    let mut cost = TierCosts::default();
    for case_report in &case_reports {
        escalations.add(case_report.escalations);
        cost.add(case_report.cost);
    }
    let cases_passed = case_reports.iter().filter(|case| case.passed).count();
    let report = EvalReport {
        started_unix_secs,
        models: models.expect("at least one case ran"),
        cases_run: case_reports.len(),
        cases_passed,
        pass_rate: cases_passed as f64 / case_reports.len() as f64,
        escalations,
        cost,
        cases: case_reports,
    };
    print_summary(&report);

    let report_path = options.out.clone().unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/eval")
            .join(format!("eval-{started_unix_secs}.json"))
    });
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
    }
    let report_json = serde_json::to_string_pretty(&report).expect("the report serializes");
    fs::write(&report_path, report_json + "\n")
        .map_err(|err| format!("{}: {err}", report_path.display()))?;
    Ok(report_path)
}

fn print_summary(report: &EvalReport) {
    println!();
    println!(
        "Passed {}/{} ({:.0}%)",
        report.cases_passed,
        report.cases_run,
        report.pass_rate * 100.0
    );
    let escalations = report.escalations;
    println!(
        "Escalations: {} Writer retries, {} failed Work Units, {} failed Task Checks, {} \
         Revisions ({} rejected), {} Replans, {} operator stops",
        escalations.writer_retries,
        escalations.work_units_failed,
        escalations.task_checks_failed,
        escalations.revisions,
        escalations.revisions_rejected,
        escalations.replans,
        escalations.operator_stops
    );
    for (tier_name, tier_cost) in [
        ("Planner", report.cost.planner),
        ("Scaffolder", report.cost.scaffolder),
        ("Writer", report.cost.writer),
    ] {
        println!(
            "{tier_name:<10} ${:.4}  {} in / {} out tokens ({} cache hits)",
            tier_cost.usd,
            tier_cost.input_tokens,
            tier_cost.output_tokens,
            tier_cost.cache_read_tokens
        );
    }
    println!("Total      ${:.4}", report.cost.total_usd);
}

/// Runs one case in a fresh copy of its sample repo. An error setting the case up ends the eval;
/// anything that goes wrong in the Session itself is reported as a failed case.
async fn run_case(
    case: &EvalCase,
    user_config: Option<&Path>,
    client: &OpenRouterClient,
) -> Result<(CaseReport, Config), String> {
    let place = tempfile::tempdir().map_err(|err| err.to_string())?;
    let checkout = place.path().join("project");
    copy_dir(Path::new(case.sample_repo), &checkout)?;
    for git_args in [
        &["init", "--quiet", "-b", "main"][..],
        &["config", "user.name", "Arbor Eval"],
        &["config", "user.email", "eval@arbor.invalid"],
        &["config", "commit.gpgsign", "false"],
        &["config", "core.autocrlf", "false"],
        &["add", "--all"],
        &["commit", "--quiet", "-m", "Start"],
    ] {
        git(&checkout, git_args)?;
    }
    let repo = Repo::open(&checkout).map_err(|err| err.to_string())?;
    let mainline = repo.mainline(None).map_err(|err| err.to_string())?;
    let branch = SessionBranch::for_feature(case.name).map_err(|err| err.to_string())?;
    let worktree = repo
        .create_worktree(&mainline, &branch, &place.path().join("session"))
        .map_err(|err| err.to_string())?;
    let mut config = Config::load(user_config, worktree.path()).map_err(|err| err.to_string())?;
    // Nobody is there to answer clarifying questions.
    config.clarification_mode = ClarificationMode::Assume;

    let mut autopilot = Autopilot::new(&worktree, &mainline, client, config.clone())
        .map_err(|err| err.to_string())?;
    let request = Request {
        text: case.request.to_owned(),
        issue: None,
    };
    let session_end = autopilot.run(request).await;
    let mut escalations = count_escalations(autopilot.events());
    escalations.writer_retries = autopilot
        .writer_runs()
        .iter()
        .map(|run| run.attempts.len().saturating_sub(1))
        .sum();
    let mut acceptance = "not run: the Session didn't finish".to_owned();
    let (passed, end) = match session_end {
        Ok(SessionEnd::Finished) => {
            let acceptance_result = run_acceptance_tests(case, worktree.path());
            let passed = acceptance_result.is_ok();
            acceptance = match acceptance_result {
                Ok(()) => "passed".to_owned(),
                Err(failure) => format!("failed: {failure}"),
            };
            (passed, "finished".to_owned())
        }
        Ok(SessionEnd::NeedsOperator(effect)) => {
            escalations.operator_stops = 1;
            (false, format!("stopped for the operator at {effect:?}"))
        }
        Ok(SessionEnd::TierStopped { tier, reason }) => {
            escalations.operator_stops = 1;
            (false, format!("the {tier:?} stopped: {reason}"))
        }
        Ok(SessionEnd::ScaffoldChecksFailed { check_output }) => {
            (false, format!("the Scaffold Check failed: {check_output}"))
        }
        Err(err) => (false, format!("error: {err}")),
    };
    let case_report = CaseReport {
        name: case.name.to_owned(),
        passed,
        end,
        acceptance,
        escalations,
        cost: TierCosts::of(autopilot.session()),
    };
    Ok((case_report, config))
}

/// The tests that run only after the Session, so a pass needs more than the tests the
/// Scaffolder wrote (ADR-0003).
fn acceptance_tests_path(case: &EvalCase) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("eval-acceptance")
        .join(format!("{}.rs", case.name))
}

const ACCEPTANCE_TEST_TARGET: &str = "arbor_eval_acceptance";

/// Copies the case's held-out tests into the worktree and runs them. `Err` holds the end of the
/// test output.
fn run_acceptance_tests(case: &EvalCase, worktree: &Path) -> Result<(), String> {
    let tests_dir = worktree.join("tests");
    fs::create_dir_all(&tests_dir).map_err(|err| format!("{}: {err}", tests_dir.display()))?;
    let source = acceptance_tests_path(case);
    let target = tests_dir.join(format!("{ACCEPTANCE_TEST_TARGET}.rs"));
    fs::copy(&source, &target).map_err(|err| format!("{}: {err}", source.display()))?;
    let cargo_test = Command::new("cargo")
        .args(["test", "--test", ACCEPTANCE_TEST_TARGET])
        .current_dir(worktree)
        .output()
        .map_err(|err| format!("cargo: {err}"))?;
    if cargo_test.status.success() {
        return Ok(());
    }
    let test_output = String::from_utf8_lossy(&cargo_test.stdout).into_owned()
        + &String::from_utf8_lossy(&cargo_test.stderr);
    let output_lines: Vec<&str> = test_output.lines().collect();
    let kept_from = output_lines.len().saturating_sub(ACCEPTANCE_OUTPUT_LINES);
    Err(output_lines[kept_from..].join("\n"))
}

const ACCEPTANCE_OUTPUT_LINES: usize = 40;

fn count_escalations(events: &[Event]) -> Escalations {
    let count =
        |is_counted: fn(&Event) -> bool| events.iter().filter(|event| is_counted(event)).count();
    Escalations {
        work_units_failed: count(|event| matches!(event, Event::WorkUnitFailed { .. })),
        task_checks_failed: count(|event| matches!(event, Event::TaskChecksFailed { .. })),
        revisions: count(|event| matches!(event, Event::RevisionSubmitted { .. })),
        revisions_rejected: count(|event| matches!(event, Event::RevisionRejected { .. })),
        replans: count(|event| matches!(event, Event::ReplanRequested)),
        ..Escalations::default()
    }
}

/// Copies the sample repo with `\n` line endings, whatever the checkout of Arbor itself used.
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|err| format!("{}: {err}", to.display()))?;
    let entries = fs::read_dir(from).map_err(|err| format!("{}: {err}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            if entry.file_name() != "target" {
                copy_dir(&source, &target)?;
            }
        } else {
            let file_text = fs::read_to_string(&source)
                .map_err(|err| format!("{}: {err}", source.display()))?;
            fs::write(&target, file_text.replace("\r\n", "\n"))
                .map_err(|err| format!("{}: {err}", target.display()))?;
        }
    }
    Ok(())
}

fn git(dir: &Path, git_args: &[&str]) -> Result<(), String> {
    let git_output = Command::new("git")
        .args(git_args)
        .current_dir(dir)
        .output()
        .map_err(|err| format!("git: {err}"))?;
    if git_output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git {} failed: {}",
            git_args.join(" "),
            String::from_utf8_lossy(&git_output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_has_held_out_acceptance_tests() {
        for case in CASES {
            let path = acceptance_tests_path(case);
            assert!(path.is_file(), "{} has no {}", case.name, path.display());
        }
    }
}
