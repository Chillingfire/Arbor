use std::fs;
use std::path::Path;

use globset::{Glob, GlobBuilder, GlobMatcher};
use regex::RegexBuilder;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::read::{capped, cut, looks_binary};
use crate::worktree::Worktree;

/// The most paths `glob` returns, and the most matches `grep` returns.
const MAX_RESULTS: usize = 200;
const MAX_MATCH_CHARS: usize = 300;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct GlobArgs {
    /// A glob such as `**/*.rs`, matched against paths relative to `path`. `*` doesn't cross `/`.
    pattern: String,
    /// Directory to search, relative to the worktree root. Defaults to the root.
    path: Option<String>,
}

pub(crate) fn glob(worktree: &Worktree, args: GlobArgs) -> Result<String, String> {
    let matcher = GlobBuilder::new(&args.pattern)
        .literal_separator(true)
        .build()
        .map_err(|err| format!("invalid glob: {err}"))?
        .compile_matcher();
    let root = worktree.root()?;
    let dir = worktree.resolve(args.path.as_deref().unwrap_or("."))?;
    let found: Vec<String> = worktree
        .walk(&root, &dir, None)
        .filter(|entry| entry.file_type().is_some_and(|kind| !kind.is_dir()))
        .filter(|entry| {
            let relative = worktree.display(&dir, entry.path());
            matcher.is_match(relative)
        })
        .map(|entry| worktree.display(&root, entry.path()))
        .collect();
    if found.is_empty() {
        return Ok(format!("No files match {}.", args.pattern));
    }
    Ok(capped(found, MAX_RESULTS, "files"))
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct GrepArgs {
    /// A regular expression (Rust `regex` syntax), matched against each line.
    pattern: String,
    /// File or directory to search, relative to the worktree root. Defaults to the root.
    path: Option<String>,
    /// Only search files whose path matches this glob, e.g. `*.rs`.
    glob: Option<String>,
    /// Ignore case. Defaults to false.
    case_insensitive: Option<bool>,
}

pub(crate) fn grep(worktree: &Worktree, args: GrepArgs) -> Result<String, String> {
    let regex = RegexBuilder::new(&args.pattern)
        .case_insensitive(args.case_insensitive.unwrap_or(false))
        .build()
        .map_err(|err| format!("invalid pattern: {err}"))?;
    let filter: Option<GlobMatcher> = args
        .glob
        .as_deref()
        .map(|glob| {
            Glob::new(glob)
                .map(|glob| glob.compile_matcher())
                .map_err(|err| format!("invalid glob: {err}"))
        })
        .transpose()?;
    let root = worktree.root()?;
    let start = worktree.resolve(args.path.as_deref().unwrap_or("."))?;

    let files: Vec<_> = if start.is_file() {
        vec![start.clone()]
    } else {
        worktree
            .walk(&root, &start, None)
            // Symlinked files are skipped: reading one could leak a file outside the worktree.
            .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
            .map(ignore::DirEntry::into_path)
            .collect()
    };

    let mut matches = Vec::new();
    for file in files {
        let shown = worktree.display(&root, &file);
        if filter.as_ref().is_some_and(|glob| !glob.is_match(&shown)) {
            continue;
        }
        let Some(text) = read_text(&file) else {
            continue;
        };
        for (number, line) in (1..).zip(text.lines()) {
            if regex.is_match(line) {
                matches.push(format!("{shown}:{number}: {}", cut(line, MAX_MATCH_CHARS)));
            }
        }
    }
    if matches.is_empty() {
        return Ok(format!("No matches for {}.", args.pattern));
    }
    Ok(capped(matches, MAX_RESULTS, "matches"))
}

/// A file's text, or `None` if it can't be read or looks binary.
fn read_text(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    if looks_binary(&bytes) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}
