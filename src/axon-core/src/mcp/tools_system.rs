use serde_json::{json, Value};

use super::format::{evidence_by_mode, format_standard_contract, format_table_from_json};
use super::McpServer;
use crate::embedder::{
    current_embedding_provider_diagnostics, current_gpu_memory_pressure_active,
    current_gpu_memory_snapshot, embedding_lane_config_from_env, gpu_memory_soft_limit_mb,
    gpu_telemetry_backend_name, gpu_telemetry_cache_ttl_ms, gpu_telemetry_device_index,
};
use crate::embedding_contract::{
    CHUNK_MODEL_ID, DIMENSION, MAX_LENGTH, MODEL_NAME, NATIVE_DIMENSION, STORAGE_TYPE,
};
use crate::graph_query::ReadFreshness;
use crate::ingress_buffer::ingress_metrics_snapshot;
use crate::optimizer::{
    collect_host_snapshot, collect_operator_policy_snapshot, collect_recent_analytics_window,
    collect_runtime_signals_window,
};
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_observability::{
    duckdb_memory_snapshot, duckdb_storage_snapshot, process_memory_snapshot,
};
use crate::service_guard;
use crate::vector_control::{
    allowed_gpu_vector_workers, current_vector_batch_controller_diagnostics,
    current_vector_drain_state, gpu_pressure_embed_batch_chunks, gpu_pressure_files_per_cycle,
};

fn format_bytes_human(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    let bytes_f = bytes as f64;
    if bytes_f >= GIB {
        format!("{:.2} GB", bytes_f / GIB)
    } else if bytes_f >= MIB {
        format!("{:.0} MB", bytes_f / MIB)
    } else if bytes_f >= KIB {
        format!("{:.0} KB", bytes_f / KIB)
    } else {
        format!("{} B", bytes)
    }
}

fn json_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => {
            if let Some(v) = number.as_i64() {
                Some(v)
            } else if let Some(v) = number.as_u64() {
                i64::try_from(v).ok()
            } else {
                number.as_f64().map(|v| v.round() as i64)
            }
        }
        Value::String(s) => s
            .parse::<i64>()
            .ok()
            .or_else(|| s.parse::<f64>().ok().map(|v| v.round() as i64)),
        _ => None,
    }
}

fn parse_reason_count_rows(raw: &str) -> Vec<(String, i64)> {
    if let Ok(rows) = serde_json::from_str::<Vec<Vec<Value>>>(raw) {
        let parsed: Vec<(String, i64)> = rows
            .into_iter()
            .filter_map(|row| {
                let reason = row.first()?.as_str()?.to_string();
                let count = json_i64(row.get(1)?)?;
                Some((reason, count))
            })
            .collect();
        if !parsed.is_empty() {
            return parsed;
        }
    }

    serde_json::from_str::<Vec<serde_json::Map<String, Value>>>(raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            let reason = row
                .get("status_reason")
                .or_else(|| row.get("coalesce(status_reason, 'unknown')"))?
                .as_str()?
                .to_string();
            let count = row
                .get("count(*)")
                .or_else(|| row.get("count_star()"))
                .and_then(json_i64)?;
            Some((reason, count))
        })
        .collect()
}

fn parse_scalar_count_row(raw: &str) -> Option<i64> {
    if let Ok(rows) = serde_json::from_str::<Vec<Vec<Value>>>(raw) {
        if let Some(v) = rows.first().and_then(|row| row.first()).and_then(json_i64) {
            return Some(v);
        }
    }

    let rows = serde_json::from_str::<Vec<serde_json::Map<String, Value>>>(raw).ok()?;
    for row in rows {
        if let Some(v) = row
            .get("count(*)")
            .or_else(|| row.get("count_star()"))
            .or_else(|| row.get("count"))
            .and_then(json_i64)
        {
            return Some(v);
        }
        if let Some(v) = row.values().next().and_then(json_i64) {
            return Some(v);
        }
    }
    None
}

impl McpServer {
    pub(crate) fn axon_resume_vectorization(&self, _args: &Value) -> Option<Value> {
        let runtime_mode = AxonRuntimeMode::from_env();
        match self.graph_store.backfill_file_vectorization_queue() {
            Ok(count) => {
                let mut evidence = format!(
                    "Queued {} file(s) for deferred chunk vectorization.\nRuntime mode: {}.\n",
                    count,
                    runtime_mode.as_str()
                );
                if runtime_mode.semantic_workers_enabled() {
                    evidence.push_str(
                        "Semantic workers are active; queued files can be consumed immediately.\n",
                    );
                } else {
                    evidence.push_str(
                        "Semantic workers are disabled in the current runtime mode; processing remains deferred until a `full` restart.\n",
                    );
                }
                let summary = if count == 0 {
                    "no missing vectorization backlog found"
                } else {
                    "vectorization backlog re-queued"
                };
                let report = format!(
                    "### 🧠 Resume Vectorization\n\n{}",
                    format_standard_contract(
                        "ok",
                        summary,
                        "workspace:*",
                        &evidence,
                        &[
                            "restart in `full` mode to let semantic workers consume the queue",
                            "use `health` or `debug` to inspect graph/vector readiness and queue depth",
                        ],
                        "high",
                    )
                );
                Some(json!({
                    "content": [{ "type": "text", "text": report }],
                    "data": {
                        "queued_files": count,
                        "runtime_mode": runtime_mode.as_str(),
                        "semantic_workers_enabled": runtime_mode.semantic_workers_enabled()
                    }
                }))
            }
            Err(err) => Some(json!({
                "content": [{ "type": "text", "text": format!("Resume vectorization error: {}", err) }],
                "isError": true
            })),
        }
    }

