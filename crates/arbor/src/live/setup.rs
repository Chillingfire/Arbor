//! Settles the API key and each role's model and temperature before the first Session (grilling
//! log Round 3 decision 6): asks for a key when none is configured, and when the config has no
//! cached probe, asks the Operator to set the roles or take the recommended ones, then probes them.

use std::io;
use std::ops::ControlFlow;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;

use arbor_config::{
    ProbeCache, RoleConfig, Roles, probe_cache_path, save_api_key, save_changed_roles,
};
use arbor_llm::{LlmClient, OpenRouterClient, ProbeReport, probe_model};
use arbor_tui::{App, KeyStorage, SetupDialog, SetupOutcome, Tone, draw_setup};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, Event as TerminalEvent, KeyCode, KeyEventKind, KeyModifiers,
};

use super::{FRAME, Project};

/// `Break` means the Operator chose to leave.
pub(super) fn settle(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    project: &mut Project,
) -> io::Result<ControlFlow<()>> {
    if settle_api_key(terminal, app, project)?.is_break() {
        return Ok(ControlFlow::Break(()));
    }
    settle_models(terminal, app, project)
}

fn settle_api_key(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    project: &mut Project,
) -> io::Result<ControlFlow<()>> {
    // A config error was already reported at load; the Session start reports it again.
    if !matches!(project.api_key(), Ok(None)) {
        return Ok(ControlFlow::Continue(()));
    }
    let mut dialog = SetupDialog::api_key(None);
    loop {
        let SetupOutcome::ApiKey { api_key, storage } = next_outcome(terminal, app, &mut dialog)?
        else {
            return Ok(ControlFlow::Break(()));
        };
        dialog.set_checking(true);
        let key_to_check = api_key.clone();
        let checked = wait_for(terminal, app, &dialog, move || {
            block_on(async move { OpenRouterClient::new(key_to_check).check_key().await })
                .and_then(|key_check| key_check.map_err(|err| err.to_string()))
        })?;
        match checked {
            None => return Ok(ControlFlow::Break(())),
            Some(Err(problem)) => dialog = SetupDialog::api_key(Some(problem)),
            Some(Ok(())) => {
                if storage == KeyStorage::UserConfig {
                    save_key(app, project, &api_key);
                }
                project.typed_api_key = Some(api_key);
                return Ok(ControlFlow::Continue(()));
            }
        }
    }
}

fn save_key(app: &mut App, project: &Project, api_key: &str) {
    let Some(user_config) = &project.user_config else {
        app.view.push_note(
            "There is no user config folder, so the key is kept for this session only.",
            Tone::Failure,
        );
        return;
    };
    match save_api_key(user_config, api_key) {
        Ok(()) => app.view.push_note(
            format!("Saved the API key to {}.", user_config.display()),
            Tone::Operator,
        ),
        Err(err) => app.view.push_note(
            format!("Could not save the API key, so it is kept for this session only: {err}"),
            Tone::Failure,
        ),
    }
}

fn settle_models(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    project: &mut Project,
) -> io::Result<ControlFlow<()>> {
    let cache_path = project.user_config.as_deref().map(probe_cache_path);
    let mut cache = cache_path
        .as_deref()
        .map(ProbeCache::load)
        .unwrap_or_default();
    let config_problems = range_problems(&project.config.roles);
    if config_problems.is_empty() && cache.unprobed(&project.config.roles).is_empty() {
        return Ok(ControlFlow::Continue(()));
    }
    let Ok(Some(api_key)) = project.api_key() else {
        return Ok(ControlFlow::Continue(()));
    };
    let mut dialog = SetupDialog::models(project.config.roles.clone(), config_problems);
    loop {
        let SetupOutcome::Roles(roles) = next_outcome(terminal, app, &mut dialog)? else {
            return Ok(ControlFlow::Break(()));
        };
        let chosen_problems = range_problems(&roles);
        if !chosen_problems.is_empty() {
            dialog = SetupDialog::models(roles, chosen_problems);
            continue;
        }
        dialog.set_checking(true);
        let unprobed = cache.unprobed(&roles);
        let key_for_probes = api_key.clone();
        let probed = wait_for(terminal, app, &dialog, move || {
            let client = OpenRouterClient::new(key_for_probes);
            block_on(async move { probe_all(&client, unprobed).await })
        })?;
        let probe_round = match probed {
            None => return Ok(ControlFlow::Break(())),
            Some(Err(problem)) => ProbeRound {
                passed: Vec::new(),
                failures: vec![problem],
            },
            Some(Ok(probe_round)) => probe_round,
        };
        for (role, _) in &probe_round.passed {
            cache.record(role);
        }
        if let Some(cache_path) = &cache_path
            && let Err(err) = cache.save(cache_path)
        {
            app.view.push_note(
                format!(
                    "Could not save the probe cache, so the models are probed again next run: {err}"
                ),
                Tone::Failure,
            );
        }
        if !probe_round.failures.is_empty() {
            dialog = SetupDialog::models(roles, probe_round.failures);
            continue;
        }
        for note in probe_notes(&roles, &probe_round.passed) {
            app.view.push_note(note, Tone::Progress);
        }
        use_roles(app, project, roles);
        return Ok(ControlFlow::Continue(()));
    }
}

