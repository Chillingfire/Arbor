//! Runs the TUI and, once there is a Request, a live Session behind it. The Session runs on its own
//! thread with the Autopilot and streams its updates to the screen. At a Checkpoint or a Tier's
//! questions it waits for the Human Operator's reply from a modal.

mod setup;

use std::env;
use std::fs;
use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use arbor_config::{
    API_KEY_ENV, ClarificationMode, Config, ConfigError, Platform, load_api_key, user_config_path,
};
use arbor_core::{Effect, Event, Phase, Plan, Request, RunOutcome, Tier};
use arbor_git::{Mainline, Repo, SESSIONS_DIR, SessionBranch, Worktree};
use arbor_llm::OpenRouterClient;
use arbor_session::{
    Autopilot, AutopilotUpdate, Checkpoint, Decision, Operator, Pending, RunLock, SessionEnd,
    SessionFolder,
};
use arbor_tui::{App, Confirmation, Focus, KeyOutcome, Modal, StatusInfo, Tone, UnfinishedSession};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event as TerminalEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use tokio::sync::oneshot;

mod browsing;

use crate::leaving::{self, StartedSession};
use crate::plan_editor::{PlanEdit, edit_plan};
use crate::startup;

/// How often the screen redraws while waiting, so the spinner moves.
const FRAME: Duration = Duration::from_millis(100);

/// What the Session thread runs: a new Session for a Request, or a stored one carried on.
enum SessionStart {
    New(Request),
    Resume(Resumed),
}

/// A stored Session claimed for a resumed run, with its event log as replayed on the screen.
struct Resumed {
    folder: SessionFolder,
    lock: RunLock,
    events: Vec<Event>,
}

/// What the Session thread sends to the screen.
enum DriverMessage {
    Started {
        branch: String,
        session_dir: PathBuf,
    },
    Update(AutopilotUpdate),
    Checkpoint {
        checkpoint: Checkpoint,
        reply: oneshot::Sender<Decision>,
    },
    Questions {
        tier: Tier,
        questions: String,
        reply: oneshot::Sender<Option<String>>,
    },
    Ended(String, Tone),
}

/// Where the Human Operator's reply to the open modal goes.
enum AwaitedReply {
    Decision(oneshot::Sender<Decision>),
    Answers(oneshot::Sender<Option<String>>),
}

/// The Human Operator at the screen: each Checkpoint and question opens a modal there, and the
/// Session waits for the reply.
struct ScreenOperator {
    sender: Sender<DriverMessage>,
    mode_switches: Receiver<ClarificationMode>,
}

/// Without a screen left to reply, the Session waits until it is cancelled rather than go on
/// undecided.
async fn screen_reply<T>(replied: oneshot::Receiver<T>) -> T {
    match replied.await {
        Ok(reply) => reply,
        Err(_) => std::future::pending().await,
    }
}

impl Operator for ScreenOperator {
    fn decide<'a>(&'a mut self, checkpoint: &'a Checkpoint) -> Pending<'a, Decision> {
        let (reply, replied) = oneshot::channel();
        let _ = self.sender.send(DriverMessage::Checkpoint {
            checkpoint: checkpoint.clone(),
            reply,
        });
        Box::pin(screen_reply(replied))
    }

    fn answer<'a>(&'a mut self, tier: Tier, questions: &'a str) -> Pending<'a, Option<String>> {
        let (reply, replied) = oneshot::channel();
        let _ = self.sender.send(DriverMessage::Questions {
            tier,
            questions: questions.to_owned(),
            reply,
        });
        Box::pin(screen_reply(replied))
    }

    fn clarification_mode_change(&mut self) -> Option<ClarificationMode> {
        self.mode_switches.try_iter().last()
    }
}

/// The Session running behind the screen.
struct RunningSession {
    messages: Receiver<DriverMessage>,
    cancel: Option<oneshot::Sender<()>>,
    driver: JoinHandle<()>,
    awaited_reply: Option<AwaitedReply>,
    mode_switches: Sender<ClarificationMode>,
    /// Set once the Session's branch and worktree exist.
    started: Option<StartedSession>,
}

struct Project {
    root: PathBuf,
    config: Config,
    user_config: Option<PathBuf>,
    /// A key the Operator typed at startup, used ahead of the configured one.
    typed_api_key: Option<String>,
}