    pub(crate) fn axon_refine_lattice(&self, _args: &Value) -> Option<Value> {
        let store = &self.graph_store;
        let refine_query = "
            MATCH (elixir:Symbol {is_nif: true})<-[:CONTAINS]-(e_file:File)
            MATCH (rust:Symbol {is_nif: true})<-[:CONTAINS]-(r_file:File)
            WHERE elixir.name = rust.name 
            MERGE (elixir)-[:CALLS_NIF]->(rust)
            RETURN elixir.name, e_file.path, r_file.path
        ";
        match store.query_json(refine_query) {
            Ok(res) => {
                let parsed: Vec<Value> = serde_json::from_str(&res).unwrap_or_default();
                let count = parsed.len();
                let report = if count > 0 {
                    format!(
                        "✨ **Lattice Refiner exécuté avec succès.**\n\nJ'ai découvert et lié **{} ponts FFI (Rustler NIFs)** entre Elixir et Rust.\n\n{}",
                        count,
                        format_table_from_json(&res, &["Nom NIF", "Fichier Elixir", "Fichier Rust"])
                    )
                } else {
                    "✅ **Lattice Refiner exécuté.**\nAucun nouveau pont FFI (Rustler NIF) non-lié n'a été détecté dans le graphe.".to_string()
                };
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("Refiner Error: {}", e) }], "isError": true }),
            ),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn axon_debug(&self) -> Option<Value> {
        self.axon_debug_with_args(&json!({}))
    }

    pub(crate) fn axon_debug_with_args(&self, args: &Value) -> Option<Value> {
        let mode = args.get("mode").and_then(|v| v.as_str());
        let canonical_count = |query: &str| -> i64 {
            self.graph_store
                .execute_raw_sql_gateway(query)
                .ok()
                .as_deref()
                .and_then(parse_scalar_count_row)
                .unwrap_or(0)
        };
        let snapshot_count = |query: &str| -> i64 {
            self.graph_store
                .query_count_on_reader_with_freshness(query, ReadFreshness::StaleOk)
                .unwrap_or(0)
        };
        let snapshot_json = |query: &str| -> String {
            self.graph_store
                .query_json_on_reader_with_freshness(query, ReadFreshness::StaleOk)
                .unwrap_or_else(|_| "[]".to_string())
        };

        let file_count = snapshot_count("SELECT count(*) FROM File");
        let pending_count = snapshot_count("SELECT count(*) FROM File WHERE status = 'pending'");
        let indexing_count = snapshot_count("SELECT count(*) FROM File WHERE status = 'indexing'");
        let degraded_count =
            snapshot_count("SELECT count(*) FROM File WHERE status = 'indexed_degraded'");
        let oversized_count = snapshot_count(
            "SELECT count(*) FROM File WHERE status = 'oversized_for_current_budget'",
        );
        let skipped_count = snapshot_count("SELECT count(*) FROM File WHERE status = 'skipped'");
        let graph_ready_count =
            snapshot_count("SELECT count(*) FROM File WHERE graph_ready = TRUE");
        let vector_ready_query = format!(
            "WITH pending_vector_chunks AS ( \
               SELECT c.file_path AS file_path \
               FROM Chunk c \
               LEFT JOIN ChunkEmbedding ce \
                 ON ce.chunk_id = c.id \
                AND ce.model_id = '{CHUNK_MODEL_ID}' \
                AND ce.source_hash = c.content_hash \
               WHERE c.file_path IS NOT NULL \
                 AND (ce.chunk_id IS NULL OR ce.source_hash IS DISTINCT FROM c.content_hash) \
               GROUP BY 1 \
             ) \
             SELECT COUNT(*) \
             FROM File f \
             LEFT JOIN pending_vector_chunks pvc ON pvc.file_path = f.path \
             WHERE f.graph_ready = TRUE AND pvc.file_path IS NULL"
        );
        let vector_ready_count = snapshot_count(&vector_ready_query);
        let (graph_projection_queue_queued, graph_projection_queue_inflight) = self
            .graph_store
            .fetch_graph_projection_queue_counts()
            .unwrap_or((0, 0));
        let graph_projection_queue_depth =
            graph_projection_queue_queued + graph_projection_queue_inflight;
        let (file_vectorization_queue_queued, file_vectorization_queue_inflight) = self
            .graph_store
            .fetch_file_vectorization_queue_counts()
            .unwrap_or((0, 0));
        let file_vectorization_queue_depth =
            file_vectorization_queue_queued + file_vectorization_queue_inflight;
        let reader_snapshot_age_ms = self.graph_store.reader_snapshot_age_ms();
        let reader_refresh_failures_total = self.graph_store.reader_refresh_failures_total();
        let reader_snapshot = self.graph_store.reader_snapshot_diagnostics();
        let canonical_file_count = canonical_count("SELECT count(*) FROM File");
        let truth_drift_files = (canonical_file_count - file_count).abs();
        let completed_count = (file_count - pending_count - indexing_count).max(0);
        let completion_rate = if file_count > 0 {
            (completed_count as f64 / file_count as f64) * 100.0
        } else {
            0.0
        };
        let symbol_count = snapshot_count("SELECT count(*) FROM Symbol");
        let edge_count = snapshot_count(
            "SELECT (SELECT count(*) FROM CONTAINS) + (SELECT count(*) FROM CALLS) + (SELECT count(*) FROM CALLS_NIF)",
        );
        let memory = process_memory_snapshot();
        let storage = duckdb_storage_snapshot(&self.graph_store);
        let duckdb_memory = duckdb_memory_snapshot(&self.graph_store);
        let ingress = ingress_metrics_snapshot();
        let provider = current_embedding_provider_diagnostics();
        let lane_config = embedding_lane_config_from_env();
        let vector_runtime = service_guard::vector_runtime_metrics();
        let vector_latency = service_guard::vector_runtime_latency_summaries();
        let vector_lane_state_record = self
            .graph_store
            .vector_lane_state_record("vector")
            .ok()
            .flatten();
        let latest_vector_worker_fault = self
            .graph_store
            .latest_vector_worker_fault("vector")
            .ok()
            .flatten();
        let vector_controller = current_vector_batch_controller_diagnostics(&lane_config);
        let optimizer_host_snapshot = collect_host_snapshot();
        let optimizer_policy_snapshot = collect_operator_policy_snapshot(&optimizer_host_snapshot);
        let optimizer_runtime_signals = collect_runtime_signals_window(&self.graph_store);
        let optimizer_recent_analytics = collect_recent_analytics_window(&self.graph_store);
        let gpu_memory_snapshot = current_gpu_memory_snapshot();
        let gpu_memory_pressure = current_gpu_memory_pressure_active();
        let gpu_memory_soft_limit = gpu_memory_soft_limit_mb();
        let interactive_active = service_guard::interactive_priority_active()
            || service_guard::interactive_requests_in_flight() > 0;
        let gpu_present = std::env::var("AXON_EMBEDDING_GPU_PRESENT")
            .ok()
            .map(|value| value.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let provider_gpu_mismatch = provider.provider_requested.eq_ignore_ascii_case("cuda")
            && !provider.provider_effective.eq_ignore_ascii_case("cuda");
        let acceleration_state = if provider.provider_effective == "cpu_missing_cuda_provider" {
            "gpu_runtime_missing_provider"
        } else if provider_gpu_mismatch && !gpu_present {
            "gpu_requested_but_unavailable"
        } else if provider.provider_effective.eq_ignore_ascii_case("cuda") {
            "gpu_active"
        } else {
            "cpu_only"
        };
        let drain_state = current_vector_drain_state(
            optimizer_runtime_signals.file_vectorization_queue_depth,
            service_guard::current_pressure(),
            interactive_active,
            &provider.provider_requested,
            &provider.provider_effective,
        );
        let gpu_background_worker_cap = if provider.provider_effective.eq_ignore_ascii_case("cuda")
        {
            allowed_gpu_vector_workers(
                optimizer_runtime_signals.file_vectorization_queue_depth,
                service_guard::current_pressure(),
            )
        } else {
            0
        };
        let avg_chunks_per_embed_call = if vector_runtime.embed_calls_total > 0 {
            vector_runtime.chunks_embedded_total as f64 / vector_runtime.embed_calls_total as f64
        } else {
            0.0
        };
        let avg_files_per_embed_call = if vector_runtime.embed_calls_total > 0 {
            vector_runtime.files_touched_total as f64 / vector_runtime.embed_calls_total as f64
        } else {
            0.0
        };
        let embed_ms_per_chunk = if vector_runtime.chunks_embedded_total > 0 {
            vector_runtime.embed_ms_total as f64 / vector_runtime.chunks_embedded_total as f64
        } else {
            0.0
        };
        let fetch_ms_per_chunk = if vector_runtime.chunks_embedded_total > 0 {
            vector_runtime.fetch_ms_total as f64 / vector_runtime.chunks_embedded_total as f64
        } else {
            0.0
        };
        let db_write_ms_per_chunk = if vector_runtime.chunks_embedded_total > 0 {
            vector_runtime.db_write_ms_total as f64 / vector_runtime.chunks_embedded_total as f64
        } else {
            0.0
        };
        let mark_done_ms_per_completed_file = if vector_runtime.files_completed_total > 0 {
            vector_runtime.mark_done_ms_total as f64 / vector_runtime.files_completed_total as f64
        } else {
            0.0
        };
        let avg_embed_input_texts_per_call = if vector_runtime.embed_calls_total > 0 {
            vector_runtime.embed_input_texts_total as f64 / vector_runtime.embed_calls_total as f64
        } else {
            0.0
        };
        let avg_embed_input_bytes_per_call = if vector_runtime.embed_calls_total > 0 {
            vector_runtime.embed_input_text_bytes_total as f64
                / vector_runtime.embed_calls_total as f64
        } else {
            0.0
        };
        let avg_embed_input_bytes_per_chunk = if vector_runtime.chunks_embedded_total > 0 {
            vector_runtime.embed_input_text_bytes_total as f64
                / vector_runtime.chunks_embedded_total as f64
        } else {
            0.0
        };
        let embed_clone_ms_per_call = if vector_runtime.embed_calls_total > 0 {
            vector_runtime.embed_clone_ms_total as f64 / vector_runtime.embed_calls_total as f64
        } else {
            0.0
        };
        let embed_transform_ms_per_call = if vector_runtime.embed_calls_total > 0 {
            vector_runtime.embed_transform_ms_total as f64 / vector_runtime.embed_calls_total as f64
        } else {
            0.0
        };
        let embed_export_ms_per_call = if vector_runtime.embed_calls_total > 0 {
            vector_runtime.embed_export_ms_total as f64 / vector_runtime.embed_calls_total as f64
        } else {
            0.0
        };
        let embed_transform_ms_per_chunk = if vector_runtime.chunks_embedded_total > 0 {
            vector_runtime.embed_transform_ms_total as f64
                / vector_runtime.chunks_embedded_total as f64
        } else {
            0.0
        };
        let embed_export_ms_per_chunk = if vector_runtime.chunks_embedded_total > 0 {
            vector_runtime.embed_export_ms_total as f64
                / vector_runtime.chunks_embedded_total as f64
        } else {
            0.0
        };
        let stage_rows = snapshot_json(
            "SELECT COALESCE(file_stage, 'unknown'), count(*) \
             FROM File \
             GROUP BY 1 \
             ORDER BY count(*) DESC, 1 ASC \
             LIMIT 6",
        );
        let stage_counts = parse_reason_count_rows(&stage_rows);
        let backlog_reason_rows = snapshot_json(
            "SELECT COALESCE(status_reason, 'unknown'), count(*) \
             FROM File \
             WHERE status IN ('pending', 'indexing') \
             GROUP BY 1 \
             ORDER BY count(*) DESC, 1 ASC \
             LIMIT 5",
        );
        let vector_queue_status_rows = snapshot_json(
            "SELECT status, count(*) \
             FROM FileVectorizationQueue \
             GROUP BY 1 \
             ORDER BY count(*) DESC, 1 ASC",
        );
        let latest_optimizer_decision_row = snapshot_json(
            "SELECT decision_id, mode, action_profile_id, at_ms, would_apply, applied, evaluation_window_start_ms, evaluation_window_end_ms \
             FROM OptimizerDecisionLog \
             ORDER BY at_ms DESC \
             LIMIT 1",
        );
        let latest_optimizer_reward_row = snapshot_json(
            "SELECT decision_id, observed_at_ms, throughput_chunks_per_hour, throughput_files_per_hour \
             FROM RewardObservationLog \
             ORDER BY observed_at_ms DESC \
             LIMIT 1",
        );
        let vector_queue_statuses = parse_reason_count_rows(&vector_queue_status_rows);
        let backlog_reasons = parse_reason_count_rows(&backlog_reason_rows);
        let latest_optimizer_decision =
            serde_json::from_str::<Vec<Vec<serde_json::Value>>>(&latest_optimizer_decision_row)
                .ok()
                .and_then(|rows| rows.into_iter().next());
        let latest_optimizer_reward =
            serde_json::from_str::<Vec<Vec<serde_json::Value>>>(&latest_optimizer_reward_row)
                .ok()
                .and_then(|rows| rows.into_iter().next());
        let backlog_reason_section = if backlog_reasons.is_empty() {
            if pending_count + indexing_count > 0 {
                format!(
                    "**Causes backlog dominantes :**\n*   `unknown` : {}\n\n",
                    pending_count + indexing_count
                )
            } else {
                "*   Causes backlog dominantes : aucune.\n".to_string()
            }
        } else {
            let lines = backlog_reasons
                .iter()
                .map(|(reason, count)| format!("*   `{}` : {}", reason, count))
                .collect::<Vec<_>>()
                .join("\n");
            format!("**Causes backlog dominantes :**\n{}\n\n", lines)
        };
        let file_stage_section = if stage_counts.is_empty() {
            "*   Stages fichiers : aucune donnée.\n\n".to_string()
        } else {
            let lines = stage_counts
                .iter()
                .map(|(stage, count)| format!("*   `{}` : {}", stage, count))
                .collect::<Vec<_>>()
                .join("\n");
            format!("**Stages canoniques :**\n{}\n\n", lines)
        };
        let vector_queue_status_section = if vector_queue_statuses.is_empty() {
            "*   File vectorization queue statuses : aucune donnée.\n\n".to_string()
        } else {
            let lines = vector_queue_statuses
                .iter()
                .map(|(status, count)| format!("*   `{}` : {}", status, count))
                .collect::<Vec<_>>()
                .join("\n");
            format!("**File vectorization queue statuses :**\n{}\n\n", lines)
        };

        let mut evidence = format!(
            "## 🤖 Axon Core V2 (Maestria) - Diagnostic Interne\n\n\
            **Architecture du Moteur :**\n\
            *   **Mode :** Embarqué (C-FFI) sans réseau TCP.\n\
            *   **Base de Graphe :** DuckDB (Local, Zero-Copy).\n\
            *   **Parseurs Actifs :** Rust, Elixir, Python, TypeScript, etc.\n\
            *   **Protection OOM :** Option B (Watchdog Process Cycling Actif à 14 Go).\n\n\
            **Mémoire Runtime :**\n\
            *   RSS total : {}\n\
            *   RSS Anon : {}\n\
            *   RSS Fichier : {}\n\
            *   RSS Shmem : {}\n\n\
            **Volume du Graphe :**\n\
            *   Fichiers connus : {}\n\
            *   Symboles extraits : {}\n\
            *   Relations (Edges) : {}\n\n\
            **État d’Indexation :**\n\
            *   Fichiers terminés : {}\n\
            *   Backlog restant : {}\n\
            *   Pending : {}\n\
            *   Indexing : {}\n\
            *   Indexed degraded : {}\n\
            *   Oversized : {}\n\
            *   Skipped : {}\n\
            *   Graph Ready : {}\n\
            *   Vector Ready : {}\n\
            *   Taux de complétion : {:.2} %\n\n\
            *   Graph Projection Queue Queued : {}\n\
            *   Graph Projection Queue Inflight : {}\n\
            *   Graph Projection Queue Pending : {}\n\n\
            *   File Vectorization Queue Queued : {}\n\
            *   File Vectorization Queue Inflight : {}\n\
            *   File Vectorization Queue Pending : {}\n\n\
            {}\
            {}\
            {}\
            **Stockage DuckDB :**\n\
            *   Fichier principal : {}\n\
            *   WAL : {}\n\
            *   Total : {}\n\n\
            **Mémoire DuckDB :**\n\
            *   Mémoire allouée : {}\n\
            *   Temporaire/spill : {}\n\n\
            **Ingress Buffer :**\n\
            *   Activé : {}\n\
            *   Entrées bufferisées : {}\n\
            *   Indices de sous-arbre : {}\n\
            *   Subtree hints en vol : {}\n\
            *   Subtree hints acceptés : {}\n\
            *   Subtree hints bloqués : {}\n\
            *   Subtree hints supprimés : {}\n\
            *   Subtree hints productifs : {}\n\
            *   Subtree hints non productifs : {}\n\
            *   Subtree hints abandonnés : {}\n\
            *   Événements collapsés : {}\n\
            *   Flushs : {}\n\
            *   Dernier flush : {} ms\n\
            *   Dernier lot promu : {}\n\n\
            **Embedding Runtime :**\n\
            *   GPU Present Detected : {}\n\
            *   Embedding Provider Requested : {}\n\
            *   Embedding Provider Effective : {}\n\
            *   Embedding Acceleration State : {}\n\
            *   Drain State : {}\n\
            *   GPU Background Worker Cap : {}\n\
            *   ORT Strategy : {}\n\
            *   Query Workers : {}\n\
            *   Vector Workers : {}\n\
            *   Graph Workers : {}\n\
            *   Chunk Batch Size : {}\n\
            *   File Vectorization Batch Size : {}\n\
            *   Graph Batch Size : {}\n\n\
            *   Max Chunks Per File : {}\n\
            *   Max Embed Batch Bytes : {}\n\n\
            **Vector Runtime Breakdown :**\n\
            *   Fetch ms total : {}\n\
            *   Embed ms total : {}\n\
            *   DB write ms total : {}\n\
            *   Completion check ms total : {}\n\
            *   Mark done ms total : {}\n\
            *   Prepare dispatch total : {}\n\
            *   Prepare prefetch total : {}\n\
            *   Prepare fallback inline total : {}\n\
            *   Prepare reply wait ms total : {}\n\
            *   Prepare send wait ms total : {}\n\
            *   Prepare queue wait ms total : {}\n\
*   Prepare queue depth current/max : {}/{}\n\
*   Embed input texts total : {}\n\
*   Embed input text bytes total : {}\n\
*   Embed clone ms total : {}\n\
*   Embed transform ms total : {}\n\
*   Embed export ms total : {}\n\
*   Finalize enqueued total : {}\n\
            *   Finalize fallback inline total : {}\n\
            *   Finalize send wait ms total : {}\n\
            *   Finalize queue wait ms total : {}\n\
            *   Finalize queue depth current/max : {}/{}\n\
            *   Batches total : {}\n\
            *   Chunks embedded total : {}\n\
            *   Files completed total : {}\n\
            *   Embed calls total : {}\n\
            *   Claimed work items total : {}\n\
            *   Partial file cycles total : {}\n\
            *   Mark done calls total : {}\n\
            *   Files touched total : {}\n\
*   Avg chunks per embed call : {:.2}\n\
*   Avg files per embed call : {:.2}\n\
*   Avg embed input texts per call : {:.2}\n\
*   Avg embed input bytes per call : {:.2}\n\
*   Avg embed input bytes per chunk : {:.2}\n\
*   Embed clone ms per call : {:.2}\n\
*   Embed transform ms per call : {:.2}\n\
*   Embed export ms per call : {:.2}\n\
*   Embed ms per chunk : {:.2}\n\
*   Embed transform ms per chunk : {:.2}\n\
*   Embed export ms per chunk : {:.2}\n\
            *   Fetch ms per chunk : {:.2}\n\
            *   DB write ms per chunk : {:.2}\n\
            *   Mark done ms per completed file : {:.2}\n\n\
            **Vector Stage Latencies (recent window) :**\n\
            *   Fetch p50/p95/max ms : {}/{}/{} (samples: {})\n\
            *   Embed p50/p95/max ms : {}/{}/{} (samples: {})\n\
            *   DB write p50/p95/max ms : {}/{}/{} (samples: {})\n\
            *   Mark done p50/p95/max ms : {}/{}/{} (samples: {})\n\n\
            **Vector Batch Controller :**\n\
            *   State : {}\n\
            *   Reason : {}\n\
            *   Adjustments total : {}\n\
            *   Last adjustment ms : {}\n\
            *   Target embed batch chunks : {}\n\
            *   Target files per cycle : {}\n\
            *   Window embed calls : {}\n\
            *   Window chunks : {}\n\
            *   Window files touched : {}\n\n\
            **Shadow Optimizer :**\n\
            *   Latest decision id : {}\n\
            *   Latest decision mode : {}\n\
            *   Latest action profile : {}\n\
            *   Latest decision at ms : {}\n\
            *   Latest would_apply/applied : {}/{}\n\
            *   Latest reward decision id : {}\n\
            *   Latest reward at ms : {}\n\
            *   Latest throughput chunks/hour : {:.2}\n\
            *   Latest throughput files/hour : {:.2}\n\n\
            *Note aux Agents IA : Toute erreur 'TCP auth closed' observée dans des logs Elixir n'est pas liée à ce serveur MCP. Axon Core V2 est 100% autonome.*",
            format_bytes_human(memory.rss_bytes),
            format_bytes_human(memory.rss_anon_bytes),
            format_bytes_human(memory.rss_file_bytes),
            format_bytes_human(memory.rss_shmem_bytes),
            file_count,
            symbol_count,
            edge_count,
            completed_count,
            pending_count + indexing_count,
            pending_count,
            indexing_count,
            degraded_count,
            oversized_count,
            skipped_count,
            graph_ready_count,
            vector_ready_count,
            completion_rate,
            graph_projection_queue_queued,
            graph_projection_queue_inflight,
            graph_projection_queue_depth,
            file_vectorization_queue_queued,
            file_vectorization_queue_inflight,
            file_vectorization_queue_depth,
            file_stage_section,
            backlog_reason_section,
            vector_queue_status_section,
            format_bytes_human(storage.db_file_bytes),
            format_bytes_human(storage.db_wal_bytes),
            format_bytes_human(storage.db_total_bytes),
            format_bytes_human(duckdb_memory.memory_usage_bytes),
            format_bytes_human(duckdb_memory.temporary_storage_bytes),
            if ingress.enabled { "oui" } else { "non" },
            ingress.buffered_entries,
            ingress.subtree_hints,
            ingress.subtree_hint_in_flight,
            ingress.subtree_hint_accepted_total,
            ingress.subtree_hint_blocked_total,
            ingress.subtree_hint_suppressed_total,
            ingress.subtree_hint_productive_total,
            ingress.subtree_hint_unproductive_total,
            ingress.subtree_hint_dropped_total,
            ingress.collapsed_total,
            ingress.flush_count,
            ingress.last_flush_duration_ms,
            ingress.last_promoted_count,
            if gpu_present { "yes" } else { "no" },
            provider.provider_requested,
            provider.provider_effective,
            acceleration_state,
            drain_state.as_str(),
            gpu_background_worker_cap,
            provider.ort_strategy,
            lane_config.query_workers,
            lane_config.vector_workers,
            lane_config.graph_workers,
            lane_config.chunk_batch_size,
            lane_config.file_vectorization_batch_size,
            lane_config.graph_batch_size,
            lane_config.max_chunks_per_file,
            lane_config.max_embed_batch_bytes,
            vector_runtime.fetch_ms_total,
            vector_runtime.embed_ms_total,
            vector_runtime.db_write_ms_total,
            vector_runtime.completion_check_ms_total,
            vector_runtime.mark_done_ms_total,
            vector_runtime.prepare_dispatch_total,
            vector_runtime.prepare_prefetch_total,
            vector_runtime.prepare_fallback_inline_total,
            vector_runtime.prepare_reply_wait_ms_total,
            vector_runtime.prepare_send_wait_ms_total,
            vector_runtime.prepare_queue_wait_ms_total,
            vector_runtime.prepare_queue_depth_current,
            vector_runtime.prepare_queue_depth_max,
            vector_runtime.embed_input_texts_total,
            vector_runtime.embed_input_text_bytes_total,
            vector_runtime.embed_clone_ms_total,
            vector_runtime.embed_transform_ms_total,
            vector_runtime.embed_export_ms_total,
            vector_runtime.finalize_enqueued_total,
            vector_runtime.finalize_fallback_inline_total,
            vector_runtime.finalize_send_wait_ms_total,
            vector_runtime.finalize_queue_wait_ms_total,
            vector_runtime.finalize_queue_depth_current,
            vector_runtime.finalize_queue_depth_max,
            vector_runtime.batches_total,
            vector_runtime.chunks_embedded_total,
            vector_runtime.files_completed_total,
            vector_runtime.embed_calls_total,
            vector_runtime.claimed_work_items_total,
            vector_runtime.partial_file_cycles_total,
            vector_runtime.mark_done_calls_total,
            vector_runtime.files_touched_total,
            avg_chunks_per_embed_call,
            avg_files_per_embed_call,
            avg_embed_input_texts_per_call,
            avg_embed_input_bytes_per_call,
            avg_embed_input_bytes_per_chunk,
            embed_clone_ms_per_call,
            embed_transform_ms_per_call,
            embed_export_ms_per_call,
            embed_ms_per_chunk,
            embed_transform_ms_per_chunk,
            embed_export_ms_per_chunk,
            fetch_ms_per_chunk,
            db_write_ms_per_chunk,
            mark_done_ms_per_completed_file,
            vector_latency.fetch.p50_ms,
            vector_latency.fetch.p95_ms,
            vector_latency.fetch.max_ms,
            vector_latency.fetch.samples,
            vector_latency.embed.p50_ms,
            vector_latency.embed.p95_ms,
            vector_latency.embed.max_ms,
            vector_latency.embed.samples,
            vector_latency.db_write.p50_ms,
            vector_latency.db_write.p95_ms,
            vector_latency.db_write.max_ms,
            vector_latency.db_write.samples,
            vector_latency.mark_done.p50_ms,
            vector_latency.mark_done.p95_ms,
            vector_latency.mark_done.max_ms,
            vector_latency.mark_done.samples,
            vector_controller.state.as_str(),
            vector_controller.reason.clone(),
            vector_controller.adjustments_total,
            vector_controller.last_adjustment_ms,
            vector_controller.target_embed_batch_chunks,
            vector_controller.target_files_per_cycle,
            vector_controller.window_embed_calls,
            vector_controller.window_chunks,
            vector_controller.window_files_touched,
            latest_optimizer_decision
                .as_ref()
                .and_then(|row| row.first())
                .and_then(|value| value.as_str())
                .unwrap_or("none"),
            latest_optimizer_decision
                .as_ref()
                .and_then(|row| row.get(1))
                .and_then(|value| value.as_str())
                .unwrap_or("none"),
            latest_optimizer_decision
                .as_ref()
                .and_then(|row| row.get(2))
                .and_then(|value| value.as_str())
                .unwrap_or("hold"),
            latest_optimizer_decision
                .as_ref()
                .and_then(|row| row.get(3))
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
            latest_optimizer_decision
                .as_ref()
                .and_then(|row| row.get(4))
                .and_then(|value| value.as_bool())
                .unwrap_or(false),
            latest_optimizer_decision
                .as_ref()
                .and_then(|row| row.get(5))
                .and_then(|value| value.as_bool())
                .unwrap_or(false),
            latest_optimizer_reward
                .as_ref()
                .and_then(|row| row.first())
                .and_then(|value| value.as_str())
                .unwrap_or("none"),
            latest_optimizer_reward
                .as_ref()
                .and_then(|row| row.get(1))
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
            latest_optimizer_reward
                .as_ref()
                .and_then(|row| row.get(2))
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0),
            latest_optimizer_reward
                .as_ref()
                .and_then(|row| row.get(3))
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0),
        );
        if reader_snapshot_age_ms == u64::MAX {
            evidence
                .push_str("\n**Reader Snapshot:** indisponible (mode mémoire ou non initialisé)\n");
        } else {
            evidence.push_str(&format!(
                "\n**Reader Snapshot:** age={} ms, commit_epoch={}, reader_epoch={}, lag={}, refresh_inflight={}, refresh_failures_total={}, reads_on_reader_total={}, reads_on_writer_total={}, refresh_coalesced_total={}\n",
                reader_snapshot_age_ms,
                reader_snapshot.commit_epoch,
                reader_snapshot.reader_epoch,
                reader_snapshot.reader_epoch_lag,
                reader_snapshot.refresh_inflight,
                reader_refresh_failures_total,
                reader_snapshot.reads_on_reader_total,
                reader_snapshot.reads_on_writer_total,
                reader_snapshot.refresh_coalesced_total
            ));
        }
        evidence.push_str(&format!(
            "**Vector Lane:** state={}, transition_at_ms={}, last_success_at_ms={}, last_fault_at_ms={}, restarts_total={}\n",
            vector_runtime.vector_lane_state.as_str(),
            vector_runtime.vector_lane_last_transition_at_ms,
            vector_runtime.vector_lane_last_success_at_ms,
            vector_runtime.vector_lane_last_fault_at_ms,
            vector_runtime.vector_worker_restarts_total,
        ));
        if let Some(record) = vector_lane_state_record.as_ref() {
            evidence.push_str(&format!(
                "**Vector Lane Record:** state={}, updated_at_ms={}, restart_attempt={}, last_fault_id={}\n",
                record.state,
                record.updated_at_ms,
                record.restart_attempt,
                record.last_fault_id.as_deref().unwrap_or("none"),
            ));
        }
        if let Some(fault) = latest_vector_worker_fault.as_ref() {
            evidence.push_str(&format!(
                "**Latest Vector Fault:** stage={}, class={}, provider={}, batch_id={}, restart_attempt={}, at_ms={}\n",
                fault.fatal_stage,
                fault.fatal_class,
                fault.provider,
                fault.batch_id.as_deref().unwrap_or("none"),
                fault.restart_attempt,
                fault.occurred_at_ms,
            ));
        }
        evidence.push_str(&format!(
            "**Truth Drift (File count):** canonical={} vs reader={} (delta={})\n",
            canonical_file_count, file_count, truth_drift_files
        ));
        let report = format!(
            "## 🤖 Axon Debug\n\n{}",
            format_standard_contract(
                "ok",
                "runtime diagnostics collected",
                "workspace:*",
                &evidence_by_mode(&evidence, mode),
                &[
                    "run `truth_check` to inspect canonical vs reader drift",
                    "run `health` for project-level view"
                ],
                "high",
            )
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
                "data": {
                "embedding_contract": {
                    "model_name": MODEL_NAME,
                    "dimension": DIMENSION,
                    "native_dimension": NATIVE_DIMENSION,
                    "max_length": MAX_LENGTH,
                    "storage_type": STORAGE_TYPE,
                    "gpu_present_detected": gpu_present,
                    "execution_provider": provider.provider_effective,
                    "provider_requested": provider.provider_requested,
                    "provider_effective": provider.provider_effective,
                    "provider_init_error": provider.provider_init_error,
                    "provider_gpu_mismatch": provider_gpu_mismatch,
                    "acceleration_state": acceleration_state,
                    "drain_state": drain_state.as_str(),
                    "gpu_background_worker_cap": gpu_background_worker_cap,
                    "ort_strategy": provider.ort_strategy,
                    "ort_dylib_path": provider.ort_dylib_path,
                    "query_workers": lane_config.query_workers,
                    "vector_workers": lane_config.vector_workers,
                    "graph_workers": lane_config.graph_workers,
                    "chunk_batch_size": lane_config.chunk_batch_size,
                    "file_vectorization_batch_size": lane_config.file_vectorization_batch_size,
                    "graph_batch_size": lane_config.graph_batch_size,
                    "max_chunks_per_file": lane_config.max_chunks_per_file,
                    "max_embed_batch_bytes": lane_config.max_embed_batch_bytes,
                    "gpu_memory": {
                        "backend": gpu_telemetry_backend_name(),
                        "device_index": gpu_telemetry_device_index(),
                        "cache_ttl_ms": gpu_telemetry_cache_ttl_ms(),
                        "soft_limit_mb": gpu_memory_soft_limit,
                        "pressure_embed_batch_chunks": gpu_pressure_embed_batch_chunks(
                            lane_config.chunk_batch_size,
                            (lane_config.chunk_batch_size.max(1) / 2).max(8),
                        ),
                        "pressure_files_per_cycle": gpu_pressure_files_per_cycle(
                            lane_config.file_vectorization_batch_size,
                        ),
                        "pressure_active": gpu_memory_pressure,
                        "available": gpu_memory_snapshot.is_some(),
                        "total_mb": gpu_memory_snapshot.map(|snapshot| snapshot.total_mb),
                        "used_mb": gpu_memory_snapshot.map(|snapshot| snapshot.used_mb),
                        "free_mb": gpu_memory_snapshot.map(|snapshot| snapshot.free_mb),
                    },
                    "vector_runtime": {
                        "fetch_ms_total": vector_runtime.fetch_ms_total,
                        "embed_ms_total": vector_runtime.embed_ms_total,
                        "db_write_ms_total": vector_runtime.db_write_ms_total,
                        "completion_check_ms_total": vector_runtime.completion_check_ms_total,
                        "mark_done_ms_total": vector_runtime.mark_done_ms_total,
                        "prepare_dispatch_total": vector_runtime.prepare_dispatch_total,
                        "prepare_prefetch_total": vector_runtime.prepare_prefetch_total,
                        "prepare_fallback_inline_total": vector_runtime.prepare_fallback_inline_total,
                        "prepared_work_items_total": vector_runtime.prepared_work_items_total,
                        "prepare_empty_batches_total": vector_runtime.prepare_empty_batches_total,
                        "prepare_immediate_completed_total": vector_runtime.prepare_immediate_completed_total,
                        "prepare_failed_fetches_total": vector_runtime.prepare_failed_fetches_total,
                        "prepare_reply_wait_ms_total": vector_runtime.prepare_reply_wait_ms_total,
                        "prepare_send_wait_ms_total": vector_runtime.prepare_send_wait_ms_total,
                        "prepare_queue_wait_ms_total": vector_runtime.prepare_queue_wait_ms_total,
                        "prepare_queue_depth_current": vector_runtime.prepare_queue_depth_current,
                        "prepare_queue_depth_max": vector_runtime.prepare_queue_depth_max,
                        "prepare_inflight_current": vector_runtime.prepare_inflight_current,
                        "prepare_inflight_max": vector_runtime.prepare_inflight_max,
                        "ready_queue_depth_current": vector_runtime.ready_queue_depth_current,
                        "ready_queue_depth_max": vector_runtime.ready_queue_depth_max,
                        "active_claimed_current": vector_runtime.active_claimed_current,
                        "prepare_claimed_current": vector_runtime.prepare_claimed_current,
                        "ready_claimed_current": vector_runtime.ready_claimed_current,
                        "embed_input_texts_total": vector_runtime.embed_input_texts_total,
                        "embed_input_text_bytes_total": vector_runtime.embed_input_text_bytes_total,
                        "embed_clone_ms_total": vector_runtime.embed_clone_ms_total,
                        "embed_transform_ms_total": vector_runtime.embed_transform_ms_total,
                        "embed_export_ms_total": vector_runtime.embed_export_ms_total,
                        "embed_attempts_total": vector_runtime.embed_attempts_total,
                        "embed_inflight_started_at_ms": vector_runtime.embed_inflight_started_at_ms,
                        "embed_inflight_texts_current": vector_runtime.embed_inflight_texts_current,
                        "embed_inflight_text_bytes_current": vector_runtime.embed_inflight_text_bytes_current,
                        "vector_workers_started_total": vector_runtime.vector_workers_started_total,
                        "vector_workers_stopped_total": vector_runtime.vector_workers_stopped_total,
                        "vector_workers_active_current": vector_runtime.vector_workers_active_current,
                        "vector_worker_heartbeat_at_ms": vector_runtime.vector_worker_heartbeat_at_ms,
                        "vector_worker_restarts_total": vector_runtime.vector_worker_restarts_total,
                        "vector_lane_state": vector_runtime.vector_lane_state.as_str(),
                        "vector_lane_last_transition_at_ms": vector_runtime.vector_lane_last_transition_at_ms,
                        "vector_lane_last_success_at_ms": vector_runtime.vector_lane_last_success_at_ms,
                        "vector_lane_last_fault_at_ms": vector_runtime.vector_lane_last_fault_at_ms,
                        "finalize_enqueued_total": vector_runtime.finalize_enqueued_total,
                        "finalize_fallback_inline_total": vector_runtime.finalize_fallback_inline_total,
                        "finalize_send_wait_ms_total": vector_runtime.finalize_send_wait_ms_total,
                        "finalize_queue_wait_ms_total": vector_runtime.finalize_queue_wait_ms_total,
                        "finalize_queue_depth_current": vector_runtime.finalize_queue_depth_current,
                        "finalize_queue_depth_max": vector_runtime.finalize_queue_depth_max,
                        "persist_queue_depth_current": vector_runtime.persist_queue_depth_current,
                        "persist_queue_depth_max": vector_runtime.persist_queue_depth_max,
                        "persist_claimed_current": vector_runtime.persist_claimed_current,
                        "persist_send_wait_ms_total": vector_runtime.persist_send_wait_ms_total,
                        "persist_queue_wait_ms_total": vector_runtime.persist_queue_wait_ms_total,
                        "gpu_idle_wait_ms_total": vector_runtime.gpu_idle_wait_ms_total,
                        "canonical_backlog_depth_current": vector_runtime.canonical_backlog_depth_current,
                        "canonical_backlog_depth_max": vector_runtime.canonical_backlog_depth_max,
                        "oldest_ready_batch_age_ms_current": vector_runtime.oldest_ready_batch_age_ms_current,
                        "oldest_ready_batch_age_ms_max": vector_runtime.oldest_ready_batch_age_ms_max,
                        "batches_total": vector_runtime.batches_total,
                        "chunks_embedded_total": vector_runtime.chunks_embedded_total,
                        "files_completed_total": vector_runtime.files_completed_total,
                        "embed_calls_total": vector_runtime.embed_calls_total,
                        "claimed_work_items_total": vector_runtime.claimed_work_items_total,
                        "partial_file_cycles_total": vector_runtime.partial_file_cycles_total,
                        "mark_done_calls_total": vector_runtime.mark_done_calls_total,
                        "files_touched_total": vector_runtime.files_touched_total,
                        "avg_chunks_per_embed_call": avg_chunks_per_embed_call,
                        "avg_files_per_embed_call": avg_files_per_embed_call,
                        "avg_embed_input_texts_per_call": avg_embed_input_texts_per_call,
                        "avg_embed_input_bytes_per_call": avg_embed_input_bytes_per_call,
                        "avg_embed_input_bytes_per_chunk": avg_embed_input_bytes_per_chunk,
                        "embed_clone_ms_per_call": embed_clone_ms_per_call,
                        "embed_transform_ms_per_call": embed_transform_ms_per_call,
                        "embed_export_ms_per_call": embed_export_ms_per_call,
                        "embed_ms_per_chunk": embed_ms_per_chunk,
                        "embed_transform_ms_per_chunk": embed_transform_ms_per_chunk,
                        "embed_export_ms_per_chunk": embed_export_ms_per_chunk,
                        "fetch_ms_per_chunk": fetch_ms_per_chunk,
                        "db_write_ms_per_chunk": db_write_ms_per_chunk,
                        "mark_done_ms_per_completed_file": mark_done_ms_per_completed_file,
                        "latency_recent": {
                            "fetch": {
                                "samples": vector_latency.fetch.samples,
                                "p50_ms": vector_latency.fetch.p50_ms,
                                "p95_ms": vector_latency.fetch.p95_ms,
                                "max_ms": vector_latency.fetch.max_ms
                            },
                            "embed": {
                                "samples": vector_latency.embed.samples,
                                "p50_ms": vector_latency.embed.p50_ms,
                                "p95_ms": vector_latency.embed.p95_ms,
                                "max_ms": vector_latency.embed.max_ms
                            },
                            "db_write": {
                                "samples": vector_latency.db_write.samples,
                                "p50_ms": vector_latency.db_write.p50_ms,
                                "p95_ms": vector_latency.db_write.p95_ms,
                                "max_ms": vector_latency.db_write.max_ms
                            },
                            "mark_done": {
                                "samples": vector_latency.mark_done.samples,
                                "p50_ms": vector_latency.mark_done.p50_ms,
                                "p95_ms": vector_latency.mark_done.p95_ms,
                                "max_ms": vector_latency.mark_done.max_ms
                            }
                        }
                    },
                    "file_vectorization_queue_statuses": vector_queue_statuses
                        .iter()
                        .map(|(status, count)| json!({"status": status, "count": count}))
                        .collect::<Vec<_>>(),
                    "vector_batch_controller": {
                        "state": vector_controller.state.as_str(),
                        "reason": vector_controller.reason,
                        "adjustments_total": vector_controller.adjustments_total,
                        "last_adjustment_ms": vector_controller.last_adjustment_ms,
                        "target_embed_batch_chunks": vector_controller.target_embed_batch_chunks,
                        "target_files_per_cycle": vector_controller.target_files_per_cycle,
                        "window_embed_calls": vector_controller.window_embed_calls,
                        "window_chunks": vector_controller.window_chunks,
                        "window_files_touched": vector_controller.window_files_touched,
                        "avg_chunks_per_embed_call": vector_controller.avg_chunks_per_embed_call,
                        "avg_files_per_embed_call": vector_controller.avg_files_per_embed_call,
                        "embed_ms_per_chunk": vector_controller.embed_ms_per_chunk
                    },
                    "chunk_model_id": CHUNK_MODEL_ID
                },
                "traceability": {
                    "host_snapshot": serde_json::to_value(&optimizer_host_snapshot).unwrap_or_else(|_| json!({})),
                    "policy_snapshot": serde_json::to_value(&optimizer_policy_snapshot).unwrap_or_else(|_| json!({})),
                    "runtime_signals_window": serde_json::to_value(&optimizer_runtime_signals).unwrap_or_else(|_| json!({})),
                    "recent_analytics_window": serde_json::to_value(&optimizer_recent_analytics).unwrap_or_else(|_| json!({})),
                    "latest_optimizer_decision": latest_optimizer_decision.as_ref().map(|row| json!({
                        "decision_id": row.first().and_then(|value| value.as_str()),
                        "mode": row.get(1).and_then(|value| value.as_str()),
                        "action_profile_id": row.get(2).and_then(|value| value.as_str()),
                        "at_ms": row.get(3).and_then(|value| value.as_i64()),
                        "would_apply": row.get(4).and_then(|value| value.as_bool()),
                        "applied": row.get(5).and_then(|value| value.as_bool()),
                        "evaluation_window_start_ms": row.get(6).and_then(|value| value.as_i64()),
                        "evaluation_window_end_ms": row.get(7).and_then(|value| value.as_i64())
                    })),
                    "latest_reward_observation": latest_optimizer_reward.as_ref().map(|row| json!({
                        "decision_id": row.first().and_then(|value| value.as_str()),
                        "observed_at_ms": row.get(1).and_then(|value| value.as_i64()),
                        "throughput_chunks_per_hour": row.get(2).and_then(|value| value.as_f64()),
                        "throughput_files_per_hour": row.get(3).and_then(|value| value.as_f64())
                    }))
                },
                "reader_snapshot": serde_json::to_value(&reader_snapshot).unwrap_or_else(|_| json!({})),
                "vector_lane_state_record": serde_json::to_value(&vector_lane_state_record).unwrap_or_else(|_| json!(null)),
                "latest_vector_worker_fault": serde_json::to_value(&latest_vector_worker_fault).unwrap_or_else(|_| json!(null))
            }
        }))
    }

    pub(crate) fn axon_schema_overview(&self, _args: &Value) -> Option<Value> {
        let tables = self
            .graph_store
            .query_json_on_reader_with_freshness(
                "SELECT table_schema, table_name \
                 FROM information_schema.tables \
                 WHERE table_schema IN ('main', 'soll') \
                 ORDER BY table_schema, table_name",
                ReadFreshness::StaleOk,
            )
            .unwrap_or_else(|_| "[]".to_string());
        let columns = self
            .graph_store
            .query_json_on_reader_with_freshness(
                "SELECT table_schema, table_name, COUNT(*) \
                 FROM information_schema.columns \
                 WHERE table_schema IN ('main', 'soll') \
                 GROUP BY 1,2 \
                 ORDER BY 1,2",
                ReadFreshness::StaleOk,
            )
            .unwrap_or_else(|_| "[]".to_string());

        let report = format!(
            "## 🧭 Axon Schema Overview\n\n\
             **Tables (main + soll):**\n{}\n\n\
             **Column count by table:**\n{}\n",
            format_table_from_json(&tables, &["Schema", "Table"]),
            format_table_from_json(&columns, &["Schema", "Table", "Columns"])
        );
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_list_labels_tables(&self, _args: &Value) -> Option<Value> {
        let rels = self
            .graph_store
            .query_json_on_reader_with_freshness(
                "SELECT table_name \
                 FROM information_schema.tables \
                 WHERE table_schema = 'main' \
                   AND table_name IN ('File','Symbol','Chunk','CONTAINS','CALLS','CALLS_NIF','IMPACTS','SUBSTANTIATES','GraphEmbedding','GraphProjection','GraphProjectionQueue','FileVectorizationQueue') \
                 ORDER BY table_name",
                ReadFreshness::StaleOk,
            )
            .unwrap_or_else(|_| "[]".to_string());
        let table_rows: Vec<Vec<Value>> = serde_json::from_str(&rels).unwrap_or_default();
        let mut core_tables = Vec::new();
        let mut derived_optional_tables = Vec::new();
        for row in table_rows {
            let Some(table_name) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let rendered = vec![Value::String(table_name.to_string())];
            if matches!(
                table_name,
                "GraphEmbedding"
                    | "GraphProjection"
                    | "GraphProjectionQueue"
                    | "FileVectorizationQueue"
            ) {
                derived_optional_tables.push(rendered);
            } else {
                core_tables.push(rendered);
            }
        }
        let cols = self
            .graph_store
            .query_json_on_reader_with_freshness(
                "SELECT table_name, column_name, data_type \
                 FROM information_schema.columns \
                 WHERE table_schema = 'main' \
                   AND table_name IN ('File','Symbol','CALLS','CALLS_NIF','CONTAINS','GraphEmbedding') \
                 ORDER BY table_name, ordinal_position",
                ReadFreshness::StaleOk,
            )
            .unwrap_or_else(|_| "[]".to_string());
        let report = format!(
            "## 🗂️ Labels / Tables Discovery\n\n\
             **Core tables:**\n{}\n\n\
             **Derived optional tables:**\n{}\n\n\
             **Key columns:**\n{}\n",
            format_table_from_json(
                &serde_json::to_string(&core_tables).unwrap_or_else(|_| "[]".to_string()),
                &["Table"]
            ),
            format_table_from_json(
                &serde_json::to_string(&derived_optional_tables)
                    .unwrap_or_else(|_| "[]".to_string()),
                &["Table"]
            ),
            format_table_from_json(&cols, &["Table", "Column", "Type"])
        );
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_query_examples(&self, _args: &Value) -> Option<Value> {
        let examples = r#"## 📚 Query Examples (SQL gateway / cypher tool)

1) Workspace status
`SELECT status, count(*) FROM File GROUP BY 1 ORDER BY 2 DESC;`

2) Project health
`SELECT project_code, count(*) AS known, SUM(CASE WHEN status IN ('indexed','indexed_degraded','skipped','deleted') THEN 1 ELSE 0 END) AS completed FROM File GROUP BY 1 ORDER BY known DESC;`

3) Top backlog reasons
`SELECT COALESCE(status_reason,'unknown'), count(*) FROM File WHERE status IN ('pending','indexing') GROUP BY 1 ORDER BY 2 DESC LIMIT 10;`

