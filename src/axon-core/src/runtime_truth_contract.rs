use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeFreshnessState {
    Fresh,
    Degraded,
    Stale,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeFreshnessContract {
    pub state: RuntimeFreshnessState,
    pub stale: bool,
    pub observed_age_ms: Option<u64>,
    pub stale_after_ms: u64,
    pub degraded_reason: Option<String>,
}

impl RuntimeFreshnessContract {
    pub fn fresh(stale_after_ms: u64) -> Self {
        Self {
            state: RuntimeFreshnessState::Fresh,
            stale: false,
            observed_age_ms: Some(0),
            stale_after_ms,
            degraded_reason: None,
        }
    }

    pub fn degraded(
        observed_age_ms: Option<u64>,
        stale_after_ms: u64,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            state: RuntimeFreshnessState::Degraded,
            stale: false,
            observed_age_ms,
            stale_after_ms,
            degraded_reason: Some(reason.into()),
        }
    }

    pub fn stale(observed_age_ms: u64, stale_after_ms: u64, reason: impl Into<String>) -> Self {
        Self {
            state: RuntimeFreshnessState::Stale,
            stale: true,
            observed_age_ms: Some(observed_age_ms),
            stale_after_ms,
            degraded_reason: Some(reason.into()),
        }
    }

    pub fn unknown(stale_after_ms: u64, reason: impl Into<String>) -> Self {
        Self {
            state: RuntimeFreshnessState::Unknown,
            stale: false,
            observed_age_ms: None,
            stale_after_ms,
            degraded_reason: Some(reason.into()),
        }
    }
}
