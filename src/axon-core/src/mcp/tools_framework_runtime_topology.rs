use crate::bridge::RuntimeTruthFeed;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_topology::{
    current_runtime_shadow_role, AxonProcessRole, RuntimeTopologyStatus,
};
use crate::runtime_truth_contract::RuntimeFreshnessState;
use crate::service_guard;
use serde_json::{json, Value};

use super::runtime_topology_support::{
    runtime_truth_feed_snapshot, split_peer_runtime_info, split_run_root,
    split_runtime_state_from_file,
};
use super::McpServer;

impl McpServer {
    pub(super) fn runtime_topology_snapshot(&self, runtime_mode: AxonRuntimeMode) -> Value {
        let mut status = RuntimeTopologyStatus::from_runtime_mode(runtime_mode);
        let instance_kind =
            std::env::var("AXON_INSTANCE_KIND").unwrap_or_else(|_| "dev".to_string());
        let project_root = std::env::var("AXON_PROJECT_ROOT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|_| ".".to_string())
            });
        let local_feed = service_guard::current_runtime_truth_feed();
        let mut indexer_feed = local_feed.clone();
        let ist_snapshot = self.graph_store.reader_snapshot_freshness_contract();
        let reader_snapshot_diagnostics = self.graph_store.reader_snapshot_diagnostics();
        let reader_alias_direct = self.graph_store.reader_snapshot_is_writer_alias();
        let shadow_role = current_runtime_shadow_role();
        // AXON_SPLIT_SHADOW_ONLY was a DuckDB-era split-process knob ;
        // under PG canonical the brain never carries indexer authority.
        let split_shadow_only = false;
        let mut peer_runtime_version = json!({
            "available": false,
            "release_version": Value::Null,
            "build_id": Value::Null,
            "install_generation": Value::Null,
            "runtime_mode": Value::Null
        });
        let mut peer_runtime_telemetry = Value::Null;
        let mut brain_ready = runtime_mode.control_plane_enabled();
        let mut indexer_ready = runtime_mode.ingestion_enabled();

        let split_process_role = AxonProcessRole::from_runtime_shadow_role(&shadow_role)
            .filter(|role| matches!(role, AxonProcessRole::Brain | AxonProcessRole::Indexer));
        match split_process_role {
            Some(AxonProcessRole::Brain) => {
                if let Some(peer) =
                    split_peer_runtime_info(&project_root, &instance_kind, "indexer")
                {
                    indexer_feed = peer.runtime_truth_feed.clone();
                    indexer_ready = peer.runtime_state_present;
                    peer_runtime_version = json!({
                        "available": true,
                        "release_version": peer.release_version,
                        "build_id": peer.build_id,
                        "install_generation": peer.install_generation,
                        "runtime_mode": peer.runtime_mode
                    });
                    peer_runtime_telemetry = peer.runtime_telemetry.unwrap_or(Value::Null);
                } else {
                    indexer_feed = RuntimeTruthFeed::from_observed_times(
                        0,
                        None,
                        None,
                        RuntimeTruthFeed::DEFAULT_STALE_AFTER_MS,
                        Some("indexer_feed_unavailable"),
                    );
                    indexer_ready = false;
                }
            }
            Some(AxonProcessRole::Indexer) => {
                brain_ready = split_runtime_state_from_file(
                    &split_run_root(&project_root, &instance_kind, "brain").join("runtime.env"),
                )
                .is_some();
                indexer_ready = true;
            }
            _ => {}
        }

        let indexer_feed_healthy = !indexer_feed.stale && indexer_feed.degraded_reason.is_none();
        let ist_snapshot_healthy = matches!(ist_snapshot.state, RuntimeFreshnessState::Fresh);
        let split_ready =
            brain_ready && indexer_ready && indexer_feed_healthy && ist_snapshot_healthy;
        status.brain_ready = brain_ready;
        status.indexer_ready = indexer_ready;
        status.ist_snapshot = ist_snapshot.clone();
        status.system_converged = if split_process_role.is_some() {
            !split_shadow_only && split_ready
        } else {
            match runtime_mode.declared_process_role() {
                AxonProcessRole::Brain => brain_ready,
                AxonProcessRole::Indexer => indexer_ready,
            }
        };

        if let Some(process_role) = split_process_role {
            status.apply_process_role(process_role);
        }

        json!({
            "process_role": status.process_role.as_str(),
            "public_mcp_authority": status.public_mcp_authority.as_str(),
            "soll_writer_authority": status.soll_writer_authority.as_str(),
            "ist_writer_authority": status.ist_writer_authority.as_str(),
            "brain_ready": status.brain_ready,
            "indexer_ready": status.indexer_ready,
            "system_converged": status.system_converged,
            "indexer_feed": runtime_truth_feed_snapshot(&indexer_feed),
            "indexer_runtime": {
                "available": !peer_runtime_telemetry.is_null(),
                "telemetry_source": if peer_runtime_telemetry.is_null() {
                    Value::Null
                } else {
                    Value::String("runtime_heartbeat".to_string())
                },
                "telemetry": peer_runtime_telemetry,
            },
            "peer_runtime_version": peer_runtime_version,
            "ist_snapshot": json!({
                "state": ist_snapshot.state,
                "stale": ist_snapshot.stale,
                "observed_age_ms": ist_snapshot.observed_age_ms,
                "stale_after_ms": ist_snapshot.stale_after_ms,
                "degraded_reason": ist_snapshot.degraded_reason,
                "unsafe_read": !matches!(ist_snapshot.state, RuntimeFreshnessState::Fresh),
                "computed_by": "GraphStore::reader_snapshot_freshness_contract",
                "trust_boundary": if reader_alias_direct {
                    "graph_store.writer_alias_direct_read"
                } else {
                    "graph_store.reader_snapshot_diagnostics"
                },
                "read_path": if reader_alias_direct {
                    "writer_alias_direct"
                } else {
                    "reader_snapshot"
                },
                "diagnostics": {
                    "commit_epoch": reader_snapshot_diagnostics.commit_epoch,
                    "reader_epoch": reader_snapshot_diagnostics.reader_epoch,
                    "reader_epoch_lag": reader_snapshot_diagnostics.reader_epoch_lag,
                    "refresh_inflight": reader_snapshot_diagnostics.refresh_inflight,
                    "refresh_requested_epoch": reader_snapshot_diagnostics.refresh_requested_epoch,
                    "last_refresh_started_ms": reader_snapshot_diagnostics.last_refresh_started_ms,
                    "last_refresh_completed_ms": reader_snapshot_diagnostics.last_refresh_completed_ms,
                    "reader_refresh_failures_total": reader_snapshot_diagnostics.reader_refresh_failures_total,
                }
            }),
            "compatibility_shim": status.compatibility_shim,
            "compatibility_reason": status.compatibility_reason,
        })
    }
}
