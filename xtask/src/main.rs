//! Dev tooling. Run with `cargo xtask <command>`.

use std::process::{Command, ExitCode};

/// Crates that would give `arbor-core` I/O. The core must stay pure (ADR-0005).
const IO_CRATES: &[&str] = &[
    "tokio",
    "async-std",
    "smol",
    "mio",
    "reqwest",
    "hyper",
    "ureq",
    "git2",
    "gix",
    "walkdir",
    "notify",
    "tempfile",
];

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("check-core-deps") => check_core_deps(),
        _ => {
            eprintln!("usage: cargo xtask check-core-deps");
            ExitCode::FAILURE
        }
    }
}

fn check_core_deps() -> ExitCode {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "arbor-core",
            "-e",
            "normal",
            "--prefix",
            "none",
        ])
        .output()
        .expect("failed to run cargo tree");
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return ExitCode::FAILURE;
    }
    let tree = String::from_utf8_lossy(&output.stdout);
    let found = forbidden_deps(&tree);
    if found.is_empty() {
        println!("arbor-core has no I/O dependencies");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "arbor-core must not depend on I/O crates: {}",
            found.join(", ")
        );
        ExitCode::FAILURE
    }
}

/// Names of forbidden crates in `cargo tree --prefix none` output.
fn forbidden_deps(tree: &str) -> Vec<&str> {
    let mut found: Vec<&str> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| IO_CRATES.contains(name))
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::forbidden_deps;

    #[test]
    fn clean_tree_passes() {
        let tree = "arbor-core v0.1.0 (/repo/crates/arbor-core)\nserde v1.0.0\n";
        assert!(forbidden_deps(tree).is_empty());
    }

    #[test]
    fn io_crates_are_reported_once() {
        let tree = "arbor-core v0.1.0\ntokio v1.0.0\nreqwest v0.12.0\ntokio v1.0.0 (*)\n";
        assert_eq!(forbidden_deps(tree), ["reqwest", "tokio"]);
    }

    #[test]
    fn similar_names_are_not_flagged() {
        let tree = "arbor-core v0.1.0\ntokio-macros-lookalike v1.0.0\n";
        assert!(forbidden_deps(tree).is_empty());
    }
}
