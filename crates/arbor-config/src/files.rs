use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table, value};

use crate::config::{Checks, Config, ConfigError, ConfigFile, UserFile, parse};

/// The project config file, at the repository root.
pub const PROJECT_CONFIG_FILE: &str = "arbor.toml";

impl Config {
    /// Loads the user config at `user_path` (if any) and `arbor.toml` in `project_root`. Missing
    /// files count as empty.
    pub fn load(user_path: Option<&Path>, project_root: &Path) -> Result<Self, ConfigError> {
        let user = user_path.map(read_optional).transpose()?.flatten();
        let project = read_optional(&project_root.join(PROJECT_CONFIG_FILE))?;
        Self::from_toml(user.as_deref(), project.as_deref())
    }
}

/// Writes `checks` into `arbor.toml` in `project_root` as its `[checks]` table, keeping everything
/// else in the file as it was, and returns the file's path.
pub fn save_project_checks(project_root: &Path, checks: &Checks) -> Result<PathBuf, ConfigError> {
    let path = project_root.join(PROJECT_CONFIG_FILE);
    let text = read_optional(&path)?.unwrap_or_default();
    // Refuse to rewrite a file we can't read, so a typo never costs the operator their settings.
    Config::from_toml(None, Some(&text))?;
    let mut doc: DocumentMut = text.parse().expect("valid TOML parses as a document");
    doc["checks"] = Item::Table(checks_table(checks));
    let updated = doc.to_string();
    Config::from_toml(None, Some(&updated))?;

    fs::write(&path, updated).map_err(|source| ConfigError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

fn checks_table(checks: &Checks) -> Table {
    let mut table = Table::new();
    let commands = [
        ("scaffold", &checks.scaffold),
        ("unit", &checks.unit),
        ("unit_filtered", &checks.unit_filtered),
    ];
    for (name, command) in commands {
        if let Some(command) = command {
            table[name] = value(command.as_str());
        }
    }
    table
}

/// Writes the key into the user config at `path`, keeping everything else in the file as it was.
/// The file is made readable only by its owner where the platform supports it.
pub fn save_api_key(path: &Path, key: &str) -> Result<(), ConfigError> {
    let io_error = |source| ConfigError::Io {
        path: path.to_owned(),
        source,
    };
    let text = read_optional(path)?.unwrap_or_default();
    // Refuse to rewrite a file we can't read, so a typo never costs the operator their settings.
    parse::<UserFile>(Some(&text), ConfigFile::User)?;
    let mut doc: DocumentMut = text.parse().expect("valid TOML parses as a document");
    doc["api_key"] = value(key);

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io_error)?;
    }
    write_private(path, &doc.to_string()).map_err(io_error)
}

fn read_optional(path: &Path) -> Result<Option<String>, ConfigError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    // `mode` only applies when the file is created, so tighten an existing file too.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(contents.as_bytes())
}
