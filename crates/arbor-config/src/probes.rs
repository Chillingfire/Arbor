use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{ConfigError, RoleConfig, Roles};

/// The probe cache, next to the user config.
pub const PROBE_CACHE_FILE: &str = "probes.toml";

/// Where the probe cache lives for the user config at `user_config`.
pub fn probe_cache_path(user_config: &Path) -> PathBuf {
    user_config.with_file_name(PROBE_CACHE_FILE)
}

/// The (model, temperature) pairs whose probe call succeeded (grilling log Round 3 decision 6).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProbeCache {
    #[serde(default, rename = "probe")]
    probes: Vec<ProbedModel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ProbedModel {
    model: String,
    temperature: f64,
}

impl ProbeCache {
    /// A missing, unreadable or malformed cache counts as empty: the worst case is probing again.
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|cache_toml| toml::from_str(&cache_toml).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let io_error = |source| ConfigError::Io {
            path: path.to_owned(),
            source,
        };
        let cache_toml = toml::to_string(self).expect("the probe cache serializes");
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(io_error)?;
        }
        fs::write(path, cache_toml).map_err(io_error)
    }

    pub fn contains(&self, role: &RoleConfig) -> bool {
        self.probes
            .iter()
            .any(|probed| probed.model == role.model && probed.temperature == role.temperature)
    }

    pub fn record(&mut self, role: &RoleConfig) {
        if !self.contains(role) {
            self.probes.push(ProbedModel {
                model: role.model.clone(),
                temperature: role.temperature,
            });
        }
    }

    /// The roles with no successful probe on record, each (model, temperature) once.
    pub fn unprobed(&self, roles: &Roles) -> Vec<RoleConfig> {
        let mut unprobed: Vec<RoleConfig> = Vec::new();
        for (_, role) in roles.named() {
            if !self.contains(role) && !unprobed.contains(role) {
                unprobed.push(role.clone());
            }
        }
        unprobed
    }
}