impl Project {
    /// Read again for each Session, so a key set after a failed start is picked up.
    fn api_key(&self) -> Result<Option<String>, ConfigError> {
        if let Some(typed_api_key) = &self.typed_api_key {
            return Ok(Some(typed_api_key.clone()));
        }
        let api_key = load_api_key(self.user_config.as_deref(), |name| env::var(name).ok())?;
        Ok(api_key.map(|key| key.expose().to_owned()))
    }

    fn repo(&self) -> Result<Repo, String> {
        Repo::open(&self.root).map_err(|err| err.to_string())
    }
}

pub fn run(initial_request: Option<Request>) -> ExitCode {
    let mut project = match load_project() {
        Ok(project) => project,
        Err(message) => {
            eprintln!("arbor: {message}");
            return ExitCode::FAILURE;
        }
    };
    startup::check(&project.root, &project.config);
    let roles = &project.config.roles;
    let mut app = App::new(StatusInfo {
        models: [
            roles.planner.model.clone(),
            roles.scaffolder.model.clone(),
            roles.writer.model.clone(),
        ],
        clarification_mode: project.config.clarification_mode,
        branch: None,
        activity: None,
    });
    browsing::prune_at_startup(&project, &mut app);
    let outcome = ratatui::run(|terminal| {
        // The legacy Windows console can't do bracketed paste; there a paste arrives as keys.
        let _ = execute!(io::stdout(), EnableBracketedPaste);
        let looped = match setup::settle(terminal, &mut app, &mut project) {
            Ok(ControlFlow::Continue(())) => {
                event_loop(terminal, &mut app, &project, initial_request)
            }
            settled => settled.map(|_| Vec::new()),
        };
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        looped
    });
    match outcome {
        Ok(started_sessions) => {
            leaving::warn_on_stderr(&started_sessions);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("arbor: the terminal failed: {err}");
            ExitCode::FAILURE
        }
    }
}

fn load_project() -> Result<Project, String> {
    let root =
        env::current_dir().map_err(|err| format!("could not read the current directory: {err}"))?;
    let repo =
        Repo::open(&root).map_err(|err| format!("{err}; run arbor inside a git repository"))?;
    let root = repo.root().to_owned();
    let user_config = user_config_path(Platform::current(), |name| env::var(name).ok());
    let config = Config::load(user_config.as_deref(), &root).map_err(|err| err.to_string())?;
    let project = Project {
        root,
        config,
        user_config,
        typed_api_key: None,
    };
    project.api_key().map_err(|err| err.to_string())?;
    Ok(project)
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    project: &Project,
    initial_request: Option<Request>,
) -> io::Result<Vec<StartedSession>> {
    let mut run = Run::default();
    match initial_request {
        Some(request) => run.start(app, project, request),
        None => app.ui_state.focus = Focus::Prompt,
    }
    loop {
        run.follow(app);
        terminal.draw(|frame| app.draw(frame))?;
        // Take every event already waiting before drawing again, so a paste doesn't trail behind.
        let mut event_waiting = event::poll(FRAME)?;
        while event_waiting {
            let terminal_event = event::read()?;
            event_waiting = event::poll(Duration::ZERO)?;
            let key = match terminal_event {
                TerminalEvent::Paste(pasted) => {
                    app.ui_state.paste(&pasted);
                    continue;
                }
                TerminalEvent::Key(key) if key.kind == KeyEventKind::Press => key,
                _ => continue,
            };
            let key = if event_waiting {
                as_pasted(key, app.ui_state.focus)
            } else {
                key
            };
            let key_flow = run.handle_key(app, project, key);
            if let Some(plan) = run.plan_to_edit.take() {
                run.send_edited_plan(app, edit_plan(terminal, &plan)?);
            }
            if key_flow.is_break() {
                app.view
                    .push_note("Stopping the Session before leaving…", Tone::Operator);
                terminal.draw(|frame| app.draw(frame))?;
                run.stop();
                return Ok(run.started_sessions);
            }
        }
    }
}

/// Where bracketed paste isn't available, a paste arrives as keys all at once. An Enter with more
/// keys right behind it is a pasted newline, not the Human Operator sending the prompt.
fn as_pasted(key: KeyEvent, focus: Focus) -> KeyEvent {
    if focus == Focus::Prompt && key.code == KeyCode::Enter && key.modifiers.is_empty() {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)
    } else {
        key
    }
}

