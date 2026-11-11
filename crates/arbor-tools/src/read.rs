use std::fmt::Write;
use std::fs;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::worktree::Worktree;

/// The most lines `read_file` returns at once.
pub(crate) const MAX_LINES: usize = 400;
/// Longer lines are cut so one minified file can't flood the context.
const MAX_LINE_CHARS: usize = 2000;
/// The most entries `list_dir` returns.
const MAX_ENTRIES: usize = 500;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct ReadFileArgs {
    /// Path relative to the worktree root.
    path: String,
    /// The 1-based line to start at. Defaults to 1.
    offset: Option<usize>,
    /// How many lines to read, at most 400. Defaults to 400.
    limit: Option<usize>,
}

pub(crate) fn read_file(worktree: &Worktree, args: ReadFileArgs) -> Result<String, String> {
    let path = worktree.resolve(&args.path)?;
    if path.is_dir() {
        return Err(format!("{}: is a directory; use list_dir", args.path));
    }
    let bytes = fs::read(&path).map_err(|err| format!("{}: {err}", args.path))?;
    if looks_binary(&bytes) {
        return Err(format!("{}: looks like a binary file", args.path));
    }
    let file_text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = file_text.lines().collect();
    let total = lines.len();
    if total == 0 {
        return Ok(format!("{} is empty.", args.path));
    }
    let start = args.offset.unwrap_or(1).max(1);
    if start > total {
        let unit = if total == 1 { "line" } else { "lines" };
        return Err(format!(
            "offset {start} is past the end: {} has {total} {unit}",
            args.path
        ));
    }
    let limit = args.limit.unwrap_or(MAX_LINES).clamp(1, MAX_LINES);
    let end = (start - 1 + limit).min(total);

    let mut numbered = String::new();
    for (number, line) in (start..).zip(&lines[start - 1..end]) {
        let _ = writeln!(numbered, "{number:>6}\t{}", cut(line, MAX_LINE_CHARS));
    }
    if end < total {
        let _ = write!(
            numbered,
            "[Lines {start}-{end} of {total}. Call read_file with offset={} to read more.]",
            end + 1
        );
    }
    Ok(numbered)
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct ListDirArgs {
    /// Directory relative to the worktree root. Defaults to the root.
    path: Option<String>,
}

pub(crate) fn list_dir(worktree: &Worktree, args: ListDirArgs) -> Result<String, String> {
    let shown = args.path.as_deref().unwrap_or(".");
    let root = worktree.root()?;
    let dir = worktree.resolve(shown)?;
    if !dir.is_dir() {
        return Err(format!("{shown}: is not a directory"));
    }
    let entries: Vec<String> = worktree
        .walk(&root, &dir, Some(1))
        .map(|entry| {
            let name = worktree.display(&root, entry.path());
            if entry.file_type().is_some_and(|kind| kind.is_dir()) {
                name + "/"
            } else {
                name
            }
        })
        .collect();
    if entries.is_empty() {
        return Ok(format!(
            "{shown} has no entries, or .gitignore excludes all of them."
        ));
    }
    Ok(capped(entries, MAX_ENTRIES, "entries"))
}

/// One line per entry, with a note on how many were left out past `max`.
pub(crate) fn capped(entries: Vec<String>, max: usize, noun: &str) -> String {
    let more = entries.len().saturating_sub(max);
    let mut listing: String = entries
        .into_iter()
        .take(max)
        .map(|entry| entry + "\n")
        .collect();
    if more > 0 {
        let _ = writeln!(
            listing,
            "[{more} more {noun} not shown. Narrow the search.]"
        );
    }
    listing
}

/// Cuts a line to `max` characters, marking that it was cut.
pub(crate) fn cut(line: &str, max: usize) -> String {
    match line.char_indices().nth(max) {
        Some((index, _)) => format!("{}… [line cut]", &line[..index]),
        None => line.to_owned(),
    }
}

/// Whether a file looks binary: a NUL byte near the start.
pub(crate) fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|&byte| byte == 0)
}
