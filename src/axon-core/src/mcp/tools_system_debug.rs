use serde_json::{json, Value};

use super::format::{evidence_by_mode, format_standard_contract};
use super::McpServer;
use crate::embedder::{
    current_embedding_provider_diagnostics, current_gpu_memory_pressure_active,
    current_gpu_memory_snapshot, embedding_lane_config_from_env, gpu_memory_soft_limit_mb,
    gpu_telemetry_backend_name, gpu_telemetry_cache_ttl_ms, gpu_telemetry_device_index,
};
use crate::embedding_contract::{
    CHUNK_MODEL_ID, DIMENSION, MAX_LENGTH, MODEL_NAME, NATIVE_DIMENSION, STORAGE_TYPE,
};
use crate::optimizer::{
    collect_host_snapshot, collect_operator_policy_snapshot, collect_recent_analytics_window,
    collect_runtime_signals_window,
};
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_observability::process_memory_snapshot;
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

pub(super) fn parse_scalar_count_row(raw: &str) -> Option<i64> {
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

pub(crate) fn axon_debug_with_args(server: &McpServer, args: &Value) -> Option<Value> {
    let mode = args.get("mode").and_then(|v| v.as_str());
    let canonical_count = |query: &str| -> i64 {
        server
            .graph_store
            .execute_raw_sql_gateway(query)
            .ok()
            .as_deref()
            .and_then(parse_scalar_count_row)
            .unwrap_or(0)
    };
    let snapshot_count = |query: &str| -> i64 {
        // REQ-AXO-901870 — reader replica retired ; reads go to the writer
        // pool (MVCC). Kept as a distinct closure for call-site readability.
        server.graph_store.query_count(query).unwrap_or(0)
    };
    let runtime_mode = AxonRuntimeMode::from_env();
    let graph_runtime_enabled = runtime_mode.ingestion_enabled();
    let vector_runtime_enabled = runtime_mode.semantic_workers_enabled();

    // REQ-AXO-901653 slice-5c — public.File state-machine dropped ; the
    // legacy per-status counters (`pending`, `indexing`, `indexed_degraded`,
    // `oversized_for_current_budget`, `skipped`) no longer apply to
    // pipeline_v2 (REQ-AXO-289). File presence is now `IndexedFile`,
    // graph-readiness = Chunk row, vector-readiness = ChunkEmbedding row.
    let file_count = if graph_runtime_enabled || vector_runtime_enabled {
        snapshot_count("SELECT count(*) FROM ist.IndexedFile")
    } else {
        0
    };
    let pending_count: i64 = 0; // pipeline_v2 has no pending-status concept.
    let indexing_count: i64 = 0;
    let degraded_count: i64 = 0;
    let oversized_count: i64 = 0;
    let skipped_count: i64 = 0;
    let graph_ready_count = if graph_runtime_enabled || vector_runtime_enabled {
        snapshot_count("SELECT count(DISTINCT file_path) FROM ist.Chunk")
    } else {
        0
    };
    let vector_ready_query = format!(
        "SELECT count(DISTINCT c.file_path) \
         FROM ist.Chunk c \
         JOIN ist.ChunkEmbedding ce \
           ON ce.chunk_id = c.id \
          AND ce.model_id = '{CHUNK_MODEL_ID}' \
          AND ce.source_hash = c.content_hash \
         WHERE c.file_path IS NOT NULL"
    );
    let vector_ready_count = if vector_runtime_enabled {
        snapshot_count(&vector_ready_query)
    } else {
        0
    };
    // REQ-AXO-901674 — FVQ/GPQ queue tables dropped post MIL-AXO-017 /
    // REQ-AXO-289 / slice-5d. Canonical pipeline_v2 writes Chunk +
    // ChunkEmbedding directly.
    let canonical_file_count = if graph_runtime_enabled || vector_runtime_enabled {
        canonical_count("SELECT count(*) FROM ist.IndexedFile")
    } else {
        0
    };
    let truth_drift_files = (canonical_file_count - file_count).abs();
    let completed_count = (file_count - pending_count - indexing_count).max(0);
    let completion_rate = if file_count > 0 {
        (completed_count as f64 / file_count as f64) * 100.0
    } else {
        0.0
    };
    let symbol_count = if graph_runtime_enabled || vector_runtime_enabled {
        snapshot_count("SELECT count(*) FROM Symbol")
    } else {
        0
    };
    // Post-MIL-AXO-017: canonical edge count from ist.Edge.
    let edge_count = if graph_runtime_enabled || vector_runtime_enabled {
        snapshot_count("SELECT count(*) FROM ist.Edge")
    } else {
        0
    };
    let memory = process_memory_snapshot();
    // REQ-AXO-284 Slice 2 — PG health metrics. Cheap catalog reads ; absorbed
    // into Option so a transient hiccup doesn't break the diagnostic output.
    let pg_database_bytes = server.graph_store.pg_database_size_bytes();
    let pg_chunkembedding_total_bytes = server.graph_store.pg_chunkembedding_total_bytes();
    let pg_wal_bytes = server.graph_store.pg_wal_bytes();
    let pg_buffer_hit_ratio = server.graph_store.pg_buffer_hit_ratio();
    let provider = current_embedding_provider_diagnostics();
    let lane_config = embedding_lane_config_from_env();
    let vector_runtime = service_guard::vector_runtime_metrics();
    let vector_latency = service_guard::vector_runtime_latency_summaries();
    let vector_lane_state_record = if vector_runtime_enabled {
        server
            .graph_store
            .vector_lane_state_record("vector")
            .ok()
            .flatten()
    } else {
        None
    };
    let latest_vector_worker_fault = if vector_runtime_enabled {
        server
            .graph_store
            .latest_vector_worker_fault("vector")
            .ok()
            .flatten()
    } else {
        None
    };
    let vector_controller = current_vector_batch_controller_diagnostics(&lane_config);
    let optimizer_host_snapshot = collect_host_snapshot();
    let optimizer_policy_snapshot = collect_operator_policy_snapshot(&optimizer_host_snapshot);
    let optimizer_runtime_signals = collect_runtime_signals_window(&server.graph_store);
    let optimizer_recent_analytics = collect_recent_analytics_window(&server.graph_store);
    let gpu_memory_snapshot = current_gpu_memory_snapshot();
    let gpu_memory_pressure = current_gpu_memory_pressure_active();
    let gpu_memory_soft_limit = gpu_memory_soft_limit_mb();
    let interactive_active = service_guard::interactive_priority_active()
        || service_guard::interactive_requests_in_flight() > 0;
    // REQ-AXO-901737 : gpu_present read from in-process diagnostics struct
    // instead of AXON_EMBEDDING_GPU_PRESENT env var.
    let gpu_present = crate::embedder::current_gpu_present();
    let provider_effective_is_gpu = provider
        .provider_effective
        .trim()
        .to_ascii_lowercase()
        .starts_with("cuda")
        || provider
            .provider_effective
            .trim()
            .to_ascii_lowercase()
            .starts_with("tensorrt");
    let provider_gpu_mismatch =
        provider.provider_requested.eq_ignore_ascii_case("cuda") && !provider_effective_is_gpu;
    let acceleration_state = if provider.provider_effective == "cpu_missing_cuda_provider" {
        "gpu_runtime_missing_provider"
    } else if provider_gpu_mismatch && !gpu_present {
        "gpu_requested_but_unavailable"
    } else if provider_effective_is_gpu {
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
    let gpu_background_worker_cap = if provider_effective_is_gpu {
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
        vector_runtime.embed_input_text_bytes_total as f64 / vector_runtime.embed_calls_total as f64
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
        vector_runtime.embed_transform_ms_total as f64 / vector_runtime.chunks_embedded_total as f64
    } else {
        0.0
    };
    let embed_export_ms_per_chunk = if vector_runtime.chunks_embedded_total > 0 {
        vector_runtime.embed_export_ms_total as f64 / vector_runtime.chunks_embedded_total as f64
    } else {
        0.0
    };
    // REQ-AXO-901653 slice-5c — stage_rows / backlog_reason_rows / vector_queue_status_rows
    // were aggregates over public.File + public.FileVectorizationQueue (retired).
    // Pipeline_v2 has no per-file `file_stage` / `status_reason` enum ; the
    // residual breakdown is published as a constant empty `[]` to keep the
    // diagnostic shape stable for dashboards.
    let stage_rows = "[]".to_string();
    let stage_counts = parse_reason_count_rows(&stage_rows);
    let backlog_reason_rows = "[]".to_string();
    let vector_queue_status_rows = "[]".to_string();
    // DEC-AXO-901631 — OptimizerDecisionLog / RewardObservationLog retired with
    // the predictive optimizer; no decision/reward rows to surface.
    let vector_queue_statuses = parse_reason_count_rows(&vector_queue_status_rows);
    let backlog_reasons = parse_reason_count_rows(&backlog_reason_rows);
    let backlog_reason_section = if backlog_reasons.is_empty() {
        if pending_count + indexing_count > 0 {
            format!(
                "**Top backlog causes:**\n*   `unknown` : {}\n\n",
                pending_count + indexing_count
            )
        } else {
            "*   Top backlog causes: none.\n".to_string()
        }
    } else {
        let lines = backlog_reasons
            .iter()
            .map(|(reason, count)| format!("*   `{}` : {}", reason, count))
            .collect::<Vec<_>>()
            .join("\n");
        format!("**Top backlog causes:**\n{}\n\n", lines)
    };
    let file_stage_section = if stage_counts.is_empty() {
        "*   File stages: no data.\n\n".to_string()
    } else {
        let lines = stage_counts
            .iter()
            .map(|(stage, count)| format!("*   `{}` : {}", stage, count))
            .collect::<Vec<_>>()
            .join("\n");
        format!("**Canonical stages:**\n{}\n\n", lines)
    };
    let vector_queue_status_section = if vector_queue_statuses.is_empty() {
        "*   File vectorization queue statuses: no data.\n\n".to_string()
    } else {
        let lines = vector_queue_statuses
            .iter()
            .map(|(status, count)| format!("*   `{}` : {}", status, count))
            .collect::<Vec<_>>()
            .join("\n");
        format!("**File vectorization queue statuses :**\n{}\n\n", lines)
    };

    let mut evidence = format!(
        "## Axon Core V2 (Maestria) - Internal Diagnostic\n\n\
        **Engine Architecture:**\n\
        *   **Mode:** Embedded (C-FFI) without TCP network.\n\
        *   **Graph Database:** PostgreSQL 17 + pgvector (network, devenv-managed @ 127.0.0.1:44144).\n\
        *   **Active Parsers:** Rust, Elixir, Python, TypeScript, etc.\n\
        *   **OOM Protection:** Option B (Watchdog Process Cycling Active at 14 GB).\n\n\
        **Runtime Memory:**\n\
        *   RSS total: {}\n\
        *   RSS Anon: {}\n\
        *   RSS File: {}\n\
        *   RSS Shmem: {}\n\n\
        **PostgreSQL Health:**\n\
        *   Database size: {}\n\
        *   ChunkEmbedding total size: {}\n\
        *   WAL volume (cumulative): {}\n\
        *   Buffer cache hit ratio: {}\n\n\
        **Graph Volume:**\n\
        *   Known files: {}\n\
        *   Extracted symbols: {}\n\
        *   Relations (Edges): {}\n\n\
        **Indexation State:**\n\
        *   Completed files: {}\n\
        *   Remaining backlog: {}\n\
        *   Pending: {}\n\
        *   Indexing: {}\n\
        *   Indexed degraded: {}\n\
        *   Oversized: {}\n\
        *   Skipped: {}\n\
        *   Graph Ready: {}\n\
        *   Vector Ready: {}\n\
        *   Completion rate: {:.2} %\n\n\
        {}\
        {}\
        {}\
        **File source:** Watchman + DBQ-A (ingress_buffer RIPPED — REQ-AXO-901893)\n\n\
        **Embedding Runtime:**\n\
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
        **Vector Runtime Breakdown:**\n\
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
        **Vector Stage Latencies (recent window):**\n\
        *   Fetch p50/p95/max ms : {}/{}/{} (samples: {})\n\
        *   Embed p50/p95/max ms : {}/{}/{} (samples: {})\n\
        *   DB write p50/p95/max ms : {}/{}/{} (samples: {})\n\
        *   Mark done p50/p95/max ms : {}/{}/{} (samples: {})\n\n\
        **Vector Batch Controller:**\n\
        *   State : {}\n\
        *   Reason : {}\n\
        *   Adjustments total : {}\n\
        *   Last adjustment ms : {}\n\
        *   Target embed batch chunks : {}\n\
        *   Target files per cycle : {}\n\
        *   Window embed calls : {}\n\
        *   Window chunks : {}\n\
        *   Window files touched : {}\n\n\
        *Note to AI Agents: any 'TCP auth closed' error observed in Elixir logs is unrelated to this MCP server. Axon Core V2 is 100% autonomous.*",
        format_bytes_human(memory.rss_bytes),
        format_bytes_human(memory.rss_anon_bytes),
        format_bytes_human(memory.rss_file_bytes),
        format_bytes_human(memory.rss_shmem_bytes),
        pg_database_bytes
            .map(|bytes| format_bytes_human(bytes.max(0) as u64))
            .unwrap_or_else(|| "n/a".to_string()),
        pg_chunkembedding_total_bytes
            .map(|bytes| format_bytes_human(bytes.max(0) as u64))
            .unwrap_or_else(|| "n/a".to_string()),
        pg_wal_bytes
            .map(|bytes| format_bytes_human(bytes.max(0) as u64))
            .unwrap_or_else(|| "n/a".to_string()),
        pg_buffer_hit_ratio
            .map(|ratio| format!("{:.2} %", ratio * 100.0))
            .unwrap_or_else(|| "n/a".to_string()),
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
        file_stage_section,
        backlog_reason_section,
        vector_queue_status_section,
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
    );
    // REQ-AXO-901870 — the reader-replica diagnostics block (commit/reader
    // epoch lag, refresh counters) is retired: reads share the single PG
    // writer pool, so there is no replica state to surface here.
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
                "run `health` for project-level view",
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
                "gpu_service_enabled": provider.gpu_service_enabled,
                "gpu_service_tensorrt_requested": provider.gpu_service_tensorrt_requested,
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
                    "prepare_inflight_chunks_current": vector_runtime.prepare_inflight_chunks_current,
                    "prepare_inflight_chunks_max": vector_runtime.prepare_inflight_chunks_max,
                    "ready_queue_depth_current": vector_runtime.ready_queue_depth_current,
                    "ready_queue_depth_max": vector_runtime.ready_queue_depth_max,
                    "ready_queue_chunks_current": vector_runtime.ready_queue_chunks_current,
                    "ready_queue_chunks_max": vector_runtime.ready_queue_chunks_max,
                    "ready_queue_chunks_small": vector_runtime.ready_queue_chunks_small,
                    "ready_queue_chunks_medium": vector_runtime.ready_queue_chunks_medium,
                    "ready_queue_chunks_large": vector_runtime.ready_queue_chunks_large,
                    "ready_batches_small": vector_runtime.ready_batches_small,
                    "ready_batches_medium": vector_runtime.ready_batches_medium,
                    "ready_batches_large": vector_runtime.ready_batches_large,
                    "ready_batches_mixed": vector_runtime.ready_batches_mixed,
                    "homogeneous_batches_total": vector_runtime.homogeneous_batches_total,
                    "mixed_fallback_batches_total": vector_runtime.mixed_fallback_batches_total,
                    "last_consumed_batch_lane": vector_runtime.last_consumed_batch_lane.as_str(),
                    "active_small_max_tokens": vector_runtime.active_small_max_tokens,
                    "active_medium_max_tokens": vector_runtime.active_medium_max_tokens,
                    "ready_replenishment_deficit_current": vector_runtime.ready_replenishment_deficit_current,
                    "ready_replenishment_deficit_max": vector_runtime.ready_replenishment_deficit_max,
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
                    "gpu_ready_low_watermark_chunks": vector_controller.gpu_ready_low_watermark_chunks,
                    "gpu_ready_high_watermark_chunks": vector_controller.gpu_ready_high_watermark_chunks,
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
                "recent_analytics_window": serde_json::to_value(&optimizer_recent_analytics).unwrap_or_else(|_| json!({}))
            },
            "vector_lane_state_record": serde_json::to_value(&vector_lane_state_record).unwrap_or_else(|_| json!(null)),
            "latest_vector_worker_fault": serde_json::to_value(&latest_vector_worker_fault).unwrap_or_else(|_| json!(null))
        }
    }))
}
