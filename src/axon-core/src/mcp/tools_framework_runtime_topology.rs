use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_topology::{
    current_runtime_shadow_role, AxonProcessRole, RuntimeTopologyStatus,
};
use crate::runtime_truth_contract::RuntimeFreshnessState;
use serde_json::{json, Value};

use super::runtime_topology_support::{
    resolve_indexer_liveness, runtime_truth_feed_snapshot, split_peer_runtime_info,
    split_run_root, split_runtime_state_from_file, EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
};
use super::McpServer;

impl McpServer {
    pub(super) fn runtime_topology_snapshot(&self, runtime_mode: AxonRuntimeMode) -> Value {
        let mut status = RuntimeTopologyStatus::from_runtime_mode(runtime_mode);
        // REQ-AXO-901657 slice 4 cluster A : canonical = AXON_INSTANCE.
        let instance_kind = crate::env_alias::read_with_alias_or(
            "AXON_INSTANCE",
            "AXON_INSTANCE_KIND",
            "dev",
        );
        let project_root = std::env::var("AXON_PROJECT_ROOT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|_| ".".to_string())
            });
        let ist_snapshot = self.graph_store.reader_snapshot_freshness_contract();
        let reader_snapshot_diagnostics = self.graph_store.reader_snapshot_diagnostics();
        let reader_alias_direct = self.graph_store.reader_snapshot_is_writer_alias();
        let shadow_role = current_runtime_shadow_role();
        let mut peer_runtime_version = json!({
            "available": false,
            "release_version": Value::Null,
            "build_id": Value::Null,
            "install_generation": Value::Null,
            "runtime_mode": Value::Null
        });
        let mut peer_runtime_telemetry = Value::Null;
        // REQ-AXO-901836 — lane_parameters block published by indexer's
        // heartbeat, surfacing vector_workers / graph_workers / batch sizes.
        // (DEC-AXO-901626 removed the sibling embedder_provider block: the
        // effective provider is now derived observably by the brain composer
        // from the indexer pid + nvidia-smi, not forwarded from a raced slot.)
        let mut peer_lane_parameters = Value::Null;
        let mut brain_ready = runtime_mode.control_plane_enabled();

        // REQ-AXO-901859 — the file/shadow-role peer info is consulted ONLY
        // for version/telemetry/lane metadata now. Indexer liveness no longer
        // comes from here.
        let split_process_role = AxonProcessRole::from_runtime_shadow_role(&shadow_role)
            .filter(|role| matches!(role, AxonProcessRole::Brain | AxonProcessRole::Indexer));
        match split_process_role {
            Some(AxonProcessRole::Brain) => {
                if let Some(peer) =
                    split_peer_runtime_info(&project_root, &instance_kind, "indexer")
                {
                    peer_runtime_version = json!({
                        "available": true,
                        "release_version": peer.release_version,
                        "build_id": peer.build_id,
                        "install_generation": peer.install_generation,
                        "runtime_mode": peer.runtime_mode
                    });
                    peer_runtime_telemetry = peer.runtime_telemetry.unwrap_or(Value::Null);
                    peer_lane_parameters = peer.lane_parameters.unwrap_or(Value::Null);
                }
            }
            Some(AxonProcessRole::Indexer) => {
                brain_ready = split_runtime_state_from_file(
                    &split_run_root(&project_root, &instance_kind, "brain").join("runtime.env"),
                )
                .is_some();
            }
            _ => {}
        }

        // REQ-AXO-901859 — SINGLE canonical liveness authority: the PG
        // heartbeat (`axon_runtime.EmbedderLifecycleHeartbeat`), the same
        // source `embedding_status` trusts. No file/shadow-role fallback —
        // that second source is exactly what let `status` and
        // `embedding_status` disagree (one saw a frozen file feed, the other
        // the live heartbeat). Now every consumer reads one truth
        // (PIL-AXO-001). If no fresh heartbeat exists the indexer is not
        // provably alive and we say so loudly rather than infer from launch
        // mode.
        let indexer_heartbeat_ms = self
            .graph_store
            .latest_lifecycle_heartbeat("indexer")
            .ok()
            .flatten()
            .map(|row| row.heartbeat_ms);
        let indexer_liveness = resolve_indexer_liveness(
            Self::now_unix_ms(),
            indexer_heartbeat_ms,
            EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
        );
        let indexer_liveness_source = indexer_liveness.source;
        let indexer_ready = indexer_liveness.ready;
        let indexer_feed = indexer_liveness.feed;

        let indexer_feed_healthy = !indexer_feed.stale && indexer_feed.degraded_reason.is_none();
        let ist_snapshot_healthy = matches!(ist_snapshot.state, RuntimeFreshnessState::Fresh);
        let split_ready =
            brain_ready && indexer_ready && indexer_feed_healthy && ist_snapshot_healthy;
        status.brain_ready = brain_ready;
        status.indexer_ready = indexer_ready;
        status.ist_snapshot = ist_snapshot.clone();
        status.system_converged = if split_process_role.is_some() {
            split_ready
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
            // REQ-AXO-901859 — fail-loud provenance of the liveness verdict:
            // `pg_heartbeat` / `pg_heartbeat_stale` / `no_heartbeat`.
            "indexer_liveness_source": indexer_liveness_source,
            "indexer_runtime": {
                "available": !peer_runtime_telemetry.is_null(),
                "telemetry_source": if peer_runtime_telemetry.is_null() {
                    Value::Null
                } else {
                    Value::String("runtime_heartbeat".to_string())
                },
                "telemetry": peer_runtime_telemetry,
                // REQ-AXO-901836 — lane_parameters block exposes indexer's
                // effective vector_workers / graph_workers / batch sizes so
                // brain status surfaces the indexer's truth in resource_policy
                // and runtime_authority.lane_parameters instead of its own
                // brain_only-clamped values.
                "lane_parameters": peer_lane_parameters,
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
