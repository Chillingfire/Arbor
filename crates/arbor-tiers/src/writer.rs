//! The Writer: one non-agentic call per attempt that fills one Region (ADR-0001).

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use arbor_config::Checks;
use arbor_core::{Usage, WorkUnit, WorkUnitId};
use arbor_git::{CommitId, CommitMessage, GitError, Worktree};
use arbor_llm::{ChatRequest, ContentPart, FinishReason, LlmClient, LlmError, Message};
use arbor_prompts::Prompt;
use arbor_tools::{Tools, Toolset};
use serde_json::json;

use crate::agent::core_usage;
use crate::markers::{Markers, splice, strip};
use crate::reply::{ReplyError, WriterReply, parse_reply};

pub const MAX_WRITER_ATTEMPTS: usize = 3;

/// Any fixed value works. Changing it changes what every Writer writes, so treat it like a change
/// to the Writer's prompt.
pub const WRITER_SEED: u64 = 7;

/// About 1,000 tokens of Check output per retry, so retries stay well inside the Writer Cap.
pub const RETRY_OUTPUT_CHARS: usize = 4000;

const MAX_SUBJECT_CHARS: usize = 72;

pub struct WriterRun<'a> {
    pub prompt: &'a Prompt,
    pub work_unit: &'a WorkUnit,
    pub model: String,
    pub temperature: f64,
    pub worktree: &'a Path,
    pub checks: Checks,
}

impl WriterRun<'_> {
    /// # Panics
    ///
    /// If the prompt isn't the Writer's.
    pub fn start(self) -> Writer {
        assert_eq!(self.prompt.tier, arbor_prompts::Tier::Writer);
        Writer {
            work_unit: self.work_unit.clone(),
            model: self.model,
            temperature: self.temperature,
            worktree: self.worktree.to_owned(),
            tools: Tools::new(self.worktree, Toolset::Scaffolder, self.checks.clone()),
            checks: self.checks,
            messages: self.prompt.messages(),
            failures: Vec::new(),
            finished: false,
        }
    }
}

pub struct Writer {
    work_unit: WorkUnit,
    model: String,
    temperature: f64,
    worktree: PathBuf,
    checks: Checks,
    tools: Tools,
    messages: Vec<Message>,
    failures: Vec<Failure>,
    finished: bool,
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
    /// `None` when the code changed nothing, so there was nothing to commit.
    Passed {
        commit: Option<CommitId>,
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
    NoRegion {
        file: String,
        id: WorkUnitId,
    },
    Git(GitError),
    Finished,
    NoAttemptsLeft,
}

impl fmt::Display for WriterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Llm(err) => write!(f, "the Writer's model call failed: {err}"),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::NoRegion { file, id } => {
                write!(f, "{file} doesn't hold exactly one Region for {id}")
            }
            Self::Git(err) => write!(f, "{err}"),
            Self::Finished => f.write_str("the Writer has already finished"),
            Self::NoAttemptsLeft => write!(
                f,
                "the Writer has used all {MAX_WRITER_ATTEMPTS} of its attempts"
            ),
        }
    }
}

