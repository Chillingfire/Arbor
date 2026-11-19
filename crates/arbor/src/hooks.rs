//! The git hooks `arbor init` installs as a second guard against Markers, for git commands run
//! outside Arbor (ADR-0002). Arbor's own commits run them too, so a Session worktree's commits
//! are let through.

use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use arbor_git::Repo;

/// Marks a hook as Arbor's own, so it is never mistaken for one the operator wrote.
const HEADER: &str = "# Installed by `arbor init` to keep Markers out of commits and pushes.";

/// Lists, in the shared git directory, the hooks Arbor installed, so a missing one is noticed.
const RECORD_FILE: &str = "arbor-hooks";

/// A hook fails closed when it can't run the check, so a missing `arbor` never lets Markers by.
const REQUIRE_ARBOR: &str = "if ! command -v arbor >/dev/null 2>&1; then
  echo \"arbor is not on PATH, so this hook can't check for Markers.\" >&2
  exit 1
fi
";

const STAGED_CHECK: &str = "arbor check-markers --staged --allow-session-worktree || exit 1\n";

/// A fast-forward merge makes no commit, so the push is where its Markers are caught.
const PUSH_CHECK: &str = "while read -r local_ref local_oid remote_ref remote_oid; do
  case \"$local_oid\" in
    *[!0]*) arbor check-markers --rev \"$local_oid\" || exit 1 ;;
  esac
done
";

pub struct MarkerHook {
    pub name: &'static str,
    /// What the hook runs, and what to add by hand to a hook Arbor won't touch.
    pub check_lines: &'static str,
}

pub const MARKER_HOOKS: [MarkerHook; 3] = [
    MarkerHook {
        name: "pre-commit",
        check_lines: STAGED_CHECK,
    },
    MarkerHook {
        name: "pre-merge-commit",
        check_lines: STAGED_CHECK,
    },
    MarkerHook {
        name: "pre-push",
        check_lines: PUSH_CHECK,
    },
];

impl MarkerHook {
    pub fn script(&self) -> String {
        format!("#!/bin/sh\n{HEADER}\n{REQUIRE_ARBOR}{}", self.check_lines)
    }
}

/// What [`install`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Installation {
    /// `core.hooksPath` is set, so nothing was written; the operator adds the checks there.
    HooksPathConfigured { hooks_path: String },
    Done {
        /// Arbor's hooks, new or already there.
        installed: Vec<&'static str>,
        /// The operator's own hooks of the same name, left as they are.
        kept: Vec<&'static str>,
    },
}

/// Installs each Marker hook that doesn't exist yet. An existing hook is never overwritten.
pub fn install(repo: &Repo) -> Result<Installation, String> {
    if let Some(hooks_path) = repo.configured_hooks_path() {
        return Ok(Installation::HooksPathConfigured { hooks_path });
    }
    let hooks_dir = default_hooks_dir(repo)?;
    let mut installed = Vec::new();
    let mut kept = Vec::new();
    for hook in &MARKER_HOOKS {
        let hook_path = hooks_dir.join(hook.name);
        match fs::read_to_string(&hook_path) {
            Ok(existing) if existing == hook.script() => installed.push(hook.name),
            Ok(_) => kept.push(hook.name),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                write_hook(&hook_path, hook)?;
                installed.push(hook.name);
            }
            Err(err) => return Err(format!("{}: {err}", hook_path.display())),
        }
    }
    record(repo, &installed)?;
    Ok(Installation::Done { installed, kept })
}

/// What `arbor init` tells the operator about the hooks.
pub fn installation_notice(installation: &Installation) -> String {
    match installation {
        Installation::HooksPathConfigured { hooks_path } => {
            let mut notice = format!(
                "core.hooksPath is set to {hooks_path}, so Arbor installed no hooks. Add these \
                 lines to the hooks there:\n"
            );
            notice.push_str(&check_lines_of(&MARKER_HOOKS.iter().collect::<Vec<_>>()));
            notice
        }
        Installation::Done { installed, kept } => {
            let mut notice = String::new();
            if !installed.is_empty() {
                notice.push_str(&format!(
                    "Installed the {} hooks, which refuse commits and pushes that hold Markers.\n",
                    installed.join(", ")
                ));
            }
            if !kept.is_empty() {
                let kept_hooks: Vec<&MarkerHook> = MARKER_HOOKS
                    .iter()
                    .filter(|hook| kept.contains(&hook.name))
                    .collect();
                notice.push_str(&format!(
                    "Left your own {} hooks as they are. Add these lines to them, near the top:\n",
                    kept.join(", ")
                ));
                notice.push_str(&check_lines_of(&kept_hooks));
            }
            notice
        }
    }
}

