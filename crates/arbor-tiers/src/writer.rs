//! The Writer: one non-agentic call per attempt that fills one Region (ADR-0001).

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use arbor_config::Checks;
use arbor_core::{BrokenRegion, FailedAttempt, Usage, WorkUnit, WorkUnitFailure, WorkUnitId};
use arbor_git::{CommitId, CommitMessage, GitError, Worktree};
use arbor_llm::{ChatRequest, ContentPart, FinishReason, LlmClient, LlmError, Message};
use arbor_prompts::{Prompt, estimate_tokens};
use arbor_tools::{CheckName, run_check};

use crate::agent::{core_usage, supports};
use crate::markers::{Markers, Region, splice, strip};
use crate::reply::{CodeReply, ReplyError, WriterReply, parse_reply};
use crate::writer_cap::{with_margin, writer_input_tokens};

pub const MAX_WRITER_ATTEMPTS: usize = 3;

/// Any fixed value works. Changing it changes what every Writer writes, so treat it like a change
/// to the Writer's prompt.
pub const WRITER_SEED: u64 = 7;

/// About 1,000 tokens: the most Check output a retry shows, and less when the Writer Cap needs the
/// room.
pub const RETRY_OUTPUT_CHARS: usize = 4000;

const MAX_SUBJECT_CHARS: usize = 72;

pub struct WriterRun<'a> {
    pub prompt: &'a Prompt,
    pub work_unit: &'a WorkUnit,
    pub model: String,
    pub temperature: f64,
    /// The model's `supported_parameters`. Temperature and the fixed seed are sent only when listed,
    /// since `require_parameters` would otherwise leave the call no provider (ADR-0007).
    pub supported_parameters: Vec<String>,
    pub worktree: &'a Path,
    pub checks: Checks,
    pub cap_tokens: u32,
    /// `Session::may_send_missing_response`.
    pub missing_response_allowed: bool,
}

impl WriterRun<'_> {
    /// # Panics
    ///
    /// If the prompt isn't the Writer's.
    pub fn start(self) -> Writer {
        assert_eq!(self.prompt.template, arbor_prompts::Template::Writer);
        Writer {
            work_unit: self.work_unit.clone(),
            model: self.model,
            temperature: self.temperature,
            sends_temperature: supports(&self.supported_parameters, "temperature"),
            sends_seed: supports(&self.supported_parameters, "seed"),
            worktree: self.worktree.to_owned(),
            checks: self.checks,
            cap_tokens: self.cap_tokens,
            prompt: self.prompt.messages(),
            retry: Vec::new(),
            retry_tokens: 0,
            failures: Vec::new(),
            finished: false,
            missing_response_allowed: self.missing_response_allowed,
            missing_response: None,
            broken_region: None,
        }
    }
}

pub struct Writer {
    work_unit: WorkUnit,
    model: String,
    temperature: f64,
    sends_temperature: bool,
    sends_seed: bool,
    worktree: PathBuf,
    checks: Checks,
    cap_tokens: u32,
    prompt: Vec<Message>,
    /// The latest failed reply, when it fits, and the message asking again. Earlier failures
    /// are left out so retries stay within the Writer Cap.
    retry: Vec<Message>,
    retry_tokens: u32,
    failures: Vec<Failure>,
    finished: bool,
    missing_response_allowed: bool,
    missing_response: Option<Vec<String>>,
    broken_region: Option<BrokenRegion>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attempt {
    /// The model that actually answered.
    pub model: String,
    pub usage: Usage,
    pub outcome: AttemptOutcome,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttemptOutcome {
    Passed {
        commit: CommitId,
        /// The reply whose code passed.
        reply: String,
    },
    Failed(Failure),
    /// Goes to a Revision rather than a retry.
    Missing {
        items: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub reply: String,
    pub reason: FailureReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureReason {
    Rejected(ReplyError),
    ChecksFailed { output: String },
}

impl fmt::Display for FailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(error) => write!(f, "rejected: {error}"),
            Self::ChecksFailed { output } => write!(f, "the Checks failed:\n{output}"),
        }
    }
}

#[derive(Debug)]
pub enum WriterError {
    /// No attempt was used up.
    Llm(LlmError),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// The Work Unit's file doesn't hold exactly one well-formed Region for it. The Writer is
    /// finished, and [`Writer::work_unit_failure`] sends the problem to a Revision.
    RegionBroken {
        file: String,
        id: WorkUnitId,
        broken: Box<BrokenRegion>,
    },
    Git(GitError),
    /// The request would be bigger than the Writer Cap, so it wasn't sent.
    OverCap {
        tokens: u32,
        cap_tokens: u32,
    },
    NotCommitted {
        file: String,
    },
    /// A Check couldn't run at all, e.g. its command wasn't found.
    Check(String),
    Finished,
    NoAttemptsLeft,
}

impl fmt::Display for WriterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Llm(err) => write!(f, "the Writer's model call failed: {err}"),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::RegionBroken { file, id, broken } => {
                write!(
                    f,
                    "{file} has no usable Region for {id}: {}",
                    broken.problem
                )
            }
            Self::Git(err) => write!(f, "{err}"),
            Self::OverCap { tokens, cap_tokens } => write!(
                f,
                "the Writer's input would be about {tokens} tokens, over the Writer Cap of \
                 {cap_tokens}"
            ),
            Self::NotCommitted { file } => write!(
                f,
                "git found nothing to commit after {file} was filled; it may be ignored by git"
            ),
            Self::Check(message) => write!(f, "a Check couldn't run: {message}"),
            Self::Finished => f.write_str("the Writer has already finished"),
            Self::NoAttemptsLeft => write!(
                f,
                "the Writer has used all {MAX_WRITER_ATTEMPTS} of its attempts"
            ),
        }
    }
}