4) Parser/ingestion failures
`SELECT COALESCE(last_error_reason,'unknown'), count(*) FROM File WHERE last_error_reason IS NOT NULL GROUP BY 1 ORDER BY 2 DESC;`

5) Inter-language bridge visibility
`SELECT COUNT(*) AS calls, (SELECT COUNT(*) FROM CALLS_NIF) AS calls_nif FROM CALLS;`

6) Symbol lookup by project
`SELECT id, name, kind FROM Symbol WHERE project_code = 'BookingSystem' ORDER BY name LIMIT 50;`
"#;
        Some(json!({ "content": [{ "type": "text", "text": examples }] }))
    }

    pub(crate) fn axon_truth_check(&self, _args: &Value) -> Option<Value> {
        let canonical_count = |query: &str| -> i64 {
            self.graph_store
                .execute_raw_sql_gateway(query)
                .ok()
                .as_deref()
                .and_then(parse_scalar_count_row)
                .unwrap_or(0)
        };
        let reader_count =
            |query: &str| -> i64 { self.graph_store.query_count(query).unwrap_or(0) };

        let checks = vec![
            ("File", "SELECT count(*) FROM File"),
            ("Symbol", "SELECT count(*) FROM Symbol"),
            ("CALLS", "SELECT count(*) FROM CALLS"),
            ("CALLS_NIF", "SELECT count(*) FROM CALLS_NIF"),
            ("CONTAINS", "SELECT count(*) FROM CONTAINS"),
        ];

        let mut rows = Vec::new();
        let mut drift_count = 0_i64;
        for (name, query) in checks {
            let canonical = canonical_count(query);
            let reader = reader_count(query);
            let delta = (canonical - reader).abs();
            if delta > 0 {
                drift_count += 1;
            }
            rows.push(json!([name, canonical, reader, delta]));
        }
        let table = serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string());
        let status = if drift_count == 0 {
            "aligned"
        } else {
            "drift_detected"
        };
        let report = format!(
            "## 🧪 Truth Contract Check\n\n\
             **Status:** {}\n\
             **Drifted counters:** {}\n\n\
             {}\n",
            status,
            drift_count,
            format_table_from_json(
                &table,
                &["Counter", "Canonical(writer)", "Reader-path", "Delta"]
            )
        );
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_cypher(&self, args: &Value) -> Option<Value> {
        let cypher = args.get("cypher")?.as_str()?;
        let q = cypher.trim();
        let ql = q.to_ascii_lowercase();

        // Minimal robust support for common multi-hop CALLS checks.
        if ql.contains("match") && ql.contains("[:calls") && ql.contains("return count(*)") {
            if ql.contains("[:calls*1..3]") {
                let sql = "WITH RECURSIVE hops(source_id, target_id, depth) AS (
                             SELECT source_id, target_id, 1 FROM CALLS
                             UNION ALL
                             SELECT h.source_id, c.target_id, h.depth + 1
                             FROM hops h
                             JOIN CALLS c ON c.source_id = h.target_id
                             WHERE h.depth < 3
                           )
                           SELECT count(*) FROM hops WHERE depth BETWEEN 1 AND 3";
                return match self.graph_store.query_json(sql) {
                    Ok(result) => Some(json!({ "content": [{ "type": "text", "text": result }] })),
                    Err(e) => Some(json!({
                        "content": [{ "type": "text", "text": format!("Cypher Translation Error: {}", e) }],
                        "isError": true
                    })),
                };
            }

            if ql.matches("[:calls]").count() >= 2 {
                let sql = "SELECT count(*) \
                           FROM CALLS c1 \
                           JOIN CALLS c2 ON c2.source_id = c1.target_id";
                return match self.graph_store.query_json(sql) {
                    Ok(result) => Some(json!({ "content": [{ "type": "text", "text": result }] })),
                    Err(e) => Some(json!({
                        "content": [{ "type": "text", "text": format!("Cypher Translation Error: {}", e) }],
                        "isError": true
                    })),
                };
            }
        }

        match self.graph_store.query_json(q) {
            Ok(result) => {
                if result.trim() == "[]" && ql.contains("match") {
                    let calls_count = self
                        .graph_store
                        .query_count("SELECT count(*) FROM CALLS")
                        .unwrap_or(0);
                    let note = format!(
                        "[]\n\nStatus: warn_empty_result\nHint: requête de style Cypher détectée. Le backend accepte d'abord SQL; pour multi-hop CALLS, utiliser `MATCH ... [:CALLS*1..3] ... RETURN count(*)` ou les exemples `query_examples`.\nCALLS_count={}",
                        calls_count
                    );
                    Some(json!({ "content": [{ "type": "text", "text": note }] }))
                } else {
                    Some(json!({ "content": [{ "type": "text", "text": result }] }))
                }
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("Cypher Error: {}", e) }], "isError": true }),
            ),
        }
    }

    pub(crate) fn axon_batch(&self, args: &Value) -> Option<Value> {
        let calls = args.get("calls")?.as_array()?;
        let mut all_results = Vec::new();

        for call in calls {
            let tool_name = call.get("tool")?.as_str()?;
            let normalized_tool_name = tool_name.strip_prefix("axon_").unwrap_or(tool_name);
            let tool_args = call.get("args")?;

            let res = match normalized_tool_name {
                "query" => self.axon_query(tool_args),
                "inspect" => self.axon_inspect(tool_args),
                "impact" => self.axon_impact(tool_args),
                _ => None,
            };

            if let Some(r) = res {
                all_results.push(json!({
                    "name": tool_name,
                    "result": r
                }));
            }
        }

        Some(
            json!({ "content": [{ "type": "text", "text": serde_json::to_string(&all_results).unwrap_or_default() }] }),
        )
    }
}
