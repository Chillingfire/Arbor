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

/// Where a project overrides the built-in templates, relative to the repository root. Each Tier
/// reads `<tier>.md` from here if it exists.
pub const PROMPTS_DIR: &str = ".arbor/prompts";

/// The static part of the prompt: Role, Inputs, Output contract and Rules.
const INSTRUCTIONS_BLOCK: &str = "instructions";
/// The Session inputs, rendered after the Conventions.
const INPUTS_BLOCK: &str = "inputs";

/// One of the three roles a model can fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    Planner,
    Scaffolder,
    Writer,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Planner, Tier::Scaffolder, Tier::Writer];

    /// The lowercase name used for template and prompt files, e.g. `planner`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Planner => "planner",
            Self::Scaffolder => "scaffolder",
            Self::Writer => "writer",
        }
    }

    fn builtin_template(self) -> &'static str {
        match self {
            Self::Planner => include_str!("../templates/planner.md"),
            Self::Scaffolder => include_str!("../templates/scaffolder.md"),
            Self::Writer => include_str!("../templates/writer.md"),
        }
    }

    /// Variables the instructions may use. They stay the same for a whole Session.
    fn static_variables(self) -> &'static [&'static str] {
        match self {
            Self::Planner => &["clarification_mode"],
            Self::Scaffolder => &["clarification_mode", "cap_tokens"],
            Self::Writer => &[],
        }
    }

    /// Variables only the inputs may use. They change from call to call.
    fn input_variables(self) -> &'static [&'static str] {
        match self {
            Self::Planner => &["request"],
            Self::Scaffolder => &["request", "plan", "task", "revision"],
            Self::Writer => &["work_unit"],
        }
    }
}

impl fmt::Display for Tier {
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

#[derive(Debug)]
pub enum PromptError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// The template doesn't parse, lacks a block, or fails to render.
    Template {
        tier: Tier,
        path: PathBuf,
        message: String,
    },
    /// The template uses variables its Tier doesn't have.
    UnknownVariables {
        tier: Tier,
        path: PathBuf,
        variables: Vec<String>,
    },
    /// The instructions block uses a Session input, which would defeat prompt caching.
    NotStatic {
        tier: Tier,
        path: PathBuf,
    },
}

