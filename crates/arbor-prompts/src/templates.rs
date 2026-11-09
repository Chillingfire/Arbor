use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use arbor_config::ClarificationMode;
use arbor_core::{
    Plan, Priority, Request, Revision, RevisionCause, RevisionRound, Snippet, Task, WorkUnit,
    WorkUnitId,
};
use minijinja::syntax::SyntaxConfig;
use minijinja::value::Serde;
use minijinja::{Environment, UndefinedBehavior, Value};
use serde::Serialize;

use crate::Conventions;
use crate::prompt::Prompt;

/// Where a project overrides the built-in templates, relative to the repository root. Each
/// template is read from `<name>.md` here if it exists.
pub const PROMPTS_DIR: &str = ".arbor/prompts";

/// The static part of the prompt: Role, Inputs, Output contract and Rules.
const INSTRUCTIONS_BLOCK: &str = "instructions";
/// The Session inputs, rendered after the Conventions.
const INPUTS_BLOCK: &str = "inputs";

/// A prompt template. The Planner, Scaffolder and Writer each have one, and the Scaffolder has a
/// second for proposing Checks when automatic init can't find working ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Template {
    Planner,
    Scaffolder,
    Writer,
    CheckProposal,
}

impl Template {
    pub const ALL: [Template; 4] = [
        Template::Planner,
        Template::Scaffolder,
        Template::Writer,
        Template::CheckProposal,
    ];

    /// The lowercase name used for template and prompt files, e.g. `planner`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Planner => "planner",
            Self::Scaffolder => "scaffolder",
            Self::Writer => "writer",
            Self::CheckProposal => "check_proposal",
        }
    }

    fn builtin_template(self) -> &'static str {
        match self {
            Self::Planner => include_str!("../templates/planner.md"),
            Self::Scaffolder => include_str!("../templates/scaffolder.md"),
            Self::Writer => include_str!("../templates/writer.md"),
            Self::CheckProposal => include_str!("../templates/check_proposal.md"),
        }
    }

    /// Variables the instructions may use. They stay the same for a whole Session.
    fn static_variables(self) -> &'static [&'static str] {
        match self {
            Self::Planner => &["clarification_mode"],
            Self::Scaffolder => &["clarification_mode", "cap_tokens"],
            Self::Writer | Self::CheckProposal => &[],
        }
    }

    /// Variables only the inputs may use. They change from call to call.
    fn input_variables(self) -> &'static [&'static str] {
        match self {
            Self::Planner => &["request"],
            Self::Scaffolder => &["request", "plan", "task", "revision"],
            Self::Writer => &["work_unit"],
            Self::CheckProposal => &["detection", "tried_checks"],
        }
    }
}

impl fmt::Display for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What the Planner is rendered with.
#[derive(Debug, Clone, Copy)]
pub struct PlannerInputs<'a> {
    pub request: &'a Request,
    pub clarification_mode: ClarificationMode,
}

/// What the Scaffolder is rendered with. `cap_tokens` is the Writer Cap.
#[derive(Debug, Clone, Copy)]
pub struct ScaffolderInputs<'a> {
    pub request: &'a Request,
    pub plan: &'a Plan,
    pub task: &'a Task,
    pub clarification_mode: ClarificationMode,
    pub cap_tokens: u32,
    /// Set when the run is a Revision of the Task's Work Units rather than its scaffolding.
    pub revision: Option<RevisionInputs<'a>>,
}

/// What a Scaffolder Revision sees on top of its Task.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct RevisionInputs<'a> {
    pub cause: &'a RevisionCause,
    pub task_diff: &'a str,
    /// The Task's Work Units as they stand.
    pub work_units: &'a [WorkUnit],
    /// The Task's earlier Revisions, which this one must not repeat.
    pub earlier: &'a [RevisionRound],
}

