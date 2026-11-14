use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};

use arbor_config::Checks;
use arbor_core::{WorkUnit, WorkUnitId};
use arbor_git::Repo;
use arbor_prompts::estimate_tokens;
use arbor_tools::{Tools, Toolset};

use crate::markers::{MarkerError, Markers, is_comment_less, is_marker_line};

/// Added to the estimated Writer input because the token estimate is rough.
pub const WRITER_CAP_MARGIN_PERCENT: u64 = 10;

pub const NO_SCAFFOLD_CHECK: &str = "No scaffold check is configured in arbor.toml, so Arbor \
                                     can't verify that the Skeleton builds. Skipping it.";

pub fn writer_input_tokens(unit: &WorkUnit) -> u32 {
    let criteria = unit
        .acceptance_criteria
        .iter()
        .map(|criterion| estimate_tokens(criterion));
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
    revision_field_problems(units, first_id, &[])
}

/// Like [`field_problems`], but a Work Unit may also keep one of the `revisable` IDs.
pub fn revision_field_problems(
    units: &[WorkUnit],
    first_id: WorkUnitId,
    revisable: &[WorkUnitId],
) -> Vec<String> {
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
        if id < first_id && !revisable.contains(&id) {
            problems.push(id_taken(id, first_id, revisable));
        }
        if relative_path(&unit.file).is_none() {
            problems.push(format!(
                "{id}: file {:?} must be a path inside the worktree, relative to its root",
                unit.file
            ));
        }
        if is_comment_less(&unit.file) {
            problems.push(format!(
                "{id}: {} can't hold a Marker because its format has no comments. Write that \
                 file yourself as part of the Skeleton and don't make it a Work Unit",
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

fn id_taken(id: WorkUnitId, first_id: WorkUnitId, revisable: &[WorkUnitId]) -> String {
    if revisable.is_empty() {
        format!("{id} is taken by an earlier Task; number Work Units from {first_id}")
    } else {
        format!(
            "{id} is not one of this Task's Work Units; keep a revised Work Unit's ID and number \
             new ones from {first_id}"
        )
    }
}

/// A Region for a Work Unit not in `ids` is a problem too.
pub fn marker_problems(file: &str, source: &str, ids: &[WorkUnitId]) -> Vec<String> {
    marker_problems_keeping(file, source, ids, &[])
}

/// `kept` Regions belong to Work Units still waiting for their Writer, outside the Revision.
fn marker_problems_keeping(
    file: &str,
    source: &str,
    ids: &[WorkUnitId],
    kept: &[WorkUnitId],
) -> Vec<String> {
    let markers = Markers::parse(source);
    let mut problems: Vec<String> = markers
        .check(ids)
        .iter()
        .map(|error: &MarkerError| format!("{file}: {error}"))
        .collect();
    let mut strays: Vec<WorkUnitId> = markers
        .regions()
        .iter()
        .map(|region| region.id)
        .filter(|id| !ids.contains(id) && !kept.contains(id))
        .collect();
    strays.sort_unstable();
    strays.dedup();
    problems.extend(strays.into_iter().map(|id| {
        format!("{file}: {id} has a Region here, but no submitted Work Unit names this file")
    }));
    problems
}

/// Marker lines in the changed files whose format has no comments, where a Marker would change
/// what the file means. Marker text inside a string value is data, e.g. a recorded prompt, so it
/// doesn't count.
pub fn comment_less_marker_problems(
    changed_files: &[String],
    read_file: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for file in changed_files.iter().filter(|file| is_comment_less(file)) {
        let Some(file_text) = read_file(file) else {
            continue;
        };
        problems.extend(
            file_text
                .lines()
                .enumerate()
                .filter(|(_, line)| is_marker_line(line) && !is_inside_string(line))
                .map(|(index, _)| {
                    format!(
                        "{file}: line {} holds a Marker, but this format has no comments. Remove \
                         it and write the change yourself",
                        index + 1
                    )
                }),
        );
    }
    problems
}

/// Whether the line's Marker text sits inside a `"…"` string, i.e. it is data such as a recorded
/// prompt, not a stray Marker. A stray one is a bare line like `// ARBOR:BEGIN WU-7`.
fn is_inside_string(line: &str) -> bool {
    let Some(marker_start) = line.find("ARBOR:") else {
        return false;
    };
    let mut inside = false;
    let mut escaped = false;
    for line_char in line[..marker_start].chars() {
        match line_char {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => inside = !inside,
            _ => {}
        }
    }
    inside
}

pub struct ScaffoldCheck {
    worktree: PathBuf,
    checks: Checks,
    tools: Tools,
    cap_tokens: u32,
    first_id: WorkUnitId,
    revisable: Vec<WorkUnitId>,
    kept_regions: Vec<WorkUnitId>,
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
            revisable: Vec::new(),
            kept_regions: Vec::new(),
        }
    }

    /// Checks a Revision, whose Work Units may also keep the IDs of the Task's own.
    pub fn revising(mut self, revisable: Vec<WorkUnitId>) -> Self {
        self.revisable = revisable;
        self
    }

    /// Lets these Work Units keep their Regions without being submitted, e.g. the ones still
    /// waiting for a Writer when a single Work Unit is revised.
    pub fn keeping_regions(mut self, kept_regions: Vec<WorkUnitId>) -> Self {
        self.kept_regions = kept_regions;
        self
    }

    pub fn warning(&self) -> Option<&'static str> {
        self.checks.scaffold.is_none().then_some(NO_SCAFFOLD_CHECK)
    }

    pub fn review(&self, units: &[WorkUnit]) -> Result<(), String> {
        let mut problems = revision_field_problems(units, self.first_id, &self.revisable);
        for (file, ids) in by_file(units) {
            let Some(path) = relative_path(file) else {
                continue;
            };
            match std::fs::read_to_string(self.worktree.join(path)) {
                Ok(source) => problems.extend(marker_problems_keeping(
                    file,
                    &source,
                    &ids,
                    &self.kept_regions,
                )),
                Err(err) if err.kind() == io::ErrorKind::NotFound => problems.push(format!(
                    "{file} doesn't exist; write the Skeleton there before submitting"
                )),
                Err(err) => problems.push(format!("{file} can't be read: {err}")),
            }
        }
        problems.extend(self.stray_comment_less_markers());
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

    /// The Skeleton isn't committed until it is approved, so the files to scan are the
    /// worktree's uncommitted ones.
    fn stray_comment_less_markers(&self) -> Vec<String> {
        let changed_files =
            match Repo::open(&self.worktree).and_then(|repo| repo.uncommitted_changes()) {
                Ok(changed_files) => changed_files,
                Err(err) => {
                    return vec![format!(
                        "the worktree's changed files can't be listed: {err}"
                    )];
                }
            };
        comment_less_marker_problems(&changed_files, |file| {
            std::fs::read_to_string(self.worktree.join(file)).ok()
        })
    }

    pub async fn run(&self, units: &[WorkUnit]) -> Result<(), String> {
        self.review(units)?;
        if self.checks.scaffold.is_none() {
            return Ok(());
        }
        let scaffold_check = self
            .tools
            .call("run_checks", r#"{"check":"scaffold"}"#)
            .await;
        if scaffold_check.is_error {
            Err(format!(
                "the Scaffold Check failed, so the Skeleton doesn't build. Fix it, then submit \
                 again.\n\n{}",
                scaffold_check.text
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

fn thousands(number: u32) -> String {
    let digits = number.to_string();
    let mut grouped = String::new();
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}