/// Puts the chosen roles in force for this run and saves them when they changed.
fn use_roles(app: &mut App, project: &mut Project, roles: Roles) {
    if roles != project.config.roles
        && let Some(user_config) = &project.user_config
        && let Err(err) = save_changed_roles(user_config, &roles, &project.config.roles)
    {
        app.view.push_note(
            format!("Could not save the models to the user config, so they hold for this run only: {err}"),
            Tone::Failure,
        );
    }
    app.status.models = roles.named().map(|(_, role)| role.model.clone());
    project.config.roles = roles;
}

fn range_problems(roles: &Roles) -> Vec<String> {
    roles
        .temperature_problems()
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// The probes of one choice of roles: those that passed, and why the others failed.
#[derive(Debug, Default, PartialEq)]
struct ProbeRound {
    passed: Vec<(RoleConfig, ProbeReport)>,
    failures: Vec<String>,
}

async fn probe_all(client: &dyn LlmClient, unprobed: Vec<RoleConfig>) -> ProbeRound {
    let mut probe_round = ProbeRound::default();
    for role in unprobed {
        match probe_model(client, &role.model, role.temperature).await {
            Ok(report) => probe_round.passed.push((role, report)),
            Err(failure) => probe_round.failures.push(failure.to_string()),
        }
    }
    probe_round
}

/// The event log lines after the probes pass: the parameters a model doesn't list, and that a
/// passed probe can't show a silent clamp.
fn probe_notes(roles: &Roles, passed: &[(RoleConfig, ProbeReport)]) -> Vec<String> {
    let mut notes = Vec::new();
    for (role_name, role) in roles.named() {
        let Some((_, report)) = passed.iter().find(|(probed, _)| probed == role) else {
            continue;
        };
        if !report.supports_temperature {
            notes.push(format!(
                "The {role_name} model {} doesn't list temperature as supported, so providers may \
                 ignore its temperature {}.",
                role.model, role.temperature
            ));
        }
        if role_name == "Writer" && !report.supports_seed {
            notes.push(format!(
                "The Writer model {} doesn't list seed as supported, so Writer replies are not \
                 reproducible.",
                role.model
            ));
        }
    }
    let cost_usd: f64 = passed.iter().map(|(_, report)| report.cost_usd).sum();
    notes.push(format!(
        "Probed {} model{} for ${cost_usd:.4}. A provider may still clamp a temperature silently.",
        passed.len(),
        if passed.len() == 1 { "" } else { "s" }
    ));
    notes
}

fn block_on<T>(future: impl Future<Output = T>) -> Result<T, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("Could not start the async runtime: {err}"))?;
    Ok(runtime.block_on(future))
}

fn draw(terminal: &mut DefaultTerminal, app: &App, dialog: &SetupDialog) -> io::Result<()> {
    terminal.draw(|frame| {
        app.draw(frame);
        draw_setup(frame, dialog);
    })?;
    Ok(())
}

fn is_ctrl_c(terminal_event: &TerminalEvent) -> bool {
    matches!(
        terminal_event,
        TerminalEvent::Key(key)
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
    )
}

/// Shows the dialog until the Operator settles it. Ctrl+C quits.
fn next_outcome(
    terminal: &mut DefaultTerminal,
    app: &App,
    dialog: &mut SetupDialog,
) -> io::Result<SetupOutcome> {
    loop {
        draw(terminal, app, dialog)?;
        if !event::poll(FRAME)? {
            continue;
        }
        let terminal_event = event::read()?;
        if is_ctrl_c(&terminal_event) {
            return Ok(SetupOutcome::Quit);
        }
        match terminal_event {
            TerminalEvent::Paste(pasted) => dialog.paste(&pasted),
            TerminalEvent::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some(outcome) = dialog.handle_key(key) {
                    return Ok(outcome);
                }
            }
            _ => {}
        }
    }
}

