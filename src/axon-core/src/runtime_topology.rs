use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_truth_contract::RuntimeFreshnessContract;

const DEFAULT_RUNTIME_FEED_STALE_AFTER_MS: u64 = 5_000;
const DEFAULT_IST_SNAPSHOT_STALE_AFTER_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxonProcessRole {
    Brain,
    Indexer,
}

impl AxonProcessRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Brain => "brain",
            Self::Indexer => "indexer",
        }
    }

    pub fn serves_public_mcp(self) -> bool {
        matches!(self, Self::Brain)
    }

    pub fn runtime_binary_name(self) -> &'static str {
        match self {
            Self::Brain => "axon-brain",
            Self::Indexer => "axon-indexer",
        }
    }

    pub fn owns_soll_writes(self) -> bool {
        matches!(self, Self::Brain)
    }

    pub fn owns_ist_writes(self) -> bool {
        matches!(self, Self::Indexer)
    }

    pub fn from_runtime_shadow_role(role: &str) -> Option<Self> {
        match role.trim() {
            "brain" => Some(Self::Brain),
            "indexer" => Some(Self::Indexer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTopologyStatus {
    pub process_role: AxonProcessRole,
    pub public_mcp_authority: AxonProcessRole,
    pub soll_writer_authority: AxonProcessRole,
    pub ist_writer_authority: AxonProcessRole,
    pub brain_ready: bool,
    pub indexer_ready: bool,
    pub system_converged: bool,
    pub indexer_feed: RuntimeFreshnessContract,
    pub ist_snapshot: RuntimeFreshnessContract,
    pub compatibility_shim: bool,
    pub compatibility_reason: Option<String>,
}

impl RuntimeTopologyStatus {
    pub fn from_runtime_mode(mode: AxonRuntimeMode) -> Self {
        let process_role = mode.declared_process_role();
        let indexer_feed = match process_role {
            AxonProcessRole::Brain => RuntimeFreshnessContract::degraded(
                None,
                DEFAULT_RUNTIME_FEED_STALE_AFTER_MS,
                "indexer_feed_unavailable",
            ),
            AxonProcessRole::Indexer => {
                RuntimeFreshnessContract::fresh(DEFAULT_RUNTIME_FEED_STALE_AFTER_MS)
            }
        };

        let ist_snapshot = match process_role {
            AxonProcessRole::Brain => {
                RuntimeFreshnessContract::fresh(DEFAULT_IST_SNAPSHOT_STALE_AFTER_MS)
            }
            AxonProcessRole::Indexer => RuntimeFreshnessContract::unknown(
                DEFAULT_IST_SNAPSHOT_STALE_AFTER_MS,
                "ist_snapshot_requires_brain_surface",
            ),
        };

        Self {
            process_role,
            public_mcp_authority: AxonProcessRole::Brain,
            soll_writer_authority: AxonProcessRole::Brain,
            ist_writer_authority: AxonProcessRole::Indexer,
            brain_ready: matches!(process_role, AxonProcessRole::Brain),
            indexer_ready: matches!(process_role, AxonProcessRole::Indexer),
            system_converged: false,
            indexer_feed,
            ist_snapshot,
            compatibility_shim: false,
            compatibility_reason: None,
        }
    }

    pub fn apply_process_role(&mut self, process_role: AxonProcessRole) {
        self.process_role = process_role;
        self.public_mcp_authority = AxonProcessRole::Brain;
        self.soll_writer_authority = AxonProcessRole::Brain;
        self.ist_writer_authority = AxonProcessRole::Indexer;
        match process_role {
            AxonProcessRole::Brain => self.brain_ready = true,
            AxonProcessRole::Indexer => self.indexer_ready = true,
        }
    }

    pub fn as_json(&self) -> Value {
        json!({
            "process_role": self.process_role.as_str(),
            "public_mcp_authority": self.public_mcp_authority.as_str(),
            "soll_writer_authority": self.soll_writer_authority.as_str(),
            "ist_writer_authority": self.ist_writer_authority.as_str(),
            "brain_ready": self.brain_ready,
            "indexer_ready": self.indexer_ready,
            "system_converged": self.system_converged,
            "indexer_feed": {
                "state": self.indexer_feed.state,
                "stale": self.indexer_feed.stale,
                "observed_age_ms": self.indexer_feed.observed_age_ms,
                "stale_after_ms": self.indexer_feed.stale_after_ms,
                "degraded_reason": self.indexer_feed.degraded_reason,
            },
            "ist_snapshot": {
                "state": self.ist_snapshot.state,
                "stale": self.ist_snapshot.stale,
                "observed_age_ms": self.ist_snapshot.observed_age_ms,
                "stale_after_ms": self.ist_snapshot.stale_after_ms,
                "degraded_reason": self.ist_snapshot.degraded_reason,
            },
            "compatibility_shim": false,
            "compatibility_reason": Value::Null,
        })
    }
}

pub fn current_runtime_shadow_role() -> String {
    std::env::var("AXON_RUNTIME_SHADOW_ROLE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            AxonRuntimeMode::from_env()
                .declared_process_role()
                .as_str()
                .to_string()
        })
}

pub fn current_runtime_process_role() -> AxonProcessRole {
    AxonProcessRole::from_runtime_shadow_role(&current_runtime_shadow_role())
        .unwrap_or_else(|| AxonRuntimeMode::from_env().declared_process_role())
}

#[cfg(test)]
mod tests {
    use super::{AxonProcessRole, RuntimeTopologyStatus};
    use crate::runtime_mode::AxonRuntimeMode;
    use crate::runtime_truth_contract::RuntimeFreshnessState;

    #[test]
    fn process_roles_encode_brain_and_indexer_authority() {
        assert!(AxonProcessRole::Brain.serves_public_mcp());
        assert!(AxonProcessRole::Brain.owns_soll_writes());
        assert!(!AxonProcessRole::Brain.owns_ist_writes());
        assert!(AxonProcessRole::Indexer.owns_ist_writes());
        assert!(!AxonProcessRole::Indexer.serves_public_mcp());
    }

    #[test]
    fn runtime_authority_requires_peer_runtime_for_full_green() {
        let status = RuntimeTopologyStatus::from_runtime_mode(AxonRuntimeMode::BrainOnly);
        assert!(status.brain_ready);
        assert!(!status.system_converged);
        assert_eq!(status.process_role, AxonProcessRole::Brain);
    }

    #[test]
    fn explicit_runtime_modes_map_to_process_roles() {
        let brain = RuntimeTopologyStatus::from_runtime_mode(AxonRuntimeMode::BrainOnly);
        assert_eq!(brain.process_role, AxonProcessRole::Brain);
        assert_eq!(brain.indexer_feed.state, RuntimeFreshnessState::Degraded);

        let indexer = RuntimeTopologyStatus::from_runtime_mode(AxonRuntimeMode::IndexerGraph);
        assert_eq!(indexer.process_role, AxonProcessRole::Indexer);
        assert_eq!(indexer.indexer_feed.state, RuntimeFreshnessState::Fresh);
    }
}
