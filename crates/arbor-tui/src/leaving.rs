//! What the Human Operator is told when leaving arbor with a Session unfinished: in the quit
//! confirmation, and again on stderr once the TUI has closed so it stays in the scrollback.

use std::path::PathBuf;

/// A Session whose branch and worktree outlive arbor with Markers and Placeholders in them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfinishedSession {
    pub branch: String,
    pub worktree: PathBuf,
    /// Marker lines in the worktree's tracked files.
    pub markers: usize,
    /// Regions still holding their Placeholder: one per BEGIN Marker.
    pub placeholders: usize,
    /// The Session thread is still running.
    pub running: bool,
}

impl UnfinishedSession {
    /// The warning, one paragraph per line.
    pub fn warning(&self) -> Vec<String> {
        // Rebuilt from its components so a path joined from `.arbor/sessions` shows one kind of
        // separator on Windows.
        let worktree: PathBuf = self.worktree.components().collect();
        let worktree = worktree.display();
        let branch = &self.branch;
        let state = if self.running {
            "Leaving stops the running Session."
        } else {
            "The Session is unfinished."
        };
        vec![
            format!(
                "{state} The branch {branch} and its worktree hold {} and {}.",
                count(self.markers, "Marker"),
                count(self.placeholders, "Placeholder")
            ),
            "Your own checkout and the mainline are untouched.".to_owned(),
            format!(
                "To resume, finish the Regions by hand in {worktree}; arbor can't resume a \
                 Session yet."
            ),
            format!(
                "To delete the Session: git worktree remove --force {worktree} && git branch -D \
                 {branch}"
            ),
        ]
    }
}

fn count(number: usize, noun: &str) -> String {
    match number {
        0 => format!("no {noun}s"),
        1 => format!("1 {noun}"),
        _ => format!("{number} {noun}s"),
    }
}