impl std::error::Error for WriterError {}

/// The model's usage comes with the error when the call was made, so the Budget Cap still counts
/// it.
#[derive(Debug)]
pub struct AttemptError {
    pub error: WriterError,
    pub usage: Option<Usage>,
}

impl fmt::Display for AttemptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for AttemptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl From<WriterError> for AttemptError {
    fn from(error: WriterError) -> Self {
        Self { error, usage: None }
    }
}

impl From<GitError> for WriterError {
    fn from(err: GitError) -> Self {
        Self::Git(err)
    }
}

impl Writer {
    pub fn work_unit(&self) -> &WorkUnit {
        &self.work_unit
    }

    pub fn failures(&self) -> &[Failure] {
        &self.failures
    }

    pub fn attempts_left(&self) -> usize {
        MAX_WRITER_ATTEMPTS.saturating_sub(self.failures.len())
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Once the Writer has given up, by using up its retries, sending a Missing Response, or
    /// finding its Region broken, the failure to record as `WorkUnitFailed` and hand to a
    /// Revision.
    pub fn work_unit_failure(&self) -> Option<WorkUnitFailure> {
        let gave_up = self.missing_response.is_some()
            || self.broken_region.is_some()
            || self.attempts_left() == 0;
        gave_up.then(|| WorkUnitFailure {
            id: self.work_unit.id,
            attempts: self
                .failures
                .iter()
                .map(|failure| FailedAttempt {
                    reply: failure.reply.clone(),
                    error: failure.reason.to_string(),
                })
                .collect(),
            missing_response: self.missing_response.clone(),
            broken_region: self.broken_region.clone(),
        })
    }

    /// Estimated the same way as the Writer Cap check on the Work Units.
    pub fn input_tokens(&self) -> u32 {
        writer_input_tokens(&self.work_unit).saturating_add(self.retry_tokens)
    }

    pub fn request(&self) -> ChatRequest {
        let mut messages = self.prompt.clone();
        messages.extend(self.retry.iter().cloned());
        ChatRequest {
            model: self.model.clone(),
            messages,
            tools: Vec::new(),
            tool_choice: None,
            temperature: self.sends_temperature.then_some(self.temperature),
            seed: self.sends_seed.then_some(WRITER_SEED),
            max_tokens: None,
            require_parameters: true,
            cache_transcript: false,
        }
    }

    /// An `Err` uses up no attempt. A broken Region is found before the model call when it can
    /// be, and ends the Writer either way.
    pub async fn attempt(&mut self, client: &dyn LlmClient) -> Result<Attempt, AttemptError> {
        self.attempt_reporting_checks(client, &mut || {}).await
    }

    /// Like [`Self::attempt`], calling `checks_started` when the reply is spliced and its Checks
    /// start, so a caller can show that the Check, not the model, is running.
    pub async fn attempt_reporting_checks(
        &mut self,
        client: &dyn LlmClient,
        checks_started: &mut dyn FnMut(),
    ) -> Result<Attempt, AttemptError> {
        let attempted = self.attempt_on_intact_region(client, checks_started).await;
        if let Err(AttemptError {
            error: WriterError::RegionBroken { broken, .. },
            ..
        }) = &attempted
        {
            self.broken_region = Some(BrokenRegion::clone(broken));
            self.finished = true;
        }
        attempted
    }

    async fn attempt_on_intact_region(
        &mut self,
        client: &dyn LlmClient,
        checks_started: &mut dyn FnMut(),
    ) -> Result<Attempt, AttemptError> {
        if self.finished {
            return Err(WriterError::Finished.into());
        }
        if self.attempts_left() == 0 {
            return Err(WriterError::NoAttemptsLeft.into());
        }
        let tokens = self.input_tokens();
        if tokens > self.cap_tokens {
            return Err(WriterError::OverCap {
                tokens,
                cap_tokens: self.cap_tokens,
            }
            .into());
        }
        self.region_before_call()?;
        let chat_response = client
            .chat(&self.request())
            .await
            .map_err(WriterError::Llm)?;
        let usage = core_usage(&chat_response.usage);
        let reply = chat_response.content.unwrap_or_default();
        let outcome = self
            .handle(&reply, &chat_response.finish_reason, checks_started)
            .await
            .map_err(|error| AttemptError {
                error,
                usage: Some(usage),
            })?;
        match &outcome {
            AttemptOutcome::Failed(failure) => {
                self.set_retry(failure);
                self.failures.push(failure.clone());
            }
            AttemptOutcome::Missing { items } => self.missing_response = Some(items.clone()),
            AttemptOutcome::Passed { .. } => {}
        }
        self.finished = !matches!(outcome, AttemptOutcome::Failed(_));
        Ok(Attempt {
            model: chat_response.model,
            usage,
            outcome,
        })
    }

    async fn handle(
        &self,
        reply: &str,
        finish: &FinishReason,
        checks_started: &mut dyn FnMut(),
    ) -> Result<AttemptOutcome, WriterError> {
        if *finish == FinishReason::Length {
            return Ok(failed(reply, FailureReason::Rejected(ReplyError::CutOff)));
        }
        match parse_reply(reply) {
            Err(error) => Ok(failed(reply, FailureReason::Rejected(error))),
            Ok(WriterReply::Missing(_)) if !self.missing_response_allowed => Ok(failed(
                reply,
                FailureReason::Rejected(ReplyError::MissingResponseUsed),
            )),
            Ok(WriterReply::Missing(items)) => Ok(AttemptOutcome::Missing { items }),
            Ok(WriterReply::Code(code)) => self.fill_region(&code, reply, checks_started).await,
        }
    }

    /// Splices the code into the Region, runs the Checks, and commits the code that passes.
    async fn fill_region(
        &self,
        code: &CodeReply,
        reply: &str,
        checks_started: &mut dyn FnMut(),
    ) -> Result<AttemptOutcome, WriterError> {
        let unit = &self.work_unit;
        let path = self.worktree.join(&unit.file);
        let original = read(&path)?;
        let region = self.intact_region(&original)?;
        let lines: Vec<&str> = original.lines().collect();
        let before = &lines[region.context_before()];
        let after = &lines[region.context_after(lines.len())];
        let body = match code.fit(before, after) {
            Ok(body) => body,
            Err(error) => return Ok(failed(reply, FailureReason::Rejected(error))),
        };

        let spliced = splice(&original, &region, &body);
        write(&path, &spliced)?;
        checks_started();
        if let Some(output) = restoring(&path, &original, self.run_checks().await)? {
            write(&path, &original)?;
            return Ok(failed(reply, FailureReason::ChecksFailed { output }));
        }
        // A Check may rewrite the file, e.g. a formatter that moves a Marker line. What is
        // committed is the spliced code that passed, so that can't break the Region.
        let commit = restoring(&path, &original, self.commit(&path, &spliced))?;
        Ok(AttemptOutcome::Passed {
            commit,
            reply: reply.to_owned(),
        })
    }

    fn commit(&self, path: &Path, spliced: &str) -> Result<CommitId, WriterError> {
        let unit = &self.work_unit;
        let region = self.intact_region(spliced)?;
        write(path, &strip(spliced, &region))?;
        let message = CommitMessage::new(&commit_subject(&unit.spec, &unit.file))?;
        Worktree::open(&self.worktree)?
            .commit(&message)?
            .ok_or_else(|| WriterError::NotCommitted {
                file: unit.file.clone(),
            })
    }

    /// The output of the first Check that fails.
    async fn run_checks(&self) -> Result<Option<String>, WriterError> {
        let checks = &self.checks;
        let filter = Some(self.work_unit.test_filter.as_str());
        let runs = [
            (CheckName::Scaffold, checks.scaffold.is_some(), None),
            (
                CheckName::UnitFiltered,
                checks.unit_filtered.is_some(),
                filter,
            ),
        ];
        for (check, configured, filter) in runs {
            if !configured {
                continue;
            }
            let check_output = run_check(&self.worktree, checks, check, filter)
                .await
                .map_err(WriterError::Check)?;
            if check_output.is_error {
                return Ok(Some(check_output.text));
            }
        }
        Ok(None)
    }

    fn set_retry(&mut self, failure: &Failure) {
        let room = self
            .cap_tokens
            .saturating_sub(writer_input_tokens(&self.work_unit));
        let (reply, message) = retry_parts(failure, room);
        self.retry_tokens = retry_tokens(reply, &message);
        self.retry.clear();
        if let Some(reply) = reply {
            self.retry.push(Message::Assistant {
                content: Some(reply.to_owned()),
                tool_calls: Vec::new(),
            });
        }
        self.retry.push(Message::User {
            content: vec![ContentPart::text(message)],
        });
    }

    /// Checks the Region before the model call, so a broken one costs no call. A deleted file is a
    /// broken Region too.
    fn region_before_call(&self) -> Result<(), WriterError> {
        let path = self.worktree.join(&self.work_unit.file);
        match fs::read_to_string(&path) {
            Ok(file_text) => self.intact_region(&file_text).map(|_| ()),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                Err(self.region_broken(format!("{} is gone", self.work_unit.file), ""))
            }
            Err(source) => Err(WriterError::Io { path, source }),
        }
    }

