//! `arbor init`: automatic `arbor.toml` init, with the Human Operator confirming a proposal on
//! stdin until the TUI's modal exists.

use std::env;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::hooks;
use arbor_config::{
    API_KEY_ENV, Config, Detection, Platform, detect_project_type_in, load_api_key,
    user_config_path,
};
use arbor_git::Repo;
use arbor_llm::{BoxFuture, LlmClient, OpenRouterClient};
use arbor_prompts::{Conventions, Prompts};
use arbor_tiers::{
    CheckRun, ConfirmChecks, Confirmation, ProposalError, ProposeChecks, ProposedChecks,
    ScaffolderProposer, init,
};

pub fn run() -> ExitCode {
    let root = match project_root() {
        Ok(root) => root,
        Err(err) => {
            eprintln!("arbor: could not read the current directory: {err}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("arbor: could not start: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Detection::Found(project_type) = detect_project_type_in(&root) {
        println!(
            "Detected a {} project; running `{}` and `{}` once to validate them.",
            project_type.name, project_type.scaffold, project_type.unit
        );
    }
    let mut proposer = OpenRouterScaffolder { root: root.clone() };
    let checks_written = match runtime.block_on(init(&root, &mut proposer, &mut StdinOperator)) {
        Ok(outcome) => {
            println!("{}", outcome.notice());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("arbor: {err}");
            ExitCode::FAILURE
        }
    };
    offer_hooks(&root);
    checks_written
}

/// Outside a git repository there is nothing to hook.
fn offer_hooks(root: &Path) {
    let Ok(repo) = Repo::open(root) else {
        return;
    };
    if !hooks::ask_yes(
        "
Install git hooks that refuse commits, merges and pushes holding Markers? [Y/n] ",
    ) {
        return;
    }
    match hooks::install(&repo) {
        Ok(installation) => print!("{}", hooks::installation_notice(&installation)),
        Err(err) => eprintln!("arbor: could not install the git hooks: {err}"),
    }
}

/// The repository's top level, or the current directory outside a repository.
fn project_root() -> io::Result<PathBuf> {
    let dir = env::current_dir()?;
    Ok(Repo::open(&dir).map_or(dir, |repo| repo.root().to_owned()))
}

/// Sets up the Scaffolder only once a proposal is needed, so detected Checks that pass never need
/// an API key.
struct OpenRouterScaffolder {
    root: PathBuf,
}

impl ProposeChecks for OpenRouterScaffolder {
    fn propose<'a>(
        &'a mut self,
        detection: &'a Detection,
        failed_runs: &'a [CheckRun],
    ) -> BoxFuture<'a, Result<ProposedChecks, ProposalError>> {
        Box::pin(propose(&self.root, detection, failed_runs))
    }
}

async fn propose(
    root: &Path,
    detection: &Detection,
    failed_runs: &[CheckRun],
) -> Result<ProposedChecks, ProposalError> {
    println!("{detection}");
    for failed_run in failed_runs {
        println!("`{}` failed:", failed_run.command);
        for output_line in failed_run.output.lines() {
            println!("    {output_line}");
        }
    }
    let unavailable = |err: &dyn std::fmt::Display| ProposalError::Unavailable(err.to_string());
    let user_path = user_config_path(Platform::current(), |name| env::var(name).ok());
    let config = Config::load(user_path.as_deref(), root).map_err(|err| unavailable(&err))?;
    let api_key = load_api_key(user_path.as_deref())
        .map_err(|err| unavailable(&err))?
        .ok_or_else(|| {
            ProposalError::Unavailable(format!(
                "the Scaffolder can't propose Checks without an OpenRouter API key; set \
                 {API_KEY_ENV} or api_key in the user config"
            ))
        })?;
    let client = OpenRouterClient::new(api_key.expose());
    let role = &config.roles.scaffolder;
    let model = client
        .model(&role.model)
        .await
        .map_err(|err| unavailable(&err))?;
    let prompts = Prompts::load(root).map_err(|err| unavailable(&err))?;
    let conventions =
        Conventions::load(root, &config.context_files).map_err(|err| unavailable(&err))?;

    println!(
        "Asking the Scaffolder ({}) to propose Checks from the repository.",
        role.model
    );
    let mut scaffolder = ScaffolderProposer {
        client: &client,
        prompts: &prompts,
        conventions: &conventions,
        model,
        temperature: role.temperature,
        worktree: root,
        usage: Default::default(),
    };
    let proposal = scaffolder.propose(detection, failed_runs).await;
    println!("The proposal cost ${:.4}.", scaffolder.usage.cost.dollars());
    proposal
}

struct StdinOperator;

impl ConfirmChecks for StdinOperator {
    fn confirm<'a>(&'a mut self, proposal: &'a ProposedChecks) -> BoxFuture<'a, Confirmation> {
        let confirmation = ask(proposal).unwrap_or(Confirmation::Skip);
        Box::pin(async move { confirmation })
    }
}

fn ask(proposal: &ProposedChecks) -> io::Result<Confirmation> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "\nThe Scaffolder proposes these Checks:")?;
    let commands = [
        ("scaffold", &proposal.scaffold),
        ("unit", &proposal.unit),
        ("unit_filtered", &proposal.unit_filtered),
    ];
    for (name, command) in commands {
        let command = command.as_deref().unwrap_or("(none)");
        writeln!(stdout, "  {name:<14} {command}")?;
    }
    writeln!(stdout, "\n{}\n", proposal.explanation.trim())?;
    write!(stdout, "Write them to arbor.toml? [y/N] ")?;
    stdout.flush()?;

    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    Ok(match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Confirmation::Accept,
        _ => Confirmation::Skip,
    })
}