/// What the Writer is rendered with: only its Work Unit, never the Conventions.
#[derive(Debug, Clone, Copy)]
pub struct WriterInputs<'a> {
    pub work_unit: &'a WorkUnit,
}

/// What the Scaffolder proposes Checks from when automatic init found none that work.
#[derive(Debug, Clone, Copy)]
pub struct CheckProposalInputs<'a> {
    /// What manifest detection found, as a sentence.
    pub detection: &'a str,
    /// The detected commands that were run and failed, if any.
    pub tried_checks: &'a [TriedCheck],
}

/// A detected Check command that automatic init ran, and its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriedCheck {
    /// `scaffold` or `unit`.
    pub check: String,
    pub command: String,
    /// The trimmed failure output, so the Scaffolder can tell a wrong command from a failing
    /// project.
    pub output: String,
}

#[derive(Debug)]
pub enum PromptError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// The template doesn't parse, lacks a block, or fails to render.
    Invalid {
        template: Template,
        path: PathBuf,
        message: String,
    },
    /// The template uses variables it isn't rendered with.
    UnknownVariables {
        template: Template,
        path: PathBuf,
        variables: Vec<String>,
    },
    /// The instructions block uses a Session input, which would defeat prompt caching.
    NotStatic {
        template: Template,
        path: PathBuf,
    },
}

impl fmt::Display for PromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "could not read {}: {source}", path.display()),
            Self::Invalid {
                template,
                path,
                message,
            } => write!(f, "{template} template {}: {message}", path.display()),
            Self::UnknownVariables {
                template,
                path,
                variables,
            } => write!(
                f,
                "{template} template {} uses unknown variables: {}. It may use: {}",
                path.display(),
                variables.join(", "),
                [template.static_variables(), template.input_variables()]
                    .concat()
                    .join(", ")
            ),
            Self::NotStatic { template, path } => write!(
                f,
                "{template} template {}: the {INSTRUCTIONS_BLOCK} block must not use {}; put Session \
                 inputs in the {INPUTS_BLOCK} block",
                path.display(),
                template.input_variables().join(", ")
            ),
        }
    }
}

impl std::error::Error for PromptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The validated prompt templates.
///
/// A template is Markdown with two `minijinja` blocks: `instructions` (Role, Inputs, Output
/// contract, Rules), which may use only the template's static variables, and `inputs`, which
/// renders the Session inputs. Undefined variables are errors, and every template is checked when it is
/// loaded, so a bad override fails at startup rather than mid-Session.
#[derive(Debug)]
pub struct Prompts {
    env: Environment<'static>,
    /// Where each template came from, for error messages.
    paths: HashMap<Template, PathBuf>,
}

impl Prompts {
    /// The templates compiled into Arbor.
    pub fn builtin() -> Self {
        let mut env = environment();
        let mut paths = HashMap::new();
        for template in Template::ALL {
            let path = builtin_path(template);
            add(&mut env, template, template.builtin_template(), &path)
                .expect("built-in templates are valid");
            paths.insert(template, path);
        }
        Self { env, paths }
    }

    /// The built-in templates, each replaced by `.arbor/prompts/<name>.md` under `project_root`
    /// where that file exists.
    pub fn load(project_root: &Path) -> Result<Self, PromptError> {
        let mut env = environment();
        let mut paths = HashMap::new();
        for template in Template::ALL {
            let path = project_root
                .join(PROMPTS_DIR)
                .join(format!("{}.md", template.name()));
            let path = match fs::read_to_string(&path) {
                Ok(source) => {
                    add(&mut env, template, &source, &path)?;
                    path
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    let path = builtin_path(template);
                    add(&mut env, template, template.builtin_template(), &path)?;
                    path
                }
                Err(source) => return Err(PromptError::Io { path, source }),
            };
            paths.insert(template, path);
        }
        Ok(Self { env, paths })
    }

