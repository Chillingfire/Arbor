use std::fmt;

use crate::config::{RoleConfig, Roles};

/// Anthropic's API takes temperatures up to 1; OpenRouter's other providers take up to 2.
pub fn max_temperature(model: &str) -> f64 {
    if model.starts_with("anthropic/") {
        1.0
    } else {
        2.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemperatureOutOfRange {
    pub role: &'static str,
    pub model: String,
    pub temperature: f64,
    pub max_temperature: f64,
}

impl fmt::Display for TemperatureOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the {} temperature {} is outside 0–{} for {}",
            self.role, self.temperature, self.max_temperature, self.model
        )
    }
}

impl std::error::Error for TemperatureOutOfRange {}

impl RoleConfig {
    /// The static range check (grilling log Round 3 decision 6), before any probe call.
    pub fn check_temperature(&self, role: &'static str) -> Result<(), TemperatureOutOfRange> {
        let max_temperature = max_temperature(&self.model);
        if (0.0..=max_temperature).contains(&self.temperature) {
            Ok(())
        } else {
            Err(TemperatureOutOfRange {
                role,
                model: self.model.clone(),
                temperature: self.temperature,
                max_temperature,
            })
        }
    }
}

impl Roles {
    /// Each role with the Tier name it fills, Planner first.
    pub fn named(&self) -> [(&'static str, &RoleConfig); 3] {
        [
            ("Planner", &self.planner),
            ("Scaffolder", &self.scaffolder),
            ("Writer", &self.writer),
        ]
    }

    /// Every role whose temperature fails the static range check.
    pub fn temperature_problems(&self) -> Vec<TemperatureOutOfRange> {
        self.named()
            .into_iter()
            .filter_map(|(role, role_config)| role_config.check_temperature(role).err())
            .collect()
    }

    /// The recommended model and temperature for each role.
    pub fn recommended() -> Self {
        crate::Config::default().roles
    }
}