    /// The Work Unit's Region in `file_text`, or the error saying what is wrong with its Markers.
    fn intact_region(&self, file_text: &str) -> Result<Region, WriterError> {
        let id = self.work_unit.id;
        let markers = Markers::parse(file_text);
        let problems = markers.problems_for(id);
        if let Some(region) = markers.region(id)
            && problems.is_empty()
        {
            return Ok(*region);
        }
        let problem = problems
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        Err(self.region_broken(problem, file_text))
    }

    fn region_broken(&self, problem: String, file_text: &str) -> WriterError {
        WriterError::RegionBroken {
            file: self.work_unit.file.clone(),
            id: self.work_unit.id,
            broken: Box::new(BrokenRegion {
                problem,
                file_text: file_text.to_owned(),
            }),
        }
    }
}

fn failed(reply: &str, reason: FailureReason) -> AttemptOutcome {
    AttemptOutcome::Failed(Failure {
        reply: reply.to_owned(),
        reason,
    })
}

/// Shortens the Check output first, then leaves out the failed reply, until the retry fits in
/// `room` tokens. If nothing fits, `attempt` refuses to send it.
fn retry_parts(failure: &Failure, room: u32) -> (Option<&str>, String) {
    for reply in [Some(failure.reply.as_str()), None] {
        let mut output_chars = RETRY_OUTPUT_CHARS;
        loop {
            let message = retry_message(&failure.reason, output_chars, reply.is_none());
            if retry_tokens(reply, &message) <= room {
                return (reply, message);
            }
            if output_chars == 0 {
                break;
            }
            output_chars /= 2;
        }
    }
    (None, retry_message(&failure.reason, 0, true))
}