/// The Session behind the screen, once the Human Operator has sent a Request.
#[derive(Default)]
struct Run {
    session: Option<RunningSession>,
    /// The Plan the Human Operator asked to edit at its Checkpoint, until the editor opens.
    plan_to_edit: Option<Plan>,
    /// The Sessions that have ended, to warn about any left unfinished.
    started_sessions: Vec<StartedSession>,
}

impl Run {
    fn start(&mut self, app: &mut App, project: &Project, request: Request) {
        if app.view.session().phase() != &Phase::NotStarted {
            app.start_over();
        }
        self.session = start_session(app, project, SessionStart::New(request));
    }

    /// Carries on a stored Session, already replayed on the screen.
    fn resume(&mut self, app: &mut App, project: &Project, resumed: Resumed) {
        self.session = start_session(app, project, SessionStart::Resume(resumed));
        if self.session.is_some() {
            app.ui_state.focus = Focus::Tree;
        }
    }

    /// Applies what the Session thread sent, and forgets the Session once it has ended.
    fn follow(&mut self, app: &mut App) {
        if let Some(session) = &mut self.session
            && !drain_messages(app, session)
        {
            let started = self.session.take().and_then(|session| session.started);
            app.stop_working();
            app.ui_state.confirming = None;
            app.ui_state.modal = None;
            if let Some(unfinished) = started.as_ref().and_then(|ended| ended.unfinished(false)) {
                app.view
                    .push_note(unfinished.warning().join(" "), Tone::Operator);
            }
            self.started_sessions.extend(started);
        }
    }

    /// What leaving now would leave unfinished: the running Session, or the last one that ended.
    fn leaving_warning(&self) -> Option<UnfinishedSession> {
        match &self.session {
            Some(session) => session.started.as_ref()?.unfinished(true),
            None => self
                .started_sessions
                .iter()
                .rev()
                .find_map(|ended| ended.unfinished(false)),
        }
    }

    fn cancel(&mut self) {
        if let Some(session) = &mut self.session
            && let Some(cancel) = session.cancel.take()
        {
            let _ = cancel.send(());
        }
    }

    /// Cancels the Session and waits for its thread, so a running Check is killed and the
    /// worktree is put back before the process exits.
    fn stop(&mut self) {
        self.cancel();
        let Some(RunningSession {
            messages,
            driver,
            mut started,
            ..
        }) = self.session.take()
        else {
            return;
        };
        let _ = driver.join();
        for message in messages.try_iter() {
            match message {
                DriverMessage::Started {
                    branch,
                    session_dir,
                } => started = Some(started_session(branch, &session_dir)),
                DriverMessage::Ended(_, tone) => mark_ended(&mut started, tone),
                _ => {}
            }
        }
        self.started_sessions.extend(started);
    }

    fn handle_key(&mut self, app: &mut App, project: &Project, key: KeyEvent) -> ControlFlow<()> {
        match app.ui_state.handle_key(key, &app.view) {
            KeyOutcome::Handled => {}
            KeyOutcome::Quit => {
                let leaving = self.leaving_warning();
                if self.session.is_none() && leaving.is_none() {
                    return ControlFlow::Break(());
                }
                app.ui_state.leaving = leaving;
                app.ui_state.confirming = Some(Confirmation::Quit);
            }
            KeyOutcome::Cancel if self.session.is_some() => {
                app.ui_state.confirming = Some(Confirmation::Cancel);
            }
            KeyOutcome::Cancel => {}
            KeyOutcome::Confirmed(Confirmation::Quit) => return ControlFlow::Break(()),
            KeyOutcome::Confirmed(Confirmation::Cancel) => self.cancel(),
            KeyOutcome::Submitted(text) if self.session.is_none() => {
                self.start(app, project, Request { text, issue: None });
                if self.session.is_some() {
                    app.ui_state.focus = Focus::Tree;
                }
            }
            KeyOutcome::Submitted(_) => app.view.push_note(
                "A Session is running; its Checkpoints and questions open in a modal.",
                Tone::Progress,
            ),
            KeyOutcome::Decided(decision) => self.send_decision(app, decision),
            KeyOutcome::Answered(answers) => self.send_answers(app, answers),
            KeyOutcome::SwitchClarificationMode => self.switch_clarification_mode(app),
            KeyOutcome::EditPlan(plan) => self.plan_to_edit = Some(plan),
            KeyOutcome::OpenBrowser => browsing::open(app, project),
            KeyOutcome::Browser(outcome) => {
                let running_here = self.session.is_some();
                if let Some(resumed) = browsing::act(app, project, outcome, running_here) {
                    self.resume(app, project, resumed);
                }
            }
        }
        ControlFlow::Continue(())
    }