fn check_lines_of(hooks: &[&MarkerHook]) -> String {
    hooks
        .iter()
        .map(|hook| {
            let indented: String = hook
                .check_lines
                .lines()
                .map(|check_line| format!("    {check_line}\n"))
                .collect();
            format!("  {}:\n{indented}", hook.name)
        })
        .collect()
}

/// Why a hook Arbor installed no longer guards against Markers.
#[derive(Debug, PartialEq, Eq)]
pub enum HookProblem {
    Missing(&'static str),
    Edited(&'static str),
    /// `core.hooksPath` was set after the hooks were installed, so git no longer runs them.
    NotRun {
        hooks_path: String,
    },
}

impl HookProblem {
    fn describe(&self) -> String {
        match self {
            Self::Missing(name) => format!("the {name} hook is missing"),
            Self::Edited(name) => format!("the {name} hook was changed"),
            Self::NotRun { hooks_path } => format!(
                "core.hooksPath is now {hooks_path}, so git doesn't run the hooks Arbor installed"
            ),
        }
    }

    fn can_reinstall(&self) -> bool {
        !matches!(self, Self::NotRun { .. })
    }
}

/// What is wrong with the hooks Arbor installed. Empty when they are intact or were never
/// installed.
pub fn problems(repo: &Repo) -> Result<Vec<HookProblem>, String> {
    let recorded = recorded(repo)?;
    if recorded.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(hooks_path) = repo.configured_hooks_path() {
        return Ok(vec![HookProblem::NotRun { hooks_path }]);
    }
    let hooks_dir = default_hooks_dir(repo)?;
    let mut hook_problems = Vec::new();
    for hook in MARKER_HOOKS
        .iter()
        .filter(|hook| recorded.iter().any(|name| name == hook.name))
    {
        match fs::read_to_string(hooks_dir.join(hook.name)) {
            Ok(existing) if existing == hook.script() => {}
            Ok(_) => hook_problems.push(HookProblem::Edited(hook.name)),
            Err(_) => hook_problems.push(HookProblem::Missing(hook.name)),
        }
    }
    Ok(hook_problems)
}

/// Writes Arbor's hooks again over the ones in `hook_problems`. The operator asked for it. An
/// edited hook is first copied aside, and the copies' paths are returned.
pub fn reinstall(repo: &Repo, hook_problems: &[HookProblem]) -> Result<Vec<PathBuf>, String> {
    let hooks_dir = default_hooks_dir(repo)?;
    let mut kept_copies = Vec::new();
    for hook_problem in hook_problems {
        let (HookProblem::Missing(name) | HookProblem::Edited(name)) = hook_problem else {
            continue;
        };
        let Some(hook) = MARKER_HOOKS.iter().find(|hook| hook.name == *name) else {
            continue;
        };
        let hook_path = hooks_dir.join(hook.name);
        if matches!(hook_problem, HookProblem::Edited(_)) {
            kept_copies.push(keep_copy(&hook_path)?);
        }
        write_hook(&hook_path, hook)?;
    }
    Ok(kept_copies)
}

/// Copies an edited hook to `<name>.arbor-replaced`, or a numbered name if that is taken, so
/// the operator's changes survive the reinstall.
fn keep_copy(hook_path: &Path) -> Result<PathBuf, String> {
    let failed = |err: io::Error| format!("{}: {err}", hook_path.display());
    let file_name = hook_path.file_name().unwrap_or_default().to_string_lossy();
    let copy_path = (0..)
        .map(|attempt| {
            let suffix = if attempt == 0 {
                String::new()
            } else {
                format!(".{attempt}")
            };
            hook_path.with_file_name(format!("{file_name}.arbor-replaced{suffix}"))
        })
        .find(|copy_path| !copy_path.exists())
        .unwrap_or_default();
    fs::copy(hook_path, &copy_path).map_err(failed)?;
    Ok(copy_path)
}

/// Replacing a hook the operator changed needs a yes typed out; a missing one is reinstalled on
/// Enter.
fn reinstall_question(hook_problems: &[HookProblem]) -> (&'static str, bool) {
    if hook_problems
        .iter()
        .any(|hook_problem| matches!(hook_problem, HookProblem::Edited(_)))
    {
        (
            "Reinstall them? A changed hook is kept as <name>.arbor-replaced. [y/N] ",
            false,
        )
    } else {
        ("Reinstall them? [Y/n] ", true)
    }
}

/// Warns about broken hooks on stderr and, at a terminal, offers to reinstall them.
pub fn check_health(repo: &Repo) {
    let hook_problems = match problems(repo) {
        Ok(hook_problems) => hook_problems,
        Err(err) => {
            eprintln!("arbor: could not check the git hooks: {err}");
            return;
        }
    };
    if hook_problems.is_empty() {
        return;
    }
    eprintln!("arbor: WARNING: the git hooks that refuse Markers no longer guard this repository:");
    for hook_problem in &hook_problems {
        eprintln!("  {}", hook_problem.describe());
    }
    if !hook_problems.iter().any(HookProblem::can_reinstall) || !io::stdin().is_terminal() {
        return;
    }
    let (question, default_yes) = reinstall_question(&hook_problems);
    if !ask(question, default_yes) {
        return;
    }
    match reinstall(repo, &hook_problems) {
        Ok(kept_copies) => {
            eprintln!("arbor: reinstalled the hooks.");
            for kept_copy in kept_copies {
                eprintln!(
                    "arbor: your changed hook is kept as {}",
                    kept_copy.display()
                );
            }
        }
        Err(err) => eprintln!("arbor: could not reinstall the hooks: {err}"),
    }
}

/// Reads a yes or no from stdin. An empty answer, or none at all, is `default_yes`.
pub fn ask(question: &str, default_yes: bool) -> bool {
    eprint!("{question}");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    if io::stdin().lock().read_line(&mut answer).is_err() {
        return default_yes;
    }
    answer_means_yes(&answer, default_yes)
}

fn answer_means_yes(answer: &str, default_yes: bool) -> bool {
    match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default_yes,
    }
}

fn default_hooks_dir(repo: &Repo) -> Result<PathBuf, String> {
    let common_dir = repo.common_dir().map_err(|err| err.to_string())?;
    Ok(common_dir.join("hooks"))
}

fn write_hook(hook_path: &Path, hook: &MarkerHook) -> Result<(), String> {
    let failed = |err: io::Error| format!("{}: {err}", hook_path.display());
    if let Some(hooks_dir) = hook_path.parent() {
        fs::create_dir_all(hooks_dir).map_err(failed)?;
    }
    fs::write(hook_path, hook.script()).map_err(failed)?;
    make_executable(hook_path).map_err(failed)
}

#[cfg(unix)]
fn make_executable(hook_path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(hook_path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_hook_path: &Path) -> io::Result<()> {
    Ok(())
}

fn record_path(repo: &Repo) -> Result<PathBuf, String> {
    let common_dir = repo.common_dir().map_err(|err| err.to_string())?;
    Ok(common_dir.join(RECORD_FILE))
}

fn recorded(repo: &Repo) -> Result<Vec<String>, String> {
    match fs::read_to_string(record_path(repo)?) {
        Ok(record_text) => Ok(record_text.lines().map(str::to_owned).collect()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err.to_string()),
    }
}

fn record(repo: &Repo, installed: &[&str]) -> Result<(), String> {
    let record_path = record_path(repo)?;
    if installed.is_empty() {
        return Ok(());
    }
    let record_text: String = installed.iter().map(|name| format!("{name}\n")).collect();
    fs::write(&record_path, record_text).map_err(|err| format!("{}: {err}", record_path.display()))
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use tempfile::{TempDir, tempdir};

    use super::*;

    fn repository() -> (TempDir, Repo) {
        let dir = tempdir().unwrap();
        let init = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(init.success());
        let repo = Repo::open(dir.path()).unwrap();
        (dir, repo)
    }

    fn hook_path(repo: &Repo, name: &str) -> PathBuf {
        default_hooks_dir(repo).unwrap().join(name)
    }

    #[test]
    fn hooks_never_installed_have_no_problems() {
        let (_dir, repo) = repository();
        assert_eq!(problems(&repo).unwrap(), []);
    }

    #[test]
    fn a_missing_or_edited_hook_is_noticed_and_reinstalled() {
        let (_dir, repo) = repository();
        install(&repo).unwrap();
        assert_eq!(problems(&repo).unwrap(), []);
        fs::remove_file(hook_path(&repo, "pre-push")).unwrap();
        fs::write(hook_path(&repo, "pre-commit"), "#!/bin/sh\nexit 0\n").unwrap();

        let hook_problems = problems(&repo).unwrap();

        assert_eq!(
            hook_problems,
            [
                HookProblem::Edited("pre-commit"),
                HookProblem::Missing("pre-push")
            ]
        );
        let kept_copies = reinstall(&repo, &hook_problems).unwrap();
        assert_eq!(problems(&repo).unwrap(), []);
        assert_eq!(kept_copies, [hook_path(&repo, "pre-commit.arbor-replaced")]);
        assert_eq!(
            fs::read_to_string(&kept_copies[0]).unwrap(),
            "#!/bin/sh\nexit 0\n"
        );
    }

    #[test]
    fn a_second_reinstall_keeps_the_earlier_copy() {
        let (_dir, repo) = repository();
        install(&repo).unwrap();
        for edit in ["first", "second"] {
            fs::write(hook_path(&repo, "pre-commit"), edit).unwrap();
            reinstall(&repo, &problems(&repo).unwrap()).unwrap();
        }
        let kept = |name| fs::read_to_string(hook_path(&repo, name)).unwrap();
        assert_eq!(kept("pre-commit.arbor-replaced"), "first");
        assert_eq!(kept("pre-commit.arbor-replaced.1"), "second");
    }

    #[test]
    fn replacing_a_changed_hook_needs_a_typed_yes() {
        let (question, default_yes) = reinstall_question(&[
            HookProblem::Missing("pre-push"),
            HookProblem::Edited("pre-commit"),
        ]);
        assert!(question.ends_with("[y/N] "), "{question}");
        assert!(!default_yes);
        let (question, default_yes) = reinstall_question(&[HookProblem::Missing("pre-push")]);
        assert!(question.ends_with("[Y/n] "), "{question}");
        assert!(default_yes);
    }

    #[test]
    fn an_empty_or_unclear_answer_takes_the_default() {
        for default_yes in [true, false] {
            assert_eq!(answer_means_yes("\n", default_yes), default_yes);
            assert_eq!(answer_means_yes("maybe\n", default_yes), default_yes);
            assert!(answer_means_yes(" Yes\n", default_yes));
            assert!(!answer_means_yes("N\n", default_yes));
        }
    }

    #[test]
    fn only_the_hooks_arbor_installed_are_watched() {
        let (_dir, repo) = repository();
        fs::create_dir_all(default_hooks_dir(&repo).unwrap()).unwrap();
        fs::write(hook_path(&repo, "pre-commit"), "#!/bin/sh\nexit 0\n").unwrap();
        let installation = install(&repo).unwrap();
        assert_eq!(
            installation,
            Installation::Done {
                installed: vec!["pre-merge-commit", "pre-push"],
                kept: vec!["pre-commit"],
            }
        );
        fs::write(hook_path(&repo, "pre-commit"), "#!/bin/sh\nexit 1\n").unwrap();
        assert_eq!(problems(&repo).unwrap(), []);
    }

    #[test]
    fn hooks_installed_before_core_hooks_path_was_set_no_longer_run() {
        let (dir, repo) = repository();
        install(&repo).unwrap();
        let config = Command::new("git")
            .args(["config", "core.hooksPath", "elsewhere"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(config.success());

        let hook_problems = problems(&repo).unwrap();

        assert_eq!(
            hook_problems,
            [HookProblem::NotRun {
                hooks_path: "elsewhere".to_owned()
            }]
        );
        assert!(!hook_problems[0].can_reinstall());
    }
}