impl std::error::Error for WriterError {}

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

    pub fn request(&self) -> ChatRequest {
        ChatRequest {
            model: self.model.clone(),
            messages: self.messages.clone(),
            tools: Vec::new(),
            tool_choice: None,
            temperature: Some(self.temperature),
            seed: Some(WRITER_SEED),
            max_tokens: None,
            require_parameters: true,
        }
    }

    /// An `Err` uses up no attempt.
    pub async fn attempt(&mut self, client: &dyn LlmClient) -> Result<Attempt, WriterError> {
        if self.finished {
            return Err(WriterError::Finished);
        }
        if self.attempts_left() == 0 {
            return Err(WriterError::NoAttemptsLeft);
        }
        let response = client
            .chat(&self.request())
            .await
            .map_err(WriterError::Llm)?;
        let reply = response.content.unwrap_or_default();
        let outcome = match self.handle(&reply, &response.finish_reason).await? {
            Ok(outcome) => outcome,
            Err(reason) => {
                let failure = Failure { reply, reason };
                self.messages.push(Message::Assistant {
                    content: Some(failure.reply.clone()),
                    tool_calls: Vec::new(),
                });
                self.messages.push(Message::User {
                    content: vec![ContentPart::text(retry_message(&failure.reason))],
                });
                self.failures.push(failure.clone());
                AttemptOutcome::Failed(failure)
            }
        };
        self.finished = !matches!(outcome, AttemptOutcome::Failed(_));
        Ok(Attempt {
            model: response.model,
            usage: core_usage(&response.usage),
            outcome,
        })
    }

    async fn handle(
        &self,
        reply: &str,
        finish: &FinishReason,
    ) -> Result<Result<AttemptOutcome, FailureReason>, WriterError> {
        if *finish == FinishReason::Length {
            return Ok(Err(FailureReason::Rejected(ReplyError::CutOff)));
        }
        let code = match parse_reply(reply) {
            Err(error) => return Ok(Err(FailureReason::Rejected(error))),
            Ok(WriterReply::Missing(items)) => return Ok(Ok(AttemptOutcome::Missing { items })),
            Ok(WriterReply::Code(code)) => code,
        };

        let unit = &self.work_unit;
        let path = self.worktree.join(&unit.file);
        let original = read(&path)?;
        let region = *Markers::parse(&original)
            .region(unit.id)
            .ok_or_else(|| self.no_region())?;
        let lines: Vec<&str> = original.lines().collect();
        let before = &lines[region.context_before()];
        let after = &lines[region.context_after(lines.len())];
        let body = match code.fit(before, after) {
            Ok(body) => body,
            Err(error) => return Ok(Err(FailureReason::Rejected(error))),
        };

        let spliced = splice(&original, &region, &body);
        write(&path, &spliced)?;
        if let Some(output) = self.run_checks().await {
            write(&path, &original)?;
            return Ok(Err(FailureReason::ChecksFailed { output }));
        }

        let region = *Markers::parse(&spliced)
            .region(unit.id)
            .ok_or_else(|| self.no_region())?;
        write(&path, &strip(&spliced, &region))?;
        let message = CommitMessage::new(&commit_subject(&unit.spec, &unit.file))?;
        let commit = Worktree::open(&self.worktree)?.commit(&message)?;
        Ok(Ok(AttemptOutcome::Passed { commit }))
    }

    /// The output of the first Check that fails.
    async fn run_checks(&self) -> Option<String> {
        let mut runs = Vec::new();
        if self.checks.scaffold.is_some() {
            runs.push(json!({ "check": "scaffold" }));
        }
        if self.checks.unit_filtered.is_some() {
            runs.push(json!({ "check": "unit_filtered", "filter": self.work_unit.test_filter }));
        }
        for args in runs {
            let output = self.tools.call("run_checks", &args.to_string()).await;
            if output.is_error {
                return Some(output.text);
            }
        }
        None
    }

    fn no_region(&self) -> WriterError {
        WriterError::NoRegion {
            file: self.work_unit.file.clone(),
            id: self.work_unit.id,
        }
    }
}

fn retry_message(reason: &FailureReason) -> String {
    match reason {
        FailureReason::Rejected(error) => format!(
            "Your reply was rejected: {error}\n\nReply again with exactly one fenced block, \
             following the output contract."
        ),
        FailureReason::ChecksFailed { output } => format!(
            "Arbor put your code between the Markers, but the Checks failed, so the Region holds \
             its Placeholder again. Their output:\n\n```text\n{}\n```\n\nFix the code and reply \
             again with exactly one fenced block, following the output contract.",
            shorten(output, RETRY_OUTPUT_CHARS)
        ),
    }
}

/// Keeps the start and end of `text`, where compilers and test runners put what matters.
fn shorten(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    let half = max_chars / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(count - half).collect();
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
            .split(|c: char| !c.is_alphanumeric())
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
fn first_sentence(text: &str) -> &str {
    let bytes = text.as_bytes();
    for (i, c) in text.char_indices() {
        if matches!(c, '.' | '?' | '!')
            && bytes.get(i + 1) == Some(&b' ')
            && text[i + 2..].starts_with(char::is_uppercase)
        {
            return &text[..=i];
        }
    }
    text
}

fn read(path: &Path) -> Result<String, WriterError> {
    fs::read_to_string(path).map_err(|source| WriterError::Io {
        path: path.to_owned(),
        source,
    })
}

fn write(path: &Path, text: &str) -> Result<(), WriterError> {
    fs::write(path, text).map_err(|source| WriterError::Io {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::shorten;

    #[test]
    fn long_check_output_keeps_its_start_and_end() {
        assert_eq!(shorten("abcdef", 6), "abcdef");
        assert_eq!(shorten("abcdefgh", 4), "ab\n[4 characters omitted]\ngh");
        assert_eq!(shorten("ééééé", 2), "é\n[3 characters omitted]\né");
    }
}