    /// An edited Plan goes back to the Plan Checkpoint, which shows it again. An edit that changed
    /// nothing or can't be used leaves the modal open.
    fn send_edited_plan(&mut self, app: &mut App, plan_edit: PlanEdit) {
        match plan_edit {
            Ok(Some(edited)) => {
                app.ui_state.modal = None;
                self.send_decision(app, Decision::EditPlan(edited));
            }
            Ok(None) => app.view.push_note("The Plan is unchanged.", Tone::Progress),
            Err(message) => app.view.push_note(message, Tone::Failure),
        }
    }

    /// A running Session takes the new mode from its next Planner or Scaffolder run; otherwise
    /// the next Session starts with it.
    fn switch_clarification_mode(&mut self, app: &mut App) {
        let mode = match app.status.clarification_mode {
            ClarificationMode::Ask => ClarificationMode::Assume,
            ClarificationMode::Assume => ClarificationMode::Ask,
        };
        app.status.clarification_mode = mode;
        let applies_from = match &self.session {
            Some(session) if session.mode_switches.send(mode).is_ok() => {
                "the next Planner or Scaffolder run"
            }
            _ => "the next Session",
        };
        app.view.push_note(
            format!("Clarification Mode {mode:?} from {applies_from}."),
            Tone::Operator,
        );
    }

    fn take_awaited_reply(&mut self) -> Option<AwaitedReply> {
        self.session
            .as_mut()
            .and_then(|session| session.awaited_reply.take())
    }

    fn send_decision(&mut self, app: &mut App, decision: Decision) {
        if let Some(AwaitedReply::Decision(reply)) = self.take_awaited_reply() {
            app.view.push_note(decision_note(&decision), Tone::Operator);
            let _ = reply.send(decision);
        }
    }

    fn send_answers(&mut self, app: &mut App, answers: String) {
        if let Some(AwaitedReply::Answers(reply)) = self.take_awaited_reply() {
            app.view
                .push_note("Answered the Tier's questions.", Tone::Operator);
            let _ = reply.send(Some(answers));
        }
    }
}

