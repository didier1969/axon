use crate::bridge::RuntimeTruthFeed;
use crate::embedder::{
    bootstrap_runtime_tuning_state, current_embedding_provider_diagnostics,
    current_runtime_tuning_state, embedding_lane_config_from_env,
};
use crate::ingress_buffer::ingress_metrics_snapshot;
use crate::optimizer;
use crate::runtime_command_proxy::RuntimeCommandProxy;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_operational_profile::AxonRuntimeOperationalProfile;
use crate::runtime_profile::{
    canonical_watcher_first_priority_lanes, current_admission_controller_state,
    current_graph_production_state, current_runtime_priority_contract_state,
    current_vector_downstream_state, recommend_admission_controller_profile,
    recommend_embedding_lane_sizing, RuntimeProfile,
};
use crate::runtime_topology::{AxonProcessRole, RuntimeTopologyKind, RuntimeTopologyStatus};
use crate::runtime_truth_contract::RuntimeFreshnessState;
use crate::service_guard;
use crate::vector_control::{
    apply_semantic_policy_runtime_tuning, baseline_semantic_policy,
    current_gpu_vector_lease_diagnostics, current_utility_first_scheduler_diagnostics,
    current_vector_batch_controller_diagnostics, target_semantic_policy_with_graph,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use super::catalog::tools_catalog;
use super::format::{evidence_by_mode, format_standard_contract};
use super::McpServer;

type FrameworkCache = HashMap<String, (i64, Value)>;

static ANOMALIES_CACHE: OnceLock<Mutex<FrameworkCache>> = OnceLock::new();
static CONCEPTION_CACHE: OnceLock<Mutex<FrameworkCache>> = OnceLock::new();
static STATUS_CACHE: OnceLock<Mutex<FrameworkCache>> = OnceLock::new();
static WHY_CACHE: OnceLock<Mutex<FrameworkCache>> = OnceLock::new();

#[allow(dead_code)]
const FRAMEWORK_CACHE_TTL_MS: i64 = 5_000;
const CONCEPTION_CACHE_TTL_MS: i64 = 60_000;
const STATUS_CACHE_TTL_MS: i64 = 180_000;
const STATUS_FULL_CACHE_TTL_MS: i64 = 1_000;
const WHY_CACHE_TTL_MS: i64 = 180_000;
const ANOMALIES_CACHE_TTL_MS: i64 = 180_000;

#[derive(Debug, Clone)]
struct SplitPeerRuntimeInfo {
    runtime_truth_feed: RuntimeTruthFeed,
    release_version: Option<String>,
    build_id: Option<String>,
    install_generation: Option<String>,
    runtime_mode: Option<String>,
    runtime_telemetry: Option<Value>,
    runtime_state_present: bool,
}

impl McpServer {
    fn split_now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
    }

    fn split_run_root(project_root: &str, instance_kind: &str, role_slug: &str) -> PathBuf {
        let mut path = PathBuf::from(project_root);
        if instance_kind == "dev" {
            path.push(".axon-dev");
        } else {
            path.push(".axon");
        }
        path.push(format!("run-{role_slug}"));
        path
    }

    fn split_runtime_state_from_file(path: &PathBuf) -> Option<HashMap<String, String>> {
        let file = OpenOptions::new().read(true).open(path).ok()?;
        let reader = BufReader::new(file);
        let mut values = HashMap::new();
        for line in reader.lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((key, value)) = trimmed.split_once('=') else {
                continue;
            };
            values.insert(
                key.trim().to_string(),
                value.trim().trim_matches('"').to_string(),
            );
        }
        Some(values)
    }

    fn split_runtime_heartbeat_path(
        project_root: &str,
        instance_kind: &str,
        role_slug: &str,
    ) -> PathBuf {
        Self::split_run_root(project_root, instance_kind, role_slug).join("runtime-heartbeat.json")
    }

    fn split_runtime_truth_feed_from_heartbeat(
        path: &PathBuf,
    ) -> Option<(RuntimeTruthFeed, Value)> {
        let payload = fs::read_to_string(path).ok()?;
        let payload: Value = serde_json::from_str(&payload).ok()?;
        let runtime_truth_feed = payload
            .get("runtime_truth_feed")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .or_else(|| {
                let now_ms = Self::split_now_ms();
                Some(RuntimeTruthFeed::from_observed_times(
                    now_ms,
                    payload.get("last_heartbeat_at_ms").and_then(Value::as_u64),
                    payload
                        .get("last_good_payload_at_ms")
                        .and_then(Value::as_u64),
                    payload
                        .get("stale_after_ms")
                        .and_then(Value::as_u64)
                        .unwrap_or(RuntimeTruthFeed::DEFAULT_STALE_AFTER_MS),
                    payload
                        .get("degraded_reason")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .as_deref(),
                ))
            })?;
        Some((runtime_truth_feed, payload))
    }

    fn split_pid_file_path(project_root: &str, instance_kind: &str, role_slug: &str) -> PathBuf {
        Self::split_run_root(project_root, instance_kind, role_slug)
            .join(format!("axon-{role_slug}.pid"))
    }

    fn pid_is_live(pid: u32) -> bool {
        PathBuf::from(format!("/proc/{pid}")).exists()
    }

    fn file_mtime_ms(path: &PathBuf) -> Option<u64> {
        let modified = fs::metadata(path).ok()?.modified().ok()?;
        let elapsed = modified.duration_since(UNIX_EPOCH).ok()?;
        Some(elapsed.as_millis() as u64)
    }

    fn split_runtime_truth_feed_from_runtime_state(
        runtime_state_path: &PathBuf,
        pid_path: &PathBuf,
    ) -> Option<RuntimeTruthFeed> {
        let runtime_state = Self::split_runtime_state_from_file(runtime_state_path)?;
        let pid = fs::read_to_string(pid_path)
            .ok()
            .and_then(|raw| raw.trim().parse::<u32>().ok());
        let now_ms = Self::split_now_ms();
        let last_heartbeat_at_ms = Self::file_mtime_ms(runtime_state_path)
            .or_else(|| Self::file_mtime_ms(pid_path))
            .or(Some(now_ms));
        let degraded_reason = match pid {
            Some(pid) if Self::pid_is_live(pid) => None,
            Some(_) => Some("indexer_process_not_live"),
            None => Some("indexer_pid_missing"),
        };
        let stale_after_ms = RuntimeTruthFeed::DEFAULT_STALE_AFTER_MS;
        let mut feed = RuntimeTruthFeed::from_observed_times(
            now_ms,
            last_heartbeat_at_ms,
            last_heartbeat_at_ms,
            stale_after_ms,
            degraded_reason,
        );
        if degraded_reason.is_none() {
            feed.stale = false;
            feed.degraded_reason = None;
        }
        let _ = runtime_state;
        Some(feed)
    }

    fn split_peer_runtime_info(
        project_root: &str,
        instance_kind: &str,
        role_slug: &str,
    ) -> Option<SplitPeerRuntimeInfo> {
        let run_root = Self::split_run_root(project_root, instance_kind, role_slug);
        let runtime_state_path = run_root.join("runtime.env");
        let runtime_state = Self::split_runtime_state_from_file(&runtime_state_path);
        let pid_path = Self::split_pid_file_path(project_root, instance_kind, role_slug);
        let runtime_heartbeat_path =
            Self::split_runtime_heartbeat_path(project_root, instance_kind, role_slug);
        let (runtime_truth_feed, payload) = if let Some((feed, payload)) =
            Self::split_runtime_truth_feed_from_heartbeat(&runtime_heartbeat_path)
        {
            (feed, payload)
        } else {
            let feed =
                Self::split_runtime_truth_feed_from_runtime_state(&runtime_state_path, &pid_path)?;
            (feed, json!({}))
        };
        let release_version = payload
            .get("release_version")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .or_else(|| runtime_state.as_ref()?.get("AXON_RELEASE_VERSION").cloned());
        let build_id = payload
            .get("build_id")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .or_else(|| runtime_state.as_ref()?.get("AXON_BUILD_ID").cloned());
        let install_generation = payload
            .get("install_generation")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .or_else(|| {
                runtime_state
                    .as_ref()?
                    .get("AXON_INSTALL_GENERATION")
                    .cloned()
            });
        let runtime_mode = payload
            .get("runtime_mode")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .or_else(|| runtime_state.as_ref()?.get("AXON_RUNTIME_MODE").cloned());
        let runtime_telemetry = payload.get("runtime_telemetry").cloned();

        Some(SplitPeerRuntimeInfo {
            runtime_truth_feed,
            release_version,
            build_id,
            install_generation,
            runtime_mode,
            runtime_telemetry,
            runtime_state_present: runtime_state.is_some(),
        })
    }

    fn runtime_truth_feed_snapshot(feed: &RuntimeTruthFeed) -> Value {
        let state = if feed.stale {
            "stale"
        } else if feed.degraded_reason.is_some() {
            "degraded"
        } else {
            "fresh"
        };

        json!({
            "state": state,
            "stale": feed.stale,
            "observed_age_ms": feed.observed_age_ms,
            "stale_after_ms": feed.stale_after_ms,
            "last_heartbeat_at_ms": feed.last_heartbeat_at_ms,
            "last_good_payload_at_ms": feed.last_good_payload_at_ms,
            "degraded_reason": feed.degraded_reason
        })
    }

    fn runtime_topology_snapshot(&self, runtime_mode: AxonRuntimeMode) -> Value {
        let mut status = RuntimeTopologyStatus::legacy_compatibility_shim(runtime_mode);
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
        let shadow_role = std::env::var("AXON_RUNTIME_SHADOW_ROLE").ok();
        let split_shadow_only = std::env::var("AXON_SPLIT_SHADOW_ONLY")
            .ok()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false);
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

        if let Some(role) = shadow_role.as_deref().map(str::trim) {
            match role {
                "brain" | "brain_shadow" => {
                    if let Some(peer) =
                        Self::split_peer_runtime_info(&project_root, &instance_kind, "indexer")
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
                "indexer" | "indexer_shadow" => {
                    brain_ready = Self::split_runtime_state_from_file(
                        &Self::split_run_root(&project_root, &instance_kind, "brain")
                            .join("runtime.env"),
                    )
                    .is_some();
                    indexer_ready = true;
                }
                _ => {}
            }
        }

        let indexer_feed_healthy = !indexer_feed.stale && indexer_feed.degraded_reason.is_none();
        let ist_snapshot_healthy = matches!(ist_snapshot.state, RuntimeFreshnessState::Fresh);
        let split_ready =
            brain_ready && indexer_ready && indexer_feed_healthy && ist_snapshot_healthy;
        status.brain_ready = brain_ready;
        status.indexer_ready = indexer_ready;
        status.ist_snapshot = ist_snapshot.clone();
        status.system_converged = match shadow_role.as_deref().map(str::trim) {
            Some("brain") | Some("brain_shadow") | Some("indexer") | Some("indexer_shadow") => {
                !split_shadow_only && split_ready
            }
            _ => matches!(runtime_mode, AxonRuntimeMode::Full) && split_ready,
        };

        if let Some(role) = shadow_role.as_deref().map(str::trim) {
            match role {
                "brain" | "brain_shadow" => {
                    status.topology = RuntimeTopologyKind::BrainIndexerSplit;
                    status.process_role = AxonProcessRole::Brain;
                    status.public_mcp_authority = AxonProcessRole::Brain;
                    status.soll_writer_authority = AxonProcessRole::Brain;
                    status.ist_writer_authority = AxonProcessRole::Indexer;
                    status.brain_ready = true;
                    status.compatibility_shim = false;
                    status.compatibility_reason = split_shadow_only.then(|| {
                        format!("split brain shadow role active (shadow_only={split_shadow_only})")
                    });
                }
                "indexer" | "indexer_shadow" => {
                    status.topology = RuntimeTopologyKind::BrainIndexerSplit;
                    status.process_role = AxonProcessRole::Indexer;
                    status.public_mcp_authority = AxonProcessRole::Brain;
                    status.soll_writer_authority = AxonProcessRole::Brain;
                    status.ist_writer_authority = AxonProcessRole::Indexer;
                    status.indexer_ready = true;
                    status.compatibility_shim = false;
                    status.compatibility_reason = split_shadow_only.then(|| {
                        format!(
                            "split indexer shadow role active (shadow_only={split_shadow_only})"
                        )
                    });
                }
                _ => {}
            }
        }

        json!({
            "topology": status.topology,
            "process_role": status.process_role,
            "public_mcp_authority": status.public_mcp_authority,
            "soll_writer_authority": status.soll_writer_authority,
            "ist_writer_authority": status.ist_writer_authority,
            "brain_ready": status.brain_ready,
            "indexer_ready": status.indexer_ready,
            "system_converged": status.system_converged,
            "indexer_feed": Self::runtime_truth_feed_snapshot(&indexer_feed),
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

    fn priority_contract_snapshot(
        runtime_mode: &str,
        semantic_backlog_responsible: bool,
        ingress_buffered_entries: usize,
        structural_graph_backlog_depth: usize,
        graph_projection_queue_depth: usize,
        file_backlog_depth: usize,
        utility_scheduler: crate::vector_control::UtilityFirstSchedulerDiagnostics,
        semantic_profile: &'static str,
    ) -> Value {
        let lanes = canonical_watcher_first_priority_lanes();
        let priority_state = current_runtime_priority_contract_state(
            runtime_mode,
            ingress_buffered_entries,
            structural_graph_backlog_depth,
        );
        let semantic_runtime_enabled =
            !matches!(runtime_mode, "graph_only" | "read_only" | "mcp_only")
                && semantic_backlog_responsible;
        let vector_gate_active = semantic_runtime_enabled && priority_state.graph_backlog_present;

        json!({
            "contract_version": "watcher_graph_vector_v1",
            "authority_state": "declared_runtime_truth",
            "scheduler_enforcement_state": priority_state.enforcement_state,
            "pipeline_order": lanes.iter().map(|lane| {
                json!({
                    "lane": lane.lane,
                    "priority": lane.priority,
                    "admission_requires": lane.admission_requires
                })
            }).collect::<Vec<_>>(),
            "lane_gates": {
                "watcher_identification": {
                    "backlog_gated": priority_state.watcher_identification_backlog_gated,
                    "gate_kind": "none",
                    "current_gate_reason": "watcher_and_scan_admission_do_not_wait_for_graph_or_vector_backlog"
                },
                "graphing_after_enqueue": {
                    "backlog_gated": priority_state.graphing_after_enqueue_backlog_gated,
                    "gate_kind": if priority_state.graphing_after_enqueue_backlog_gated {
                        "upstream_ingress_priority"
                    } else {
                        "none"
                    },
                    "current_gate_reason": if priority_state.graphing_after_enqueue_backlog_gated {
                        "higher_priority_ingress_is_still_buffered_before_graph_ownership"
                    } else {
                        "graph_backlog_itself_is_the_actionable_lane"
                    }
                },
                "vectorization_after_graph_ready": {
                    "backlog_gated": priority_state.vectorization_after_graph_ready_backlog_gated,
                    "gate_kind": if vector_gate_active { "soft_priority_gate" } else { "none" },
                    "current_gate_reason": if vector_gate_active {
                        "graph_backlog_present_so_vectorization_is_contractually_deprioritized_after_graph_work"
                    } else if !semantic_runtime_enabled {
                        "semantic_runtime_not_owned_by_current_mode_or_instance"
                    } else {
                        "no_active_graph_priority_gate"
                    },
                    "semantic_policy_profile": semantic_profile,
                    "scheduler_state": utility_scheduler.state.as_str(),
                    "scheduler_reason": utility_scheduler.reason
                }
            },
            "backlog_scope": {
                "structural_graph_backlog_depth": structural_graph_backlog_depth,
                "graph_projection_queue_depth": graph_projection_queue_depth,
                "vector_queue_depth": file_backlog_depth
            },
            "vectorization_can_advance_ahead_of_graph_backlog": {
                "allowed_by_contract": priority_state.vectorization_allowed_ahead_of_graph_backlog,
                "allowed_under_current_runtime": semantic_runtime_enabled,
                "enforcement_state": if vector_gate_active {
                    "deprioritized_not_hard_blocked"
                } else if !semantic_runtime_enabled {
                    "not_owned_by_current_runtime"
                } else {
                    "allowed_without_active_graph_priority_gate"
                },
                "reason": if vector_gate_active {
                    "current_runtime_slows_semantic_work_when_graph_backlog_is_present_but_does_not_hard_block_it"
                } else if !semantic_runtime_enabled {
                    "current_runtime_or_instance_is_not_responsible_for_semantic_drain"
                } else {
                    "no_graph_priority_gate_is_active_now"
                }
            }
        })
    }

    fn admission_controller_snapshot(
        runtime_mode: &str,
        ingress: crate::ingress_buffer::IngressMetricsSnapshot,
        persisted_file_pending_current: usize,
        graph_wip_current: usize,
    ) -> Value {
        let profile = RuntimeProfile::detect();
        let controller_profile = recommend_admission_controller_profile(&profile);
        let pressure = service_guard::current_pressure();
        let controller_state = current_admission_controller_state(
            controller_profile,
            ingress.buffered_entries,
            ingress.hot_entries,
            ingress.scan_entries,
            persisted_file_pending_current,
            graph_wip_current,
            matches!(runtime_mode, "read_only" | "mcp_only"),
            matches!(pressure, service_guard::ServicePressure::Critical),
        );

        json!({
            "owner": "admission_controller",
            "control_model_state": "proposed",
            "buffered_discovery_current": ingress.buffered_entries,
            "watcher_buffered_current": ingress.hot_entries,
            "scan_buffered_current": ingress.scan_entries,
            "persisted_file_pending_current": persisted_file_pending_current,
            "admission_flush_count": ingress.flush_count,
            "admission_last_flush_duration_ms": ingress.last_flush_duration_ms,
            "admission_last_promoted_count": ingress.last_promoted_count,
            "admission_promoted_total": ingress.promoted_total,
            "admission_last_durably_persisted_count": ingress.last_durably_persisted_count,
            "admission_durably_persisted_total": ingress.durably_persisted_total,
            "admission_last_excluded_from_pending_count": ingress.last_excluded_from_pending_count,
            "admission_excluded_from_pending_total": ingress.excluded_from_pending_total,
            "graph_wip_current": graph_wip_current,
            "admission_wip_current": graph_wip_current,
            "target_band": controller_state.profile.target_band,
            "reorder_point": controller_state.profile.reorder_point,
            "max_wip": controller_state.profile.max_wip,
            "hold_window_ms": controller_state.profile.hold_window_ms,
            "forced_bulk_fill_threshold": controller_state.profile.forced_bulk_fill_threshold,
            "admission_completion_surface": "File(status='pending', graph_ready=FALSE, eligible_for_graph=TRUE)",
            "admission_completion_diagnostics": {
                "flush_happened": ingress.flush_count > 0,
                "durable_file_persistence_completed": ingress.last_durably_persisted_count > 0,
                "persisted_but_excluded_from_pending": ingress.last_excluded_from_pending_count > 0,
            },
            "diagnostic_notes": {
                "durably_persisted_count_semantics": "Counts promoted paths that are durably visible in File after the flush wave, not only newly inserted rows."
            },
            "blocking_authority": controller_state.blocking_authority,
            "allowed_by_contract": !matches!(runtime_mode, "read_only" | "mcp_only"),
            "allowed_under_current_runtime": controller_state.admission_open,
            "bulk_fill_preferred": controller_state.bulk_fill_preferred,
            "watcher_hot_priority": ingress.hot_entries > 0,
            "notes": "Controls the canonical buffered_discovery -> persisted_file_pending handoff."
        })
    }

    fn canonical_edge_control_snapshot(
        runtime_mode: &str,
        ingress: crate::ingress_buffer::IngressMetricsSnapshot,
        persisted_file_pending_current: usize,
        graph_wip_current: usize,
        graph_ready_current: usize,
        structural_graph_backlog_depth: usize,
    ) -> Value {
        let profile = RuntimeProfile::detect();
        let controller_profile = recommend_admission_controller_profile(&profile);
        let pressure = service_guard::current_pressure();
        let runtime_processing_disabled = matches!(runtime_mode, "read_only" | "mcp_only");
        let critical_pressure = matches!(pressure, service_guard::ServicePressure::Critical);
        let semantic_runtime_enabled =
            !matches!(runtime_mode, "graph_only" | "read_only" | "mcp_only");

        let admission = current_admission_controller_state(
            controller_profile,
            ingress.buffered_entries,
            ingress.hot_entries,
            ingress.scan_entries,
            persisted_file_pending_current,
            graph_wip_current,
            runtime_processing_disabled,
            critical_pressure,
        );
        let graph = current_graph_production_state(
            persisted_file_pending_current,
            graph_wip_current,
            (controller_profile.max_wip / 2).max(1),
            runtime_processing_disabled,
            critical_pressure,
        );
        let vector = current_vector_downstream_state(
            graph_ready_current,
            structural_graph_backlog_depth,
            semantic_runtime_enabled,
            critical_pressure,
        );

        json!({
            "admission_edge": {
                "owner": "admission_controller",
                "boundary": "buffered_discovery_to_persisted_file_pending",
                "blocking_authority": admission.blocking_authority,
                "allowed_by_contract": !runtime_processing_disabled,
                "allowed_under_current_runtime": admission.admission_open,
                "source_stock_current": ingress.buffered_entries,
                "target_stock_current": persisted_file_pending_current,
                "wip_current": graph_wip_current,
            },
            "graph_production_edge": {
                "owner": "graph_production_controller",
                "boundary": "persisted_file_pending_to_graph_ready",
                "blocking_authority": graph.blocking_authority,
                "allowed_by_contract": !runtime_processing_disabled,
                "allowed_under_current_runtime": graph.graph_open,
                "source_stock_current": persisted_file_pending_current,
                "target_stock_current": graph_ready_current,
                "wip_current": graph_wip_current,
            },
            "vector_downstream_edge": {
                "owner": "vector_downstream_controller",
                "boundary": "graph_ready_to_vector_ready",
                "blocking_authority": vector.blocking_authority,
                "allowed_by_contract": semantic_runtime_enabled,
                "allowed_under_current_runtime": vector.vector_open,
                "source_stock_current": graph_ready_current,
                "target_stock_current": 0,
                "wip_current": structural_graph_backlog_depth,
            }
        })
    }

    fn loop_semantics_snapshot() -> Value {
        json!({
            "upstream_push_loop": {
                "mode": "push",
                "boundary": "buffered_discovery_to_graph_ready",
                "summary_scope": "high_level_loop_summary",
                "control_layers": [
                    "supply_discovery",
                    "admission_production"
                ],
                "stages": [
                    "buffered_discovery",
                    "persisted_file_pending",
                    "graph_ready"
                ],
                "critical_throughput_stock": "persisted_file_pending",
                "notes": "Watcher and scan should create and replenish canonical work without waiting for GPU cadence. This high-level push loop aggregates supply/discovery and admission/production; persisted_file_pending remains the primary global throughput stock unless runtime evidence disproves it."
            },
            "gpu_paced_downstream_loop": {
                "mode": "pull",
                "boundary": "graph_ready_to_vector_ready",
                "paced_by": "gpu_capacity_and_vram",
                "stages": [
                    "graph_ready",
                    "prepare",
                    "ready_batches",
                    "gpu",
                    "vector_ready"
                ],
                "idle_when_source_stock_empty": true,
                "notes": "The downstream lane should wake and drain according to real GPU demand and may idle cleanly when graph_ready is empty."
            },
            "finalize": {
                "mode": "async",
                "hot_path": false,
                "notes": "Finalize is downstream of GPU execution and must not sit on the hot feed path unless a hard safety invariant requires it."
            }
        })
    }

    fn canonical_ingestion_stage_model_snapshot(&self) -> Value {
        let ingress = ingress_metrics_snapshot();
        let persisted_file_count = self
            .graph_store
            .query_count("SELECT count(*) FROM File")
            .unwrap_or(0);
        let structural_graph_queued_count =
            self.graph_store.count_persisted_file_pending().unwrap_or(0);
        let structural_graph_inflight_count = self.graph_store.count_graph_wip_files().unwrap_or(0);
        let structural_graph_backlog_count =
            structural_graph_queued_count.saturating_add(structural_graph_inflight_count);
        let (graph_queue_queued_count, graph_queue_inflight_count) = self
            .graph_store
            .fetch_graph_projection_queue_counts()
            .unwrap_or((0usize, 0usize));
        let graph_queue_owned_count =
            u64::try_from(graph_queue_queued_count.saturating_add(graph_queue_inflight_count))
                .unwrap_or(0);
        let graph_ready_count = self
            .graph_store
            .query_count("SELECT count(*) FROM File WHERE COALESCE(graph_ready, FALSE) = TRUE")
            .unwrap_or(0);
        let vector_queue_owned_count = self
            .graph_store
            .query_count(
                "SELECT count(*) FROM FileVectorizationQueue WHERE status IN ('queued', 'paused_for_interactive_priority', 'inflight')",
            )
            .unwrap_or(0);
        let vector_ready_count = self
            .graph_store
            .query_count("SELECT count(*) FROM File WHERE COALESCE(vector_ready, FALSE) = TRUE")
            .unwrap_or(0);
        let explicitly_excluded_count = self
            .graph_store
            .query_count(
                "SELECT count(*) FROM File \
                 WHERE status IN ('deleted', 'skipped', 'oversized_for_current_budget') \
                    OR COALESCE(file_stage, '') IN ('deleted', 'skipped', 'oversized')",
            )
            .unwrap_or(0);

        json!({
            "authority_state": "canonical",
            "model_version": "watcher_file_graph_vector_v1",
            "freshness": {
                "brief_status_cache_ttl_ms": STATUS_CACHE_TTL_MS,
                "full_status_cache_ttl_ms": STATUS_FULL_CACHE_TTL_MS,
                "recommended_mode_for_current_counts": "full",
                "notes": "Brief status is cached. Use status(mode=\"full\") when exact current counts matter."
            },
            "ingress_buffered": {
                "status": "tracked",
                "ownership_surface": "ingress_buffer",
                "current_count": ingress.buffered_entries as u64,
                "notes": "All buffered ingress entries before canonical File persistence."
            },
            "watcher_buffered": {
                "status": "tracked",
                "stage_labels": ["buffered", "staged"],
                "ownership_surface": "ingress_buffer",
                "current_count": ingress.hot_entries as u64,
                "notes": "Watcher-originated ingress still buffered before canonical File persistence."
            },
            "scan_buffered": {
                "status": "tracked",
                "stage_labels": ["buffered", "staged"],
                "ownership_surface": "ingress_buffer",
                "current_count": ingress.scan_entries as u64,
                "notes": "Scan-originated ingress still buffered before canonical File persistence."
            },
            "ingress_promotion": {
                "status": "tracked",
                "ownership_surface": "ingress_buffer",
                "flush_count": ingress.flush_count,
                "last_flush_duration_ms": ingress.last_flush_duration_ms,
                "last_promoted_count": ingress.last_promoted_count,
                "promoted_total": ingress.promoted_total,
                "last_durably_persisted_count": ingress.last_durably_persisted_count,
                "durably_persisted_total": ingress.durably_persisted_total,
                "last_excluded_from_pending_count": ingress.last_excluded_from_pending_count,
                "excluded_from_pending_total": ingress.excluded_from_pending_total,
                "notes": "Ingress promoter activity over the current runtime lifetime. Durable persistence counts mean promoted paths are durably visible in File after a flush wave, not only newly inserted rows."
            },
            "persisted_file": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": persisted_file_count
            },
            "persisted_file_pending": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": structural_graph_queued_count,
                "notes": "Durably persisted canonical file work that is still eligible and pending graph production."
            },
            "graph_wip": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": structural_graph_inflight_count,
                "notes": "Canonical file work currently owned by the graph worker pool."
            },
            "structural_graph_backlog": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": structural_graph_backlog_count,
                "queue_breakdown": {
                    "queued": structural_graph_queued_count,
                    "inflight": structural_graph_inflight_count
                },
                "notes": "Canonical file graphing backlog before graph_ready. This is the primary graphing stage for watcher->graph->vector."
            },
            "graph_projection_queue_owned": {
                "status": "tracked",
                "ownership_surface": "GraphProjectionQueue",
                "current_count": graph_queue_owned_count,
                "queue_breakdown": {
                    "queued": graph_queue_queued_count,
                    "inflight": graph_queue_inflight_count
                },
                "notes": "Secondary graph projection or graph embedding work. Diagnostic only; not the canonical file graphing backlog."
            },
            "graph_ready": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": graph_ready_count
            },
            "file_vectorization_queue_owned": {
                "status": "tracked",
                "ownership_surface": "FileVectorizationQueue",
                "current_count": vector_queue_owned_count,
                "notes": "These files are currently owned by the vector lane for downstream work."
            },
            "vector_ready": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": vector_ready_count
            },
            "explicitly_excluded_from_vectorization": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": explicitly_excluded_count,
                "notes": "Counted from explicit deleted/skipped/oversized file states only."
            }
        })
    }

    fn lane_parameter_snapshot(
        seed: usize,
        target: usize,
        effective: usize,
        clamp_reason: Option<&'static str>,
        authority_state: &'static str,
        target_source: &'static str,
        effective_source: &'static str,
    ) -> Value {
        json!({
            "seed": seed,
            "target": target,
            "effective": effective,
            "clamp_visible": clamp_reason.is_some(),
            "clamp_reason": clamp_reason,
            "authority_state": authority_state,
            "target_source": target_source,
            "effective_source": effective_source
        })
    }

    fn runtime_lane_authority_snapshot(
        structural_graph_backlog_depth: usize,
        file_backlog_depth: usize,
    ) -> Value {
        let profile = RuntimeProfile::detect();
        let provider_requested =
            std::env::var("AXON_EMBEDDING_PROVIDER").unwrap_or_else(|_| "cpu".to_string());
        let mut lane_profile = profile.clone();
        lane_profile.gpu_present =
            profile.gpu_present && provider_requested.eq_ignore_ascii_case("cuda");
        let seed = recommend_embedding_lane_sizing(&lane_profile);
        let runtime_seed = bootstrap_runtime_tuning_state();
        let target = current_runtime_tuning_state();
        let effective = embedding_lane_config_from_env();
        let batch_controller = current_vector_batch_controller_diagnostics(&effective);
        let gpu_vector_lease = current_gpu_vector_lease_diagnostics();
        let vector_runtime = service_guard::vector_runtime_metrics();
        let cadence_seed = baseline_semantic_policy(file_backlog_depth);
        let cadence_target = target_semantic_policy_with_graph(
            file_backlog_depth,
            structural_graph_backlog_depth,
            service_guard::current_pressure(),
        );
        let cadence_effective = apply_semantic_policy_runtime_tuning(cadence_target);
        let oversubscription_allowed = std::env::var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION")
            .ok()
            .map(|value| value.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let vector_workers_clamp_reason = if effective.vector_workers != target.vector_workers {
            if provider_requested.eq_ignore_ascii_case("cuda")
                && !oversubscription_allowed
                && target.vector_workers > effective.vector_workers
            {
                Some("hard_safety_clamp:gpu_vector_workers_capped_without_oversubscription")
            } else {
                Some("clamp_visible:effective_vector_workers_diverge_from_target")
            }
        } else {
            None
        };

        let graph_workers_clamp_reason = if effective.graph_workers != target.graph_workers {
            if target.graph_workers > seed.graph_workers {
                Some("startup_bound:graph_worker_pool_cannot_exceed_bootstrap")
            } else if seed.graph_workers > 0 && effective.graph_workers == 0 {
                Some("hard_safety_clamp:graph_embeddings_disabled")
            } else {
                Some("runtime_tuning:graph_worker_pool_admission_reduced")
            }
        } else {
            None
        };

        let chunk_batch_clamp_reason = if effective.chunk_batch_size != target.chunk_batch_size {
            Some("clamp_visible:effective_chunk_batch_size_diverges_from_target")
        } else {
            None
        };

        let file_batch_clamp_reason =
            if effective.file_vectorization_batch_size != target.file_vectorization_batch_size {
                Some("clamp_visible:effective_file_vectorization_batch_size_diverges_from_target")
            } else {
                None
            };

        json!({
            "vector_workers": Self::lane_parameter_snapshot(
                seed.vector_workers,
                target.vector_workers,
                effective.vector_workers,
                vector_workers_clamp_reason,
                "partially_unified",
                "runtime_tuning_controller",
                "embedding_lane_config",
            ),
            "graph_workers": Self::lane_parameter_snapshot(
                seed.graph_workers,
                target.graph_workers,
                effective.graph_workers,
                graph_workers_clamp_reason,
                "partially_unified",
                "runtime_tuning_controller",
                "embedding_lane_config",
            ),
            "chunk_batch_size": Self::lane_parameter_snapshot(
                seed.chunk_batch_size,
                target.chunk_batch_size,
                effective.chunk_batch_size,
                chunk_batch_clamp_reason,
                "partially_unified",
                "runtime_tuning_controller",
                "embedding_lane_config",
            ),
            "file_vectorization_batch_size": Self::lane_parameter_snapshot(
                seed.file_vectorization_batch_size,
                target.file_vectorization_batch_size,
                effective.file_vectorization_batch_size,
                file_batch_clamp_reason,
                "partially_unified",
                "runtime_tuning_controller",
                "embedding_lane_config",
            ),
            "vector_ready_queue_depth": Self::lane_parameter_snapshot(
                runtime_seed.vector_ready_queue_depth,
                target.vector_ready_queue_depth,
                vector_runtime.ready_queue_depth_current as usize,
                None,
                "partially_unified",
                "runtime_tuning_controller",
                "service_guard.current_ready_queue_depth",
            ),
            "vector_persist_queue_bound": Self::lane_parameter_snapshot(
                runtime_seed.vector_persist_queue_bound,
                target.vector_persist_queue_bound,
                vector_runtime.persist_queue_depth_current as usize,
                None,
                "partially_unified",
                "runtime_tuning_controller",
                "service_guard.current_persist_queue_depth",
            ),
            "vector_max_inflight_persists": Self::lane_parameter_snapshot(
                runtime_seed.vector_max_inflight_persists,
                target.vector_max_inflight_persists,
                vector_runtime.persist_claimed_current as usize,
                None,
                "partially_unified",
                "runtime_tuning_controller",
                "service_guard.current_persist_claims",
            ),
            "queue_persist_effective_semantics": {
                "vector_ready_queue_depth": "observed_current_queue_depth_not_capacity",
                "vector_persist_queue_bound": "observed_current_queue_depth_not_capacity",
                "vector_max_inflight_persists": "observed_current_inflight_not_limit"
            },
            "semantic_cadence": {
                "seed": {
                    "profile": cadence_seed.profile,
                    "pause": cadence_seed.pause,
                    "sleep_ms": cadence_seed.sleep.as_millis() as u64,
                    "idle_sleep_ms": cadence_seed.idle_sleep.as_millis() as u64,
                },
                "target": {
                    "profile": cadence_target.profile,
                    "pause": cadence_target.pause,
                    "sleep_ms": cadence_target.sleep.as_millis() as u64,
                    "idle_sleep_ms": cadence_target.idle_sleep.as_millis() as u64,
                },
                "effective": {
                    "profile": cadence_effective.profile,
                    "pause": cadence_effective.pause,
                    "sleep_ms": cadence_effective.sleep.as_millis() as u64,
                    "idle_sleep_ms": cadence_effective.idle_sleep.as_millis() as u64,
                },
                "clamp_visible": cadence_effective.sleep != cadence_target.sleep
                    || cadence_effective.idle_sleep != cadence_target.idle_sleep,
                "clamp_reason": if cadence_effective.sleep != cadence_target.sleep
                    || cadence_effective.idle_sleep != cadence_target.idle_sleep
                {
                    json!("runtime_tuning_scale_pct")
                } else {
                    Value::Null
                },
                "authority_state": "partially_unified",
                "target_source": "semantic_policy_controller",
                "effective_source": "runtime_tuning_scaled_policy",
                "controller_state": batch_controller.state.as_str(),
                "controller_reason": batch_controller.reason,
            },
            "gpu_vector_lease": {
                "exclusive_required": gpu_vector_lease.exclusive_required,
                "path": gpu_vector_lease.path,
                "owned_by_current_instance": gpu_vector_lease.owned_by_current_instance,
                "owner_identity": gpu_vector_lease.owner_identity
            }
        })
    }

    fn env_interval_ms(key: &str, default: u64, min: u64) -> u64 {
        std::env::var(key)
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|value| *value >= min)
            .unwrap_or(default)
    }

    fn quiescent_runtime_snapshot(
        runtime_mode: &str,
        semantic_backlog_responsible: bool,
        structural_graph_backlog_depth: usize,
        graph_projection_queue_depth: usize,
        file_backlog_depth: usize,
        semantic_profile: &'static str,
        canonical_chunks_embedded_last_minute: u64,
        canonical_files_embedded_last_minute: u64,
    ) -> Value {
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let interactive_priority = service_guard::current_interactive_priority();
        let interactive_requests = service_guard::interactive_requests_in_flight();
        let pressure = service_guard::current_pressure();
        let pressure_label = match pressure {
            service_guard::ServicePressure::Healthy => "healthy",
            service_guard::ServicePressure::Recovering => "recovering",
            service_guard::ServicePressure::Degraded => "degraded",
            service_guard::ServicePressure::Critical => "critical",
        };
        let vector_runtime = service_guard::vector_runtime_metrics();
        let embedding_provider = current_embedding_provider_diagnostics();
        let gpu_access_policy =
            std::env::var("AXON_GPU_ACCESS_POLICY").unwrap_or_else(|_| "unknown".to_string());
        let wake_summary = service_guard::runtime_wake_summary(
            structural_graph_backlog_depth as u64,
            file_backlog_depth as u64,
        );
        let utility_scheduler = current_utility_first_scheduler_diagnostics(
            structural_graph_backlog_depth,
            file_backlog_depth,
            pressure,
        );
        let dominant_wake_source = [
            ("background", wake_summary.wake_source_background_total),
            (
                "semantic_vector",
                wake_summary.wake_source_semantic_vector_total,
            ),
            ("graph", wake_summary.wake_source_graph_total),
        ]
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(label, _)| label)
        .unwrap_or("background");
        let dominant_wake_total = match dominant_wake_source {
            "semantic_vector" => wake_summary.wake_source_semantic_vector_total,
            "graph" => wake_summary.wake_source_graph_total,
            _ => wake_summary.wake_source_background_total,
        };
        let total_wake_sources = wake_summary
            .wake_source_background_total
            .saturating_add(wake_summary.wake_source_semantic_vector_total)
            .saturating_add(wake_summary.wake_source_graph_total);
        let dominant_wake_share_pct = if total_wake_sources > 0 {
            dominant_wake_total.saturating_mul(100) / total_wake_sources
        } else {
            0
        };
        let wake_noise_level = match wake_summary.wakeups_last_60s {
            0..=2 => "low",
            3..=10 => "moderate",
            _ => "high",
        };
        let vector_worker_heartbeat_age_ms =
            now_ms.saturating_sub(vector_runtime.vector_worker_heartbeat_at_ms);
        let vector_lane_last_success_age_ms =
            now_ms.saturating_sub(vector_runtime.vector_lane_last_success_at_ms);
        let vector_lane_last_fault_age_ms =
            now_ms.saturating_sub(vector_runtime.vector_lane_last_fault_at_ms);
        let reader_refresh_interval_ms =
            Self::env_interval_ms("AXON_READER_REFRESH_INTERVAL_MS", 5_000, 250);
        let optimizer_loop_interval_ms =
            Self::env_interval_ms("AXON_OPT_LOOP_INTERVAL_MS", 15_000, 1);
        let runtime_trace_interval_ms =
            Self::env_interval_ms("AXON_RUNTIME_TRACE_INTERVAL_MS", 5_000, 1_000);
        let watcher_promoter_poll_interval_ms = 50_u64;

        let (effective_graph_backlog_depth, effective_semantic_backlog_depth, backlog_scope) =
            match runtime_mode {
                "graph_only" => (structural_graph_backlog_depth, 0_usize, "graph_only"),
                "read_only" | "mcp_only" => (0_usize, 0_usize, "runtime_processing_disabled"),
                _ => (
                    structural_graph_backlog_depth,
                    if semantic_backlog_responsible {
                        file_backlog_depth
                    } else {
                        0_usize
                    },
                    if semantic_backlog_responsible {
                        "graph_and_semantic"
                    } else {
                        "graph_only_current_instance_responsibility"
                    },
                ),
            };

        let state = if interactive_requests > 0 {
            "interactive_guarded"
        } else if effective_graph_backlog_depth > 0 || effective_semantic_backlog_depth > 0 {
            "active_backlog"
        } else if vector_runtime.ready_queue_depth_current > 0
            || vector_runtime.prepare_queue_depth_current > 0
            || vector_runtime.persist_queue_depth_current > 0
            || vector_runtime.active_claimed_current > 0
        {
            "draining_residual_work"
        } else {
            "quiescent_candidate"
        };
        let healthy_semantic_drain_candidate = state == "active_backlog"
            && effective_semantic_backlog_depth > 0
            && vector_runtime.vector_lane_state == service_guard::VectorLaneState::Healthy
            && vector_worker_heartbeat_age_ms <= 5_000
            && vector_lane_last_success_age_ms <= 15_000
            && vector_runtime.active_claimed_current > 0;
        let semantic_progress_measured =
            canonical_files_embedded_last_minute > 0 || canonical_chunks_embedded_last_minute > 0;
        let semantic_drain_health = if !semantic_backlog_responsible
            || effective_semantic_backlog_depth == 0
        {
            if embedding_provider.provider_effective == "cuda" && file_backlog_depth > 0 {
                "gpu_lease_not_owned"
            } else {
                "not_applicable"
            }
        } else if embedding_provider.provider_effective == "cpu" && gpu_access_policy == "avoid" {
            "cpu_policy_limited"
        } else if healthy_semantic_drain_candidate && semantic_progress_measured {
            "healthy_draining"
        } else if healthy_semantic_drain_candidate {
            "warming_or_long_batch"
        } else if utility_scheduler.semantic_underfeed
            || (vector_runtime.active_claimed_current == 0
                && vector_runtime.ready_queue_depth_current == 0
                && vector_runtime.prepare_queue_depth_current == 0
                && vector_runtime.persist_queue_depth_current == 0)
        {
            "underfed"
        } else if vector_lane_last_success_age_ms > 30_000
            && vector_worker_heartbeat_age_ms > 10_000
        {
            "stalled"
        } else {
            "draining_uncertain"
        };
        let semantic_drain_recommendation = match semantic_drain_health {
            "healthy_draining" => "measure_backlog_burn_rate",
            "gpu_lease_not_owned" => "qualify_on_gpu_owner_instance_or_release_lease",
            "cpu_policy_limited" => "use_gpu_qualified_runtime_for_throughput_measurement",
            "warming_or_long_batch" => "extend_burn_rate_probe_before_calling_stall",
            "underfed" => "investigate_semantic_feed_path",
            "stalled" => "investigate_semantic_lane_stall",
            "draining_uncertain" => "observe_drain_progress_over_time",
            _ => "not_applicable",
        };
        let estimated_semantic_backlog_drain_minutes =
            if effective_semantic_backlog_depth > 0 && canonical_files_embedded_last_minute > 0 {
                Some(
                    ((effective_semantic_backlog_depth as f64)
                        / (canonical_files_embedded_last_minute as f64))
                        .ceil(),
                )
            } else {
                None
            };
        let burn_rate_state = match semantic_drain_health {
            "not_applicable" => "not_applicable",
            "healthy_draining"
                if canonical_files_embedded_last_minute > 0
                    || canonical_chunks_embedded_last_minute > 0 =>
            {
                "measurable_progress"
            }
            "healthy_draining" => "warming_or_long_batch",
            "underfed" => "not_progressing_underfed",
            "stalled" => "not_progressing_stalled",
            _ => "progress_uncertain",
        };
        let burn_rate_recommendation = match burn_rate_state {
            "measurable_progress" => "track_burn_rate_until_backlog_turns_down",
            "warming_or_long_batch" => "observe_one_more_measurement_window",
            "not_progressing_underfed" => "repair_semantic_feed_before_idle_tuning",
            "not_progressing_stalled" => "repair_semantic_lane_before_idle_tuning",
            "progress_uncertain" => "observe_another_window_before_concluding",
            _ => "not_applicable",
        };
        let healthy_semantic_drain_in_progress = semantic_drain_health == "healthy_draining";
        let operator_focus = match (
            dominant_wake_source,
            wake_summary.dominant_background_wake_detail,
            wake_summary.last_quiescent_exit_reason,
            state,
        ) {
            _ if semantic_drain_health == "gpu_lease_not_owned" => {
                "gpu_vector_lease_not_owned_by_current_instance"
            }
            _ if semantic_drain_health == "cpu_policy_limited" => {
                "cpu_vector_lane_policy_limits_throughput"
            }
            _ if healthy_semantic_drain_in_progress => "healthy_semantic_drain_in_progress",
            _ if semantic_drain_health == "warming_or_long_batch" => {
                "semantic_drain_long_batch_probe_in_progress"
            }
            ("background", "federation_orchestrator", _, "active_backlog") => {
                "project_discovery_is_active_under_backlog"
            }
            ("background", "ingress_promoter", _, _) => {
                "ingress_promoter_dominates_background_wakes"
            }
            ("background", "autonomous_ingestor", _, _) => {
                "autonomous_ingestor_dominates_background_wakes"
            }
            ("background", _, _, "quiescent_candidate") => "background_loops_still_dominate_idle",
            ("semantic_vector", _, _, "quiescent_candidate") => {
                "semantic_lane_still_wakes_while_idle"
            }
            ("graph", _, _, "quiescent_candidate") => "graph_lane_still_wakes_while_idle",
            (_, _, "interactive_guarded", _) => "interactive_load_breaks_quiescence",
            (_, _, "draining_residual_work", _) => "residual_work_prevents_full_quiescence",
            (_, _, "active_backlog", _) => "backlog_prevents_quiescence",
            _ => "quiescent_state_nominal",
        };
        let focus_recommendation = match operator_focus {
            "gpu_vector_lease_not_owned_by_current_instance" => {
                "qualify_on_gpu_owner_instance_or_release_lease"
            }
            "cpu_vector_lane_policy_limits_throughput" => {
                "qualify_on_gpu_enabled_runtime_or_override_policy"
            }
            "healthy_semantic_drain_in_progress" => "measure_backlog_burn_rate_not_idle_tuning",
            "semantic_drain_long_batch_probe_in_progress" => {
                "extend_burn_rate_probe_before_calling_stall"
            }
            "project_discovery_is_active_under_backlog" => {
                "measure_again_after_registry_stabilizes"
            }
            "ingress_promoter_dominates_background_wakes" => "tighten_ingress_promoter_first",
            "autonomous_ingestor_dominates_background_wakes" => "tighten_autonomous_ingestor_first",
            "background_loops_still_dominate_idle" => "tighten_background_pollers_first",
            "semantic_lane_still_wakes_while_idle" => "tighten_semantic_lane_idle_first",
            "graph_lane_still_wakes_while_idle" => "tighten_graph_lane_idle_first",
            "interactive_load_breaks_quiescence" => "measure_under_lower_interactive_load",
            "residual_work_prevents_full_quiescence" => "drain_residual_queues_before_idle_tuning",
            "backlog_prevents_quiescence" => "measure_again_after_backlog_drain",
            _ => "no_immediate_quiescent_hotspot_detected",
        };
        let confidence = if total_wake_sources < 3 {
            "low"
        } else if dominant_wake_share_pct >= 70 {
            "high"
        } else if dominant_wake_share_pct >= 50 {
            "medium"
        } else {
            "low"
        };
        let measurement_readiness = match (state, confidence, wake_noise_level) {
            ("quiescent_candidate", "high", _) => "actionable",
            ("quiescent_candidate", "medium", "low" | "moderate") => "actionable_with_caution",
            ("quiescent_candidate", _, "high") => "observe_longer_before_tuning",
            ("active_backlog", _, _) => "blocked_by_backlog",
            ("draining_residual_work", _, _) => "blocked_by_residual_work",
            ("interactive_guarded", _, _) => "blocked_by_interactive_load",
            _ => "observe_longer_before_tuning",
        };
        let measurement_readiness = if operator_focus == "project_discovery_is_active_under_backlog"
        {
            "blocked_by_project_discovery"
        } else if operator_focus == "gpu_vector_lease_not_owned_by_current_instance" {
            "blocked_by_gpu_vector_lease"
        } else if operator_focus == "cpu_vector_lane_policy_limits_throughput" {
            "blocked_by_cpu_vector_policy"
        } else if operator_focus == "semantic_drain_long_batch_probe_in_progress" {
            "blocked_by_long_batch_probe"
        } else if operator_focus == "healthy_semantic_drain_in_progress" {
            "blocked_by_healthy_semantic_drain"
        } else {
            measurement_readiness
        };
        let recommended_next_measurement = match measurement_readiness {
            "actionable" => "tune_dominant_wake_source_now",
            "actionable_with_caution" => "tune_gently_and_remeasure",
            "blocked_by_gpu_vector_lease" => "qualify_on_gpu_owner_instance",
            "blocked_by_cpu_vector_policy" => "qualify_on_gpu_enabled_runtime",
            "blocked_by_healthy_semantic_drain" => "measure_semantic_backlog_burn_rate",
            "blocked_by_long_batch_probe" => "extend_semantic_burn_rate_probe",
            "blocked_by_project_discovery" => "rerun_after_registry_stabilizes",
            "blocked_by_backlog" => "rerun_after_backlog_drain",
            "blocked_by_residual_work" => "rerun_after_residual_drain",
            "blocked_by_interactive_load" => "rerun_under_lower_interactive_load",
            _ => "extend_observation_window",
        };
        let qualification_verdict = match (state, confidence, wake_noise_level) {
            ("quiescent_candidate", "high", "low" | "moderate") => "pass",
            ("quiescent_candidate", "medium", "low" | "moderate") => "watch",
            ("quiescent_candidate", _, "high") => "watch",
            ("active_backlog", _, _) => "blocked",
            ("draining_residual_work", _, _) => "blocked",
            ("interactive_guarded", _, _) => "blocked",
            _ => "watch",
        };
        let qualification_reason = match qualification_verdict {
            "pass" => "quiescent_state_is_stable_enough_to_act",
            "blocked" => measurement_readiness,
            _ => "signal_exists_but_is_not_yet_strong_enough",
        };
        let mut blocking_factors = Vec::new();
        if operator_focus == "healthy_semantic_drain_in_progress" {
            blocking_factors.push("healthy_semantic_drain_active");
        }
        if operator_focus == "gpu_vector_lease_not_owned_by_current_instance" {
            blocking_factors.push("gpu_vector_lease_not_owned");
        }
        if operator_focus == "cpu_vector_lane_policy_limits_throughput" {
            blocking_factors.push("cpu_vector_policy_active");
        }
        if operator_focus == "semantic_drain_long_batch_probe_in_progress" {
            blocking_factors.push("long_batch_probe_active");
        }
        if operator_focus == "project_discovery_is_active_under_backlog" {
            blocking_factors.push("project_discovery_active");
        }
        if state == "active_backlog" {
            blocking_factors.push("backlog_active");
        }
        if state == "draining_residual_work" {
            blocking_factors.push("residual_work_active");
        }
        if state == "interactive_guarded" {
            blocking_factors.push("interactive_load_active");
        }
        if confidence == "low" {
            blocking_factors.push("low_confidence_signal");
        }
        if wake_noise_level == "high" {
            blocking_factors.push("high_wake_noise");
        }
        let actionable_now = matches!(
            measurement_readiness,
            "actionable" | "actionable_with_caution"
        );

        json!({
            "state": state,
            "authority_state": "transitional",
            "wake_contract_state": "fragmented",
            "wake_observability_state": "partial",
            "diagnosis": {
                "operator_focus": operator_focus,
                "focus_recommendation": focus_recommendation,
                "dominant_background_wake_detail": wake_summary.dominant_background_wake_detail,
                "confidence": confidence,
                "wake_noise_level": wake_noise_level,
                "dominant_wake_share_pct": dominant_wake_share_pct,
                "measurement_readiness": measurement_readiness,
                "recommended_next_measurement": recommended_next_measurement,
                "qualification_verdict": qualification_verdict,
                "qualification_reason": qualification_reason,
                "actionable_now": actionable_now,
                "blocking_factors": blocking_factors
            },
            "interactive_priority": interactive_priority.as_str(),
            "interactive_requests_in_flight": interactive_requests,
            "service_pressure": pressure_label,
            "backlog_scope": backlog_scope,
            "semantic_backlog_responsible": semantic_backlog_responsible,
            "graph_backlog_depth": structural_graph_backlog_depth,
            "graph_projection_queue_depth": graph_projection_queue_depth,
            "semantic_backlog_depth": file_backlog_depth,
            "effective_graph_backlog_depth": effective_graph_backlog_depth,
            "effective_semantic_backlog_depth": effective_semantic_backlog_depth,
            "semantic_policy_profile": semantic_profile,
            "backlog_drain": {
                "semantic_health": semantic_drain_health,
                "recommendation": semantic_drain_recommendation,
                "utility_scheduler_state": utility_scheduler.state.as_str(),
                "utility_scheduler_reason": utility_scheduler.reason,
                "semantic_underfeed": utility_scheduler.semantic_underfeed,
                "ready_reserve_target": utility_scheduler.ready_reserve_target,
                "provider_requested": embedding_provider.provider_requested,
                "provider_effective": embedding_provider.provider_effective,
                "gpu_access_policy": gpu_access_policy,
                "vector_lane_state": vector_runtime.vector_lane_state.as_str(),
                "vector_worker_heartbeat_age_ms": vector_worker_heartbeat_age_ms,
                "vector_lane_last_success_age_ms": vector_lane_last_success_age_ms,
                "active_claimed_current": vector_runtime.active_claimed_current,
                "prepare_queue_depth_current": vector_runtime.prepare_queue_depth_current,
                "ready_queue_depth_current": vector_runtime.ready_queue_depth_current,
                "persist_queue_depth_current": vector_runtime.persist_queue_depth_current,
                "files_completed_total": vector_runtime.files_completed_total,
                "chunks_embedded_total": vector_runtime.chunks_embedded_total,
                "burn_rate": {
                    "measurement_window_sec": 60,
                    "state": burn_rate_state,
                    "recommendation": burn_rate_recommendation,
                    "files_vector_ready_last_minute": canonical_files_embedded_last_minute,
                    "chunks_embedded_last_minute": canonical_chunks_embedded_last_minute,
                    "effective_semantic_backlog_depth": effective_semantic_backlog_depth,
                    "estimated_semantic_backlog_drain_minutes": estimated_semantic_backlog_drain_minutes
                }
            },
            "loop_intervals_ms": {
                "reader_refresh": reader_refresh_interval_ms,
                "optimizer_loop": optimizer_loop_interval_ms,
                "runtime_trace": runtime_trace_interval_ms,
                "ingress_promoter_poll": watcher_promoter_poll_interval_ms
            },
            "wake_activity": {
                "wakeups_last_60s": wake_summary.wakeups_last_60s,
                "last_wakeup_at_ms": wake_summary.last_wakeup_at_ms,
                "quiescent_entered_at_ms": wake_summary.quiescent_entered_at_ms,
                "last_quiescent_exited_at_ms": wake_summary.last_quiescent_exited_at_ms,
                "quiescent_dwell_ms_current": wake_summary.quiescent_dwell_ms_current,
                "resume_latency_samples": wake_summary.resume_latency_samples,
                "resume_latency_p50_ms": wake_summary.resume_latency_p50_ms,
                "resume_latency_p95_ms": wake_summary.resume_latency_p95_ms,
                "resume_latency_max_ms": wake_summary.resume_latency_max_ms,
                "useful_resume_latency_samples": wake_summary.useful_resume_latency_samples,
                "useful_resume_latency_p50_ms": wake_summary.useful_resume_latency_p50_ms,
                "useful_resume_latency_p95_ms": wake_summary.useful_resume_latency_p95_ms,
                "useful_resume_latency_max_ms": wake_summary.useful_resume_latency_max_ms,
                "last_useful_resume_at_ms": wake_summary.last_useful_resume_at_ms,
                "last_quiescent_exit_reason": wake_summary.last_quiescent_exit_reason,
                "exit_due_to_active_backlog_total": wake_summary.exit_due_to_active_backlog_total,
                "exit_due_to_draining_residual_total": wake_summary.exit_due_to_draining_residual_total,
                "exit_due_to_interactive_guarded_total": wake_summary.exit_due_to_interactive_guarded_total,
                "last_wake_source": wake_summary.last_wake_source,
                "dominant_wake_source": dominant_wake_source,
                "last_background_wake_detail": wake_summary.last_background_wake_detail,
                "dominant_background_wake_detail": wake_summary.dominant_background_wake_detail,
                "wake_source_background_total": wake_summary.wake_source_background_total,
                "wake_source_semantic_vector_total": wake_summary.wake_source_semantic_vector_total,
                "wake_source_graph_total": wake_summary.wake_source_graph_total,
                "background_wake_memory_reclaimer_total": wake_summary.background_wake_memory_reclaimer_total,
                "background_wake_shadow_optimizer_total": wake_summary.background_wake_shadow_optimizer_total,
                "background_wake_runtime_trace_total": wake_summary.background_wake_runtime_trace_total,
                "background_wake_reader_refresh_total": wake_summary.background_wake_reader_refresh_total,
                "background_wake_autonomous_ingestor_total": wake_summary.background_wake_autonomous_ingestor_total,
                "background_wake_ingress_promoter_total": wake_summary.background_wake_ingress_promoter_total,
                "background_wake_federation_orchestrator_total": wake_summary.background_wake_federation_orchestrator_total
            },
            "lane_liveness": {
                "vector_worker_heartbeat_at_ms": vector_runtime.vector_worker_heartbeat_at_ms,
                "vector_worker_heartbeat_age_ms": vector_worker_heartbeat_age_ms,
                "vector_lane_state": vector_runtime.vector_lane_state.as_str(),
                "vector_lane_last_success_at_ms": vector_runtime.vector_lane_last_success_at_ms,
                "vector_lane_last_success_age_ms": vector_lane_last_success_age_ms,
                "vector_lane_last_fault_at_ms": vector_runtime.vector_lane_last_fault_at_ms,
                "vector_lane_last_fault_age_ms": vector_lane_last_fault_age_ms
            },
            "observed_residual_work": {
                "prepare_queue_depth_current": vector_runtime.prepare_queue_depth_current,
                "ready_queue_depth_current": vector_runtime.ready_queue_depth_current,
                "persist_queue_depth_current": vector_runtime.persist_queue_depth_current,
                "active_claimed_current": vector_runtime.active_claimed_current
            }
        })
    }

    fn runtime_limiting_factors_snapshot(
        runtime_mode: &str,
        semantic_backlog_responsible: bool,
        structural_graph_backlog_depth: usize,
        graph_projection_queue_depth: usize,
        file_backlog_depth: usize,
        runtime_signals: &optimizer::RuntimeSignalsWindow,
    ) -> Value {
        let host = optimizer::collect_host_snapshot();
        let policy = optimizer::collect_operator_policy_snapshot(&host);
        let effective = embedding_lane_config_from_env();
        let runtime_target = current_runtime_tuning_state();
        let batch_controller = current_vector_batch_controller_diagnostics(&effective);
        let utility_scheduler = current_utility_first_scheduler_diagnostics(
            structural_graph_backlog_depth,
            file_backlog_depth,
            service_guard::current_pressure(),
        );
        let embedding_provider = current_embedding_provider_diagnostics();
        let gpu_access_policy =
            std::env::var("AXON_GPU_ACCESS_POLICY").unwrap_or_else(|_| "unknown".to_string());
        let gpu_vector_lease = current_gpu_vector_lease_diagnostics();
        let avg_chunk_density_ratio = if batch_controller.target_embed_batch_chunks > 0 {
            (batch_controller.avg_chunks_per_embed_call
                / batch_controller.target_embed_batch_chunks as f64)
                .clamp(0.0, 4.0)
        } else {
            0.0
        };
        let ready_buffer_thin = runtime_signals.ready_queue_depth_current
            < utility_scheduler.ready_reserve_target as u64;
        let prepare_pipeline_shallow = runtime_signals.prepare_inflight_current
            <= u64::from(runtime_signals.vector_workers_active_current > 0)
            && runtime_signals.prepare_claimed_current
                <= batch_controller.target_files_per_cycle as u64;
        let ram_pressure = runtime_signals.ram_available_ratio > 0.0
            && runtime_signals.ram_available_ratio < policy.min_ram_available_ratio;
        let vram_pressure =
            policy.max_vram_used_mb > 0 && runtime_signals.vram_used_mb >= policy.max_vram_used_mb;
        let persist_congested = runtime_signals.persist_queue_depth_current
            >= runtime_target.vector_persist_queue_bound as u64
            || runtime_signals.persist_claimed_current
                >= runtime_target.vector_max_inflight_persists as u64;
        let gpu_effective = embedding_provider
            .provider_effective
            .eq_ignore_ascii_case("cuda");
        let gpu_compute_bound = gpu_effective
            && file_backlog_depth > 0
            && runtime_signals.gpu_utilization_ratio >= 0.80
            && !vram_pressure;
        let cpu_prepare_underfeed = gpu_effective
            && file_backlog_depth > 0
            && runtime_signals.gpu_utilization_ratio <= 0.45
            && !vram_pressure
            && runtime_signals.cpu_usage_ratio < policy.max_cpu_ratio * 0.75
            && runtime_signals.ram_available_ratio > policy.min_ram_available_ratio
            && (ready_buffer_thin || prepare_pipeline_shallow);
        let batch_density_collapse = gpu_effective
            && file_backlog_depth > 0
            && batch_controller.window_embed_calls > 0
            && avg_chunk_density_ratio < 0.60;

        let mut secondary = Vec::new();
        let (primary, reason, actionable) = if runtime_signals.interactive_requests_in_flight > 0 {
            (
                "interactive_guarded",
                "interactive traffic currently suppresses aggressive backlog tuning",
                false,
            )
        } else if ram_pressure {
            (
                "ram_bound",
                "available RAM is below the operator policy floor",
                true,
            )
        } else if vram_pressure {
            (
                "vram_bound",
                "VRAM usage is at or above the operator safety budget",
                true,
            )
        } else if persist_congested {
            (
                "persist_congested",
                "persist queue depth or inflight persists reached the current bound",
                true,
            )
        } else if semantic_backlog_responsible
            && !gpu_effective
            && gpu_access_policy.eq_ignore_ascii_case("avoid")
        {
            (
                "cpu_vector_policy_limited",
                "the current runtime policy keeps semantic vectorization on CPU",
                true,
            )
        } else if semantic_backlog_responsible
            && !gpu_effective
            && gpu_vector_lease.exclusive_required
            && !gpu_vector_lease.owned_by_current_instance
        {
            (
                "gpu_scaling_blocked",
                "the current instance does not own the exclusive GPU vector lease",
                true,
            )
        } else if batch_density_collapse {
            (
                "batch_density_collapse",
                "GPU batches are materially thinner than the controller target",
                true,
            )
        } else if cpu_prepare_underfeed {
            (
                "cpu_prepare_underfeed",
                "GPU demand is higher than the current CPU-side prepare and ready pipeline feed",
                true,
            )
        } else if gpu_compute_bound {
            (
                "gpu_compute_bound",
                "GPU utilization is already high while backlog remains active",
                false,
            )
        } else if file_backlog_depth == 0 && structural_graph_backlog_depth == 0 {
            (
                "not_currently_limited",
                "no active graph or semantic backlog is currently competing for throughput",
                false,
            )
        } else {
            (
                "not_currently_limited",
                "no single hard limiter is currently dominant above the observation threshold",
                false,
            )
        };

        if ram_pressure && primary != "ram_bound" {
            secondary.push("ram_bound");
        }
        if vram_pressure && primary != "vram_bound" {
            secondary.push("vram_bound");
        }
        if persist_congested && primary != "persist_congested" {
            secondary.push("persist_congested");
        }
        if batch_density_collapse && primary != "batch_density_collapse" {
            secondary.push("batch_density_collapse");
        }
        if cpu_prepare_underfeed && primary != "cpu_prepare_underfeed" {
            secondary.push("cpu_prepare_underfeed");
        }
        if gpu_compute_bound && primary != "gpu_compute_bound" {
            secondary.push("gpu_compute_bound");
        }

        json!({
            "primary": primary,
            "secondary": secondary,
            "actionable": actionable,
            "reason": reason,
            "controller_reason": batch_controller.reason,
            "scheduler_reason": utility_scheduler.reason,
            "runtime_mode": runtime_mode,
            "signals": {
                "cpu_usage_ratio": runtime_signals.cpu_usage_ratio,
                "ram_available_ratio": runtime_signals.ram_available_ratio,
                "gpu_utilization_ratio": runtime_signals.gpu_utilization_ratio,
                "vram_used_mb": runtime_signals.vram_used_mb,
                "vram_total_mb": host.vram_total_mb,
                "ready_queue_depth_current": runtime_signals.ready_queue_depth_current,
                "prepare_inflight_current": runtime_signals.prepare_inflight_current,
                "prepare_claimed_current": runtime_signals.prepare_claimed_current,
                "persist_queue_depth_current": runtime_signals.persist_queue_depth_current,
                "avg_chunks_per_embed_call": batch_controller.avg_chunks_per_embed_call,
                "target_embed_batch_chunks": batch_controller.target_embed_batch_chunks,
                "target_files_per_cycle": batch_controller.target_files_per_cycle,
                "ready_reserve_target": utility_scheduler.ready_reserve_target,
                "semantic_backlog_depth": file_backlog_depth,
                "graph_backlog_depth": structural_graph_backlog_depth,
                "graph_projection_queue_depth": graph_projection_queue_depth
            },
            "thresholds": {
                "max_cpu_ratio": policy.max_cpu_ratio,
                "min_ram_available_ratio": policy.min_ram_available_ratio,
                "max_vram_used_mb": policy.max_vram_used_mb,
                "density_collapse_ratio": 0.60,
                "gpu_underutilized_ratio": 0.45
            },
            "policy": {
                "gpu_access_policy": gpu_access_policy,
                "provider_requested": embedding_provider.provider_requested,
                "provider_effective": embedding_provider.provider_effective,
                "gpu_lease_owned_by_current_instance": gpu_vector_lease.owned_by_current_instance,
                "semantic_backlog_responsible": semantic_backlog_responsible
            }
        })
    }

    fn anomalies_cache() -> &'static Mutex<FrameworkCache> {
        ANOMALIES_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn conception_cache() -> &'static Mutex<FrameworkCache> {
        CONCEPTION_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn status_cache() -> &'static Mutex<FrameworkCache> {
        STATUS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn why_cache() -> &'static Mutex<FrameworkCache> {
        WHY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(crate) fn axon_mcp_surface_diagnostics(&self, _args: &Value) -> Option<Value> {
        let public_tools = tools_catalog(false)
            .get("tools")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let public_tool_names = public_tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(|value| value.as_str()))
            .map(str::to_string)
            .collect::<Vec<_>>();
        let async_allowlisted_tools = McpServer::ASYNC_JOB_TOOL_NAMES
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let monitored_sync_mutation_tools = McpServer::MONITORED_SYNC_MUTATION_TOOLS
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let public_host = std::env::var("AXON_PUBLIC_HOST").unwrap_or_default();
        let public_host_source =
            std::env::var("AXON_PUBLIC_HOST_SOURCE").unwrap_or_else(|_| "unresolved".to_string());
        let advertised_mcp_url = std::env::var("AXON_MCP_PUBLIC_URL").unwrap_or_default();
        let advertised_sql_url = std::env::var("AXON_SQL_PUBLIC_URL").unwrap_or_default();
        let advertised_dashboard_url =
            std::env::var("AXON_DASHBOARD_PUBLIC_URL").unwrap_or_default();
        let advertised_available =
            std::env::var("AXON_PUBLIC_ENDPOINTS_AVAILABLE").unwrap_or_default() == "1"
                && !advertised_mcp_url.is_empty();

        Some(json!({
            "content": [{
                "type": "text",
                "text": "Surface MCP diagnostics assembled. Server truth is authoritative for catalog and dispatch. Client session freshness is explicit below; refresh the client session when a freshly advertised tool is missing locally."
            }],
            "data": {
                "server_truth": {
                    "public_tool_count": public_tool_names.len(),
                    "critical_tools": [
                        "status",
                        "job_status",
                        "project_registry_lookup",
                        "axon_init_project",
                        "soll_apply_plan",
                        "soll_commit_revision"
                    ],
                    "public_tools": public_tool_names
                },
                "async_policy": {
                    "mode": "allowlist",
                    "sync_by_default": true,
                    "latency_target_p95_ms": 200,
                    "allowlisted_tools": async_allowlisted_tools,
                    "monitored_sync_mutation_tools": monitored_sync_mutation_tools,
                    "semantic_async_triggers": [
                        "batch",
                        "restore_import",
                        "queue_pipeline",
                        "vectorization_indexation",
                        "deep_analytics"
                    ]
                },
                "async_contract": {
                    "canonical_follow_up_tool": "job_status",
                    "acceptance_fields": ["job_id", "known_ids", "next_action", "result_contract", "polling_guidance", "recovery_hint"],
                    "runtime_command_proxy": {
                        "enabled": RuntimeCommandProxy::enabled(),
                        "mode": RuntimeCommandProxy::proxy_mode(),
                        "timeout_ms": RuntimeCommandProxy::timeout_ms(),
                        "timeout_kind": RuntimeCommandProxy::timeout_kind(),
                        "ownership": {
                            "proxy_role": "brain",
                            "execution_role": "indexer",
                            "mutation_owner": "indexer",
                            "duplicate_execution_prevented": true
                        },
                        "retry_policy": {
                            "retryable": true,
                            "max_attempts": 1,
                            "idempotent": true,
                            "duplicate_execution_prevented": true
                        }
                    },
                    "preferred_identity_tools": ["project_registry_lookup", "axon_init_project"]
                },
                "advertised_endpoints": {
                    "available": advertised_available,
                    "public_host": public_host,
                    "public_host_source": public_host_source,
                    "mcp_url": advertised_mcp_url,
                    "sql_url": advertised_sql_url,
                    "dashboard_url": advertised_dashboard_url
                },
                "client_binding_notes": {
                    "stale_client_binding_possible": true,
                    "session_freshness_status": "unknown_outside_server",
                    "operator_action": "If a freshly advertised public tool is not callable in the current client session, refresh or restart the client session and compare again.",
                    "canonical_refresh_instruction": "Refresh or reconnect the MCP client session, then compare its visible tool surface with `mcp_surface_diagnostics.server_truth.public_tools`.",
                    "safe_to_rely_on_now": [
                        "server truth for catalog and dispatch",
                        "advertised_endpoints for isolated clients",
                        "existing tools already visible in the active session"
                    ],
                    "may_require_client_refresh": [
                        "newly added public tools",
                        "freshly promoted endpoint changes",
                        "session-local tool bindings cached by the client"
                    ],
                    "guarantee_boundary": "The server guarantees catalog truth, dispatch truth, and advertised endpoint truth. Client session bindings are outside direct server control.",
                    "external_endpoint_rule": "Do not use instance_identity.*_url as an external endpoint. Isolated clients must prefer advertised_endpoints.* when available."
                }
            }
        }))
    }

    #[cfg(not(test))]
    fn cache_read(
        cache: &'static Mutex<FrameworkCache>,
        key: &str,
        now_ms: i64,
        ttl_ms: i64,
    ) -> Option<Value> {
        let guard = cache.lock().ok()?;
        let (stored_at, value) = guard.get(key)?;
        if now_ms.saturating_sub(*stored_at) > ttl_ms {
            return None;
        }
        Some(value.clone())
    }

    #[cfg(test)]
    fn cache_read(
        _cache: &'static Mutex<FrameworkCache>,
        _key: &str,
        _now_ms: i64,
        _ttl_ms: i64,
    ) -> Option<Value> {
        None
    }

    #[cfg(not(test))]
    fn cache_write(cache: &'static Mutex<FrameworkCache>, key: String, now_ms: i64, value: &Value) {
        if let Ok(mut guard) = cache.lock() {
            guard.insert(key, (now_ms, value.clone()));
        }
    }

    #[cfg(test)]
    fn cache_write(
        _cache: &'static Mutex<FrameworkCache>,
        _key: String,
        _now_ms: i64,
        _value: &Value,
    ) {
    }

    fn structural_history_dir() -> PathBuf {
        std::env::var("AXON_STRUCTURAL_HISTORY_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".axon/structural-history"))
    }

    fn structural_history_path(project_code: &str) -> PathBuf {
        Self::structural_history_dir().join(format!("{project_code}.jsonl"))
    }

    fn compact_runtime_path(path: String) -> String {
        let current_dir = std::env::current_dir().ok();
        let current_dir = current_dir.as_ref().map(|dir| dir.as_path());
        let as_path = PathBuf::from(&path);
        if let Some(root) = current_dir {
            if let Ok(stripped) = as_path.strip_prefix(root) {
                let display = stripped.display().to_string();
                return if display.is_empty() {
                    ".".to_string()
                } else {
                    format!("./{}", display)
                };
            }
        }
        if let Some(name) = as_path.file_name().and_then(|value| value.to_str()) {
            return format!("<{}>", name);
        }
        path
    }

    fn load_structural_snapshots(project_code: &str) -> Vec<Value> {
        let path = Self::structural_history_path(project_code);
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(_) => return Vec::new(),
        };
        let reader = BufReader::new(file);
        reader
            .lines()
            .map_while(Result::ok)
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
            .collect()
    }

    fn persist_structural_snapshot(project_code: &str, snapshot: &Value) -> Result<(), String> {
        let dir = Self::structural_history_dir();
        fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let path = Self::structural_history_path(project_code);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| error.to_string())?;
        let rendered = serde_json::to_string(snapshot).map_err(|error| error.to_string())?;
        writeln!(file, "{rendered}").map_err(|error| error.to_string())
    }

    pub(crate) fn canonical_sources_snapshot() -> Value {
        json!({
            "soll_export": {
                "role": "canonical_intention_backup",
                "reimportable": true
            }
        })
    }

    fn parse_soll_vision_entry(raw: &str) -> Value {
        let parts = raw.splitn(4, '|').collect::<Vec<_>>();
        json!({
            "id": parts.first().copied().unwrap_or("unknown"),
            "title": parts.get(1).copied().unwrap_or("unknown"),
            "status": parts.get(2).copied().unwrap_or("unknown"),
            "description": parts.get(3).copied().unwrap_or("unavailable"),
            "source": "SOLL"
        })
    }

    fn metric_value(summary: &Value, key: &str) -> i64 {
        summary
            .get(key)
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
    }

    fn diff_metric_summaries(current_summary: &Value, previous_summary: &Value) -> Value {
        let delta_for = |key: &str| -> i64 {
            Self::metric_value(current_summary, key) - Self::metric_value(previous_summary, key)
        };

        json!({
            "wrapper_count_delta": delta_for("wrapper_count"),
            "feature_envy_count_delta": delta_for("feature_envy_count"),
            "detour_count_delta": delta_for("detour_count"),
            "abstraction_detour_count_delta": delta_for("abstraction_detour_count"),
            "orphan_code_count_delta": delta_for("orphan_code_count"),
            "orphan_intent_count_delta": delta_for("orphan_intent_count"),
            "cycle_count_delta": delta_for("cycle_count"),
            "god_object_count_delta": delta_for("god_object_count")
        })
    }

    fn derive_conception_view(&self, project_code: &str) -> Value {
        let escaped_project = project_code.replace('\'', "''");
        let modules_raw = self
            .graph_store
            .query_json(&format!(
                "SELECT f.path, count(rel.target_id) AS symbol_count
                 FROM File f
                 LEFT JOIN CONTAINS rel
                   ON rel.source_id = f.path
                  AND rel.project_code = '{project}'
                 WHERE f.project_code = '{}'
                 GROUP BY 1
                 ORDER BY symbol_count DESC, f.path ASC
                 LIMIT 5",
                escaped_project,
                project = escaped_project
            ))
            .unwrap_or_else(|_| "[]".to_string());
        let module_rows: Vec<Vec<Value>> = serde_json::from_str(&modules_raw).unwrap_or_default();
        let modules = module_rows
            .iter()
            .filter_map(|row| {
                Some(json!({
                    "path": row.first()?.as_str()?.to_string(),
                    "symbol_count": row.get(1).and_then(|value| value.as_u64()).unwrap_or(0)
                }))
            })
            .collect::<Vec<_>>();

        let interfaces_raw = self
            .graph_store
            .query_json(&format!(
                "SELECT s.name, f.path
                 FROM Symbol s
                 LEFT JOIN CONTAINS rel
                   ON rel.target_id = s.id
                  AND rel.project_code = '{project}'
                 LEFT JOIN File f ON f.path = rel.source_id
                 WHERE s.project_code = '{}'
                   AND s.kind = 'interface'
                 ORDER BY s.name ASC
                 LIMIT 5",
                escaped_project,
                project = escaped_project
            ))
            .unwrap_or_else(|_| "[]".to_string());
        let interface_rows: Vec<Vec<Value>> =
            serde_json::from_str(&interfaces_raw).unwrap_or_default();
        let interfaces = interface_rows
            .iter()
            .filter_map(|row| {
                Some(json!({
                    "name": row.first()?.as_str()?.to_string(),
                    "path": row.get(1).and_then(|value| value.as_str()).unwrap_or("").to_string()
                }))
            })
            .collect::<Vec<_>>();

        let contracts_raw = self
            .graph_store
            .query_json(&format!(
                "SELECT s.name, s.kind, f.path
                 FROM Symbol s
                 LEFT JOIN CONTAINS rel
                   ON rel.target_id = s.id
                  AND rel.project_code = '{project}'
                 LEFT JOIN File f ON f.path = rel.source_id
                 WHERE s.project_code = '{}'
                   AND COALESCE(s.is_public, false) = true
                   AND s.kind IN ('interface', 'module', 'class', 'struct', 'function', 'method')
                 ORDER BY s.kind ASC, s.name ASC
                 LIMIT 5",
                escaped_project,
                project = escaped_project
            ))
            .unwrap_or_else(|_| "[]".to_string());
        let contract_rows: Vec<Vec<Value>> =
            serde_json::from_str(&contracts_raw).unwrap_or_default();
        let contracts = contract_rows
            .iter()
            .filter_map(|row| {
                Some(json!({
                    "name": row.first()?.as_str()?.to_string(),
                    "kind": row.get(1).and_then(|value| value.as_str()).unwrap_or("unknown").to_string(),
                    "path": row.get(2).and_then(|value| value.as_str()).unwrap_or("").to_string()
                }))
            })
            .collect::<Vec<_>>();

        let flows_raw = self
            .graph_store
            .query_json(&format!(
                "SELECT src.name, src_rel.source_id, dst.name, dst_rel.source_id
                 FROM CALLS c
                 JOIN Symbol src ON src.id = c.source_id
                 JOIN Symbol dst ON dst.id = c.target_id
                 JOIN CONTAINS src_rel
                   ON src_rel.target_id = src.id
                  AND src_rel.project_code = '{project}'
                 JOIN CONTAINS dst_rel
                   ON dst_rel.target_id = dst.id
                  AND dst_rel.project_code = '{project}'
                 WHERE src.project_code = '{project}'
                   AND dst.project_code = '{project}'
                   AND c.project_code = '{project}'
                   AND src_rel.source_id != dst_rel.source_id
                 ORDER BY src.name ASC, dst.name ASC
                 LIMIT 5",
                project = escaped_project
            ))
            .unwrap_or_else(|_| "[]".to_string());
        let flow_rows: Vec<Vec<Value>> = serde_json::from_str(&flows_raw).unwrap_or_default();
        let flows = flow_rows
            .iter()
            .filter_map(|row| {
                Some(json!({
                    "from_symbol": row.first()?.as_str()?.to_string(),
                    "from_path": row.get(1).and_then(|value| value.as_str()).unwrap_or("").to_string(),
                    "to_symbol": row.get(2).and_then(|value| value.as_str()).unwrap_or("").to_string(),
                    "to_path": row.get(3).and_then(|value| value.as_str()).unwrap_or("").to_string()
                }))
            })
            .collect::<Vec<_>>();

        let interface_count = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM Symbol WHERE project_code = '{}' AND kind = 'interface'",
                escaped_project
            ))
            .unwrap_or(0);
        let contract_count = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM Symbol
                 WHERE project_code = '{}'
                   AND COALESCE(is_public, false) = true
                   AND kind IN ('interface', 'module', 'class', 'struct', 'function', 'method')",
                escaped_project
            ))
            .unwrap_or(0);
        let flow_count = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*)
                 FROM CALLS c
                 JOIN CONTAINS src_rel
                   ON src_rel.target_id = c.source_id
                  AND src_rel.project_code = '{project}'
                 JOIN CONTAINS dst_rel
                   ON dst_rel.target_id = c.target_id
                  AND dst_rel.project_code = '{project}'
                 JOIN Symbol src ON src.id = c.source_id
                 JOIN Symbol dst ON dst.id = c.target_id
                 WHERE src.project_code = '{project}'
                   AND dst.project_code = '{project}'
                   AND c.project_code = '{project}'
                   AND src_rel.source_id != dst_rel.source_id",
                project = escaped_project
            ))
            .unwrap_or(0);

        json!({
            "module_count": modules.len(),
            "modules": modules,
            "interface_count": interface_count,
            "interfaces": interfaces,
            "flow_count": flow_count,
            "flows": flows,
            "contract_count": contract_count,
            "contracts": contracts,
            "boundaries": [],
            "owners": [],
            "confidence": "medium",
            "provenance": "derived_read_only_view"
        })
    }

    fn cached_conception_view(&self, project_code: &str) -> Value {
        let now_ms = Self::now_unix_ms();
        let cache_key = project_code.to_string();
        if let Some(cached) = Self::cache_read(
            Self::conception_cache(),
            &cache_key,
            now_ms,
            CONCEPTION_CACHE_TTL_MS,
        ) {
            return cached;
        }

        let conception = self.derive_conception_view(project_code);
        Self::cache_write(Self::conception_cache(), cache_key, now_ms, &conception);
        conception
    }

    fn build_project_status_delta(
        previous_summary: Option<&Value>,
        current_summary: &Value,
    ) -> Value {
        let Some(previous) = previous_summary else {
            return json!({ "available": false });
        };
        json!({
            "available": true,
            "metric_delta": Self::diff_metric_summaries(current_summary, previous),
            "wrapper_count_delta": Self::metric_value(current_summary, "wrapper_count") - Self::metric_value(previous, "wrapper_count"),
            "feature_envy_count_delta": Self::metric_value(current_summary, "feature_envy_count") - Self::metric_value(previous, "feature_envy_count"),
            "detour_count_delta": Self::metric_value(current_summary, "detour_count") - Self::metric_value(previous, "detour_count"),
            "abstraction_detour_count_delta": Self::metric_value(current_summary, "abstraction_detour_count") - Self::metric_value(previous, "abstraction_detour_count"),
            "orphan_code_count_delta": Self::metric_value(current_summary, "orphan_code_count") - Self::metric_value(previous, "orphan_code_count"),
            "orphan_intent_count_delta": Self::metric_value(current_summary, "orphan_intent_count") - Self::metric_value(previous, "orphan_intent_count"),
            "cycle_count_delta": Self::metric_value(current_summary, "cycle_count") - Self::metric_value(previous, "cycle_count"),
            "god_object_count_delta": Self::metric_value(current_summary, "god_object_count") - Self::metric_value(previous, "god_object_count")
        })
    }

    fn summarize_change_safety(
        coverage_signals: &Value,
        traceability_signals: &Value,
        validation_signals: &Value,
    ) -> (&'static str, Vec<String>, Vec<String>, &'static str) {
        let tested = coverage_signals
            .get("tested")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let traceability_links = traceability_signals
            .get("traceability_links")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let validation_nodes = validation_signals
            .get("validation_nodes")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let verifies_edges = validation_signals
            .get("verifies_edges")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);

        let mut reasoning = Vec::new();
        if tested {
            reasoning.push("target has direct test coverage".to_string());
        } else {
            reasoning.push("target lacks direct test coverage".to_string());
        }
        if traceability_links > 0 {
            reasoning.push(format!(
                "target has {} traceability link(s)",
                traceability_links
            ));
        } else {
            reasoning.push("target has no traceability links".to_string());
        }
        if validation_nodes > 0 || verifies_edges > 0 {
            reasoning.push("target is linked to validation evidence".to_string());
        } else {
            reasoning.push("target has no linked validation proof".to_string());
        }

        let guardrails = if tested {
            vec![
                "run focused tests before and after change".to_string(),
                "confirm rationale still holds with `why`".to_string(),
            ]
        } else if traceability_links > 0 || validation_nodes > 0 || verifies_edges > 0 {
            vec![
                "review `impact` before mutation".to_string(),
                "add or refresh validation evidence after change".to_string(),
            ]
        } else {
            vec![
                "do not mutate without human review".to_string(),
                "establish traceability or tests before refactor".to_string(),
            ]
        };

        let safety = if tested {
            "safe"
        } else if traceability_links > 0 || validation_nodes > 0 || verifies_edges > 0 {
            "caution"
        } else {
            "unsafe"
        };
        let confidence = if tested && traceability_links > 0 {
            "high"
        } else if tested || traceability_links > 0 || validation_nodes > 0 || verifies_edges > 0 {
            "medium"
        } else {
            "low"
        };
        (safety, reasoning, guardrails, confidence)
    }

    fn change_safety_operator_guidance(
        change_safety: &str,
        coverage_signals: &Value,
        traceability_signals: &Value,
        validation_signals: &Value,
    ) -> Value {
        let tested = coverage_signals
            .get("tested")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let traceability_links = traceability_signals
            .get("traceability_links")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let validation_nodes = validation_signals
            .get("validation_nodes")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let verifies_edges = validation_signals
            .get("verifies_edges")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);

        let mut blocking_factors = Vec::<Value>::new();
        if !tested {
            blocking_factors.push(json!({
                "factor": "missing_direct_test_coverage",
                "severity": "high",
                "recommended_action": "add or refresh direct tests before mutation"
            }));
        }
        if traceability_links == 0 {
            blocking_factors.push(json!({
                "factor": "missing_traceability_links",
                "severity": "high",
                "recommended_action": "link the target to canonical SOLL intent or evidence before mutation"
            }));
        }
        if validation_nodes == 0 && verifies_edges == 0 {
            blocking_factors.push(json!({
                "factor": "missing_validation_proof",
                "severity": "medium",
                "recommended_action": "create or attach validation proof before high-risk mutation"
            }));
        }

        let mutation_class_recommendation = match change_safety {
            "safe" => "safe_for_direct_mutation",
            "caution" if traceability_links > 0 => "safe_for_guarded_mutation",
            "caution" => "prefer_small_guarded_mutation",
            _ => "defer_or_prepare_evidence_first",
        };
        let recommended_next_step = match change_safety {
            "safe" => "proceed_with_mutation_after_impact_check",
            "caution" if !tested => "add_targeted_tests_then_reassess",
            "caution" if traceability_links == 0 => "link_traceability_then_reassess",
            "caution" => "use_small_guarded_mutation_with_explicit_validation_plan",
            _ => "stop_and_prepare_traceability_tests_and_validation_first",
        };
        let remediation_actions = blocking_factors
            .iter()
            .filter_map(|factor| {
                factor
                    .get("recommended_action")
                    .and_then(|value| value.as_str())
                    .map(|value| Value::from(value.to_string()))
            })
            .collect::<Vec<_>>();

        json!({
            "mutation_class_recommendation": mutation_class_recommendation,
            "recommended_next_step": recommended_next_step,
            "actionable_now": change_safety == "safe",
            "blocking_factors": blocking_factors,
            "remediation_actions": remediation_actions
        })
    }

    fn project_status_operator_guidance(
        degraded_notes: &[String],
        snapshot_storage: &Value,
        vision: &Value,
    ) -> Value {
        let mut blocking_factors = Vec::<Value>::new();

        for note in degraded_notes {
            blocking_factors.push(json!({
                "factor": "runtime_degraded_note",
                "severity": "high",
                "detail": note,
                "recommended_action": "inspect `status` and clear degraded runtime conditions before relying on project-wide conclusions"
            }));
        }

        if snapshot_storage
            .get("persisted")
            .and_then(|value| value.as_bool())
            == Some(false)
        {
            blocking_factors.push(json!({
                "factor": "snapshot_persistence_failed",
                "severity": "medium",
                "recommended_action": "repair structural snapshot persistence before depending on historical delta tracking"
            }));
        }

        if vision
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("unavailable")
            == "unavailable"
        {
            blocking_factors.push(json!({
                "factor": "vision_unavailable",
                "severity": "medium",
                "recommended_action": "refresh SOLL context so project steering is anchored on a canonical vision"
            }));
        }

        blocking_factors.push(json!({
            "factor": "anomalies_decoupled",
            "severity": "low",
            "recommended_action": "run `anomalies` explicitly when you need the full structural findings payload"
        }));

        let remediation_actions = blocking_factors
            .iter()
            .filter_map(|factor| {
                factor
                    .get("recommended_action")
                    .and_then(|value| value.as_str())
                    .map(|value| Value::from(value.to_string()))
            })
            .collect::<Vec<_>>();

        let recommended_next_step = if !degraded_notes.is_empty() {
            "inspect_runtime_status_then_refresh_project_status"
        } else if snapshot_storage
            .get("persisted")
            .and_then(|value| value.as_bool())
            == Some(false)
        {
            "repair_snapshot_storage_then_refresh_project_status"
        } else if vision
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("unavailable")
            == "unavailable"
        {
            "refresh_soll_context_then_reassess_project_status"
        } else {
            "run_anomalies_explicitly_then_follow_with_why_or_path"
        };

        let next_action = match recommended_next_step {
            "inspect_runtime_status_then_refresh_project_status" => json!({
                "kind": "inspect_runtime_status",
                "tool": "status",
                "when": "now"
            }),
            "repair_snapshot_storage_then_refresh_project_status" => json!({
                "kind": "repair_snapshot_storage",
                "tool": "project_status",
                "when": "after_storage_fix"
            }),
            "refresh_soll_context_then_reassess_project_status" => json!({
                "kind": "refresh_soll_context",
                "tool": "soll_query_context",
                "when": "now"
            }),
            _ => json!({
                "kind": "expand_structural_findings",
                "tool": "anomalies",
                "when": "now"
            }),
        };

        json!({
            "recommended_next_step": recommended_next_step,
            "actionable_now": degraded_notes.is_empty(),
            "blocking_factors": blocking_factors,
            "remediation_actions": remediation_actions,
            "follow_up_tools": ["anomalies", "why", "path"],
            "next_action": next_action
        })
    }

    fn summarize_why_response(args: &Value, response: &mut Value) {
        let Some(data) = response
            .get_mut("data")
            .and_then(|value| value.as_object_mut())
        else {
            return;
        };
        let planner = data.get("planner").cloned().unwrap_or_else(|| json!({}));
        let packet = data.get("packet").cloned().unwrap_or_else(|| json!({}));
        let mode = args
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or("brief");
        let brief_mode = mode == "brief";

        let mut relevant_soll_entities = packet
            .get("relevant_soll_entities")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut direct_evidence = packet
            .get("direct_evidence")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut supporting_chunks = packet
            .get("supporting_chunks")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut structural_neighbors = packet
            .get("structural_neighbors")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let missing_evidence = packet
            .get("missing_evidence")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let excluded_because = packet
            .get("excluded_because")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let confidence = packet
            .get("confidence")
            .cloned()
            .unwrap_or_else(|| json!({}));

        if brief_mode {
            relevant_soll_entities.truncate(4);
            direct_evidence.truncate(3);
            supporting_chunks.truncate(3);
            structural_neighbors.truncate(3);
        }

        let linked_validations = relevant_soll_entities
            .iter()
            .filter(|entity| {
                entity
                    .get("type")
                    .and_then(|value| value.as_str())
                    .map(|kind| kind.eq_ignore_ascii_case("Validation"))
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();

        let summary = json!({
            "target": {
                "question": args.get("question").and_then(|value| value.as_str()),
                "symbol": args.get("symbol").and_then(|value| value.as_str()),
                "project": args.get("project").and_then(|value| value.as_str()).unwrap_or("*")
            },
            "route": planner.get("route").and_then(|value| value.as_str()).unwrap_or("unknown"),
            "linked_intentions": relevant_soll_entities,
            "linked_validations": linked_validations,
            "supporting_artifacts": {
                "direct_evidence": direct_evidence,
                "supporting_chunks": supporting_chunks,
                "structural_neighbors": structural_neighbors
            },
            "missing_evidence": missing_evidence,
            "confidence": confidence,
            "provenance": "aggregated",
            "evidence_sources": ["retrieve_context", "soll_query_context", "traceability"],
            "safe_to_act": false,
            "needs_human_confirmation": true,
            "degradation": {
                "service_pressure": planner.get("service_pressure").cloned().unwrap_or(Value::Null),
                "degraded_reason": planner.get("degraded_reason").cloned().unwrap_or(Value::Null),
                "excluded_because": excluded_because
            },
            "canonical_sources": Self::canonical_sources_snapshot()
        });
        data.insert("why".to_string(), summary);
        if brief_mode {
            data.remove("planner");
            data.remove("packet");
        }
    }

    fn symbol_validation_signals(&self, project: &str, symbol_name: &str) -> Value {
        let escaped_project = project.replace('\'', "''");
        let escaped_name = symbol_name.replace('\'', "''");
        let resolved_symbol_id = if project == "*" {
            self.resolve_scoped_symbol_id_canonical(symbol_name, None)
        } else {
            self.resolve_scoped_symbol_id_canonical(symbol_name, Some(project))
        };
        let symbol_match_clause = if let Some(symbol_id) = resolved_symbol_id.as_deref() {
            format!(
                "(s.name = '{escaped_name}' OR s.id = '{}')",
                symbol_id.replace('\'', "''")
            )
        } else {
            format!("s.name = '{escaped_name}'")
        };
        let artifact_match_clause = if let Some(symbol_id) = resolved_symbol_id.as_deref() {
            format!(
                "(t.artifact_ref = s.id OR t.artifact_ref = s.name OR t.artifact_ref = '{}')",
                symbol_id.replace('\'', "''")
            )
        } else {
            "t.artifact_ref = s.id OR t.artifact_ref = s.name".to_string()
        };
        let scoped_clause = if project == "*" {
            String::new()
        } else {
            format!(" AND s.project_code = '{}'", escaped_project)
        };
        let query = format!(
            "SELECT
                COALESCE(MAX(CASE WHEN s.tested THEN 1 ELSE 0 END), 0) AS tested,
                COUNT(DISTINCT t.id) AS traceability_links
             FROM Symbol s
             LEFT JOIN soll.Traceability t
               ON t.artifact_type = 'Symbol'
              AND ({artifact_match_clause})
             WHERE {symbol_match_clause}
             {}",
            scoped_clause
        );
        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let tested = rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
            > 0;
        let traceability_links = rows
            .first()
            .and_then(|row| row.get(1))
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        json!({
            "tested": tested,
            "traceability_links": traceability_links
        })
    }

    fn batch_symbol_validation_signals(
        &self,
        project: &str,
        symbol_names: &[String],
    ) -> HashMap<String, Value> {
        let mut result = HashMap::new();
        if symbol_names.is_empty() {
            return result;
        }

        let escaped_project = project.replace('\'', "''");
        let scoped_clause = if project == "*" {
            String::new()
        } else {
            format!(" AND s.project_code = '{}'", escaped_project)
        };
        let names_sql = symbol_names
            .iter()
            .map(|name| format!("'{}'", name.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT
                s.name,
                COALESCE(MAX(CASE WHEN s.tested THEN 1 ELSE 0 END), 0) AS tested,
                COUNT(DISTINCT t.id) AS traceability_links
             FROM Symbol s
             LEFT JOIN soll.Traceability t
               ON t.artifact_type = 'Symbol'
              AND (t.artifact_ref = s.id OR t.artifact_ref = s.name)
             WHERE s.name IN ({names_sql})
             {scoped_clause}
             GROUP BY s.name"
        );
        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        for row in rows {
            if let Some(name) = row.first().and_then(|value| value.as_str()) {
                let tested = row.get(1).and_then(|value| value.as_i64()).unwrap_or(0) > 0;
                let traceability_links = row.get(2).and_then(|value| value.as_u64()).unwrap_or(0);
                result.insert(
                    name.to_string(),
                    json!({
                        "tested": tested,
                        "traceability_links": traceability_links
                    }),
                );
            }
        }
        for name in symbol_names {
            result
                .entry(name.clone())
                .or_insert_with(|| json!({"tested": false, "traceability_links": 0}));
        }
        result
    }

    fn intent_validation_signals(&self, project: &str, entity_id: &str) -> Value {
        let escaped_project = project.replace('\'', "''");
        let escaped_id = entity_id.replace('\'', "''");
        let scoped_clause = if project == "*" {
            String::new()
        } else {
            format!(" AND n.project_code = '{}'", escaped_project)
        };
        let query = format!(
            "SELECT
                COUNT(DISTINCT t.id) AS traceability_links,
                COUNT(DISTINCT e.source_id) FILTER (WHERE e.relation_type = 'VERIFIES') AS verifies_edges,
                COUNT(DISTINCT v.id) AS validation_nodes
             FROM soll.Node n
             LEFT JOIN soll.Traceability t
               ON lower(t.soll_entity_type) = lower(n.type)
              AND t.soll_entity_id = n.id
             LEFT JOIN soll.Edge e
               ON e.target_id = n.id
             LEFT JOIN soll.Node v
               ON v.id = e.source_id
              AND v.type = 'Validation'
             WHERE n.id = '{}'
             {}",
            escaped_id, scoped_clause
        );
        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let traceability_links = rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let verifies_edges = rows
            .first()
            .and_then(|row| row.get(1))
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let validation_nodes = rows
            .first()
            .and_then(|row| row.get(2))
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        json!({
            "traceability_links": traceability_links,
            "verifies_edges": verifies_edges,
            "validation_nodes": validation_nodes
        })
    }

    fn batch_intent_validation_signals(
        &self,
        project: &str,
        entity_ids: &[String],
    ) -> HashMap<String, Value> {
        let mut result = HashMap::new();
        if entity_ids.is_empty() {
            return result;
        }

        let escaped_project = project.replace('\'', "''");
        let scoped_clause = if project == "*" {
            String::new()
        } else {
            format!(" AND n.project_code = '{}'", escaped_project)
        };
        let ids_sql = entity_ids
            .iter()
            .map(|id| format!("'{}'", id.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT
                n.id,
                COUNT(DISTINCT t.id) AS traceability_links,
                COUNT(DISTINCT CASE WHEN e.relation_type = 'VERIFIES' THEN e.source_id END) AS verifies_edges,
                COUNT(DISTINCT v.id) AS validation_nodes
             FROM soll.Node n
             LEFT JOIN soll.Traceability t
               ON lower(t.soll_entity_type) = lower(n.type)
              AND t.soll_entity_id = n.id
             LEFT JOIN soll.Edge e
               ON e.target_id = n.id
             LEFT JOIN soll.Node v
               ON v.id = e.source_id
              AND v.type = 'Validation'
             WHERE n.id IN ({ids_sql})
             {scoped_clause}
             GROUP BY n.id"
        );
        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        for row in rows {
            if let Some(id) = row.first().and_then(|value| value.as_str()) {
                let traceability_links = row.get(1).and_then(|value| value.as_u64()).unwrap_or(0);
                let verifies_edges = row.get(2).and_then(|value| value.as_u64()).unwrap_or(0);
                let validation_nodes = row.get(3).and_then(|value| value.as_u64()).unwrap_or(0);
                result.insert(
                    id.to_string(),
                    json!({
                        "traceability_links": traceability_links,
                        "verifies_edges": verifies_edges,
                        "validation_nodes": validation_nodes
                    }),
                );
            }
        }
        for id in entity_ids {
            result.entry(id.clone()).or_insert_with(|| {
                json!({
                    "traceability_links": 0,
                    "verifies_edges": 0,
                    "validation_nodes": 0
                })
            });
        }
        result
    }

    fn recommend_effort_and_risk(
        kind: &str,
        validation_signals: &Value,
    ) -> (&'static str, &'static str) {
        match kind {
            "wrapper" => {
                let tested = validation_signals
                    .get("tested")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                if tested {
                    ("low", "low")
                } else {
                    ("low", "medium")
                }
            }
            "orphan_code" => {
                let tested = validation_signals
                    .get("tested")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                if tested {
                    ("medium", "medium")
                } else {
                    ("medium", "high")
                }
            }
            "feature_envy" => ("medium", "medium"),
            "detour" => ("medium", "medium"),
            "abstraction_detour" => ("medium", "medium"),
            "orphan_intent" => ("medium", "high"),
            "cycle" => ("high", "high"),
            "god_object" => ("high", "medium"),
            _ => ("unknown", "unknown"),
        }
    }

    pub(crate) fn axon_pre_flight_check(&self, args: &Value) -> Option<Value> {
        let diff_paths = args.get("diff_paths")?.as_array()?.clone();
        let message = args
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("pre-flight-check");
        self.axon_commit_work(&json!({
            "diff_paths": diff_paths,
            "message": message,
            "dry_run": true
        }))
    }

    pub(crate) fn axon_status(&self, args: &Value) -> Option<Value> {
        let mode = args.get("mode").and_then(|value| value.as_str());
        let now_ms = Self::now_unix_ms();
        let runtime_mode = AxonRuntimeMode::from_env();
        let runtime_shadow_role =
            std::env::var("AXON_RUNTIME_SHADOW_ROLE").unwrap_or_else(|_| "unknown".to_string());
        let split_runtime_is_indexer =
            matches!(runtime_shadow_role.as_str(), "indexer" | "indexer_shadow");
        let runtime_profile = AxonRuntimeOperationalProfile::from_mode_and_strings(
            runtime_mode.as_str(),
            std::env::var("AXON_ENABLE_AUTONOMOUS_INGESTOR")
                .ok()
                .as_deref(),
        );
        let cache_key = format!(
            "{}|{}|{}|{}|{}",
            mode.unwrap_or("brief"),
            runtime_mode.as_str(),
            runtime_profile.as_str(),
            std::env::var("AXON_INSTANCE_KIND").unwrap_or_else(|_| "unknown".to_string()),
            std::env::var("AXON_RUNTIME_IDENTITY").unwrap_or_else(|_| "unknown".to_string())
        );
        let status_cache_ttl_ms = match mode.unwrap_or("brief") {
            "full" => STATUS_FULL_CACHE_TTL_MS,
            _ => STATUS_CACHE_TTL_MS,
        };
        if let Some(cached) = Self::cache_read(
            Self::status_cache(),
            &cache_key,
            now_ms,
            status_cache_ttl_ms,
        ) {
            return Some(cached);
        }
        let public_tools = tools_catalog(false)
            .get("tools")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let public_tool_names = public_tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(|value| value.as_str()))
            .collect::<Vec<_>>();

        let debug = self
            .axon_debug_with_args(&json!({}))
            .unwrap_or_else(|| json!({"data": {}}));
        let debug_data = debug.get("data").cloned().unwrap_or_else(|| json!({}));
        let vector_queue_statuses = debug_data
            .pointer("/embedding_contract/file_vectorization_queue_statuses")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let debug_queued_files = vector_queue_statuses
            .iter()
            .filter_map(|row| {
                let status = row.get("status")?.as_str()?;
                let count = row.get("count")?.as_u64()?;
                (status == "queued" || status == "paused_for_interactive_priority").then_some(count)
            })
            .sum::<u64>();
        let debug_inflight_files = vector_queue_statuses
            .iter()
            .filter_map(|row| {
                let status = row.get("status")?.as_str()?;
                let count = row.get("count")?.as_u64()?;
                (status == "inflight").then_some(count)
            })
            .sum::<u64>();
        let (db_queued_files, db_inflight_files) = self
            .graph_store
            .fetch_file_vectorization_queue_counts()
            .unwrap_or((0, 0));
        let queued_files = debug_queued_files.max(db_queued_files as u64);
        let inflight_files = debug_inflight_files.max(db_inflight_files as u64);
        let drain_state = debug_data
            .pointer("/embedding_contract/drain_state")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let now_ms = chrono::Utc::now().timestamp_millis();
        let stale_threshold_ms = std::env::var("AXON_VECTOR_LEASE_STALE_MS")
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(120_000);
        let debug_graph_queue_depth = debug_data
            .pointer("/embedding_contract/runtime_telemetry/graph_projection_queue_depth")
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as usize;
        let (db_graph_queue_queued, db_graph_queue_inflight) = self
            .graph_store
            .fetch_graph_projection_queue_counts()
            .unwrap_or((0, 0));
        let graph_queue_depth =
            debug_graph_queue_depth.max(db_graph_queue_queued + db_graph_queue_inflight);
        let ingress = ingress_metrics_snapshot();
        let persisted_file_pending_depth =
            self.graph_store.count_persisted_file_pending().unwrap_or(0);
        let graph_wip_depth = self.graph_store.count_graph_wip_files().unwrap_or(0);
        let structural_graph_backlog_depth = persisted_file_pending_depth + graph_wip_depth;
        let graph_ready_depth = self
            .graph_store
            .query_count("SELECT count(*) FROM File WHERE COALESCE(graph_ready, FALSE) = TRUE")
            .unwrap_or(0) as usize;
        let orphan_vectorization_files = self
            .graph_store
            .count_orphaned_file_vectorization_files()
            .unwrap_or(0);
        let stale_vector_inflight_files = self
            .graph_store
            .count_stale_inflight_file_vectorization_files(now_ms, stale_threshold_ms)
            .unwrap_or(0);
        let oldest_graph_pending_age_ms = self
            .graph_store
            .oldest_graph_pending_age_ms(now_ms)
            .unwrap_or(0);
        let oldest_semantic_pending_age_ms = self
            .graph_store
            .oldest_semantic_pending_age_ms(now_ms)
            .unwrap_or(0);
        let utility_scheduler = current_utility_first_scheduler_diagnostics(
            structural_graph_backlog_depth,
            queued_files as usize + inflight_files as usize,
            service_guard::current_pressure(),
        );
        let runtime_signals = optimizer::collect_runtime_signals_window(&self.graph_store);

        let job_counts = if split_runtime_is_indexer {
            Vec::new()
        } else {
            let job_counts_raw = self
                .graph_store
                .execute_raw_sql_gateway(
                    "SELECT status, count(*) FROM soll.McpJob GROUP BY 1 ORDER BY 2 DESC, 1 ASC",
                )
                .unwrap_or_else(|_| "[]".to_string());
            let job_rows: Vec<Vec<Value>> =
                serde_json::from_str(&job_counts_raw).unwrap_or_default();
            job_rows
                .iter()
                .filter_map(|row| {
                    Some(json!({
                        "status": row.first()?.as_str()?.to_string(),
                        "count": row.get(1).and_then(|value| value.as_u64()).unwrap_or(0)
                    }))
                })
                .collect::<Vec<_>>()
        };

        let evidence = format!(
            "**Runtime mode:** `{}`\n\
**Runtime profile:** `{}`\n\
**Instance kind:** `{}`\n\
**Runtime identity:** `{}`\n\
**Advanced indexed surfaces visible:** {}\n\
**Vector backlog:** queued={} inflight={}\n\
**Utility-first scheduler:** `{}` ({})\n\
**Drain state:** `{}`\n\
**Public tools:** {}\n",
            runtime_mode.as_str(),
            runtime_profile.as_str(),
            std::env::var("AXON_INSTANCE_KIND").unwrap_or_else(|_| "unknown".to_string()),
            std::env::var("AXON_RUNTIME_IDENTITY").unwrap_or_else(|_| "unknown".to_string()),
            if public_tool_names.iter().any(|name| *name == "impact") {
                "yes"
            } else {
                "no"
            },
            queued_files,
            inflight_files,
            utility_scheduler.state.as_str(),
            utility_scheduler.reason,
            drain_state,
            public_tool_names.join(", ")
        );
        let report = format!(
            "## 📌 Axon Status\n\n{}",
            format_standard_contract(
                "ok",
                "operator truth snapshot assembled",
                &format!(
                    "runtime:{} / profile:{}",
                    runtime_mode.as_str(),
                    runtime_profile.as_str()
                ),
                &evidence_by_mode(&evidence, mode),
                &[
                    "run `anomalies` for structural risks",
                    "run `why` on a target symbol for rationale",
                    "run `path` for topology or source/sink traversal"
                ],
                "high",
            )
        );
        let instance_kind =
            std::env::var("AXON_INSTANCE_KIND").unwrap_or_else(|_| "unknown".to_string());
        let runtime_identity =
            std::env::var("AXON_RUNTIME_IDENTITY").unwrap_or_else(|_| "unknown".to_string());
        let data_root = Self::compact_runtime_path(
            std::env::var("AXON_DB_ROOT").unwrap_or_else(|_| "unknown".to_string()),
        );
        let run_root = Self::compact_runtime_path(
            std::env::var("AXON_RUN_ROOT").unwrap_or_else(|_| "unknown".to_string()),
        );
        let project_root = Self::compact_runtime_path(
            std::env::var("AXON_PROJECT_ROOT").unwrap_or_else(|_| "unknown".to_string()),
        );
        let mcp_url = std::env::var("AXON_MCP_URL").unwrap_or_else(|_| "unknown".to_string());
        let sql_url = std::env::var("AXON_SQL_URL").unwrap_or_else(|_| "unknown".to_string());
        let dashboard_url =
            std::env::var("AXON_DASHBOARD_URL").unwrap_or_else(|_| "unknown".to_string());
        let public_host = std::env::var("AXON_PUBLIC_HOST").unwrap_or_default();
        let public_host_source =
            std::env::var("AXON_PUBLIC_HOST_SOURCE").unwrap_or_else(|_| "unresolved".to_string());
        let advertised_mcp_url = std::env::var("AXON_MCP_PUBLIC_URL").unwrap_or_default();
        let advertised_sql_url = std::env::var("AXON_SQL_PUBLIC_URL").unwrap_or_default();
        let advertised_dashboard_url =
            std::env::var("AXON_DASHBOARD_PUBLIC_URL").unwrap_or_default();
        let advertised_available =
            std::env::var("AXON_PUBLIC_ENDPOINTS_AVAILABLE").unwrap_or_default() == "1"
                && !advertised_mcp_url.is_empty();
        let mutation_policy =
            std::env::var("AXON_MUTATION_POLICY").unwrap_or_else(|_| "unknown".to_string());
        let resource_priority =
            std::env::var("AXON_RESOURCE_PRIORITY").unwrap_or_else(|_| "unknown".to_string());
        let background_budget_class =
            std::env::var("AXON_BACKGROUND_BUDGET_CLASS").unwrap_or_else(|_| "unknown".to_string());
        let gpu_access_policy =
            std::env::var("AXON_GPU_ACCESS_POLICY").unwrap_or_else(|_| "unknown".to_string());
        let watcher_policy =
            std::env::var("AXON_WATCHER_POLICY").unwrap_or_else(|_| "unknown".to_string());
        let max_axon_workers =
            std::env::var("MAX_AXON_WORKERS").unwrap_or_else(|_| "unknown".to_string());
        let queue_memory_budget_bytes = std::env::var("AXON_QUEUE_MEMORY_BUDGET_BYTES")
            .unwrap_or_else(|_| "unknown".to_string());
        let watcher_subtree_hint_budget = std::env::var("AXON_WATCHER_SUBTREE_HINT_BUDGET")
            .unwrap_or_else(|_| "unknown".to_string());
        let vector_workers =
            std::env::var("AXON_VECTOR_WORKERS").unwrap_or_else(|_| "unknown".to_string());
        let graph_workers =
            std::env::var("AXON_GRAPH_WORKERS").unwrap_or_else(|_| "unknown".to_string());
        let gpu_vector_lease = current_gpu_vector_lease_diagnostics();
        let embedding_provider =
            std::env::var("AXON_EMBEDDING_PROVIDER").unwrap_or_else(|_| "unknown".to_string());
        let semantic_backlog_responsible = match runtime_mode.as_str() {
            "full" => {
                let provider_is_gpu = matches!(
                    embedding_provider.trim().to_ascii_lowercase().as_str(),
                    "cuda" | "gpu"
                );
                if provider_is_gpu {
                    !gpu_vector_lease.exclusive_required
                        || gpu_vector_lease.owned_by_current_instance
                } else {
                    true
                }
            }
            "graph_only" | "read_only" | "mcp_only" => false,
            _ => true,
        };
        let package_version = std::env::var("AXON_PACKAGE_VERSION")
            .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
        let release_version =
            std::env::var("AXON_RELEASE_VERSION").unwrap_or_else(|_| package_version.clone());
        let build_id = std::env::var("AXON_BUILD_ID").unwrap_or_else(|_| package_version.clone());
        let install_generation =
            std::env::var("AXON_INSTALL_GENERATION").unwrap_or_else(|_| "workspace".to_string());
        let async_allowlisted_tools = McpServer::ASYNC_JOB_TOOL_NAMES
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let monitored_sync_mutation_tools = McpServer::MONITORED_SYNC_MUTATION_TOOLS
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let runtime_topology = self.runtime_topology_snapshot(runtime_mode);
        let topology_converged = runtime_topology
            .get("system_converged")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let indexer_feed_state = runtime_topology
            .pointer("/indexer_feed/state")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let indexer_feed_reason = runtime_topology
            .pointer("/indexer_feed/degraded_reason")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let indexer_feed_degraded =
            indexer_feed_state != "fresh" || indexer_feed_reason.as_deref().is_some();
        let process_role = runtime_topology
            .get("process_role")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let mut degraded_notes = if public_tool_names.iter().any(|name| *name == "impact")
            || (process_role == "brain" && runtime_mode == AxonRuntimeMode::McpOnly)
        {
            Vec::<String>::new()
        } else {
            vec!["advanced_indexed_surfaces_hidden_for_current_profile".to_string()]
        };
        if indexer_feed_degraded {
            degraded_notes
                .push(indexer_feed_reason.unwrap_or_else(|| "indexer_feed_degraded".to_string()));
        }
        if !topology_converged {
            degraded_notes.push("runtime_topology_not_converged".to_string());
        }
        let response = json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "truth_status": if (public_tool_names.iter().any(|name| *name == "impact")
                    || (process_role == "brain" && runtime_mode == AxonRuntimeMode::McpOnly))
                    && degraded_notes.is_empty() {
                    "canonical"
                } else {
                    "degraded"
                },
                "runtime_mode": runtime_mode.as_str(),
                "runtime_profile": runtime_profile.as_str(),
                "drain_state": drain_state,
                "availability": {
                    "advanced_indexed_surfaces_visible": public_tool_names.iter().any(|name| *name == "impact"),
                    "degraded_notes": degraded_notes
                },
                "canonical_sources": Self::canonical_sources_snapshot(),
                "instance_identity": {
                    "instance_kind": instance_kind,
                    "runtime_identity": runtime_identity,
                    "data_root": data_root,
                    "run_root": run_root,
                    "project_root": project_root,
                    "mcp_url": mcp_url,
                    "sql_url": sql_url,
                    "dashboard_url": dashboard_url,
                    "mutation_policy": mutation_policy
                },
                "advertised_endpoints": {
                    "available": advertised_available,
                    "public_host": public_host,
                    "public_host_source": public_host_source,
                    "mcp_url": advertised_mcp_url,
                    "sql_url": advertised_sql_url,
                    "dashboard_url": advertised_dashboard_url
                },
                "client_reachability_notes": {
                    "instance_identity_is_runtime_local_only": true,
                    "external_endpoint_rule": "Use advertised_endpoints.* for isolated clients when available. instance_identity.*_url is host-local runtime truth.",
                    "stale_client_binding_possible": true
                },
                "resource_policy": {
                    "resource_priority": resource_priority,
                    "background_budget_class": background_budget_class,
                    "gpu_access_policy": gpu_access_policy,
                    "watcher_policy": watcher_policy,
                    "embedding_provider": embedding_provider,
                    "max_axon_workers": max_axon_workers,
                    "queue_memory_budget_bytes": queue_memory_budget_bytes,
                    "watcher_subtree_hint_budget": watcher_subtree_hint_budget,
                    "vector_workers": vector_workers,
                    "graph_workers": graph_workers
                },
                "runtime_authority": {
                    "proposed_control_model": "admission_first_stock_control",
                    "runtime_topology": runtime_topology,
                    "loop_semantics": Self::loop_semantics_snapshot(),
                    "canonical_ingestion_stage_model": self.canonical_ingestion_stage_model_snapshot(),
                    "admission_controller": Self::admission_controller_snapshot(
                        runtime_mode.as_str(),
                        ingress,
                        persisted_file_pending_depth,
                        graph_wip_depth,
                    ),
                    "canonical_edges": Self::canonical_edge_control_snapshot(
                        runtime_mode.as_str(),
                        ingress,
                        persisted_file_pending_depth,
                        graph_wip_depth,
                        graph_ready_depth,
                        structural_graph_backlog_depth,
                    ),
                    "priority_contract": Self::priority_contract_snapshot(
                        runtime_mode.as_str(),
                        semantic_backlog_responsible,
                        ingress.buffered_entries,
                        structural_graph_backlog_depth,
                        graph_queue_depth,
                        queued_files as usize + inflight_files as usize,
                        utility_scheduler,
                        apply_semantic_policy_runtime_tuning(target_semantic_policy_with_graph(
                            queued_files as usize + inflight_files as usize,
                            structural_graph_backlog_depth,
                            service_guard::current_pressure(),
                        ))
                        .profile,
                    ),
                    "lane_parameters": Self::runtime_lane_authority_snapshot(
                        structural_graph_backlog_depth,
                        queued_files as usize + inflight_files as usize,
                    ),
                    "quiescent_state": Self::quiescent_runtime_snapshot(
                        runtime_mode.as_str(),
                        semantic_backlog_responsible,
                        structural_graph_backlog_depth,
                        graph_queue_depth,
                        queued_files as usize + inflight_files as usize,
                        apply_semantic_policy_runtime_tuning(target_semantic_policy_with_graph(
                            queued_files as usize + inflight_files as usize,
                            structural_graph_backlog_depth,
                            service_guard::current_pressure(),
                        ))
                        .profile,
                        runtime_signals.canonical_chunks_embedded_last_minute,
                        runtime_signals.canonical_files_embedded_last_minute,
                    ),
                    "limiting_factors": Self::runtime_limiting_factors_snapshot(
                        runtime_mode.as_str(),
                        semantic_backlog_responsible,
                        structural_graph_backlog_depth,
                        graph_queue_depth,
                        queued_files as usize + inflight_files as usize,
                        &runtime_signals,
                    )
                },
                "runtime_version": {
                    "release_version": release_version,
                    "package_version": package_version,
                    "build_id": build_id,
                    "install_generation": install_generation
                },
                "file_vectorization_queue": {
                    "queued": queued_files,
                    "inflight": inflight_files
                },
                "utility_first_scheduler": {
                    "state": utility_scheduler.state.as_str(),
                    "reason": utility_scheduler.reason,
                    "semantic_underfeed": utility_scheduler.semantic_underfeed,
                    "ready_reserve_target": utility_scheduler.ready_reserve_target,
                    "hold_window_ms": utility_scheduler.hold_window_ms,
                    "orphan_vectorization_files": orphan_vectorization_files,
                    "stale_vector_inflight_files": stale_vector_inflight_files,
                    "oldest_graph_pending_age_ms": oldest_graph_pending_age_ms,
                    "oldest_semantic_pending_age_ms": oldest_semantic_pending_age_ms
                },
                "public_tools": public_tool_names,
                "async_policy": {
                    "mode": "allowlist",
                    "sync_by_default": true,
                    "latency_target_p95_ms": 200,
                    "allowlisted_tools": async_allowlisted_tools,
                    "monitored_sync_mutation_tools": monitored_sync_mutation_tools,
                    "semantic_async_triggers": [
                        "batch",
                        "restore_import",
                        "queue_pipeline",
                        "vectorization_indexation",
                        "deep_analytics"
                    ]
                },
                "async_contract": {
                    "canonical_follow_up_tool": "job_status",
                    "stale_client_binding_possible": true,
                    "preferred_identity_tools": ["project_registry_lookup", "axon_init_project"],
                    "runtime_command_proxy": {
                        "enabled": RuntimeCommandProxy::enabled(),
                        "mode": RuntimeCommandProxy::proxy_mode(),
                        "timeout_ms": RuntimeCommandProxy::timeout_ms(),
                        "timeout_kind": RuntimeCommandProxy::timeout_kind(),
                        "ownership": {
                            "proxy_role": "brain",
                            "execution_role": "indexer",
                            "mutation_owner": "indexer",
                            "duplicate_execution_prevented": true
                        },
                        "retry_policy": {
                            "retryable": true,
                            "max_attempts": 1,
                            "idempotent": true,
                            "duplicate_execution_prevented": true
                        }
                    }
                },
                "job_counts": job_counts,
                "debug_snapshot": debug_data,
                "traceability": debug_data.get("traceability").cloned().unwrap_or_else(|| json!({}))
            }
        });
        Self::cache_write(Self::status_cache(), cache_key, now_ms, &response);
        Some(response)
    }

    pub(crate) fn axon_project_status(&self, args: &Value) -> Option<Value> {
        let mode = args.get("mode").and_then(|value| value.as_str());
        let project_code = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .unwrap_or("AXO");

        let status = self.axon_status(&json!({ "mode": mode.unwrap_or("brief") }))?;
        let status_data = status.get("data").cloned().unwrap_or_else(|| json!({}));

        // Decoupled: We no longer compute anomalies inline to prevent MCP timeouts.
        // The operator must call the `anomalies` tool explicitly.
        let anomalies_data = json!({
            "summary": { "note": "Anomalies calculation decoupled to prevent timeout. Use 'anomalies' tool directly." },
            "findings": [],
            "recommendations": []
        });
        let soll_context = self.axon_soll_query_context(&json!({
            "project_code": project_code,
            "limit": 5
        }))?;
        let soll_data = soll_context
            .get("data")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let conception = self.cached_conception_view(project_code);
        let vision = soll_data
            .get("visions")
            .and_then(|value| value.as_array())
            .and_then(|items| items.first())
            .and_then(|value| value.as_str())
            .map(Self::parse_soll_vision_entry)
            .unwrap_or_else(|| {
                json!({
                    "id": "unavailable",
                    "title": "unavailable",
                    "status": "unknown",
                    "description": "unavailable",
                    "source": "SOLL"
                })
            });

        let anomaly_summary = anomalies_data
            .get("summary")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let previous_snapshot = Self::load_structural_snapshots(project_code)
            .into_iter()
            .last();
        let previous_summary = previous_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.get("anomaly_summary"));
        let snapshot_id = format!("project-status-{}-{}", project_code, Self::now_unix_ms());
        let generated_at = Self::now_unix_ms();
        let delta_vs_previous =
            Self::build_project_status_delta(previous_summary, &anomaly_summary);
        let degraded_notes = status_data
            .pointer("/availability/degraded_notes")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect::<Vec<_>>();
        let snapshot_record = json!({
            "snapshot_id": snapshot_id,
            "generated_at": generated_at,
            "project_code": project_code,
            "anomaly_summary": anomaly_summary,
            "conception_summary": {
                "module_count": conception.get("module_count").cloned().unwrap_or_else(|| json!(0)),
                "interface_count": conception.get("interface_count").cloned().unwrap_or_else(|| json!(0)),
                "contract_count": conception.get("contract_count").cloned().unwrap_or_else(|| json!(0)),
                "flow_count": conception.get("flow_count").cloned().unwrap_or_else(|| json!(0))
            },
            "provenance": "aggregated",
            "confidence": "medium"
        });
        let snapshot_storage = match Self::persist_structural_snapshot(
            project_code,
            &snapshot_record,
        ) {
            Ok(()) => json!({
                "scope": "derived_non_canonical",
                "path": Self::structural_history_path(project_code).to_string_lossy().to_string(),
                "persisted": true
            }),
            Err(error) => {
                let mut notes = degraded_notes.clone();
                notes.push(format!("snapshot_persistence_failed:{error}"));
                json!({
                    "scope": "derived_non_canonical",
                    "path": Self::structural_history_path(project_code).to_string_lossy().to_string(),
                    "persisted": false,
                    "error": error,
                    "degraded_notes": notes
                })
            }
        };
        let operator_guidance =
            Self::project_status_operator_guidance(&degraded_notes, &snapshot_storage, &vision);
        let public_tools = status_data
            .get("public_tools")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect::<Vec<_>>();

        let evidence = format!(
            "**Vision:** `{}` - {}\n\
**Vision status:** `{}`\n\
**Runtime mode/profile:** `{}` / `{}`\n\
**Drain state:** `{}`\n\
**Public tools:** {}\n\
**Wrappers / Orphan code / Orphan intent:** {} / {} / {}\n\
**Validation coverage:** {}\n\
**Degradation notes:** {}\n",
            vision
                .get("id")
                .and_then(|value| value.as_str())
                .unwrap_or("unavailable"),
            vision
                .get("title")
                .and_then(|value| value.as_str())
                .unwrap_or("unavailable"),
            vision
                .get("status")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            status_data
                .get("runtime_mode")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            status_data
                .get("runtime_profile")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            status_data
                .get("drain_state")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            if public_tools.is_empty() {
                "unknown".to_string()
            } else {
                public_tools.join(", ")
            },
            anomaly_summary
                .get("wrapper_count")
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
            anomaly_summary
                .get("orphan_code_count")
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
            anomaly_summary
                .get("orphan_intent_count")
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
            anomaly_summary
                .get("validation_coverage_score")
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            if degraded_notes.is_empty() {
                "none".to_string()
            } else {
                degraded_notes.join(", ")
            }
        );
        let report = format!(
            "## 🧭 Project Status\n\n{}",
            format_standard_contract(
                "ok",
                "live project situation assembled from MCP read surfaces",
                &format!("project:{}", project_code),
                &evidence_by_mode(&evidence, mode),
                &[
                    "use `why` on a specific symbol to inspect rationale",
                    "use `path` for source/sink topology",
                    "use `anomalies` for the full structural findings payload"
                ],
                "high",
            )
        );

        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "project_code": project_code,
                "snapshot_id": snapshot_id,
                "generated_at": generated_at,
                "delta_vs_previous": delta_vs_previous,
                "vision": vision,
                "conception": conception,
                "runtime": status_data,
                "anomalies": {
                    "summary": anomaly_summary,
                    "findings": anomalies_data.get("findings").cloned().unwrap_or_else(|| json!([])),
                    "recommendations": anomalies_data.get("recommendations").cloned().unwrap_or_else(|| json!([]))
                },
                "snapshot_storage": snapshot_storage,
                "operator_guidance": operator_guidance.clone(),
                "next_action": operator_guidance
                    .get("next_action")
                    .cloned()
                    .unwrap_or(Value::Null),
                "soll_context": {
                    "visions": soll_data.get("visions").cloned().unwrap_or_else(|| json!([])),
                    "requirements": soll_data.get("requirements").cloned().unwrap_or_else(|| json!([])),
                    "decisions": soll_data.get("decisions").cloned().unwrap_or_else(|| json!([])),
                    "revisions": soll_data.get("revisions").cloned().unwrap_or_else(|| json!([]))
                },
                "canonical_sources": Self::canonical_sources_snapshot()
            }
        }))
    }

    pub(crate) fn axon_why(&self, args: &Value) -> Option<Value> {
        let mode = args
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or("brief");
        let cache_key = format!(
            "{}::{}::{}::{}",
            args.get("symbol")
                .and_then(|value| value.as_str())
                .or_else(|| args.get("question").and_then(|value| value.as_str()))
                .unwrap_or("*"),
            args.get("project")
                .and_then(|value| value.as_str())
                .unwrap_or("*"),
            mode,
            args.get("include_graph")
                .and_then(|value| value.as_bool())
                .unwrap_or(mode != "brief")
        );
        let now_ms = Self::now_unix_ms();
        if let Some(cached) =
            Self::cache_read(Self::why_cache(), &cache_key, now_ms, WHY_CACHE_TTL_MS)
        {
            return Some(cached);
        }
        let include_graph = args
            .get("include_graph")
            .and_then(|value| value.as_bool())
            .unwrap_or(mode != "brief");
        let question = args
            .get("question")
            .and_then(|value| value.as_str())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                args.get("symbol")
                    .and_then(|value| value.as_str())
                    .map(|symbol| format!("Why does {} exist?", symbol))
            })?;
        let mut response = self.axon_retrieve_context(&json!({
            "question": question,
            "project": args.get("project").and_then(|value| value.as_str()),
            "mode": mode,
            "top_k": args.get("top_k").cloned().unwrap_or_else(|| json!(if mode == "brief" { 3 } else { 6 })),
            "token_budget": args.get("token_budget").cloned().unwrap_or_else(|| json!(if mode == "brief" { 700 } else { 1400 })),
            "include_soll": true,
            "include_graph": include_graph
        }))?;
        if let Some(data) = response
            .get_mut("data")
            .and_then(|value| value.as_object_mut())
        {
            data.insert("framework_alias".to_string(), json!("why"));
        }
        Self::summarize_why_response(args, &mut response);
        Self::cache_write(Self::why_cache(), cache_key, now_ms, &response);
        Some(response)
    }

    pub(crate) fn axon_path(&self, args: &Value) -> Option<Value> {
        let source = args.get("source")?.as_str()?.trim();
        if source.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "path requires a non-empty `source`" }],
                "isError": true
            }));
        }
        let sink = args
            .get("sink")
            .and_then(|value| value.as_str())
            .map(str::trim);
        let project = args.get("project").and_then(|value| value.as_str());
        let depth = args
            .get("depth")
            .and_then(|value| value.as_u64())
            .unwrap_or(6)
            .clamp(1, 12);
        let mode = args.get("mode").and_then(|value| value.as_str());

        if sink.is_none() {
            return self.axon_bidi_trace(&json!({
                "symbol": source,
                "project": project,
                "depth": depth,
                "mode": mode.unwrap_or("brief")
            }));
        }

        let sink = sink.unwrap_or_default();
        let Some(source_id) = self.resolve_scoped_symbol_id_canonical(source, project) else {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("path source '{}' not found in current scope", source) }],
                "isError": true
            }));
        };
        let Some(sink_id) = self.resolve_scoped_symbol_id_canonical(sink, project) else {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("path sink '{}' not found in current scope", sink) }],
                "isError": true
            }));
        };

        let edge_query = if let Some(project) = project {
            format!(
                "WITH all_edges AS (
                    SELECT source_id, target_id, 'calls' AS edge_type FROM CALLS
                    UNION ALL
                    SELECT source_id, target_id, 'calls_nif' AS edge_type FROM CALLS_NIF
                )
                SELECT src.id, src.name, dst.id, dst.name, e.edge_type
                FROM all_edges e
                JOIN Symbol src ON src.id = e.source_id
                JOIN Symbol dst ON dst.id = e.target_id
                WHERE src.project_code = '{project}'
                  AND dst.project_code = '{project}'",
                project = project.replace('\'', "''")
            )
        } else {
            "WITH all_edges AS (
                SELECT source_id, target_id, 'calls' AS edge_type FROM CALLS
                UNION ALL
                SELECT source_id, target_id, 'calls_nif' AS edge_type FROM CALLS_NIF
            )
            SELECT src.id, src.name, dst.id, dst.name, e.edge_type
            FROM all_edges e
            JOIN Symbol src ON src.id = e.source_id
            JOIN Symbol dst ON dst.id = e.target_id"
                .to_string()
        };
        let raw = self
            .graph_store
            .query_json(&edge_query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut adjacency: std::collections::HashMap<String, Vec<(String, String, String)>> =
            std::collections::HashMap::new();
        let mut source_name = source.to_string();
        for row in rows {
            if row.len() < 5 {
                continue;
            }
            if row[0] == source_id {
                source_name = row[1].clone();
            }
            adjacency.entry(row[0].clone()).or_default().push((
                row[2].clone(),
                row[3].clone(),
                row[4].clone(),
            ));
        }

        let mut queue = std::collections::VecDeque::new();
        queue.push_back((
            source_id.clone(),
            vec![source_id.clone()],
            vec![source_name],
            vec!["anchor".to_string()],
            0_u64,
        ));

        let mut resolved_path: Option<(Vec<String>, Vec<String>)> = None;
        while let Some((node_id, path_ids, path_names, edge_kinds, current_depth)) =
            queue.pop_front()
        {
            if node_id == sink_id {
                resolved_path = Some((path_names, edge_kinds));
                break;
            }
            if current_depth >= depth {
                continue;
            }
            if let Some(neighbors) = adjacency.get(&node_id) {
                for (target_id, target_name, edge_type) in neighbors {
                    if path_ids.iter().any(|seen| seen == target_id) {
                        continue;
                    }
                    let mut next_ids = path_ids.clone();
                    next_ids.push(target_id.clone());
                    let mut next_names = path_names.clone();
                    next_names.push(target_name.clone());
                    let mut next_edges = edge_kinds.clone();
                    next_edges.push(edge_type.clone());
                    queue.push_back((
                        target_id.clone(),
                        next_ids,
                        next_names,
                        next_edges,
                        current_depth + 1,
                    ));
                }
            }
        }

        let Some((path, edges)) = resolved_path else {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("No path found between '{}' and '{}' within depth {}", source, sink, depth) }],
                "isError": true,
                "data": {
                    "source": source,
                    "sink": sink,
                    "depth": depth,
                    "path_found": false,
                    "path_type": "bounded_call_path",
                    "detours": [],
                    "bounded_depth_used": depth,
                    "operator_guidance": {
                        "actionable_now": false,
                        "blocking_factors": [{
                            "factor": "no_path_found_within_depth",
                            "severity": "medium",
                            "recommended_action": "increase depth or inspect the endpoints individually before assuming there is no reachable path"
                        }],
                        "remediation_actions": [
                            "increase depth or inspect the endpoints individually before assuming there is no reachable path"
                        ],
                        "follow_up_tools": ["inspect", "impact"],
                        "next_action": {
                            "kind": "inspect_endpoints_or_increase_depth",
                            "tool": "inspect",
                            "when": "now"
                        }
                    },
                    "next_action": {
                        "kind": "inspect_endpoints_or_increase_depth",
                        "tool": "inspect",
                        "when": "now"
                    },
                    "canonical_sources": Self::canonical_sources_snapshot()
                }
            }));
        };
        let evidence = format!(
            "**Source:** `{}`\n\
**Sink:** `{}`\n\
**Depth used:** {}\n\
**Path:** {}\n\
**Edges:** {}\n",
            source,
            sink,
            depth,
            path.join(" -> "),
            edges.join(" -> ")
        );
        let report = format!(
            "## 🧭 Axon Path\n\n{}",
            format_standard_contract(
                "ok",
                "bounded path computed",
                &project
                    .map(|value| format!("project:{}", value))
                    .unwrap_or_else(|| "workspace:*".to_string()),
                &evidence_by_mode(&evidence, mode),
                &[
                    "run `impact` to expand blast radius",
                    "run `why` to join rationale"
                ],
                "medium",
            )
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "source": source,
                "sink": sink,
                "depth": depth,
                "bounded_depth_used": depth,
                "path_found": true,
                "path_type": "bounded_call_path",
                "path": path,
                "edge_kinds": edges,
                "detours": [],
                "confidence": "medium",
                "provenance": "extracted_recursive_calls",
                "evidence_sources": ["CALLS", "CALLS_NIF", "CONTAINS"],
                "safe_to_act": false,
                "needs_human_confirmation": true,
                "operator_guidance": {
                    "actionable_now": true,
                    "blocking_factors": [],
                    "remediation_actions": [],
                    "follow_up_tools": ["impact", "why"],
                    "next_action": {
                        "kind": "expand_blast_radius_from_path",
                        "tool": "impact",
                        "when": "now"
                    }
                },
                "next_action": {
                    "kind": "expand_blast_radius_from_path",
                    "tool": "impact",
                    "when": "now"
                },
                "canonical_sources": Self::canonical_sources_snapshot()
            }
        }))
    }

    pub(crate) fn axon_anomalies(&self, args: &Value) -> Option<Value> {
        let project = args
            .get("project")
            .and_then(|value| value.as_str())
            .unwrap_or("*");
        let mode = args.get("mode").and_then(|value| value.as_str());
        let brief_mode = mode.unwrap_or("brief") == "brief";
        let now_ms = Self::now_unix_ms();
        let cache_key = format!("{}::{}", project, mode.unwrap_or("brief"));
        if let Some(cached) = Self::cache_read(
            Self::anomalies_cache(),
            &cache_key,
            now_ms,
            ANOMALIES_CACHE_TTL_MS,
        ) {
            return Some(cached);
        }

        let escaped_project = project.replace('\'', "''");
        let total_symbols = if project == "*" {
            self.graph_store
                .query_count("SELECT count(*) FROM Symbol WHERE kind IN ('function', 'method')")
                .unwrap_or(0)
        } else {
            self.graph_store
                .query_count(&format!(
                    "SELECT count(*) FROM Symbol WHERE project_code = '{}' AND kind IN ('function', 'method')",
                    escaped_project
                ))
                .unwrap_or(0)
        };

        let wrappers = self
            .graph_store
            .get_wrapper_candidates(project)
            .unwrap_or_default();
        let feature_envy = self
            .graph_store
            .get_feature_envy_candidates(project)
            .unwrap_or_default();
        let detours = self
            .graph_store
            .get_detour_candidates(project)
            .unwrap_or_default();
        let abstraction_detours = self
            .graph_store
            .get_abstraction_detour_candidates(project)
            .unwrap_or_default();
        let orphan_code = self
            .graph_store
            .get_orphan_code_symbols(project)
            .unwrap_or_default();
        let orphan_intent = self
            .graph_store
            .get_orphan_intent_nodes(project)
            .unwrap_or_default();
        let soll_snapshot = self
            .soll_completeness_snapshot(if project == "*" { None } else { Some(project) })
            .ok();
        let canonical_orphan_intent_ids = soll_snapshot
            .as_ref()
            .map(|snapshot| snapshot.canonical_orphan_intent_ids())
            .unwrap_or_default();
        let (circular_deps, cycle_count) = if brief_mode {
            (
                Vec::new(),
                self.graph_store
                    .get_circular_dependency_count_fast(project)
                    .unwrap_or(0) as usize,
            )
        } else {
            let cycles = self
                .graph_store
                .get_circular_dependencies(project)
                .unwrap_or_default();
            let cycle_count = cycles.len();
            (cycles, cycle_count)
        };
        let god_objects = self
            .graph_store
            .get_god_objects(project)
            .unwrap_or_default();
        let validation_coverage_score = self.graph_store.get_coverage_score(project).unwrap_or(0);
        let total_intent_nodes = if project == "*" {
            self.graph_store
                .query_count(
                    "SELECT count(*) FROM soll.Node WHERE type IN ('Requirement', 'Decision', 'Concept', 'Validation')",
                )
                .unwrap_or(0)
        } else {
            self.graph_store
                .query_count(&format!(
                    "SELECT count(*) FROM soll.Node WHERE project_code = '{}' AND type IN ('Requirement', 'Decision', 'Concept', 'Validation')",
                    escaped_project
                ))
                .unwrap_or(0)
        };

        let wrapper_entities = wrappers
            .iter()
            .take(if brief_mode { 5 } else { wrappers.len() })
            .cloned()
            .collect::<Vec<_>>();
        let feature_envy_entities = feature_envy
            .iter()
            .take(if brief_mode { 5 } else { feature_envy.len() })
            .cloned()
            .collect::<Vec<_>>();
        let detour_entities = detours
            .iter()
            .take(if brief_mode { 5 } else { detours.len() })
            .cloned()
            .collect::<Vec<_>>();
        let abstraction_detour_entities = abstraction_detours
            .iter()
            .take(if brief_mode {
                5
            } else {
                abstraction_detours.len()
            })
            .cloned()
            .collect::<Vec<_>>();
        let orphan_code_entities = orphan_code
            .iter()
            .take(if brief_mode { 8 } else { orphan_code.len() })
            .cloned()
            .collect::<Vec<_>>();
        let orphan_intent_entities = orphan_intent
            .iter()
            .take(if brief_mode { 8 } else { orphan_intent.len() })
            .cloned()
            .collect::<Vec<_>>();
        let god_object_entities = god_objects
            .keys()
            .take(if brief_mode { 3 } else { 5 })
            .cloned()
            .collect::<Vec<_>>();

        let default_symbol_validation = if brief_mode {
            json!({
                "tested": false,
                "traceability_links": 0,
                "mode": "brief_heuristic"
            })
        } else {
            json!({"tested": false, "traceability_links": 0})
        };
        let default_intent_validation = if brief_mode {
            json!({
                "traceability_links": 0,
                "verifies_edges": 0,
                "validation_nodes": 0,
                "mode": "brief_heuristic"
            })
        } else {
            json!({
                "traceability_links": 0,
                "verifies_edges": 0,
                "validation_nodes": 0
            })
        };

        let symbol_validation_map = if brief_mode {
            HashMap::new()
        } else {
            let mut symbol_signal_names = Vec::new();
            for item in wrapper_entities
                .iter()
                .chain(feature_envy_entities.iter())
                .chain(detour_entities.iter())
                .chain(abstraction_detour_entities.iter())
            {
                symbol_signal_names.push(item.split(" -> ").next().unwrap_or(item).to_string());
            }
            symbol_signal_names.extend(orphan_code_entities.iter().cloned());
            symbol_signal_names.extend(god_object_entities.iter().cloned());
            symbol_signal_names.sort();
            symbol_signal_names.dedup();
            self.batch_symbol_validation_signals(project, &symbol_signal_names)
        };

        let intent_validation_map = if brief_mode {
            HashMap::new()
        } else {
            let intent_ids = orphan_intent_entities
                .iter()
                .map(|node| node.split(' ').next().unwrap_or(node).to_string())
                .collect::<Vec<_>>();
            self.batch_intent_validation_signals(project, &intent_ids)
        };

        let mut findings = Vec::new();
        for wrapper in &wrapper_entities {
            let source_symbol = wrapper.split(" -> ").next().unwrap_or(wrapper);
            let validation_signals = symbol_validation_map
                .get(source_symbol)
                .cloned()
                .unwrap_or_else(|| default_symbol_validation.clone());
            let (estimated_effort, estimated_risk) =
                Self::recommend_effort_and_risk("wrapper", &validation_signals);
            findings.push(json!({
                "type": "wrapper",
                "entity": wrapper,
                "scope": project,
                "severity": "medium",
                "confidence": "medium",
                "provenance": "heuristic_single_outbound_call",
                "evidence_sources": ["CALLS", "Symbol", "CONTAINS"],
                "recommended_action": "inspect for direct inlining or removal",
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": validation_signals.get("tested").and_then(|value| value.as_bool()).unwrap_or(false),
                "needs_human_confirmation": !validation_signals.get("tested").and_then(|value| value.as_bool()).unwrap_or(false)
            }));
        }
        for candidate in &feature_envy_entities {
            let source_symbol = candidate.split(" -> ").next().unwrap_or(candidate);
            let validation_signals = symbol_validation_map
                .get(source_symbol)
                .cloned()
                .unwrap_or_else(|| default_symbol_validation.clone());
            let (estimated_effort, estimated_risk) =
                Self::recommend_effort_and_risk("feature_envy", &validation_signals);
            findings.push(json!({
                "type": "feature_envy",
                "entity": candidate,
                "scope": project,
                "severity": "medium",
                "confidence": "medium",
                "provenance": "cross_file_outbound_dominance",
                "evidence_sources": ["CALLS", "CONTAINS"],
                "recommended_action": "review module placement and move logic closer to its dominant collaborators",
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        for candidate in &detour_entities {
            let source_symbol = candidate.split(" -> ").next().unwrap_or(candidate);
            let validation_signals = symbol_validation_map
                .get(source_symbol)
                .cloned()
                .unwrap_or_else(|| default_symbol_validation.clone());
            let (estimated_effort, estimated_risk) =
                Self::recommend_effort_and_risk("detour", &validation_signals);
            findings.push(json!({
                "type": "detour",
                "entity": candidate,
                "scope": project,
                "severity": "medium",
                "confidence": "medium",
                "provenance": "single_inbound_single_outbound_bridge",
                "evidence_sources": ["CALLS", "CONTAINS"],
                "recommended_action": "inspect whether the intermediate hop can be inlined or collapsed",
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        for candidate in &abstraction_detour_entities {
            let source_symbol = candidate.split(" -> ").next().unwrap_or(candidate);
            let validation_signals = symbol_validation_map
                .get(source_symbol)
                .cloned()
                .unwrap_or_else(|| default_symbol_validation.clone());
            let (estimated_effort, estimated_risk) =
                Self::recommend_effort_and_risk("abstraction_detour", &validation_signals);
            findings.push(json!({
                "type": "abstraction_detour",
                "entity": candidate,
                "scope": project,
                "severity": "medium",
                "confidence": "low",
                "provenance": "single_local_interface_implementation_name_match",
                "evidence_sources": ["Symbol", "CONTAINS"],
                "recommended_action": "confirm whether the interface still provides policy value or only indirection",
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        for symbol in &orphan_code_entities {
            let validation_signals = symbol_validation_map
                .get(symbol)
                .cloned()
                .unwrap_or_else(|| default_symbol_validation.clone());
            let (estimated_effort, estimated_risk) =
                Self::recommend_effort_and_risk("orphan_code", &validation_signals);
            findings.push(json!({
                "type": "orphan_code",
                "entity": symbol,
                "scope": project,
                "severity": "high",
                "confidence": "medium",
                "provenance": "missing_traceability_links",
                "evidence_sources": ["Symbol", "soll.Traceability"],
                "recommended_action": "link to intent or delete if obsolete",
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        let mut heuristic_intent_gap_count = 0usize;
        for node in &orphan_intent_entities {
            let node_id = node.split(' ').next().unwrap_or(node);
            let validation_signals = intent_validation_map
                .get(node_id)
                .cloned()
                .unwrap_or_else(|| default_intent_validation.clone());
            let canonical_backed = canonical_orphan_intent_ids.contains(node_id);
            let anomaly_type = if canonical_backed {
                "orphan_intent"
            } else {
                heuristic_intent_gap_count += 1;
                "heuristic_intent_gap"
            };
            let (estimated_effort, estimated_risk) =
                Self::recommend_effort_and_risk("orphan_intent", &validation_signals);
            let mut enriched_validation_signals = validation_signals;
            enriched_validation_signals["canonical_backed"] = json!(canonical_backed);
            findings.push(json!({
                "type": anomaly_type,
                "entity": node,
                "scope": project,
                "severity": if canonical_backed { "high" } else { "low" },
                "confidence": if canonical_backed { "medium" } else { "low" },
                "provenance": if canonical_backed { "missing_traceability_evidence" } else { "heuristic_missing_traceability" },
                "evidence_sources": ["soll.Node", "soll.Traceability", "soll.Edge"],
                "recommended_action": if canonical_backed {
                    "attach implementation or validation evidence"
                } else {
                    "review only if this node should carry direct proof at the current project stage"
                },
                "validation_signals": enriched_validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": !canonical_backed,
                "needs_human_confirmation": true
            }));
        }
        for cycle in circular_deps.iter().take(if brief_mode { 3 } else { 5 }) {
            let validation_signals = json!({
                "tested": Value::Null,
                "traceability_links": 0,
                "verifies_edges": 0
            });
            let (estimated_effort, estimated_risk) =
                Self::recommend_effort_and_risk("cycle", &validation_signals);
            findings.push(json!({
                "type": "cycle",
                "entity": cycle,
                "scope": project,
                "severity": "high",
                "confidence": "high",
                "provenance": "recursive_call_path",
                "evidence_sources": ["CALLS"],
                "recommended_action": "review for justified or accidental recursion",
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        for name in &god_object_entities {
            let count = god_objects
                .get(name)
                .and_then(|value: &Value| value.as_i64())
                .unwrap_or(0);
            let validation_signals = symbol_validation_map
                .get(name)
                .cloned()
                .unwrap_or_else(|| default_symbol_validation.clone());
            let (estimated_effort, estimated_risk) =
                Self::recommend_effort_and_risk("god_object", &validation_signals);
            findings.push(json!({
                "type": "god_object",
                "entity": name,
                "scope": project,
                "severity": "medium",
                "confidence": "high",
                "provenance": "fan_in_threshold",
                "evidence_sources": ["CALLS"],
                "recommended_action": format!("review decomposition candidate (fan_in={})", count),
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
                "needs_human_confirmation": true
            }));
        }
        let orphan_code_rate = if total_symbols > 0 {
            ((orphan_code.len() as f64 / total_symbols as f64) * 100.0 * 10.0).round() / 10.0
        } else {
            0.0
        };
        let alignment_proxy_score = if total_symbols > 0 {
            (((total_symbols.saturating_sub(orphan_code.len() as i64)) as f64
                / total_symbols as f64)
                * 100.0
                * 10.0)
                .round()
                / 10.0
        } else {
            0.0
        };
        let rectitude_proxy_score = if total_symbols > 0 {
            let detour_like = wrappers.len() + detours.len();
            (((total_symbols.saturating_sub(detour_like as i64)) as f64 / total_symbols as f64)
                * 100.0
                * 10.0)
                .round()
                / 10.0
        } else {
            0.0
        };
        let cycle_health_score = if total_symbols > 0 {
            (((total_symbols.saturating_sub(cycle_count as i64)) as f64 / total_symbols as f64)
                * 100.0
                * 10.0)
                .round()
                / 10.0
        } else {
            100.0
        };
        let canonical_orphan_intent_count = orphan_intent_entities
            .iter()
            .filter(|node| {
                let node_id = node.split(' ').next().unwrap_or(node);
                canonical_orphan_intent_ids.contains(node_id)
            })
            .count();
        let orphan_intent_rate = if total_intent_nodes > 0 {
            ((canonical_orphan_intent_count as f64 / total_intent_nodes as f64) * 100.0 * 10.0)
                .round()
                / 10.0
        } else {
            0.0
        };

        let mut recommendations = findings
            .iter()
            .map(|finding| {
                let anomaly_type = finding
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let entity = finding
                    .get("entity")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let severity = finding
                    .get("severity")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let recommended_action = finding
                    .get("recommended_action")
                    .and_then(|value| value.as_str())
                    .unwrap_or("review manually");
                let estimated_effort = finding
                    .get("estimated_effort")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let estimated_risk = finding
                    .get("estimated_risk")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let validation_signals = finding
                    .get("validation_signals")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let sequencing_dependencies = match anomaly_type {
                    "wrapper" => vec!["confirm target API stability", "check callers with `impact`"],
                    "feature_envy" => vec!["confirm natural owning module", "review move impact with `path`"],
                    "detour" => vec!["confirm hop is not policy-bearing", "review direct caller/callee contract"],
                    "abstraction_detour" => vec!["confirm abstraction has no second implementation planned", "inspect public API commitments"],
                    "orphan_code" => vec!["search SOLL rationale with `why`", "decide link vs delete"],
                    "orphan_intent" => vec!["inspect `soll_work_plan`", "attach implementation or proof"],
                    "heuristic_intent_gap" => vec!["compare with `soll_validate` completeness axes", "defer if concept baseline is already complete"],
                    "cycle" => vec!["confirm if cycle is intentional", "review module boundary"],
                    "god_object" => vec!["inspect fan-in consumers", "stage decomposition carefully"],
                    _ => vec!["review manually"],
                };
                json!({
                    "anomaly_type": anomaly_type,
                    "entity": entity,
                    "severity": severity,
                    "why_flagged": finding.get("provenance").cloned().unwrap_or_else(|| json!("unknown")),
                    "recommended_action": recommended_action,
                    "estimated_effort": estimated_effort,
                    "estimated_risk": estimated_risk,
                    "validation_signals": validation_signals,
                    "sequencing_dependencies": sequencing_dependencies,
                    "safe_to_act": finding.get("safe_to_act").cloned().unwrap_or_else(|| json!(false)),
                    "needs_human_confirmation": finding.get("needs_human_confirmation").cloned().unwrap_or_else(|| json!(true))
                })
            })
            .collect::<Vec<_>>();
        recommendations.sort_by_key(|item| {
            match item
                .get("severity")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown")
            {
                "high" => 0,
                "medium" => 1,
                "low" => 2,
                _ => 3,
            }
        });
        if brief_mode {
            recommendations.truncate(12);
        }

        let evidence = format!(
            "**Scope:** `{}`\n\
**Wrappers:** {}\n\
**Feature envy:** {}\n\
**Detours:** {}\n\
**Abstraction detours:** {}\n\
**Orphan code:** {}\n\
**Orphan intent (canonical):** {}\n\
**Heuristic intent gaps:** {}\n\
**Cycles:** {}\n\
**God objects:** {}\n",
            project,
            wrappers.len(),
            feature_envy.len(),
            detours.len(),
            abstraction_detours.len(),
            orphan_code.len(),
            canonical_orphan_intent_count,
            heuristic_intent_gap_count,
            cycle_count,
            god_objects.len()
        );
        let report = format!(
            "## 🚨 Axon Anomalies\n\n{}",
            format_standard_contract(
                "ok",
                "structural anomalies aggregated",
                &format!("project:{}", project),
                &evidence_by_mode(&evidence, mode),
                &[
                    "review top orphan intent and orphan code first",
                    "inspect wrapper candidates before broad refactors",
                    "use `impact` on any high-risk symbol before mutation"
                ],
                "medium",
            )
        );

        let response = json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "summary": {
                    "project": project,
                    "wrapper_count": wrappers.len(),
                    "feature_envy_count": feature_envy.len(),
                    "detour_count": detours.len(),
                    "abstraction_detour_count": abstraction_detours.len(),
                    "alignment_proxy_score": alignment_proxy_score,
                    "rectitude_proxy_score": rectitude_proxy_score,
                    "cycle_health_score": cycle_health_score,
                    "orphan_code_count": orphan_code.len(),
                    "orphan_code_rate": orphan_code_rate,
                    "orphan_intent_count": canonical_orphan_intent_count,
                    "orphan_intent_rate": orphan_intent_rate,
                    "heuristic_intent_gap_count": heuristic_intent_gap_count,
                    "cycle_count": cycle_count,
                    "god_object_count": god_objects.len(),
                    "validation_coverage_score": validation_coverage_score,
                    "total_symbols": total_symbols,
                    "total_intent_nodes": total_intent_nodes,
                    "concept_completeness": soll_snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.concept_complete())
                        .unwrap_or(false),
                    "implementation_completeness": soll_snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.implementation_complete())
                        .unwrap_or(false)
                },
                "snapshot": {
                    "generated_at": Self::now_unix_ms(),
                    "provenance": "aggregated_graph_analytics",
                    "confidence": "medium",
                    "semantic_boundary": "heuristic anomaly overlays must not silently override canonical SOLL completeness"
                },
                "findings": findings,
                "recommendations": recommendations
            }
        });
        Self::cache_write(Self::anomalies_cache(), cache_key, now_ms, &response);
        Some(response)
    }

    pub(crate) fn axon_snapshot_history(&self, args: &Value) -> Option<Value> {
        let project_code = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .unwrap_or("AXO");
        let limit = args
            .get("limit")
            .and_then(|value| value.as_u64())
            .unwrap_or(10) as usize;
        let snapshots = Self::load_structural_snapshots(project_code);
        let count = snapshots.len();
        let start = count.saturating_sub(limit);
        Some(json!({
            "content": [{ "type": "text", "text": format!("snapshot_history returned {} snapshot(s) for {}", count.saturating_sub(start), project_code) }],
            "data": {
                "project_code": project_code,
                "snapshots": snapshots.into_iter().skip(start).collect::<Vec<_>>(),
                "storage": {
                    "scope": "derived_non_canonical",
                    "path": Self::structural_history_path(project_code).to_string_lossy().to_string()
                },
                "provenance": "aggregated",
                "confidence": "high",
                "evidence_sources": ["project_status_snapshots"],
                "safe_to_act": false,
                "needs_human_confirmation": false
            }
        }))
    }

    pub(crate) fn axon_snapshot_diff(&self, args: &Value) -> Option<Value> {
        let project_code = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .unwrap_or("AXO");
        let snapshots = Self::load_structural_snapshots(project_code);
        if snapshots.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("No structural snapshots found for {}", project_code) }],
                "isError": true
            }));
        }
        let from_snapshot_id = args
            .get("from_snapshot_id")
            .and_then(|value| value.as_str());
        let to_snapshot_id = args.get("to_snapshot_id").and_then(|value| value.as_str());

        let resolve = |snapshot_id: Option<&str>, prefer_last: bool| -> Option<Value> {
            snapshot_id
                .and_then(|id| {
                    snapshots
                        .iter()
                        .find(|item| {
                            item.get("snapshot_id").and_then(|value| value.as_str()) == Some(id)
                        })
                        .cloned()
                })
                .or_else(|| {
                    if prefer_last {
                        snapshots.last().cloned()
                    } else if snapshots.len() >= 2 {
                        snapshots.get(snapshots.len() - 2).cloned()
                    } else {
                        snapshots.first().cloned()
                    }
                })
        };

        let from_snapshot = resolve(from_snapshot_id, false)?;
        let to_snapshot = resolve(to_snapshot_id, true)?;
        let from_summary = from_snapshot
            .get("anomaly_summary")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let to_summary = to_snapshot
            .get("anomaly_summary")
            .cloned()
            .unwrap_or_else(|| json!({}));
        Some(json!({
            "content": [{ "type": "text", "text": format!(
                "snapshot_diff compared {} -> {}",
                from_snapshot.get("snapshot_id").and_then(|value| value.as_str()).unwrap_or("unknown"),
                to_snapshot.get("snapshot_id").and_then(|value| value.as_str()).unwrap_or("unknown")
            ) }],
            "data": {
                "project_code": project_code,
                "from_snapshot_id": from_snapshot.get("snapshot_id").cloned().unwrap_or(Value::Null),
                "to_snapshot_id": to_snapshot.get("snapshot_id").cloned().unwrap_or(Value::Null),
                "metric_delta": Self::diff_metric_summaries(&to_summary, &from_summary),
                "storage": {
                    "scope": "derived_non_canonical",
                    "path": Self::structural_history_path(project_code).to_string_lossy().to_string()
                },
                "provenance": "aggregated",
                "confidence": "high",
                "evidence_sources": ["project_status_snapshots"],
                "safe_to_act": false,
                "needs_human_confirmation": false
            }
        }))
    }

    pub(crate) fn axon_conception_view(&self, args: &Value) -> Option<Value> {
        let project_code = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .unwrap_or("AXO");
        let mode = args
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or("brief");
        let conception = self.cached_conception_view(project_code);
        let boundary_violations: Vec<Value> = if mode == "brief" {
            Vec::new()
        } else {
            // Decoupled: We no longer fetch anomalies inline to avoid timeouts.
            // The operator must call 'anomalies' directly if needed.
            Vec::new()
        };
        let evidence = format!(
            "**Project:** `{}`\n\
**Modules / Interfaces / Contracts / Flows:** {} / {} / {} / {}\n\
**Boundary violations:** {}\n",
            project_code,
            conception
                .get("module_count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            conception
                .get("interface_count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            conception
                .get("contract_count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            conception
                .get("flow_count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            boundary_violations.len()
        );
        let report = format!(
            "## 🧱 Conception View\n\n{}",
            format_standard_contract(
                "ok",
                "derived conception view assembled",
                &format!("project:{}", project_code),
                &evidence_by_mode(&evidence, Some(mode)),
                &[
                    "use `why` for rationale",
                    "use `path` to inspect a flow in detail"
                ],
                "medium",
            )
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "project_code": project_code,
                "mode": mode,
                "module_count": conception.get("module_count").cloned().unwrap_or_else(|| json!(0)),
                "modules": conception.get("modules").cloned().unwrap_or_else(|| json!([])),
                "interface_count": conception.get("interface_count").cloned().unwrap_or_else(|| json!(0)),
                "interfaces": conception.get("interfaces").cloned().unwrap_or_else(|| json!([])),
                "contract_count": conception.get("contract_count").cloned().unwrap_or_else(|| json!(0)),
                "contracts": conception.get("contracts").cloned().unwrap_or_else(|| json!([])),
                "flow_count": conception.get("flow_count").cloned().unwrap_or_else(|| json!(0)),
                "flows": conception.get("flows").cloned().unwrap_or_else(|| json!([])),
                "boundaries": conception.get("boundaries").cloned().unwrap_or_else(|| json!([])),
                "owners": conception.get("owners").cloned().unwrap_or_else(|| json!([])),
                "suspected_boundary_violation_count": boundary_violations.len(),
                "suspected_boundary_violations": boundary_violations,
                "provenance": "derived_read_only_view",
                "confidence": conception.get("confidence").cloned().unwrap_or_else(|| json!("medium")),
                "evidence_sources": ["File", "Symbol", "CALLS", "CONTAINS"],
                "safe_to_act": false,
                "needs_human_confirmation": true
            }
        }))
    }

    pub(crate) fn axon_change_safety(&self, args: &Value) -> Option<Value> {
        let project_code = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .unwrap_or("AXO");
        let target = args.get("target")?.as_str()?.trim();
        if target.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "change_safety requires a non-empty `target`" }],
                "isError": true
            }));
        }
        let target_type = args
            .get("target_type")
            .and_then(|value| value.as_str())
            .unwrap_or("symbol");
        let escaped_project = project_code.replace('\'', "''");
        let escaped_target = target.replace('\'', "''");
        let resolved_symbol_id = if target_type == "symbol" {
            self.resolve_scoped_symbol_id_canonical(target, Some(project_code))
        } else {
            None
        };
        let validation_signals = match target_type {
            "intent" => self.intent_validation_signals(project_code, target),
            "symbol" => {
                let tested = self
                    .graph_store
                    .query_count(&format!(
                        "SELECT count(*) FROM Symbol WHERE project_code = '{}' AND name = '{}' AND tested = true",
                        escaped_project, escaped_target
                    ))
                    .unwrap_or(0)
                    > 0;
                let traceability_links = if let Some(symbol_id) = resolved_symbol_id.as_deref() {
                    self.graph_store
                        .query_count(&format!(
                            "SELECT count(*) FROM soll.Traceability
                             WHERE artifact_type = 'Symbol'
                               AND (artifact_ref = '{name}' OR artifact_ref = '{id}')",
                            name = escaped_target,
                            id = symbol_id.replace('\'', "''")
                        ))
                        .unwrap_or(0)
                } else {
                    self.graph_store
                        .query_count(&format!(
                            "SELECT count(*) FROM soll.Traceability
                             WHERE artifact_type = 'Symbol'
                               AND artifact_ref = '{}'",
                            escaped_target
                        ))
                        .unwrap_or(0)
                };
                json!({
                    "tested": tested,
                    "traceability_links": traceability_links,
                    "validation_nodes": 0,
                    "verifies_edges": 0
                })
            }
            _ => self.symbol_validation_signals(project_code, target),
        };
        let coverage_signals = json!({
            "tested": validation_signals.get("tested").cloned().unwrap_or_else(|| json!(false))
        });
        let traceability_signals = json!({
            "traceability_links": validation_signals
                .get("traceability_links")
                .cloned()
                .unwrap_or_else(|| json!(0))
        });
        let (change_safety, reasoning, recommended_guardrails, confidence) =
            Self::summarize_change_safety(
                &coverage_signals,
                &traceability_signals,
                &validation_signals,
            );
        let operator_guidance = Self::change_safety_operator_guidance(
            change_safety,
            &coverage_signals,
            &traceability_signals,
            &validation_signals,
        );
        let (safe_to_act, needs_human_confirmation) =
            (change_safety == "safe", change_safety != "safe");

        let evidence = format!(
            "**Target:** `{}` ({})\n\
**Safety:** `{}`\n\
**Traceability links:** {}\n\
**Tested:** {}\n",
            target,
            target_type,
            change_safety,
            traceability_signals
                .get("traceability_links")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            coverage_signals
                .get("tested")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        );
        let report = format!(
            "## 🛡️ Change Safety\n\n{}",
            format_standard_contract(
                "ok",
                "derived change-safety summary assembled",
                &format!("project:{}", project_code),
                &evidence_by_mode(&evidence, args.get("mode").and_then(|value| value.as_str())),
                &[
                    "run `impact` before mutation",
                    "use `why` to confirm intent remains valid"
                ],
                confidence,
            )
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "project_code": project_code,
                "target": target,
                "target_type": target_type,
                "coverage_signals": coverage_signals,
                "traceability_signals": traceability_signals,
                "validation_signals": validation_signals,
                "change_safety": change_safety,
                "reasoning": reasoning,
                "recommended_guardrails": recommended_guardrails,
                "operator_guidance": operator_guidance,
                "provenance": "aggregated",
                "confidence": confidence,
                "evidence_sources": ["Symbol", "soll.Traceability", "soll.Node", "soll.Edge"],
                "safe_to_act": safe_to_act,
                "needs_human_confirmation": needs_human_confirmation
            }
        }))
    }
}