    pub fn planner(
        &self,
        conventions: &Conventions,
        inputs: &PlannerInputs,
    ) -> Result<Prompt, PromptError> {
        let context = BTreeMap::from([
            ("clarification_mode", mode(inputs.clarification_mode)),
            ("request", Value::from(Serde(inputs.request))),
        ]);
        self.render(Template::Planner, Some(conventions), context)
    }

    pub fn scaffolder(
        &self,
        conventions: &Conventions,
        inputs: &ScaffolderInputs,
    ) -> Result<Prompt, PromptError> {
        let context = BTreeMap::from([
            ("clarification_mode", mode(inputs.clarification_mode)),
            ("cap_tokens", Value::from(inputs.cap_tokens)),
            ("request", Value::from(Serde(inputs.request))),
            ("plan", Value::from(Serde(inputs.plan))),
            ("task", Value::from(Serde(inputs.task))),
            ("revision", Value::from(Serde(inputs.revision))),
        ]);
        self.render(Template::Scaffolder, Some(conventions), context)
    }

    pub fn writer(&self, inputs: &WriterInputs) -> Result<Prompt, PromptError> {
        let context = BTreeMap::from([("work_unit", Value::from(Serde(inputs.work_unit)))]);
        self.render(Template::Writer, None, context)
    }

    pub fn check_proposal(
        &self,
        conventions: &Conventions,
        inputs: &CheckProposalInputs,
    ) -> Result<Prompt, PromptError> {
        let context = BTreeMap::from([
            ("detection", Value::from(inputs.detection)),
            ("tried_checks", tried_checks(inputs.tried_checks)),
        ]);
        self.render(Template::CheckProposal, Some(conventions), context)
    }

    fn render(
        &self,
        template: Template,
        conventions: Option<&Conventions>,
        context: BTreeMap<&str, Value>,
    ) -> Result<Prompt, PromptError> {
        let (instructions, inputs) =
            render_blocks(&self.env, template, context).map_err(|err| PromptError::Invalid {
                template,
                path: self.paths[&template].clone(),
                message: describe(&err),
            })?;
        Ok(Prompt {
            template,
            instructions,
            conventions: conventions
                .filter(|conventions| !conventions.is_empty())
                .map(|conventions| format!("# Conventions\n\n{}", conventions.text())),
            inputs,
        })
    }
}

fn environment() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_syntax(
        SyntaxConfig::builder()
            .trim_blocks(true)
            .lstrip_blocks(true)
            .build()
            .expect("the default delimiters are valid"),
    );
    env
}

fn builtin_path(template: Template) -> PathBuf {
    PathBuf::from(format!("(built-in) {}.md", template.name()))
}

/// Adds the template and checks it: it parses, uses only its own variables, renders
/// both blocks, and its instructions don't change with the Session inputs.
fn add(
    env: &mut Environment<'static>,
    template: Template,
    source: &str,
    path: &Path,
) -> Result<(), PromptError> {
    let template_error = |err: minijinja::Error| PromptError::Invalid {
        template,
        path: path.to_owned(),
        message: describe(&err),
    };
    let source = source.replace("\r\n", "\n");
    env.add_template_owned(template.name(), source)
        .map_err(template_error)?;

    let compiled = env.get_template(template.name()).map_err(template_error)?;
    let globals: Vec<&str> = env.globals().map(|(name, _)| name).collect();
    let known = [template.static_variables(), template.input_variables()].concat();
    let mut unknown: Vec<String> = compiled
        .undeclared_variables(false)
        .into_iter()
        .filter(|name| !known.contains(&name.as_str()) && !globals.contains(&name.as_str()))
        .collect();
    if !unknown.is_empty() {
        unknown.sort();
        return Err(PromptError::UnknownVariables {
            template,
            path: path.to_owned(),
            variables: unknown,
        });
    }

    let (first, _) = render_blocks(env, template, sample(template, 1)).map_err(template_error)?;
    let (second, _) = render_blocks(env, template, sample(template, 2)).map_err(template_error)?;
    if first != second {
        return Err(PromptError::NotStatic {
            template,
            path: path.to_owned(),
        });
    }
    Ok(())
}

