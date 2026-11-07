use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Unix,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

/// Where the user config lives: `%APPDATA%\arbor\config.toml` on Windows, otherwise
/// `$XDG_CONFIG_HOME/arbor/config.toml` or `~/.config/arbor/config.toml`. `env` looks up an
/// environment variable. `None` means there is no home directory to put it in.
pub fn user_config_path(
    platform: Platform,
    env: impl Fn(&str) -> Option<String>,
) -> Option<PathBuf> {
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    let base = match platform {
        Platform::Windows => PathBuf::from(set("APPDATA")?),
        Platform::Unix => set("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| set("HOME").map(|home| PathBuf::from(home).join(".config")))?,
    };
    Some(base.join("arbor").join("config.toml"))
}