/// Applies every message waiting from the Session thread. Returns `false` once the run has ended.
fn drain_messages(app: &mut App, session: &mut RunningSession) -> bool {
    loop {
        match session.messages.try_recv() {
            Ok(DriverMessage::Started {
                branch,
                session_dir,
            }) => {
                session.started = Some(started_session(branch.clone(), &session_dir));
                app.status.branch = Some(branch);
                app.view.push_note(
                    format!("Session folder {}", session_dir.display()),
                    Tone::Progress,
                );
            }
            Ok(DriverMessage::Update(update)) => app.apply_update(update),
            Ok(DriverMessage::Checkpoint { checkpoint, reply }) => {
                app.stop_working();
                app.ui_state.open_modal(Modal::checkpoint(checkpoint));
                session.awaited_reply = Some(AwaitedReply::Decision(reply));
            }
            Ok(DriverMessage::Questions {
                tier,
                questions,
                reply,
            }) => {
                app.stop_working();
                app.ui_state.open_modal(Modal::questions(tier, questions));
                session.awaited_reply = Some(AwaitedReply::Answers(reply));
            }
            Ok(DriverMessage::Ended(message, tone)) => {
                mark_ended(&mut session.started, tone);
                app.view.push_note(message, tone);
                return false;
            }
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

fn started_session(branch: String, session_dir: &Path) -> StartedSession {
    StartedSession {
        branch,
        worktree: SessionFolder::new(session_dir).worktree_path(),
        finished: false,
    }
}

/// Only a Session that ended with success is finished; any other ending leaves it unfinished.
fn mark_ended(started: &mut Option<StartedSession>, tone: Tone) {
    if let Some(started) = started {
        started.finished = tone == Tone::Success;
    }
}

fn start_session(app: &mut App, project: &Project, start: SessionStart) -> Option<RunningSession> {
    let api_key = match project.api_key() {
        Ok(Some(api_key)) => api_key,
        Ok(None) => {
            app.view.push_note(
                format!(
                    "No OpenRouter API key: set {API_KEY_ENV} or api_key in the user config, \
                     then send the Request again."
                ),
                Tone::Failure,
            );
            return None;
        }
        Err(err) => {
            app.view.push_note(err.to_string(), Tone::Failure);
            return None;
        }
    };
    let (sender, messages) = mpsc::channel();
    let (cancel, cancelled) = oneshot::channel();
    let (mode_switches, switched_modes) = mpsc::channel();
    let root = project.root.clone();
    let mut config = project.config.clone();
    config.clarification_mode = app.status.clarification_mode;
    let driver = thread::spawn(move || {
        let operator = ScreenOperator {
            sender: sender.clone(),
            mode_switches: switched_modes,
        };
        let ending = drive(&root, config, &api_key, start, operator, cancelled);
        let _ = sender.send(ending);
    });
    Some(RunningSession {
        messages,
        cancel: Some(cancel),
        driver,
        awaited_reply: None,
        mode_switches,
        started: None,
    })
}

/// Runs the Session, new or resumed. Returns the last message.
fn drive(
    root: &Path,
    config: Config,
    api_key: &str,
    start: SessionStart,
    operator: ScreenOperator,
    cancelled: oneshot::Receiver<()>,
) -> DriverMessage {
    try_drive(root, config, api_key, start, operator, cancelled)
        .unwrap_or_else(|message| DriverMessage::Ended(message, Tone::Failure))
}

/// What the Autopilot starts from.
enum RunFrom {
    Request(Request),
    Log(Vec<Event>),
}

/// A new Session's branch, folder and worktree, claimed for its first run.
fn create_session(
    repo: &Repo,
    mainline: &Mainline,
    request: &Request,
) -> Result<(Worktree, SessionFolder, RunLock), String> {
    let branch = free_branch(repo, &request.text)?;
    let folder = SessionFolder::for_new_session(repo.root(), SystemTime::now(), branch.as_str());
    exclude_sessions(repo)
        .map_err(|message| format!("Could not keep {SESSIONS_DIR} out of git: {message}"))?;
    fs::create_dir_all(folder.dir())
        .map_err(|err| format!("Could not create {}: {err}", folder.dir().display()))?;
    let lock = folder.lock_run().map_err(|err| err.to_string())?;
    let worktree = repo
        .create_worktree(mainline, &branch, &folder.worktree_path())
        .map_err(|err| format!("Could not create the Session worktree: {err}"))?;
    Ok((worktree, folder, lock))
}

/// `Err` holds the message for a Session that couldn't start.
fn try_drive(
    root: &Path,
    config: Config,
    api_key: &str,
    start: SessionStart,
    operator: ScreenOperator,
    cancelled: oneshot::Receiver<()>,
) -> Result<DriverMessage, String> {
    let sender = operator.sender.clone();
    let repo = Repo::open(root).map_err(|err| format!("Could not open the repository: {err}"))?;
    let mainline = repo
        .mainline(config.mainline.as_deref())
        .map_err(|err| format!("Could not find the mainline: {err}"))?;
    let (worktree, folder, _lock, run_from) = match start {
        SessionStart::New(request) => {
            let (worktree, folder, lock) = create_session(&repo, &mainline, &request)?;
            (worktree, folder, lock, RunFrom::Request(request))
        }
        SessionStart::Resume(Resumed {
            folder,
            lock,
            events,
        }) => {
            let worktree = Worktree::open(folder.worktree_path())
                .map_err(|err| format!("Could not open the Session worktree: {err}"))?;
            (worktree, folder, lock, RunFrom::Log(events))
        }
    };
    let event_file = folder
        .append_to_log()
        .map_err(|err| format!("Could not open the event log: {err}"))?;
    let _ = sender.send(DriverMessage::Started {
        branch: worktree.branch().to_owned(),
        session_dir: folder.dir().to_owned(),
    });

    let client = OpenRouterClient::new(api_key);
    let observer_sender = sender.clone();
    let mut autopilot = Autopilot::new(&worktree, &mainline, &client, config)
        .map_err(|err| err.to_string())?
        .with_operator(operator)
        .log_events_to(event_file)
        .on_update(move |update| {
            let _ = observer_sender.send(DriverMessage::Update(update));
        });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("Could not start the async runtime: {err}"))?;
    // Dropping the run future at a cancel kills a running Check (`kill_on_drop`); the worktree is
    // put back after that, below.
    let session_end = runtime.block_on(async {
        let run = async {
            match run_from {
                RunFrom::Request(request) => autopilot.run(request).await,
                RunFrom::Log(events) => autopilot.resume(events).await,
            }
        };
        tokio::select! {
            session_end = run => Some(session_end),
            _ = cancelled => None,
        }
    });
    let Some(session_end) = session_end else {
        let message = cancelled_ending(autopilot.session().phase(), &worktree);
        let _ = autopilot.record_run_end(RunOutcome::Cancelled);
        return Ok(message);
    };
    let run_outcome = match &session_end {
        Ok(end) => end.run_outcome(),
        Err(err) => Some(RunOutcome::Failed {
            reason: err.to_string(),
        }),
    };
    if let Some(run_outcome) = run_outcome {
        let _ = autopilot.record_run_end(run_outcome);
    }
    Ok(ending(session_end))
}

/// After a cancel, puts the worktree back to its last commit when the cancelled step had written
/// files no event records yet, such as a Writer's code still in its Check, so the kept worktree
/// matches the event log.
fn cancelled_ending(phase: &Phase, worktree: &Worktree) -> DriverMessage {
    let unrecorded_edits = matches!(
        phase,
        Phase::Writing { .. } | Phase::Scaffolding { .. } | Phase::Revising { .. }
    );
    if !unrecorded_edits {
        return DriverMessage::Ended(
            "Cancelled. The branch and worktree are kept.".to_owned(),
            Tone::Operator,
        );
    }
    match worktree.head().and_then(|head| worktree.reset_to(&head)) {
        Ok(()) => DriverMessage::Ended(
            "Cancelled. The unfinished step's edits were undone; the branch and worktree are kept \
             at their last commit."
                .to_owned(),
            Tone::Operator,
        ),
        Err(err) => DriverMessage::Ended(
            format!(
                "Cancelled, but the unfinished step's edits couldn't be undone: {err}. Run \
                 `git reset --hard` in {} before using it.",
                worktree.path().display()
            ),
            Tone::Failure,
        ),
    }
}

fn ending(session_end: Result<SessionEnd, arbor_session::AutopilotError>) -> DriverMessage {
    let (message, tone) = match session_end {
        Ok(SessionEnd::Finished) => ("Session finished.".to_owned(), Tone::Success),
        Ok(SessionEnd::FinishedWithSkippedTasks { skipped_tasks }) => {
            (skipped_tasks_ending(&skipped_tasks), Tone::Operator)
        }
        Ok(SessionEnd::NeedsOperator(effect)) => (
            format!(
                "Stopped at {}. The branch and worktree are kept.",
                operator_step(&effect)
            ),
            Tone::Operator,
        ),
        Ok(SessionEnd::StoppedAtBudget) => (
            "Stopped at the Budget Cap. The branch and worktree are kept.".to_owned(),
            Tone::Operator,
        ),
        Ok(SessionEnd::Aborted) => (
            "Session aborted. The branch and worktree are kept.".to_owned(),
            Tone::Operator,
        ),
        Ok(SessionEnd::TierStopped { tier, reason }) => {
            (format!("Stopped: the {tier:?} {reason}"), Tone::Failure)
        }
        Ok(SessionEnd::ScaffoldChecksFailed { check_output }) => (
            format!(
                "Stopped: the Scaffold Check failed: {}",
                check_output.lines().last().unwrap_or_default()
            ),
            Tone::Failure,
        ),
        Err(err) => (format!("Stopped: {err}"), Tone::Failure),
    };
    DriverMessage::Ended(message, tone)
}

fn skipped_tasks_ending(skipped_tasks: &[usize]) -> String {
    let task_numbers: Vec<String> = skipped_tasks
        .iter()
        .map(|task| (task + 1).to_string())
        .collect();
    let tasks = if task_numbers.len() == 1 {
        "Task"
    } else {
        "Tasks"
    };
    format!(
        "Session finished with {tasks} {} skipped; the branch lacks that part of the Request.",
        task_numbers.join(", ")
    )
}

/// The event log line for the Human Operator's decision at a Checkpoint.
fn decision_note(decision: &Decision) -> String {
    match decision {
        Decision::Approve => "Approved the Checkpoint.".to_owned(),
        Decision::ApproveTasks(task_order) => {
            let task_numbers: Vec<String> = task_order
                .iter()
                .map(|task| (task + 1).to_string())
                .collect();
            format!(
                "Approved the Plan with Tasks {} in that order.",
                task_numbers.join(", ")
            )
        }
        Decision::Reject(feedback) => format!("Rejected the Checkpoint: {feedback}"),
        Decision::EditPlan(_) => "Edited the Plan by hand.".to_owned(),
        Decision::Fixed => "Fixed it by hand; checking the fix.".to_owned(),
        Decision::Replan => "Sent the Task to the Planner.".to_owned(),
        Decision::RetryWithHint { tier, hint } => {
            format!("Ran the {tier:?} again with a hint: {hint}")
        }
        Decision::RetryWithStrongerWriter => "Sent the Work Unit to a stronger Writer.".to_owned(),
        Decision::Skip => "Skipped the Task.".to_owned(),
        Decision::Abort => "Aborted the Session.".to_owned(),
        Decision::Stop => "Stopped at the Checkpoint.".to_owned(),
        Decision::SetBudgetCap(Some(cap)) => {
            format!("Raised the Budget Cap to ${:.2}.", cap.dollars())
        }
        Decision::SetBudgetCap(None) => "Removed the Budget Cap.".to_owned(),
    }
}

/// What the Session waits on the Human Operator for, in a few words.
fn operator_step(effect: &Effect) -> String {
    match effect {
        Effect::ShowPlanCheckpoint => "the Plan Checkpoint".to_owned(),
        Effect::ShowScaffoldCheckpoint { task } => {
            format!("the Scaffold Checkpoint of Task {}", task + 1)
        }
        Effect::ShowRevisionCheckpoint { task, .. } => {
            format!("the Revision Checkpoint of Task {}", task + 1)
        }
        Effect::ShowEscalationCheckpoint { task, .. } => {
            format!("the Escalation Checkpoint of Task {}", task + 1)
        }
        Effect::ShowReplanCheckpoint { task, .. } => {
            format!("the Replan Checkpoint of Task {}", task + 1)
        }
        Effect::ShowMarkerCheckpoint { remaining } => format!(
            "the Marker Checkpoint ({} Marker{} left)",
            remaining.len(),
            if remaining.len() == 1 { "" } else { "s" }
        ),
        Effect::ShowBudgetCheckpoint => "the Budget Checkpoint".to_owned(),
        _ => "the next step".to_owned(),
    }
}

/// A Session branch named after the Request's first words that doesn't exist yet.
fn free_branch(repo: &Repo, request_text: &str) -> Result<SessionBranch, String> {
    let feature: Vec<&str> = request_text.split_whitespace().take(6).collect();
    let feature = feature.join(" ");
    let existing = repo.branches().map_err(|err| err.to_string())?;
    for attempt in 1..100 {
        let candidate = if attempt == 1 {
            feature.clone()
        } else {
            format!("{feature} {attempt}")
        };
        let branch = SessionBranch::for_feature(&candidate).map_err(|err| err.to_string())?;
        if !existing.iter().any(|name| name == branch.as_str()) {
            return Ok(branch);
        }
    }
    Err("every Session branch name for this Request is taken".to_owned())
}

/// Adds the Sessions folder to the `info/exclude` every worktree shares, so the worktrees inside
/// the project don't show up as untracked files, even when arbor runs from a linked worktree.
fn exclude_sessions(repo: &Repo) -> Result<(), String> {
    let info_dir = repo
        .common_dir()
        .map_err(|err| err.to_string())?
        .join("info");
    fs::create_dir_all(&info_dir).map_err(|err| err.to_string())?;
    let exclude_path = info_dir.join("exclude");
    let existing = fs::read_to_string(&exclude_path).unwrap_or_default();
    let pattern = format!("/{SESSIONS_DIR}/");
    if existing.lines().any(|line| line.trim() == pattern) {
        return Ok(());
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    fs::write(&exclude_path, format!("{existing}{separator}{pattern}\n"))
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_with_skipped_tasks_ends_naming_them_not_as_plainly_finished() {
        let skipped_end = Ok(SessionEnd::FinishedWithSkippedTasks {
            skipped_tasks: vec![1, 2],
        });
        let DriverMessage::Ended(message, tone) = ending(skipped_end) else {
            panic!("a finished Session ends the run");
        };
        assert_eq!(
            message,
            "Session finished with Tasks 2, 3 skipped; the branch lacks that part of the Request."
        );
        assert!(matches!(tone, Tone::Operator));
    }
}