/// Renders the instructions and inputs blocks, each trimmed and ending in one newline.
fn render_blocks(
    env: &Environment<'static>,
    template: Template,
    context: BTreeMap<&str, Value>,
) -> Result<(String, String), minijinja::Error> {
    let compiled = env.get_template(template.name())?;
    let mut captured = compiled.render_captured(Value::from(context))?;
    captured.with_state_mut(|state| {
        let instructions = state.render_block(INSTRUCTIONS_BLOCK)?;
        let inputs = state.render_block(INPUTS_BLOCK)?;
        Ok((tidy(&instructions), tidy(&inputs)))
    })
}

fn tidy(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        String::new()
    } else {
        format!("{text}\n")
    }
}

fn mode(mode: ClarificationMode) -> Value {
    Value::from(match mode {
        ClarificationMode::Ask => "ask",
        ClarificationMode::Assume => "assume",
    })
}

fn tried_checks(tried: &[TriedCheck]) -> Value {
    tried
        .iter()
        .map(|tried| {
            Value::from(BTreeMap::from([
                ("check", Value::from(tried.check.as_str())),
                ("command", Value::from(tried.command.as_str())),
                ("output", Value::from(tried.output.as_str())),
            ]))
        })
        .collect()
}

/// Example Session inputs for checking a template. Samples with different `seed`s differ in every
/// input, so instructions that render the same for both don't depend on the inputs.
fn sample(template: Template, seed: u32) -> BTreeMap<&'static str, Value> {
    let text = |what: &str| format!("sample {what} {seed}");
    let request = Request {
        text: text("request"),
        issue: Some(u64::from(seed)),
    };
    let task = Task {
        title: text("task title"),
        description: text("task description"),
    };
    let plan = Plan {
        goal: text("goal"),
        constraints: vec![text("constraint")],
        tasks: vec![task.clone()],
    };
    let work_unit = WorkUnit {
        id: WorkUnitId(seed),
        file: text("file"),
        spec: text("spec"),
        context: vec![Snippet {
            path: text("snippet path"),
            text: text("snippet"),
            priority: Priority::Required,
        }],
        acceptance_criteria: vec![text("criterion")],
        test_filter: text("filter"),
    };
    let tried = [TriedCheck {
        check: text("check"),
        command: text("command"),
        output: text("output"),
    }];
    let cause = RevisionCause::TaskChecksFailed {
        output: text("check output"),
    };
    let earlier = [RevisionRound {
        cause: cause.clone(),
        revision: Revision {
            diagnosis: text("diagnosis"),
            work_units: vec![work_unit.clone()],
            shared_change: None,
        },
        rejection: None,
    }];
    let revision = RevisionInputs {
        cause: &cause,
        task_diff: &text("task diff"),
        work_units: std::slice::from_ref(&work_unit),
        earlier: &earlier,
    };
    let all = [
        ("clarification_mode", mode(ClarificationMode::Ask)),
        ("cap_tokens", Value::from(8000)),
        ("request", Value::from(Serde(&request))),
        ("plan", Value::from(Serde(&plan))),
        ("task", Value::from(Serde(&task))),
        ("work_unit", Value::from(Serde(&work_unit))),
        ("detection", Value::from(text("detection"))),
        ("tried_checks", tried_checks(&tried)),
        ("revision", Value::from(Serde(revision))),
    ];
    let known = [template.static_variables(), template.input_variables()].concat();
    all.into_iter()
        .filter(|(name, _)| known.contains(name))
        .collect()
}

/// A minijinja error with its line number, if any.
fn describe(err: &minijinja::Error) -> String {
    let mut message = err.to_string();
    if let Some(line) = err.line() {
        message.push_str(&format!(" (line {line})"));
    }
    message
}