impl fmt::Display for PromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "could not read {}: {source}", path.display()),
            Self::Template {
                tier,
                path,
                message,
            } => write!(f, "{tier} template {}: {message}", path.display()),
            Self::UnknownVariables {
                tier,
                path,
                variables,
            } => write!(
                f,
                "{tier} template {} uses unknown variables: {}. It may use: {}",
                path.display(),
                variables.join(", "),
                [tier.static_variables(), tier.input_variables()]
                    .concat()
                    .join(", ")
            ),
            Self::NotStatic { tier, path } => write!(
                f,
                "{tier} template {}: the {INSTRUCTIONS_BLOCK} block must not use {}; put Session \
                 inputs in the {INPUTS_BLOCK} block",
                path.display(),
                tier.input_variables().join(", ")
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

/// The validated prompt templates for every Tier.
///
/// A template is Markdown with two `minijinja` blocks: `instructions` (Role, Inputs, Output
/// contract, Rules), which may use only the Tier's static variables, and `inputs`, which renders
/// the Session inputs. Undefined variables are errors, and every template is checked when it is
/// loaded, so a bad override fails at startup rather than mid-Session.
#[derive(Debug)]
pub struct Prompts {
    env: Environment<'static>,
    /// Where each Tier's template came from, for error messages.
    paths: HashMap<Tier, PathBuf>,
}

impl Prompts {
    /// The templates compiled into Arbor.
    pub fn builtin() -> Self {
        let mut env = environment();
        let mut paths = HashMap::new();
        for tier in Tier::ALL {
            let path = builtin_path(tier);
            add(&mut env, tier, tier.builtin_template(), &path)
                .expect("built-in templates are valid");
            paths.insert(tier, path);
        }
        Self { env, paths }
    }

    /// The built-in templates, each replaced by `.arbor/prompts/<tier>.md` under `project_root`
    /// where that file exists.
    pub fn load(project_root: &Path) -> Result<Self, PromptError> {
        let mut env = environment();
        let mut paths = HashMap::new();
        for tier in Tier::ALL {
            let path = project_root
                .join(PROMPTS_DIR)
                .join(format!("{}.md", tier.name()));
            let path = match fs::read_to_string(&path) {
                Ok(source) => {
                    add(&mut env, tier, &source, &path)?;
                    path
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    let path = builtin_path(tier);
                    add(&mut env, tier, tier.builtin_template(), &path)?;
                    path
                }
                Err(source) => return Err(PromptError::Io { path, source }),
            };
            paths.insert(tier, path);
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
        self.render(Tier::Planner, Some(conventions), context)
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
        self.render(Tier::Scaffolder, Some(conventions), context)
    }

    pub fn writer(&self, inputs: &WriterInputs) -> Result<Prompt, PromptError> {
        let context = BTreeMap::from([("work_unit", Value::from(Serde(inputs.work_unit)))]);
        self.render(Tier::Writer, None, context)
    }

    fn render(
        &self,
        tier: Tier,
        conventions: Option<&Conventions>,
        context: BTreeMap<&str, Value>,
    ) -> Result<Prompt, PromptError> {
        let (instructions, inputs) =
            render_blocks(&self.env, tier, context).map_err(|err| PromptError::Template {
                tier,
                path: self.paths[&tier].clone(),
                message: describe(&err),
            })?;
        Ok(Prompt {
            tier,
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

fn builtin_path(tier: Tier) -> PathBuf {
    PathBuf::from(format!("(built-in) {}.md", tier.name()))
}

/// Adds the Tier's template and checks it: it parses, uses only its Tier's variables, renders
/// both blocks, and its instructions don't change with the Session inputs.
fn add(
    env: &mut Environment<'static>,
    tier: Tier,
    source: &str,
    path: &Path,
) -> Result<(), PromptError> {
    let template_error = |err: minijinja::Error| PromptError::Template {
        tier,
        path: path.to_owned(),
        message: describe(&err),
    };
    let source = source.replace("\r\n", "\n");
    env.add_template_owned(tier.name(), source)
        .map_err(template_error)?;

    let template = env.get_template(tier.name()).map_err(template_error)?;
    let globals: Vec<&str> = env.globals().map(|(name, _)| name).collect();
    let known = [tier.static_variables(), tier.input_variables()].concat();
    let mut unknown: Vec<String> = template
        .undeclared_variables(false)
        .into_iter()
        .filter(|name| !known.contains(&name.as_str()) && !globals.contains(&name.as_str()))
        .collect();
    if !unknown.is_empty() {
        unknown.sort();
        return Err(PromptError::UnknownVariables {
            tier,
            path: path.to_owned(),
            variables: unknown,
        });
    }

    let (first, _) = render_blocks(env, tier, sample(tier, 1)).map_err(template_error)?;
    let (second, _) = render_blocks(env, tier, sample(tier, 2)).map_err(template_error)?;
    if first != second {
        return Err(PromptError::NotStatic {
            tier,
            path: path.to_owned(),
        });
    }
    Ok(())
}

/// Renders the instructions and inputs blocks, each trimmed and ending in one newline.
fn render_blocks(
    env: &Environment<'static>,
    tier: Tier,
    context: BTreeMap<&str, Value>,
) -> Result<(String, String), minijinja::Error> {
    let template = env.get_template(tier.name())?;
    let mut captured = template.render_captured(Value::from(context))?;
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

/// Example Session inputs for checking a template. Samples with different `seed`s differ in every
/// input, so instructions that render the same for both don't depend on the inputs.
fn sample(tier: Tier, seed: u32) -> BTreeMap<&'static str, Value> {
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
    let cause = RevisionCause::TaskChecksFailed {
        output: text("check output"),
    };
    let earlier = [RevisionRound {
        cause: cause.clone(),
        revision: Revision {
            diagnosis: text("diagnosis"),
            work_units: vec![work_unit.clone()],
        },
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
        ("revision", Value::from(Serde(revision))),
    ];
    let known = [tier.static_variables(), tier.input_variables()].concat();
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
