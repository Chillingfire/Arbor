//! The checks every `arbor` run makes before the TUI opens. They can't be turned off: Markers must
//! never reach the mainline (ADR-0002), so finding one outside a Session branch is said loudly.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

use arbor_config::Config;
use arbor_git::{Repo, TreeSource, is_session_branch};
use arbor_tiers::{MarkerLine, scan_markers};

/// More Marker lines than this per place are counted, not listed.
const LISTED_LINES: usize = 20;

/// Where the startup scan looked, and the Marker lines it found there.
struct Finding {
    place: String,
    marker_lines: Vec<MarkerLine>,
}

/// Warns on stderr about Markers in the operator's checkout or on the mainline tip, and waits for
/// Enter so the warning is read before the TUI covers it.
pub fn check(root: &Path, config: &Config) {
    let repo = match Repo::open(root) {
        Ok(repo) => repo,
        Err(err) => {
            eprintln!("arbor: could not check for Markers: {err}");
            return;
        }
    };
    let findings = scan_checkout_and_mainline(&repo, config);
    let Some(warning) = marker_warning(&findings) else {
        return;
    };
    eprint!("{warning}");
    wait_for_enter();
}

fn scan_checkout_and_mainline(repo: &Repo, config: &Config) -> Vec<Finding> {
    let mut findings = Vec::new();
    let branch = repo.current_branch();
    let on_session_branch = branch.as_deref().is_some_and(is_session_branch);
    if !on_session_branch {
        let place = match &branch {
            Some(branch) => format!("your checkout ({branch})"),
            None => "your checkout".to_owned(),
        };
        findings.extend(scan(repo, place, &TreeSource::WorkingTree));
    }
    match repo.mainline(config.mainline.as_deref()) {
        Ok(mainline) => {
            let start_point = mainline.start_point();
            let place = format!("the mainline {start_point}");
            findings.extend(scan(repo, place, &TreeSource::Revision(start_point)));
        }
        Err(err) => eprintln!("arbor: could not check the mainline for Markers: {err}"),
    }
    findings
}

fn scan(repo: &Repo, place: String, source: &TreeSource) -> Option<Finding> {
    match scan_markers(repo, source) {
        Ok(marker_lines) if marker_lines.is_empty() => None,
        Ok(marker_lines) => Some(Finding {
            place,
            marker_lines,
        }),
        Err(err) => {
            eprintln!("arbor: could not check {place} for Markers: {err}");
            None
        }
    }
}

fn marker_warning(findings: &[Finding]) -> Option<String> {
    if findings.is_empty() {
        return None;
    }
    let mut warning = String::from(
        "\narbor: WARNING: Markers were found outside a Session branch. They must never reach the \
         mainline.\n",
    );
    for finding in findings {
        warning.push_str(&format!("  In {}:\n", finding.place));
        for marker_line in finding.marker_lines.iter().take(LISTED_LINES) {
            warning.push_str(&format!("    {marker_line}\n"));
        }
        let unlisted = finding.marker_lines.len().saturating_sub(LISTED_LINES);
        if unlisted > 0 {
            warning.push_str(&format!("    … and {unlisted} more\n"));
        }
    }
    warning.push_str(
        "  Remove them, or finish the Session that left them. `arbor check-markers` lists them \
         again.\n",
    );
    Some(warning)
}

fn wait_for_enter() {
    if !io::stdin().is_terminal() {
        return;
    }
    eprint!("Press Enter to open arbor anyway. ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    let _ = io::stdin().lock().read_line(&mut answer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker_line(line: usize) -> MarkerLine {
        MarkerLine {
            file: "src/lib.rs".to_owned(),
            line,
            text: "// ARBOR:BEGIN WU-1".to_owned(),
        }
    }

    #[test]
    fn no_findings_means_no_warning() {
        assert_eq!(marker_warning(&[]), None);
    }

    #[test]
    fn the_warning_lists_each_place_and_counts_what_it_does_not_list() {
        let findings = [
            Finding {
                place: "your checkout (main)".to_owned(),
                marker_lines: vec![marker_line(3)],
            },
            Finding {
                place: "the mainline origin/main".to_owned(),
                marker_lines: (1..=LISTED_LINES + 2).map(marker_line).collect(),
            },
        ];

        let warning = marker_warning(&findings).unwrap();

        assert!(warning.contains("WARNING: Markers were found outside a Session branch"));
        assert!(warning.contains("  In your checkout (main):\n    src/lib.rs:3: // ARBOR:BEGIN"));
        assert!(warning.contains("  In the mainline origin/main:\n"));
        assert!(warning.contains(&format!("src/lib.rs:{LISTED_LINES}: ")));
        assert!(!warning.contains(&format!("src/lib.rs:{}: ", LISTED_LINES + 1)));
        assert!(warning.contains("… and 2 more"));
    }
}
