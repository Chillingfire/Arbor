use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};

use arbor_config::Checks;
use arbor_core::{WorkUnit, WorkUnitId};
use arbor_prompts::estimate_tokens;
use arbor_tools::{Tools, Toolset};

use crate::markers::{MarkerError, Markers};

/// Added to the estimated Writer input because the token estimate is rough.
pub const WRITER_CAP_MARGIN_PERCENT: u64 = 10;

pub const NO_SCAFFOLD_CHECK: &str = "No scaffold check is configured in arbor.toml, so Arbor \
                                     can't verify that the Skeleton builds. Skipping it.";

pub fn writer_input_tokens(unit: &WorkUnit) -> u32 {
    let criteria = unit
        .acceptance_criteria
        .iter()
        .map(|text| estimate_tokens(text));
    let snippets = unit
        .context
        .iter()
        .map(|snippet| estimate_tokens(&snippet.path) + estimate_tokens(&snippet.text));
    let raw: u64 = std::iter::once(estimate_tokens(&unit.spec))
        .chain(criteria)
        .chain(snippets)
        .map(u64::from)
        .sum();
    with_margin(raw)
}

pub(crate) fn with_margin(raw_tokens: u64) -> u32 {
    let tokens = (raw_tokens * (100 + WRITER_CAP_MARGIN_PERCENT)).div_ceil(100);
    u32::try_from(tokens).unwrap_or(u32::MAX)
}

/// `None` if every Work Unit fits the Writer Cap.
pub fn writer_cap_report(units: &[WorkUnit], cap_tokens: u32) -> Option<String> {
    let sizes: Vec<(WorkUnitId, u32)> = units
        .iter()
        .map(|unit| (unit.id, writer_input_tokens(unit)))
        .collect();
    if sizes.iter().all(|&(_, tokens)| tokens <= cap_tokens) {
        return None;
    }
    let cap = thousands(cap_tokens);
    let lines: Vec<String> = sizes
        .into_iter()
        .map(|(id, tokens)| {
            let over = if tokens > cap_tokens {
                " — over, trim or split"
            } else {
                ""
            };
            format!("- {id}: {} / {cap}{over}", thousands(tokens))
        })
        .collect();
    Some(format!(
        "Estimated Writer input per Work Unit against the Writer Cap, in tokens \
         ({WRITER_CAP_MARGIN_PERCENT}% margin included):\n{}",
        lines.join("\n")
    ))
}

pub fn field_problems(units: &[WorkUnit], first_id: WorkUnitId) -> Vec<String> {
    let mut problems = Vec::new();
    if units.is_empty() {
        problems.push("there are no Work Units; submit at least one".to_owned());
    }
    let mut seen = Vec::new();
    for unit in units {
        let id = unit.id;
        if seen.contains(&id) {
            problems.push(format!("{id} is used by more than one Work Unit"));
        }
        seen.push(id);
        if id < first_id {
            problems.push(format!(
                "{id} is taken by an earlier Task; number Work Units from {first_id}"
            ));
        }
        if relative_path(&unit.file).is_none() {
            problems.push(format!(
                "{id}: file {:?} must be a path inside the worktree, relative to its root",
                unit.file
            ));
        }
        if unit.spec.trim().is_empty() {
            problems.push(format!("{id} has no spec"));
        }
        let filter = unit.test_filter.trim();
        if filter.is_empty() {
            problems.push(format!("{id} has no test filter"));
        } else if filter.starts_with('-') {
            problems.push(format!("{id}: a test filter may not start with -"));
        }
    }
    problems
}

/// A Region for a Work Unit not in `ids` is a problem too.
pub fn marker_problems(file: &str, text: &str, ids: &[WorkUnitId]) -> Vec<String> {
    let markers = Markers::parse(text);
    let mut problems: Vec<String> = markers
        .check(ids)
        .iter()
        .map(|error: &MarkerError| format!("{file}: {error}"))
        .collect();
    let mut strays: Vec<WorkUnitId> = markers
        .regions()
        .iter()
        .map(|region| region.id)
        .filter(|id| !ids.contains(id))
        .collect();
    strays.sort_unstable();
    strays.dedup();
    problems.extend(strays.into_iter().map(|id| {
        format!("{file}: {id} has a Region here, but no submitted Work Unit names this file")
    }));
    problems
}

pub struct ScaffoldCheck {
    worktree: PathBuf,
    checks: Checks,
    tools: Tools,
    cap_tokens: u32,
    first_id: WorkUnitId,
}

impl ScaffoldCheck {
    pub fn new(
        worktree: impl Into<PathBuf>,
        checks: Checks,
        cap_tokens: u32,
        first_id: WorkUnitId,
    ) -> Self {
        let worktree = worktree.into();
        let tools = Tools::new(&worktree, Toolset::Scaffolder, checks.clone());
        Self {
            worktree,
            checks,
            tools,
            cap_tokens,
            first_id,
        }
    }

    pub fn warning(&self) -> Option<&'static str> {
        self.checks.scaffold.is_none().then_some(NO_SCAFFOLD_CHECK)
    }

    pub fn review(&self, units: &[WorkUnit]) -> Result<(), String> {
        let mut problems = field_problems(units, self.first_id);
        for (file, ids) in by_file(units) {
            let Some(path) = relative_path(file) else {
                continue;
            };
            match std::fs::read_to_string(self.worktree.join(path)) {
                Ok(text) => problems.extend(marker_problems(file, &text, &ids)),
                Err(err) if err.kind() == io::ErrorKind::NotFound => problems.push(format!(
                    "{file} doesn't exist; write the Skeleton there before submitting"
                )),
                Err(err) => problems.push(format!("{file} can't be read: {err}")),
            }
        }
        let report = writer_cap_report(units, self.cap_tokens);
        if problems.is_empty() && report.is_none() {
            return Ok(());
        }
        let mut message = String::from("the Work Units were not accepted.");
        if !problems.is_empty() {
            message.push_str("\n\n");
            message.push_str(&bullets(&problems));
        }
        if let Some(report) = report {
            message.push_str("\n\n");
            message.push_str(&report);
        }
        Err(message)
    }

    pub async fn run(&self, units: &[WorkUnit]) -> Result<(), String> {
        self.review(units)?;
        if self.checks.scaffold.is_none() {
            return Ok(());
        }
        let output = self
            .tools
            .call("run_checks", r#"{"check":"scaffold"}"#)
            .await;
        if output.is_error {
            Err(format!(
                "the Scaffold Check failed, so the Skeleton doesn't build. Fix it, then submit \
                 again.\n\n{}",
                output.text
            ))
        } else {
            Ok(())
        }
    }
}

fn by_file(units: &[WorkUnit]) -> BTreeMap<&str, Vec<WorkUnitId>> {
    let mut files: BTreeMap<&str, Vec<WorkUnitId>> = BTreeMap::new();
    for unit in units {
        files.entry(unit.file.as_str()).or_default().push(unit.id);
    }
    files
}

fn relative_path(file: &str) -> Option<&Path> {
    let path = Path::new(file);
    let inside = !file.trim().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    inside.then_some(path)
}

fn bullets(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| format!("- {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}
