//! Edit at the Plan Checkpoint: the TUI steps aside while the Human Operator edits the Plan as TOML
//! in their own editor.

use std::env;
use std::fs;
use std::io;
use std::path::Path;
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
    // A fresh private folder rather than a guessable name in the shared temp folder, which
    // another user could plant as a symlink.
    let plan_dir = match tempfile::Builder::new().prefix("arbor-plan-").tempdir() {
        Ok(plan_dir) => plan_dir,
        Err(err) => return Ok(Err(format!("Could not make a folder for the Plan: {err}"))),
    };
    let plan_path = plan_dir.path().join("plan.toml");
    if let Err(err) = fs::write(&plan_path, plan_file_text(plan)) {
        return Ok(Err(format!(
            "Could not write {}: {err}",
            plan_path.display()
        )));
    }
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    let editor = editor_command();
    let editor_status = editor_invocation(&editor, &plan_path).status();
    *terminal = ratatui::init();
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    terminal.clear()?;
    let edited_text = fs::read_to_string(&plan_path);
    let edit = match (editor_status, edited_text) {
        (Err(err), _) => Err(format!(
            "Could not start the editor `{editor}`: {err}. Set EDITOR to your editor."
        )),
        (Ok(status), _) if !status.success() => Err(format!(
            "The editor `{editor}` exited with {status}; the Plan is unchanged. Check EDITOR, \
             quoting a path with spaces."
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

/// `$VISUAL`, then `$EDITOR`, as a command line such as `code --wait`.
fn editor_command() -> String {
    let configured = ["VISUAL", "EDITOR"]
        .into_iter()
        .filter_map(|name| env::var(name).ok())
        .find(|command| !command.trim().is_empty());
    let fallback = if cfg!(windows) { "notepad" } else { "vi" };
    with_program_quoted(configured.unwrap_or_else(|| fallback.to_owned()).trim())
}

/// Quotes an unquoted program path with spaces, such as `C:\Program Files\Notepad++\notepad++.exe
/// -multiInst`, when a leading part of the command line names an existing file.
fn with_program_quoted(editor: &str) -> String {
    if editor.starts_with('"') {
        return editor.to_owned();
    }
    let program_ends = editor
        .match_indices(' ')
        .map(|(space_at, _)| space_at)
        .chain([editor.len()])
        .rev();
    for program_end in program_ends {
        let (program, arguments) = editor.split_at(program_end);
        if program.contains(' ') && Path::new(program).is_file() {
            return format!("\"{program}\"{arguments}");
        }
    }
    editor.to_owned()
}

/// Runs `editor` through the shell, as git runs `core.editor`, so a quoted program path, its
/// arguments and a `.cmd` editor such as VS Code's `code` all work.
fn editor_invocation(editor: &str, file: &Path) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("cmd");
        // `/S` makes cmd drop only the outer quotes, keeping the quotes inside the command line.
        command.raw_arg(format!("/S /C \"{editor} \"{}\"\"", file.display()));
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("{editor} \"$@\""))
            .arg(editor)
            .arg(file);
        command
    }
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

    /// An editor script in a folder with a space that appends its first argument to the file.
    fn appending_editor(dir: &Path) -> std::path::PathBuf {
        let editors_dir = dir.join("my editors");
        fs::create_dir(&editors_dir).unwrap();
        #[cfg(windows)]
        let script_path = {
            let script_path = editors_dir.join("append arg.cmd");
            fs::write(&script_path, "@echo %~1>>%2\r\n").unwrap();
            script_path
        };
        #[cfg(not(windows))]
        let script_path = {
            use std::os::unix::fs::PermissionsExt;
            let script_path = editors_dir.join("append arg");
            fs::write(&script_path, "#!/bin/sh\necho \"$1\" >> \"$2\"\n").unwrap();
            fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
            script_path
        };
        script_path
    }

    #[test]
    fn an_editor_with_spaces_in_its_path_gets_its_arguments_and_the_plan_file() {
        let dir = tempfile::tempdir().unwrap();
        let editor_path = appending_editor(dir.path());
        let plan_path = dir.path().join("the plan.toml");
        fs::write(&plan_path, "").unwrap();
        let quoted = format!("\"{}\" --wait", editor_path.display());
        let unquoted = format!("{} --wait", editor_path.display());
        assert_eq!(with_program_quoted(&unquoted), quoted);

        for editor in [quoted, with_program_quoted(&unquoted)] {
            let editor_status = editor_invocation(&editor, &plan_path).status().unwrap();
            assert!(editor_status.success(), "{editor}: {editor_status}");
        }
        let edited_text = fs::read_to_string(&plan_path).unwrap();
        assert_eq!(edited_text.lines().collect::<Vec<_>>(), ["--wait", "--wait"]);
    }

    #[test]
    fn an_editor_found_on_the_path_is_left_as_it_is() {
        assert_eq!(with_program_quoted("code --wait"), "code --wait");
        assert_eq!(
            with_program_quoted("\"C:\\Program Files\\Editor\\edit.exe\" -n"),
            "\"C:\\Program Files\\Editor\\edit.exe\" -n"
        );
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
