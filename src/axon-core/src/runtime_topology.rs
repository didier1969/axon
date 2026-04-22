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
    LegacyMonolith,
}

impl AxonProcessRole {
    pub fn serves_public_mcp(self) -> bool {
        matches!(self, Self::Brain | Self::LegacyMonolith)
    }

    pub fn owns_soll_writes(self) -> bool {
        matches!(self, Self::Brain | Self::LegacyMonolith)
    }

    pub fn owns_ist_writes(self) -> bool {
        matches!(self, Self::Indexer | Self::LegacyMonolith)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTopologyKind {
    BrainIndexerSplit,
    LegacyMonolithCompatibility,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTopologyStatus {
    pub topology: RuntimeTopologyKind,
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
    pub fn degraded_brain_only() -> Self {
        Self {
            topology: RuntimeTopologyKind::BrainIndexerSplit,
            process_role: AxonProcessRole::Brain,
            public_mcp_authority: AxonProcessRole::Brain,
            soll_writer_authority: AxonProcessRole::Brain,
            ist_writer_authority: AxonProcessRole::Indexer,
            brain_ready: true,
            indexer_ready: false,
            system_converged: false,
            indexer_feed: RuntimeFreshnessContract::stale(
                DEFAULT_RUNTIME_FEED_STALE_AFTER_MS * 2,
                DEFAULT_RUNTIME_FEED_STALE_AFTER_MS,
                "indexer_feed_unavailable",
            ),
            ist_snapshot: RuntimeFreshnessContract::unknown(
                DEFAULT_IST_SNAPSHOT_STALE_AFTER_MS,
                "ist_snapshot_requires_indexer_refresh_contract",
            ),
            compatibility_shim: false,
            compatibility_reason: None,
        }
    }

    pub fn legacy_compatibility_shim(mode: AxonRuntimeMode) -> Self {
        let brain_ready = mode.control_plane_enabled();
        let indexer_ready = mode.ingestion_enabled();
        let system_converged = matches!(mode, AxonRuntimeMode::Full);
        let ist_snapshot = if brain_ready {
            RuntimeFreshnessContract::fresh(DEFAULT_IST_SNAPSHOT_STALE_AFTER_MS)
        } else {
            RuntimeFreshnessContract::degraded(
                None,
                DEFAULT_IST_SNAPSHOT_STALE_AFTER_MS,
                "control_plane_not_serving_snapshot_reads",
            )
        };
        let indexer_feed = if system_converged {
            RuntimeFreshnessContract::fresh(DEFAULT_RUNTIME_FEED_STALE_AFTER_MS)
        } else if brain_ready && indexer_ready {
            RuntimeFreshnessContract::degraded(
                Some(DEFAULT_RUNTIME_FEED_STALE_AFTER_MS),
                DEFAULT_RUNTIME_FEED_STALE_AFTER_MS,
                "legacy_mode_is_not_target_split_topology",
            )
        } else if brain_ready {
            RuntimeFreshnessContract::degraded(
                None,
                DEFAULT_RUNTIME_FEED_STALE_AFTER_MS,
                "legacy_monolith_has_no_split_indexer_feed",
            )
        } else {
            RuntimeFreshnessContract::unknown(
                DEFAULT_RUNTIME_FEED_STALE_AFTER_MS,
                "legacy_mode_does_not_expose_brain_readiness",
            )
        };

        Self {
            topology: RuntimeTopologyKind::LegacyMonolithCompatibility,
            process_role: AxonProcessRole::LegacyMonolith,
            public_mcp_authority: AxonProcessRole::LegacyMonolith,
            soll_writer_authority: AxonProcessRole::LegacyMonolith,
            ist_writer_authority: AxonProcessRole::LegacyMonolith,
            brain_ready,
            indexer_ready,
            system_converged,
            indexer_feed,
            ist_snapshot,
            compatibility_shim: true,
            compatibility_reason: Some(format!(
                "runtime_mode '{}' is a legacy monolith compatibility shim, not the target brain/indexer topology",
                mode.as_str()
            )),
        }
    }

    pub fn fallback_legacy_compatibility_shim(mode: AxonRuntimeMode) -> Value {
        let status = Self::legacy_compatibility_shim(mode);
        json!({
            "topology": "legacy_monolith_compatibility",
            "process_role": "legacy_monolith",
            "public_mcp_authority": "legacy_monolith",
            "soll_writer_authority": "legacy_monolith",
            "ist_writer_authority": "legacy_monolith",
            "brain_ready": status.brain_ready,
            "indexer_ready": status.indexer_ready,
            "system_converged": status.system_converged,
            "indexer_feed": {
                "state": status.indexer_feed.state,
                "stale": status.indexer_feed.stale,
                "observed_age_ms": status.indexer_feed.observed_age_ms,
                "stale_after_ms": status.indexer_feed.stale_after_ms,
                "degraded_reason": status.indexer_feed.degraded_reason,
            },
            "ist_snapshot": {
                "state": status.ist_snapshot.state,
                "stale": status.ist_snapshot.stale,
                "observed_age_ms": status.ist_snapshot.observed_age_ms,
                "stale_after_ms": status.ist_snapshot.stale_after_ms,
                "degraded_reason": status.ist_snapshot.degraded_reason,
            },
            "compatibility_shim": true,
            "compatibility_reason": status.compatibility_reason,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{AxonProcessRole, RuntimeTopologyStatus};
    use crate::runtime_mode::AxonRuntimeMode;
    use crate::runtime_truth_contract::RuntimeFreshnessState;

    #[test]
    fn topology_roles_encode_brain_and_indexer_authority() {
        assert!(AxonProcessRole::Brain.serves_public_mcp());
        assert!(AxonProcessRole::Brain.owns_soll_writes());
        assert!(!AxonProcessRole::Brain.owns_ist_writes());
        assert!(AxonProcessRole::Indexer.owns_ist_writes());
        assert!(!AxonProcessRole::Indexer.serves_public_mcp());
    }

    #[test]
    fn runtime_topology_requires_system_converged_for_full_green() {
        let topo = RuntimeTopologyStatus::degraded_brain_only();
        assert!(topo.brain_ready);
        assert!(!topo.system_converged);
    }

    #[test]
    fn legacy_compatibility_shim_mode_matrix_preserves_expected_readiness() {
        let full = RuntimeTopologyStatus::legacy_compatibility_shim(AxonRuntimeMode::Full);
        assert!(full.brain_ready);
        assert!(full.indexer_ready);
        assert!(full.system_converged);
        assert_eq!(full.indexer_feed.state, RuntimeFreshnessState::Fresh);
        assert_eq!(full.ist_snapshot.state, RuntimeFreshnessState::Fresh);

        let read_only = RuntimeTopologyStatus::legacy_compatibility_shim(AxonRuntimeMode::ReadOnly);
        assert!(read_only.brain_ready);
        assert!(!read_only.indexer_ready);
        assert!(!read_only.system_converged);
        assert_eq!(
            read_only.indexer_feed.state,
            RuntimeFreshnessState::Degraded
        );
        assert_eq!(read_only.ist_snapshot.state, RuntimeFreshnessState::Fresh);

        let mcp_only = RuntimeTopologyStatus::legacy_compatibility_shim(AxonRuntimeMode::McpOnly);
        assert!(mcp_only.brain_ready);
        assert!(!mcp_only.indexer_ready);
        assert!(!mcp_only.system_converged);
        assert_eq!(mcp_only.indexer_feed.state, RuntimeFreshnessState::Degraded);
        assert_eq!(mcp_only.ist_snapshot.state, RuntimeFreshnessState::Fresh);

        let graph_only =
            RuntimeTopologyStatus::legacy_compatibility_shim(AxonRuntimeMode::GraphOnly);
        assert!(graph_only.brain_ready);
        assert!(graph_only.indexer_ready);
        assert!(!graph_only.system_converged);
        assert_eq!(
            graph_only.indexer_feed.state,
            RuntimeFreshnessState::Degraded
        );
        assert_eq!(graph_only.ist_snapshot.state, RuntimeFreshnessState::Fresh);
    }
}
