use std::fs;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::worktree::Worktree;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct WriteFileArgs {
    /// Path relative to the worktree root. Missing parent directories are created.
    path: String,
    /// The whole new content of the file.
    content: String,
}

pub(crate) fn write_file(worktree: &Worktree, args: WriteFileArgs) -> Result<String, String> {
    let path = worktree.resolve(&args.path)?;
    if path.is_dir() {
        return Err(format!("{}: is a directory", args.path));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", args.path))?;
    }
    fs::write(&path, &args.content).map_err(|err| format!("{}: {err}", args.path))?;
    Ok(format!(
        "Wrote {} bytes to {}.",
        args.content.len(),
        args.path
    ))
}

#[derive(Deserialize, JsonSchema)]
pub(crate) struct EditFileArgs {
    /// Path relative to the worktree root.
    path: String,
    /// The exact text to replace. It must occur exactly once in the file.
    old_string: String,
    /// The text to put in its place.
    new_string: String,
}

pub(crate) fn edit_file(worktree: &Worktree, args: EditFileArgs) -> Result<String, String> {
    if args.old_string.is_empty() {
        return Err("old_string must not be empty; use write_file to create a file".to_owned());
    }
    let path = worktree.resolve(&args.path)?;
    let text = fs::read_to_string(&path).map_err(|err| format!("{}: {err}", args.path))?;

    let (old, new) = match text.matches(&args.old_string).count() {
        // Models write `\n`; let their snippet match a file with Windows line endings.
        0 if text.contains("\r\n") && args.old_string.contains('\n') => {
            (crlf(&args.old_string), crlf(&args.new_string))
        }
        _ => (args.old_string.clone(), args.new_string.clone()),
    };
    match text.matches(&old).count() {
        0 => Err(format!(
            "old_string was not found in {}. Read the file and copy the text exactly.",
            args.path
        )),
        1 => {
            fs::write(&path, text.replacen(&old, &new, 1))
                .map_err(|err| format!("{}: {err}", args.path))?;
            Ok(format!("Edited {}.", args.path))
        }
        count => Err(format!(
            "old_string occurs {count} times in {}. Include more surrounding text so it occurs once.",
            args.path
        )),
    }
}

/// Converts `\n` line endings to `\r\n`, leaving existing `\r\n` alone.
fn crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}
