//! Edit at the Plan Checkpoint: the TUI steps aside while the Human Operator edits the Plan as TOML
//! in their own editor.

use std::env;
use std::fs;
use std::io;
use std::process::Command;

use arbor_core::Plan;
use arbor_tiers::validate_plan;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use ratatui::crossterm::execute;

const PLAN_FILE_HEADER: &str = "\
# Edit the Plan, then save and close the editor to go back to the Plan Checkpoint.
# Constraints starting with \"Assumption:\" are the Planner's recorded assumptions.

";

/// The Plan as the Human Operator left it, `Ok(None)` when they changed nothing, or why the edit
/// can't be used.
pub type PlanEdit = Result<Option<Plan>, String>;

/// Suspends the TUI, opens the Plan in `$VISUAL` or `$EDITOR`, and brings the TUI back once the
/// editor exits. `Err` only when the terminal can't be restored.
pub fn edit_plan(terminal: &mut DefaultTerminal, plan: &Plan) -> io::Result<PlanEdit> {
    let plan_path = env::temp_dir().join(format!("arbor-plan-{}.toml", std::process::id()));
    if let Err(err) = fs::write(&plan_path, plan_file_text(plan)) {
        return Ok(Err(format!(
            "Could not write {}: {err}",
            plan_path.display()
        )));
    }
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    let editor = editor_command();
    let editor_status = Command::new(&editor[0])
        .args(&editor[1..])
        .arg(&plan_path)
        .status();
    *terminal = ratatui::init();
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    terminal.clear()?;
    let edited_text = fs::read_to_string(&plan_path);
    let _ = fs::remove_file(&plan_path);
    let edit = match (editor_status, edited_text) {
        (Err(err), _) => Err(format!(
            "Could not start the editor `{}`: {err}. Set EDITOR to your editor.",
            editor.join(" ")
        )),
        (Ok(status), _) if !status.success() => Err(format!(
            "The editor exited with {status}; the Plan is unchanged."
        )),
        (Ok(_), Err(err)) => Err(format!("Could not read the edited Plan: {err}")),
        (Ok(_), Ok(edited_text)) => {
            parse_plan_file(&edited_text).map(|edited| (edited != *plan).then_some(edited))
        }
    };
    Ok(edit)
}

fn plan_file_text(plan: &Plan) -> String {
    let plan_toml = toml::to_string_pretty(plan).unwrap_or_default();
    format!("{PLAN_FILE_HEADER}{plan_toml}")
}

fn parse_plan_file(edited_text: &str) -> Result<Plan, String> {
    let plan: Plan = toml::from_str(edited_text)
        .map_err(|err| format!("The edited Plan isn't valid TOML: {err}"))?;
    validate_plan(&plan)
        .map_err(|problems| format!("The edited Plan can't be used: {problems}"))?;
    Ok(plan)
}

/// `$VISUAL`, then `$EDITOR`, split into the program and its arguments, e.g. `code --wait`.
fn editor_command() -> Vec<String> {
    let configured = ["VISUAL", "EDITOR"]
        .into_iter()
        .filter_map(|name| env::var(name).ok())
        .find(|command| !command.trim().is_empty());
    let fallback = if cfg!(windows) { "notepad" } else { "vi" };
    configured
        .unwrap_or_else(|| fallback.to_owned())
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbor_core::Task;

    fn plan() -> Plan {
        Plan {
            goal: "wordy can find the longest word".to_owned(),
            constraints: vec!["Assumption: ties keep the first word".to_owned()],
            tasks: vec![Task {
                title: "Add longest_word".to_owned(),
                description: "Add `longest_word` to src/lib.rs with its tests.".to_owned(),
            }],
        }
    }

    #[test]
    fn the_plan_file_reads_back_as_the_same_plan() {
        let plan_text = plan_file_text(&plan());
        assert!(plan_text.starts_with("# Edit the Plan"), "{plan_text}");
        assert_eq!(parse_plan_file(&plan_text), Ok(plan()));
    }

    #[test]
    fn an_edited_plan_with_no_tasks_is_refused() {
        let plan_text = plan_file_text(&plan());
        let without_tasks = plan_text.split("[[tasks]]").next().unwrap();
        let refused = parse_plan_file(without_tasks).unwrap_err();
        assert!(refused.contains("isn't valid TOML"), "{refused}");

        let emptied = format!("{without_tasks}tasks = []\n");
        let refused = parse_plan_file(&emptied).unwrap_err();
        assert!(refused.contains("can't be used"), "{refused}");
    }
}