fn retry_tokens(reply: Option<&str>, message: &str) -> u32 {
    let raw = reply.map_or(0, estimate_tokens) + estimate_tokens(message);
    with_margin(u64::from(raw))
}

fn retry_message(reason: &FailureReason, output_chars: usize, reply_left_out: bool) -> String {
    let mut message = match reason {
        FailureReason::Rejected(error) => format!(
            "Your reply was rejected: {error}\n\nReply again with exactly one fenced block, \
             following the output contract."
        ),
        FailureReason::ChecksFailed { output } => format!(
            "Arbor put your code between the Markers, but the Checks failed, so the Region holds \
             its Placeholder again. Their output:\n\n```text\n{}\n```\n\nFix the code and reply \
             again with exactly one fenced block, following the output contract.",
            shorten(output, output_chars)
        ),
    };
    if reply_left_out {
        message.push_str("\n\nYour previous reply is left out here to save space.");
    }
    message
}

/// Keeps the start and end of `long_text`, where compilers and test runners put what matters.
fn shorten(long_text: &str, max_chars: usize) -> String {
    let count = long_text.chars().count();
    if count <= max_chars {
        return long_text.to_owned();
    }
    let half = max_chars / 2;
    let head: String = long_text.chars().take(half).collect();
    let tail: String = long_text.chars().skip(count - half).collect();
    format!("{head}\n[{} characters omitted]\n{tail}", count - 2 * half)
}

