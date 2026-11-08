//! Token and cost accounting, derived only from `UsageRecorded` events (ADR-0004).

use std::ops::AddAssign;

use serde::{Deserialize, Serialize};

use crate::types::Tier;

/// An amount in US dollars, held as whole nano-dollars so totals add up exactly and replay to the
/// same value. Serialized as the integer number of nano-dollars.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Usd(u64);

impl Usd {
    pub const ZERO: Self = Self(0);

    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Rounds to the nearest nano-dollar. Negative and NaN amounts count as zero.
    pub fn from_dollars(dollars: f64) -> Self {
        // `as` saturates, and turns NaN into 0.
        Self((dollars * 1e9).round() as u64)
    }

    pub const fn nanos(self) -> u64 {
        self.0
    }

    pub fn dollars(self) -> f64 {
        self.0 as f64 / 1e9
    }
}

impl AddAssign for Usd {
    fn add_assign(&mut self, other: Self) {
        self.0 = self.0.saturating_add(other.0);
    }
}

/// What one or more LLM calls used, as reported by the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Input tokens read from the prompt cache. Included in `input_tokens`.
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    #[serde(rename = "cost_nano_usd")]
    pub cost: Usd,
}

impl AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.cost += other.cost;
    }
}

/// A Session's running usage per Tier.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageTotals {
    planner: Usage,
    scaffolder: Usage,
    writer: Usage,
}

impl UsageTotals {
    pub fn tier(&self, tier: Tier) -> &Usage {
        match tier {
            Tier::Planner => &self.planner,
            Tier::Scaffolder => &self.scaffolder,
            Tier::Writer => &self.writer,
        }
    }

    /// All Tiers together.
    pub fn total(&self) -> Usage {
        let mut total = self.planner;
        total += self.scaffolder;
        total += self.writer;
        total
    }

    pub(crate) fn record(&mut self, tier: Tier, usage: Usage) {
        let slot = match tier {
            Tier::Planner => &mut self.planner,
            Tier::Scaffolder => &mut self.scaffolder,
            Tier::Writer => &mut self.writer,
        };
        *slot += usage;
    }
}
