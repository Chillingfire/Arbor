use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The most Conventions text the Planner and Scaffolder get, in estimated tokens.
pub const CONVENTIONS_CAP_TOKENS: u32 = 4000;

/// Loaded first, in this order, when present at the repository root.
const DEFAULT_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md", "CONTEXT.md"];

/// The first of these that exists is the README.
const README_NAMES: &[&str] = &["README.md", "README", "README.rst", "README.txt"];

const TRUNCATED_NOTE: &str = "[truncated: the Conventions cap was reached]\n";

/// Estimates how many tokens `text` costs: one per two and a half bytes of UTF-8, rounded up.
///
/// This is deliberately simple and deterministic, with no tokenizer. Measured against the input
/// tokens OpenRouter reported for the default Claude 5.5 models (`tests/live_tokens.rs`), real
/// text ran 2.5 to 2.9 bytes per token: Markdown Conventions 2.5, prompt instructions 2.9, Rust
/// source 2.7. Using the low end means the estimate errs on the side of overcounting. Models from
/// other providers use other tokenizers; rerun that test when adding one.
pub fn estimate_tokens(text: &str) -> u32 {
    u32::try_from((text.len() * 2).div_ceil(5)).unwrap_or(u32::MAX)
}

/// The project's own guidance documents, as given to the Planner and Scaffolder.
///
/// The files are `AGENTS.md`, `CLAUDE.md`, `CONTEXT.md` and the README if present, then each of
/// `context.files` from `arbor.toml`. Together they are capped at [`CONVENTIONS_CAP_TOKENS`]:
/// files are kept whole in that order until one doesn't fit, which is cut at a line boundary, and
/// the rest are named in a closing note so the model can read them itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Conventions {
    text: String,
    included: Vec<String>,
    truncated: Option<String>,
    omitted: Vec<String>,
    missing: Vec<String>,
}

#[derive(Debug)]
pub enum ConventionsError {
    /// A `context.files` entry is absolute or climbs out with `..`, or any Conventions file
    /// resolves outside the repository through a link. Refused so a cloned repository can't send
    /// other local files to the model.
    OutsideRepository {
        path: String,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for ConventionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsideRepository { path } => write!(
                f,
                "Conventions file {path} is outside the repository; check context.files in \
                 arbor.toml and any links"
            ),
            Self::Io { path, source } => write!(f, "could not read {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for ConventionsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::OutsideRepository { .. } => None,
        }
    }
}

impl Conventions {
    /// Loads the Conventions from the repository at `root`. `context_files` are the extra paths
    /// from `arbor.toml`, relative to `root`. Line endings are normalised to `\n`.
    pub fn load(root: &Path, context_files: &[String]) -> Result<Self, ConventionsError> {
        let mut conventions = Self::default();
        let mut seen: Vec<String> = Vec::new();
        let mut full = false;
        let readme = README_NAMES
            .iter()
            .find(|name| root.join(name).is_file())
            .map(|name| (*name).to_owned());
        let defaults = DEFAULT_FILES.iter().map(|name| (*name).to_owned());

        let candidates = defaults
            .chain(readme)
            .map(|path| (path, false))
            .chain(context_files.iter().map(|path| (path.clone(), true)));
        for (path, configured) in candidates {
            let key = normalise(&path);
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let Some(content) = read(root, &path, configured)? else {
                if configured {
                    conventions.missing.push(path);
                }
                continue;
            };
            if content.is_empty() {
                continue;
            }
            if full {
                conventions.omitted.push(path);
                continue;
            }
            let block = file_block(&path, &content, "");
            if conventions.fits(&block) {
                conventions.push(&path, &block);
                continue;
            }
            // Once one file doesn't fit, later ones are left out even if they are small, so the
            // Conventions read as a prefix of the priority order.
            full = true;
            match conventions.largest_prefix(&path, &content) {
                Some(block) => {
                    conventions.push(&path, &block);
                    conventions.truncated = Some(path);
                }
                None => conventions.omitted.push(path),
            }
        }

        if !conventions.omitted.is_empty() {
            conventions.text.push_str(&format!(
                "\nNot shown because of the Conventions cap: {}. Read them with read_file if you \
                 need them.\n",
                conventions.omitted.join(", ")
            ));
        }
        Ok(conventions)
    }

    /// The text to put in the prompt, empty when there are no Conventions.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The files shown, wholly or in part, in order.
    pub fn included(&self) -> &[String] {
        &self.included
    }

    /// The file cut short by the cap, if any.
    pub fn truncated(&self) -> Option<&str> {
        self.truncated.as_deref()
    }

    /// Files left out because the cap was reached.
    pub fn omitted(&self) -> &[String] {
        &self.omitted
    }

    /// Files listed in `context.files` that don't exist.
    pub fn missing(&self) -> &[String] {
        &self.missing
    }

    fn push(&mut self, path: &str, block: &str) {
        self.text = self.with(block);
        self.included.push(path.to_owned());
    }

    /// The text with `block` appended after a blank line.
    fn with(&self, block: &str) -> String {
        if self.text.is_empty() {
            block.to_owned()
        } else {
            format!("{}\n{block}", self.text)
        }
    }

    fn fits(&self, block: &str) -> bool {
        estimate_tokens(&self.with(block)) <= CONVENTIONS_CAP_TOKENS
    }

    /// The block for the largest whole-line prefix of `content` that still fits under the cap, or
    /// `None` if not even one line fits.
    fn largest_prefix(&self, path: &str, content: &str) -> Option<String> {
        let mut end = 0;
        for line in content.split_inclusive('\n') {
            let candidate = &content[..end + line.len()];
            if !self.fits(&file_block(path, candidate, TRUNCATED_NOTE)) {
                break;
            }
            end += line.len();
        }
        (end > 0).then(|| file_block(path, &content[..end], TRUNCATED_NOTE))
    }
}

/// `content` ends with a newline unless it is empty.
fn file_block(path: &str, content: &str, note: &str) -> String {
    format!("<file path=\"{path}\">\n{content}{note}</file>\n")
}

/// Reads a Conventions file, `None` if it doesn't exist. Line endings become `\n`, and the text
/// ends with exactly one newline unless it is blank.
fn read(root: &Path, path: &str, configured: bool) -> Result<Option<String>, ConventionsError> {
    let outside = || ConventionsError::OutsideRepository {
        path: path.to_owned(),
    };
    let relative = Path::new(path);
    if configured
        && relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(outside());
    }
    let full = root.join(relative);
    let io_error = |source| ConventionsError::Io {
        path: full.clone(),
        source,
    };
    let bytes = match fs::read(&full) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io_error(source)),
    };
    let canonical_root = fs::canonicalize(root).map_err(io_error)?;
    let canonical = fs::canonicalize(&full).map_err(io_error)?;
    if !canonical.starts_with(&canonical_root) {
        return Err(outside());
    }
    let file_text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    let trimmed = file_text.trim_end();
    Ok(Some(if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }))
}

/// A path as written in config, compared loosely so `./AGENTS.md` matches `AGENTS.md`.
fn normalise(path: &str) -> String {
    let path = path.replace('\\', "/");
    path.trim_start_matches("./").to_owned()
}