/// The first sentence of the spec (ADR-0009), or `Update <file>` if that names Arbor or a Work
/// Unit.
pub fn commit_subject(spec: &str, file: &str) -> String {
    let line = spec
        .lines()
        .map(|line| line.trim().trim_start_matches(['#', '-', '*', ' ']))
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let sentence = first_sentence(line).trim_end_matches(['.', ':', ';', ',', ' ']);
    let names_arbor = sentence.contains("WU-")
        || sentence
            .split(|character: char| !character.is_alphanumeric())
            .any(|word| word.eq_ignore_ascii_case("arbor"));
    if sentence.is_empty() || names_arbor {
        return format!("Update {file}");
    }
    let mut subject = String::new();
    for word in sentence.split_whitespace() {
        let extra = usize::from(!subject.is_empty()) + word.chars().count();
        if subject.chars().count() + extra > MAX_SUBJECT_CHARS {
            break;
        }
        if !subject.is_empty() {
            subject.push(' ');
        }
        subject.push_str(word);
    }
    if subject.is_empty() {
        subject = sentence.chars().take(MAX_SUBJECT_CHARS).collect();
    }
    let mut chars = subject.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => format!("Update {file}"),
    }
}

/// A sentence ends only before a space and a capital letter, so `e.g. x` doesn't end one.
fn first_sentence(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (index, character) in line.char_indices() {
        if matches!(character, '.' | '?' | '!')
            && bytes.get(index + 1) == Some(&b' ')
            && line[index + 2..].starts_with(char::is_uppercase)
        {
            return &line[..=index];
        }
    }
    line
}

/// Puts the file back as it was before the splice when `attempt` is an error, so the Region is
/// still there for the next attempt. The original error wins over a failed restore.
fn restoring<T>(
    path: &Path,
    original: &str,
    attempt: Result<T, WriterError>,
) -> Result<T, WriterError> {
    if attempt.is_err() {
        let _ = fs::write(path, original);
    }
    attempt
}

fn read(path: &Path) -> Result<String, WriterError> {
    fs::read_to_string(path).map_err(|source| WriterError::Io {
        path: path.to_owned(),
        source,
    })
}

fn write(path: &Path, contents: &str) -> Result<(), WriterError> {
    fs::write(path, contents).map_err(|source| WriterError::Io {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::{Failure, FailureReason, RETRY_OUTPUT_CHARS, retry_parts, retry_tokens, shorten};

    #[test]
    fn long_check_output_keeps_its_start_and_end() {
        assert_eq!(shorten("abcdef", 6), "abcdef");
        assert_eq!(shorten("abcdefgh", 4), "ab\n[4 characters omitted]\ngh");
        assert_eq!(shorten("ééééé", 2), "é\n[3 characters omitted]\né");
    }

    #[test]
    fn a_retry_shrinks_to_fit_the_room_left_under_the_writer_cap() {
        let failure = Failure {
            reply: "x".repeat(1000),
            reason: FailureReason::ChecksFailed {
                output: "e".repeat(2 * RETRY_OUTPUT_CHARS),
            },
        };
        let (reply, full) = retry_parts(&failure, u32::MAX);
        assert!(reply.is_some());
        assert!(full.contains(&"e".repeat(RETRY_OUTPUT_CHARS / 2)));

        let room = 1000;
        let (reply, trimmed) = retry_parts(&failure, room);
        assert!(reply.is_some());
        assert!(retry_tokens(reply, &trimmed) <= room);
        assert!(trimmed.len() < full.len());

        let (reply, bare) = retry_parts(&failure, 300);
        assert_eq!(reply, None);
        assert!(bare.ends_with("left out here to save space."), "{bare}");
        assert!(retry_tokens(None, &bare) <= 300);
    }
}