/// Runs `work` on its own thread while the dialog keeps drawing. `None` means the Operator pressed
/// Ctrl+C to leave instead of waiting.
fn wait_for<T: Send + 'static>(
    terminal: &mut DefaultTerminal,
    app: &App,
    dialog: &SetupDialog,
    work: impl FnOnce() -> T + Send + 'static,
) -> io::Result<Option<T>> {
    let (sender, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(work());
    });
    loop {
        draw(terminal, app, dialog)?;
        match finished.try_recv() {
            Ok(work_output) => return Ok(Some(work_output)),
            Err(TryRecvError::Disconnected) => {
                return Err(io::Error::other("the startup check stopped unexpectedly"));
            }
            Err(TryRecvError::Empty) => {}
        }
        if event::poll(FRAME)? && is_ctrl_c(&event::read()?) {
            return Ok(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use arbor_llm::{
        ChatResponse, Exchange, FinishReason, Fixture, LlmError, ModelInfo, Pricing,
        ScriptedClient, Usage, probe_request,
    };

    use super::*;

    fn model_info(id: &str, supported_parameters: &[&str]) -> ModelInfo {
        ModelInfo {
            id: id.to_owned(),
            context_length: 100_000,
            supported_parameters: supported_parameters
                .iter()
                .map(|parameter| (*parameter).to_owned())
                .collect(),
            pricing: Pricing {
                prompt: 0.0,
                completion: 0.0,
                cache_read: None,
            },
        }
    }

    fn probe_exchange(role: &RoleConfig, response: Result<ChatResponse, LlmError>) -> Exchange {
        Exchange {
            request: probe_request(&role.model, role.temperature),
            response,
        }
    }

    fn probe_reply(model: &str) -> ChatResponse {
        ChatResponse {
            id: "gen-probe".to_owned(),
            model: model.to_owned(),
            content: Some("OK".to_owned()),
            tool_calls: vec![],
            finish_reason: FinishReason::Length,
            usage: Usage {
                cost_usd: 0.0001,
                ..Usage::default()
            },
        }
    }

    fn report(supports_temperature: bool, supports_seed: bool) -> ProbeReport {
        ProbeReport {
            supports_temperature,
            supports_seed,
            cost_usd: 0.0001,
        }
    }

    #[test]
    fn probing_keeps_the_passes_and_explains_each_failure() {
        let roles = Roles::recommended();
        let refusal = LlmError::BadRequest {
            status: 400,
            message: "temperature out of range".to_owned(),
        };
        let client = ScriptedClient::new(Fixture {
            exchanges: vec![
                probe_exchange(&roles.planner, Ok(probe_reply(&roles.planner.model))),
                probe_exchange(&roles.writer, Err(refusal)),
            ],
            models: vec![
                model_info(&roles.planner.model, &["temperature"]),
                model_info(&roles.writer.model, &["temperature", "seed"]),
            ],
        });

        let unprobed = vec![roles.planner.clone(), roles.writer.clone()];
        let probe_round = block_on(probe_all(&client, unprobed)).unwrap();

        assert_eq!(
            probe_round.passed,
            vec![(roles.planner.clone(), report(true, false))]
        );
        assert_eq!(probe_round.failures.len(), 1);
        assert!(probe_round.failures[0].contains(&roles.writer.model));
        assert!(probe_round.failures[0].contains("temperature out of range"));
        assert_eq!(client.remaining(), 0);
    }

    #[test]
    fn the_notes_warn_about_unlisted_parameters_and_silent_clamping() {
        let roles = Roles::recommended();
        let passed = vec![
            (roles.planner.clone(), report(false, false)),
            (roles.scaffolder.clone(), report(true, false)),
            (roles.writer.clone(), report(true, false)),
        ];
        let notes = probe_notes(&roles, &passed);
        assert_eq!(notes.len(), 3);
        assert!(notes[0].starts_with("The Planner model"));
        assert!(notes[0].contains("temperature"));
        assert!(notes[1].starts_with("The Writer model"));
        assert!(notes[1].contains("seed"));
        assert_eq!(
            notes[2],
            "Probed 3 models for $0.0003. A provider may still clamp a temperature silently."
        );
    }

    #[test]
    fn roles_outside_the_temperature_range_are_reported_before_any_probe() {
        let mut roles = Roles::recommended();
        roles.writer.temperature = 1.5;
        let problems = range_problems(&roles);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("the Writer temperature 1.5"));
    }
}
