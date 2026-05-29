use serde_json::{json, Value};

use super::format::{format_standard_contract, format_table_from_json};
use super::tools_system_debug;
use super::McpServer;
use crate::graph_query::ReadFreshness;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_topology::{current_runtime_process_role, AxonProcessRole};

// ── Filesystem counters (cached 60 s) ──────────────────────────
// Scanning the watch root takes ~1 s; cache the result for 60 s to
// keep `embedding_status` cheap on repeated calls.
const FS_COUNTER_CACHE_TTL_SECS: u64 = 60;

struct FsCounterSnapshot {
    disk_files: i64,
    eligible_files: i64,
    computed_at: std::time::Instant,
}

static FS_COUNTER_CACHE: std::sync::Mutex<Option<FsCounterSnapshot>> =
    std::sync::Mutex::new(None);

/// Returns `(disk_files, eligible_files)`.
/// `disk_files`  = total regular files under `AXON_WATCH_DIR`.
/// `eligible_files` = subset that passes the Scanner filter stack
///   (.gitignore, .axonignore, supported extensions, etc.).
/// Returns `(-1, -1)` when `AXON_WATCH_DIR` is not set.
///
/// REQ-AXO-901806 — exposed `pub(crate)` so `dashboard_state.rs` can
/// reuse the same TTL-cached snapshot in the 1 Hz event composition.
pub(crate) fn cached_fs_counters() -> (i64, i64) {
    let watch_root = match std::env::var("AXON_WATCH_DIR") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return (-1, -1),
    };
    {
        if let Ok(guard) = FS_COUNTER_CACHE.lock() {
            if let Some(ref snap) = *guard {
                if snap.computed_at.elapsed().as_secs() < FS_COUNTER_CACHE_TTL_SECS {
                    return (snap.disk_files, snap.eligible_files);
                }
            }
        }
    }
    let scanner = crate::scanner::Scanner::new(&watch_root, "");
    let walker = ignore::WalkBuilder::new(&watch_root)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .build();
    let mut disk: i64 = 0;
    let mut eligible: i64 = 0;
    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }
        disk += 1;
        if scanner.should_process_path(entry.path()) {
            eligible += 1;
        }
    }
    if let Ok(mut guard) = FS_COUNTER_CACHE.lock() {
        *guard = Some(FsCounterSnapshot {
            disk_files: disk,
            eligible_files: eligible,
            computed_at: std::time::Instant::now(),
        });
    }
    (disk, eligible)
}

impl McpServer {
    pub(crate) fn axon_resume_vectorization(&self, _args: &Value) -> Option<Value> {
        let runtime_mode = AxonRuntimeMode::from_env();
        if matches!(runtime_mode, AxonRuntimeMode::BrainOnly)
            || matches!(current_runtime_process_role(), AxonProcessRole::Brain)
        {
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": "resume_vectorization is unavailable on axon-brain. axon-indexer is autonomous and drains its own pipeline before going idle."
                }],
                "isError": true
            }));
        }
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
                        "Semantic workers are disabled in the current runtime mode; processing remains deferred until an `indexer_full` or `indexer_vector` restart.\n",
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
                            "restart in `indexer_full` or `indexer_vector` mode to let semantic workers consume the queue",
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

    #[allow(dead_code)]
    pub(crate) fn axon_debug(&self) -> Option<Value> {
        self.axon_debug_with_args(&json!({}))
    }

    pub(crate) fn axon_debug_with_args(&self, args: &Value) -> Option<Value> {
        tools_system_debug::axon_debug_with_args(self, args)
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

    pub(crate) fn axon_query_examples(&self, _args: &Value) -> Option<Value> {
        // REQ-AXO-901653 slice-5d — examples migrated from public.File to
        // pipeline_v2 canonical (IndexedFile + Chunk + ChunkEmbedding).
        let examples = r#"## 📚 Query Examples (SQL gateway / cypher tool)

1) Workspace size (canonical pipeline_v2)
`SELECT count(*) AS indexed_files FROM public.IndexedFile;`

2) Project health (Chunk = canonical per-file per-project pivot)
`SELECT project_code, count(DISTINCT file_path) AS files, count(*) AS chunks FROM public.Chunk GROUP BY project_code ORDER BY chunks DESC;`

3) Vector embedding coverage
`SELECT c.project_code, count(DISTINCT c.file_path) AS files_with_embeddings FROM public.Chunk c JOIN public.ChunkEmbedding e ON e.chunk_id = c.id GROUP BY c.project_code ORDER BY 2 DESC;`

4) Per-file chunk distribution
`SELECT file_path, count(*) AS chunks FROM public.Chunk GROUP BY file_path ORDER BY chunks DESC LIMIT 20;`

5) Inter-language bridge visibility (Edge canonical)
`SELECT relation_type, count(*) FROM public.Edge GROUP BY relation_type ORDER BY 2 DESC;`

6) Symbol lookup by project
`SELECT id, name, kind FROM public.Symbol WHERE project_code = 'AXO' ORDER BY name LIMIT 50;`
"#;
        Some(json!({ "content": [{ "type": "text", "text": examples }] }))
    }

    pub(crate) fn axon_truth_check(&self, _args: &Value) -> Option<Value> {
        let canonical_count = |query: &str| -> i64 {
            self.graph_store
                .execute_raw_sql_gateway(query)
                .ok()
                .as_deref()
                .and_then(tools_system_debug::parse_scalar_count_row)
                .unwrap_or(0)
        };
        let reader_count =
            |query: &str| -> i64 { self.graph_store.query_count(query).unwrap_or(0) };

        // Canonical IST tables (post-MIL-AXO-017 migration).
        let checks: Vec<(&str, &str)> = vec![
            ("IndexedFile", "SELECT count(*) FROM public.IndexedFile"),
            ("Symbol", "SELECT count(*) FROM public.Symbol"),
            ("Edge", "SELECT count(*) FROM public.Edge"),
            ("Chunk", "SELECT count(*) FROM public.Chunk"),
            ("ChunkEmbedding", "SELECT count(*) FROM public.ChunkEmbedding"),
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
        // REQ-AXO-91523 (MIL-AXO-019 Tier A) — tri-modal envelope.
        // `truth_check` compares writer-side vs reader-side counters
        // for the canonical IST tables ; surface stays on
        // `graph_pg_writer` + `graph_pg_reader` (publication freshness
        // contract — CPT-AXO-029). Adding RAM cross-checks against
        // `IstSnapshotCache::approximate_bytes` is a follow-up slice.
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": status,
                "drift_count": drift_count,
                "checks": rows,
                "surfaces_used": ["graph_pg_writer", "graph_pg_reader"],
                "total_available": drift_count,
                "next_call_hint": if drift_count > 0 {
                    "diagnose_indexing for replica freshness investigation"
                } else {
                    "status mode=verbose to confirm IST projection freshness"
                }
            }
        }))
    }

    /// DEC-AXO-086 slice 2 — operator health snapshot (renamed conceptually
    /// from "embedding status" to a full storage + pipeline overview;
    /// catalog name kept for backward compat).
    ///
    /// Surfaces row counts for the canonical IST tables (Symbol / Chunk /
    /// ChunkEmbedding / Edge / IndexedFile / Project), embedding coverage,
    /// and the pipeline A + B worker / batch parameters as resolved from
    /// env vars at request time (matches what the responding process sees;
    /// indexer-side overrides may differ if the brain runs separately).
    ///
    /// `project` arg optional: when set, scopes the counts to that
    /// `project_code`; `*` (default) is global.
    pub(crate) fn axon_embedding_status(&self, args: &Value) -> Option<Value> {
        let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let where_project = if project == "*" {
            String::new()
        } else {
            let safe = project.replace('\'', "''");
            format!(" WHERE project_code = '{}'", safe)
        };

        let scalar = |query: &str| -> i64 {
            self.graph_store
                .execute_raw_sql_gateway(query)
                .ok()
                .as_deref()
                .and_then(tools_system_debug::parse_scalar_count_row)
                .unwrap_or(0)
        };

        let total_chunks = scalar(&format!("SELECT count(*) FROM public.Chunk{}", where_project));
        let embedded_chunks = scalar(&format!(
            "SELECT count(*) FROM public.ChunkEmbedding{}",
            where_project
        ));
        let symbols = scalar(&format!(
            "SELECT count(*) FROM public.Symbol{}",
            where_project
        ));
        let indexed_files = scalar(&format!(
            "SELECT count(*) FROM public.IndexedFile{}",
            where_project
        ));
        // Edge + Project tables don't carry project_code → always global.
        let edges = scalar("SELECT count(*) FROM public.Edge");
        let projects = scalar("SELECT count(*) FROM public.Project");
        let pending_chunks = (total_chunks - embedded_chunks).max(0);
        let coverage_pct = if total_chunks > 0 {
            (embedded_chunks as f64 / total_chunks as f64) * 100.0
        } else {
            0.0
        };

        // ── Filesystem counters (cached 60s) ──────────────────────
        let (disk_files, eligible_files) = cached_fs_counters();

        // ── Per-project breakdown ─────────────────────────────────
        // Only computed for global view (project == "*"); individual
        // project queries already scope the main counts above.
        let per_project_breakdown: Value = if project == "*" {
            let breakdown_sql = "\
                SELECT c.project_code, \
                       count(*) AS chunks, \
                       (SELECT count(*) FROM public.ChunkEmbedding ce WHERE ce.chunk_id IN (SELECT id FROM public.Chunk c2 WHERE c2.project_code = c.project_code)) AS embeddings, \
                       (SELECT count(*) FROM public.IndexedFile f WHERE f.project_code = c.project_code) AS indexed_files \
                FROM public.Chunk c \
                GROUP BY c.project_code \
                ORDER BY chunks DESC";
            match self.graph_store.execute_raw_sql_gateway(breakdown_sql) {
                Ok(raw) => {
                    // Result is [[project_code, chunks, embeddings, indexed_files], ...]
                    if let Ok(rows) = serde_json::from_str::<Vec<Vec<Value>>>(&raw) {
                        let arr: Vec<Value> = rows
                            .iter()
                            .filter_map(|row| {
                                let code = row.first()?.as_str()?;
                                let ch = row.get(1).and_then(|v| v.as_i64()).unwrap_or(0);
                                let emb = row.get(2).and_then(|v| v.as_i64()).unwrap_or(0);
                                let idx = row.get(3).and_then(|v| v.as_i64()).unwrap_or(0);
                                let cov = if ch > 0 {
                                    (emb as f64 / ch as f64) * 100.0
                                } else {
                                    0.0
                                };
                                Some(json!({
                                    "project_code": code,
                                    "indexed_files": idx,
                                    "chunks": ch,
                                    "embeddings": emb,
                                    "coverage_pct": (cov * 100.0).round() / 100.0,
                                }))
                            })
                            .collect();
                        json!(arr)
                    } else {
                        json!([])
                    }
                }
                Err(_) => json!([]),
            }
        } else {
            json!([])
        };

        // Pipeline params — read env (best-effort, reflects responder).
        let env_usize = |key: &str, default: usize| -> usize {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(default)
        };
        let env_u64 = |key: &str, default: u64| -> u64 {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(default)
        };
        let a1 = env_usize("AXON_A1_WORKERS", 4);
        let a2 = env_usize("AXON_A2_WORKERS", 8);
        let a3 = env_usize("AXON_A3_WORKERS", 2);
        let a3_batch = env_usize("AXON_A3_BATCH_SIZE", 32);
        let a3_timeout = env_u64("AXON_A3_BATCH_TIMEOUT_MS", 10);
        let b1 = env_usize("AXON_B1_WORKERS", 4);
        let b2 = env_usize("AXON_B2_WORKERS", 1);
        let b3 = env_usize("AXON_B3_WORKERS", 2);
        let b2_batch = env_usize(
            "AXON_B2_BATCH_SIZE",
            crate::pipeline_v2::channels::B2_BATCH_SIZE_DEFAULT,
        );
        let b2_timeout = env_u64(
            "AXON_B2_BATCH_TIMEOUT_MS",
            crate::pipeline_v2::channels::B2_BATCH_TIMEOUT_MS_DEFAULT,
        );
        let b3_batch = env_usize(
            "AXON_B3_BATCH_SIZE",
            crate::pipeline_v2::channels::B3_BATCH_SIZE_DEFAULT,
        );
        let b3_timeout = env_u64(
            "AXON_B3_BATCH_TIMEOUT_MS",
            crate::pipeline_v2::channels::B3_BATCH_TIMEOUT_MS_DEFAULT,
        );
        let coldstart_batch = env_usize(
            "AXON_B1_COLDSTART_BATCH_SIZE",
            crate::pipeline_v2::channels::B1_COLDSTART_BATCH_SIZE_DEFAULT,
        );
        // REQ-AXO-901678 — surface drain saturation knobs + counters so
        // the operator can spot A1 back-pressure without trawling
        // journalctl. Defaults mirror `PipelineChannelCaps` so an
        // unconfigured env still reports the canonical 512 / 30 s.
        let ingress_drain_batch = env_usize(
            "AXON_INGRESS_DRAIN_BATCH",
            crate::pipeline_v2::channels::INGRESS_DRAIN_BATCH_DEFAULT,
        );
        let coldstart_poll_interval_secs = env_u64(
            "AXON_B1_COLDSTART_POLL_INTERVAL_SECS",
            crate::pipeline_v2::channels::B1_COLDSTART_POLL_INTERVAL_SECS_DEFAULT,
        );
        let drain_snapshot = crate::ingress_buffer::ingress_metrics_snapshot();
        // REQ-AXO-901677 — periodic_sweep_worker telemetry. Surface the
        // configured cadence + CPU gate alongside the live counters so
        // the operator can spot a worker that's never running (chronic
        // CPU skip), never enabled (`hours=0`), or whose last sweep is
        // ancient (`last_run_at_ms` very old). All defaults mirror the
        // worker constants so a fresh process reads canonical values.
        let periodic_sweep_hours = std::env::var("AXON_PERIODIC_SWEEP_HOURS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(crate::pipeline_v2_runtime::PERIODIC_SWEEP_HOURS_DEFAULT);
        let periodic_sweep_cpu_threshold_pct = std::env::var("AXON_PERIODIC_SWEEP_CPU_THRESHOLD_PCT")
            .ok()
            .and_then(|raw| raw.trim().parse::<u8>().ok())
            .map(|v| v.min(100))
            .unwrap_or(crate::pipeline_v2_runtime::PERIODIC_SWEEP_CPU_THRESHOLD_PCT_DEFAULT);
        let periodic_sweep_snapshot = crate::ingress_buffer::periodic_sweep_metrics_snapshot();
        // REQ-AXO-901657 slice 4 cluster B : canonical name is
        // `AXON_PIPELINE_A3_TO_B1_BUFFER_CAP` (matches the read site in
        // `pipeline_v2::channels::CapsCfg::from_env`). The legacy
        // `AXON_A3_TO_B1_BUFFER` is honored with a one-shot deprecation
        // warning so the status snapshot stays consistent with what the
        // pipeline actually reads.
        let a3_to_b1_cap = crate::env_alias::read_with_alias(
            "AXON_PIPELINE_A3_TO_B1_BUFFER_CAP",
            "AXON_A3_TO_B1_BUFFER",
        )
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(crate::pipeline_v2::channels::A3_TO_B1_BUFFER_CAP_DEFAULT);

        // REQ-AXO-90009 Slice 2 — in-memory pending set heartbeat.
        // `runtime_pending` reflects what THIS process's
        // `EmbedderRuntimeState` is tracking ; `pending_chunks` above
        // is the DB-derived ground truth. The two should converge
        // within `reconcile_interval` ; a wide divergence flags a
        // NOTIFY listener drop or a missed mark_embedded.
        let runtime_pending = crate::embedder::lifecycle::process_state().pending_count();
        let runtime_pending_empty = runtime_pending == 0;

        // REQ-AXO-901816 (MIL-AXO-029 slice 6 P0) — pipeline A
        // discovered-backlog count + demand-pull feeder counters.
        // `stock_a` is NEW info (not derivable from existing fields).
        // Pipeline B backlog is already surfaced as `pending_chunks`
        // (total_chunks - embedded_chunks above) so re-exposing it
        // here would violate GUI-PRO-013 (DRY). The feeder counters
        // (replenish_a / replenish_b) come from the in-process
        // demand_pull metrics, which are independent of the DB-derived
        // backlog and surface the failure mode where a non-zero stock
        // sits behind a dead feeder loop.
        let stock_a = scalar(&format!(
            "SELECT count(*) FROM public.indexedfile WHERE status='discovered' AND retry_count<3{}",
            if project == "*" {
                String::new()
            } else {
                format!(" AND path LIKE '{}/%'", project.replace('\'', "''"))
            }
        ));
        let (replenish_a, replenish_b) = {
            let snap_a = crate::pipeline_v2_runtime::demand_pull_metrics_a()
                .map(|m| m.snapshot());
            let snap_b = crate::pipeline_v2_runtime::demand_pull_metrics_b()
                .map(|m| m.snapshot());
            let to_json = |snap: Option<crate::pipeline_v2::demand_pull::DemandPullSnapshot>| {
                match snap {
                    Some(s) => json!({
                        "pulls_total": s.pulls_total,
                        "items_fed_total": s.items_fed_total,
                        "empty_pulls_total": s.empty_pulls_total,
                        "try_send_failures_total": s.try_send_failures_total,
                        "skipped_above_threshold": s.skipped_above_threshold,
                    }),
                    None => json!(null),
                }
            };
            (to_json(snap_a), to_json(snap_b))
        };

        // REQ-AXO-90009 Slice 3A — lifecycle phase telemetry. Surfaces
        // the sleep/wake state machine so operators see when the GPU
        // session is parked vs ready, and how often it has flipped.
        // REQ-AXO-91572 option B : when running as the brain (MCP
        // server, no embedder), the local singleton is fresh-from-boot
        // and uninformative. Try the cross-process heartbeat table
        // first — the indexer UPSERTs its real state every 5 s. Stale
        // rows (> 30 s) fall back to the local singleton.
        const HEARTBEAT_FRESHNESS_MS: i64 = 30_000;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0);
        let indexer_heartbeat = self
            .graph_store
            .latest_lifecycle_heartbeat("indexer")
            .ok()
            .flatten()
            .filter(|row| (now_ms - row.heartbeat_ms).max(0) <= HEARTBEAT_FRESHNESS_MS);
        let lifecycle_source = if indexer_heartbeat.is_some() {
            "indexer_heartbeat"
        } else {
            "brain_local_singleton"
        };
        let local_lifecycle = crate::embedder::lifecycle_machine::process_lifecycle();
        let (lifecycle_phase, lifecycle_last_used_ms, lifecycle_wake_count, lifecycle_sleep_count) =
            match indexer_heartbeat.as_ref() {
                Some(row) => (
                    row.phase.as_str(),
                    row.last_used_ms,
                    row.wake_count,
                    row.sleep_count,
                ),
                None => (
                    local_lifecycle.phase().as_str(),
                    local_lifecycle.last_used_ms(),
                    local_lifecycle.wake_count(),
                    local_lifecycle.sleep_count(),
                ),
            };
        let lifecycle_heartbeat_age_ms = indexer_heartbeat
            .as_ref()
            .map(|row| (now_ms - row.heartbeat_ms).max(0));
        let heartbeat_age_suffix = lifecycle_heartbeat_age_ms
            .map(|age| format!(", heartbeat_age_ms={age}"))
            .unwrap_or_default();
        // ── Per-project breakdown text ──────────────────────────
        let breakdown_text = if project == "*" {
            if let Some(arr) = per_project_breakdown.as_array() {
                if arr.is_empty() {
                    String::new()
                } else {
                    let mut lines = String::from(
                        "\n### Per-project breakdown\n\
                         | Project      | IndexedFiles | Chunks       | Embeddings   | Coverage   |\n\
                         |--------------|--------------|--------------|--------------|------------|\n",
                    );
                    for entry in arr {
                        let code = entry.get("project_code").and_then(|v| v.as_str()).unwrap_or("?");
                        let idx = entry.get("indexed_files").and_then(|v| v.as_i64()).unwrap_or(0);
                        let ch = entry.get("chunks").and_then(|v| v.as_i64()).unwrap_or(0);
                        let emb = entry.get("embeddings").and_then(|v| v.as_i64()).unwrap_or(0);
                        let cov = entry.get("coverage_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        lines.push_str(&format!(
                            "| {code:<12} | {idx:>12} | {ch:>12} | {emb:>12} | {cov:>9.2}% |\n"
                        ));
                    }
                    lines
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let disk_files_str = if disk_files >= 0 {
            format!("{disk_files}")
        } else {
            "n/a (no AXON_WATCH_DIR)".to_string()
        };
        let eligible_files_str = if eligible_files >= 0 {
            format!("{eligible_files}")
        } else {
            "n/a".to_string()
        };

        let report = format!(
            "## Axon Status (project={project})\n\n\
             ### Filesystem (cached {FS_COUNTER_CACHE_TTL_SECS}s)\n\
             - Disk files (total):    {disk_files_str}\n\
             - Eligible files:        {eligible_files_str}\n\n\
             ### Storage\n\
             | Entity         | Count        |\n\
             |----------------|--------------|\n\
             | Symbol         | {symbols:>12} |\n\
             | Chunk          | {total_chunks:>12} |\n\
             | ChunkEmbedding | {embedded_chunks:>12} |\n\
             | Edge           | {edges:>12} |\n\
             | IndexedFile    | {indexed_files:>12} |\n\
             | Project        | {projects:>12} |\n\n\
             **Embedding coverage** : {embedded_chunks} / {total_chunks} = {coverage_pct:.2}%  (pending = {pending_chunks})\n\
             **Runtime pending set** : {runtime_pending} (in-memory ; syncé via NOTIFY + reconcile)\n\
             {breakdown_text}\n\
             ### Pipeline A — CPU (graph + chunks + FTS)\n\
             - Workers:           a1={a1}  a2={a2}  a3={a3}\n\
             - A3 batch:          {a3_batch} chunks, timeout {a3_timeout} ms\n\n\
             ### Pipeline B — GPU embedding\n\
             - Workers:           b1={b1}  b2={b2}  b3={b3}\n\
             - B2 batch:          {b2_batch} chunks, timeout {b2_timeout} ms\n\
             - B3 batch:          {b3_batch} chunks, timeout {b3_timeout} ms\n\
             - A3→B1 try_send:    cap {a3_to_b1_cap} (drops rattrapés par cold-start poll)\n\
             - NOTIFY channel:    chunk_pending_embed\n\
             - Cold-start poll:   every {coldstart_poll_interval_secs} s, batch {coldstart_batch}\n\
             - Runtime idle (pending=0): {runtime_pending_empty}\n\
             - Lifecycle phase: {lifecycle_phase}  (wake_count={lifecycle_wake_count}, sleep_count={lifecycle_sleep_count}, source={lifecycle_source}{heartbeat_age_suffix})\n\n\
             ### Pipeline drain (ingress → A1)\n\
             - Drain batch cap:      {ingress_drain_batch} (env AXON_INGRESS_DRAIN_BATCH)\n\
             - Heartbeat tick:       {drain_heartbeat_tick}\n\
             - Last batch sent:      {drain_last_batch_sent}\n\
             - Last batch dropped (A1 full): {drain_last_batch_dropped_full}\n\
             - Cumulative dropped (A1 full): {drain_dropped_full_total}\n\n\
             ### Periodic sweep (REQ-AXO-901677)\n\
             - Interval:             {periodic_sweep_hours} h (env AXON_PERIODIC_SWEEP_HOURS, 0=off)\n\
             - CPU skip threshold:   {periodic_sweep_cpu_threshold_pct}% (env AXON_PERIODIC_SWEEP_CPU_THRESHOLD_PCT)\n\
             - Last run at (ms):     {periodic_sweep_last_run_at_ms}\n\
             - Last duration (ms):   {periodic_sweep_last_duration_ms}\n\
             - Last files compared:  {periodic_sweep_last_files_compared}\n\
             - Last deltas found:    {periodic_sweep_last_deltas_found}\n\
             - Total runs:           {periodic_sweep_runs_total}\n\
             - Total deltas enqueued: {periodic_sweep_deltas_total}\n\
             - Skipped (high CPU):   {periodic_sweep_skipped_high_cpu_total}\n\n\
             Sustained backlog > 0 with NOTIFY listener up = indexer disconnected or B2 starved; run `diagnose_indexing` for triage. Worker counts shown are env-resolved by the responding process (brain or indexer).",
            drain_heartbeat_tick = drain_snapshot.drain_heartbeat_tick,
            drain_last_batch_sent = drain_snapshot.drain_last_batch_sent,
            drain_last_batch_dropped_full = drain_snapshot.drain_last_batch_dropped_full,
            drain_dropped_full_total = drain_snapshot.drain_dropped_full_total,
            periodic_sweep_last_run_at_ms = periodic_sweep_snapshot.last_run_at_ms,
            periodic_sweep_last_duration_ms = periodic_sweep_snapshot.last_duration_ms,
            periodic_sweep_last_files_compared = periodic_sweep_snapshot.last_files_compared,
            periodic_sweep_last_deltas_found = periodic_sweep_snapshot.last_deltas_found,
            periodic_sweep_runs_total = periodic_sweep_snapshot.runs_total,
            periodic_sweep_deltas_total = periodic_sweep_snapshot.deltas_total,
            periodic_sweep_skipped_high_cpu_total = periodic_sweep_snapshot.skipped_high_cpu_total,
        );

        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "structuredContent": {
                "project": project,
                "disk_files": disk_files,
                "eligible_files": eligible_files,
                "symbols": symbols,
                "total_chunks": total_chunks,
                "embedded_chunks": embedded_chunks,
                "pending_chunks": pending_chunks,
                "coverage_pct": coverage_pct,
                "edges": edges,
                "indexed_files": indexed_files,
                "projects": projects,
                "per_project": per_project_breakdown,
                "pipeline_a": {
                    "a1": a1,
                    "a2": a2,
                    "a3": a3,
                    "a3_batch_size": a3_batch,
                    "a3_batch_timeout_ms": a3_timeout,
                    // REQ-AXO-901816 slice 6 — discovered backlog + feeder counters.
                    "stock_discovered": stock_a,
                    "replenish": replenish_a
                },
                "pipeline_b": {
                    "b1": b1,
                    "b2": b2,
                    "b3": b3,
                    "b2_batch_size": b2_batch,
                    "b2_batch_timeout_ms": b2_timeout,
                    "b3_batch_size": b3_batch,
                    "b3_batch_timeout_ms": b3_timeout,
                    "a3_to_b1_buffer_cap": a3_to_b1_cap,
                    "coldstart_batch_size": coldstart_batch,
                    // REQ-AXO-901816 slice 6 — feeder counters only ; B backlog
                    // is already exposed as the top-level `pending_chunks` field.
                    "replenish": replenish_b
                },
                "notify_channel": "chunk_pending_embed",
                "coldstart_poll_interval_secs": coldstart_poll_interval_secs,
                "runtime_pending_count": runtime_pending,
                "runtime_idle": runtime_pending_empty,
                "lifecycle_phase": lifecycle_phase,
                "lifecycle_last_used_ms": lifecycle_last_used_ms,
                "lifecycle_wake_count": lifecycle_wake_count,
                "lifecycle_sleep_count": lifecycle_sleep_count,
                "lifecycle_source": lifecycle_source,
                "lifecycle_heartbeat_age_ms": lifecycle_heartbeat_age_ms,
                // REQ-AXO-901678 — drain saturation telemetry surface.
                // `batch_size` and `heartbeat_tick` reflect what the
                // runtime drain loop ran with on its last tick (0 if the
                // pipeline-v2 runtime has not started yet — e.g. brain
                // process answering on its own).
                "pipeline_drain": {
                    "batch_size": drain_snapshot.drain_batch_size,
                    "heartbeat_tick": drain_snapshot.drain_heartbeat_tick,
                    "last_batch_sent": drain_snapshot.drain_last_batch_sent,
                    "last_batch_dropped_full": drain_snapshot.drain_last_batch_dropped_full,
                    "dropped_full_total": drain_snapshot.drain_dropped_full_total,
                    "configured_batch_cap": ingress_drain_batch,
                },
                // REQ-AXO-901677 — periodic_sweep_worker telemetry.
                // Inotify-drop reconciliation safety net. All counters
                // are 0 in `brain_only` mode (worker is only spawned in
                // ingestion-enabled runtimes) or before the first
                // scheduled tick has fired (default cadence = 4 h).
                "periodic_sweep": {
                    "configured_interval_hours": periodic_sweep_hours,
                    "cpu_threshold_pct": periodic_sweep_cpu_threshold_pct,
                    "last_run_at_ms": periodic_sweep_snapshot.last_run_at_ms,
                    "last_duration_ms": periodic_sweep_snapshot.last_duration_ms,
                    "last_files_compared": periodic_sweep_snapshot.last_files_compared,
                    "last_deltas_found": periodic_sweep_snapshot.last_deltas_found,
                    "runs_total": periodic_sweep_snapshot.runs_total,
                    "deltas_total": periodic_sweep_snapshot.deltas_total,
                    "skipped_high_cpu_total": periodic_sweep_snapshot.skipped_high_cpu_total,
                },
            }
        }))
    }

    pub(crate) fn axon_sql(&self, args: &Value) -> Option<Value> {
        let sql = args.get("sql")?.as_str()?;
        let q = sql.trim();
        let ql = q.to_ascii_lowercase();

        // REQ-AXO-271 slice 2d invariant : `skip_legacy_relations` is
        // always true under PG canonical (the SQL relation tables
        // CALLS / CALLS_NIF are dropped — `public.Edge` + the
        // `WITH RECURSIVE` SQL graph functions handle traversal).
        // REQ-AXO-91501 vague 1d : the legacy `WITH RECURSIVE hops`
        // translation layer for `MATCH [:CALLS*1..3]` Cypher-style
        // queries is dead code under this invariant ; dropped. The
        // raw `query_json` path below handles every consumer.

        match self.graph_store.query_json(q) {
            Ok(result) => {
                if result.trim() == "[]" && ql.contains("match") {
                    let note =
                        "[]\n\nStatus: warn_empty_result\nHint: Cypher-style query detected. Backend accepts SQL first; for multi-hop CALLS, use the SQL graph functions in `public.path` or `query_examples`.";
                    Some(json!({ "content": [{ "type": "text", "text": note }] }))
                } else {
                    Some(json!({ "content": [{ "type": "text", "text": result }] }))
                }
            }
            Err(e) => {
                // REQ-AXO-139 binder-error parsing was DuckDB-specific (matched
                // `Candidate bindings: "X", "Y"` strings emitted by DuckDB).
                // PG produces a different error format (`column "x" does not
                // exist` + `HINT:`) ; the DuckDB-format parser was retired with
                // REQ-AXO-271 slice 7. A PG-equivalent structured repair is
                // tracked separately (REQ-AXO-91494 surface fixes).
                let raw = e.to_string();
                Some(json!({
                    "content": [{ "type": "text", "text": format!("SQL Error: {}", raw) }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "operator_guidance": {
                            "problem_class": "input_invalid",
                            "follow_up_tools": ["schema_overview", "query_examples"],
                        },
                        "diagnostic_excerpt": raw.chars().take(240).collect::<String>()
                    }
                }))
            }
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

    /// REQ-AXO-901676 — `rescan_project(project_code, full=false)`.
    ///
    /// Proportionate recovery surface for cases where the indexer's
    /// incremental state machine is suspected stale (git pull massif,
    /// backup restore, inotify drop, watcher crash). Returns
    /// synchronously with `files_scheduled` + `projection_eta_ms` ;
    /// the actual scan runs asynchronously via the existing
    /// `axon_registry_changed` NOTIFY listener wired up in
    /// `runtime_boot.rs` (REQ-AXO-901675). No new DDL / listener
    /// thread is introduced — we reuse the symmetric push pattern.
    ///
    /// Modes :
    ///  - `full=false` (default) : delta scan only ; IndexedFile
    ///    cache is preserved so the indexer skips files whose
    ///    `content_hash` already matches the disk hash.
    ///  - `full=true` : wipes `public.IndexedFile` rows whose `path`
    ///    is under the project_path prefix BEFORE triggering the
    ///    NOTIFY, so every file is forced through A1/A2/A3 + B1/B2/B3
    ///    on the next scanner pass.
    ///
    /// Error envelope follows the standard MCP shape :
    /// `{"content":[{...}], "structuredContent":{"status":"error",...},
    /// "isError": true}` so callers can distinguish a registry miss
    /// from a transport failure.
    pub(crate) fn axon_rescan_project(&self, args: &Value) -> Option<Value> {
        let project_code = args
            .get("project_code")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        let project_code = match project_code {
            Some(code) => code.to_string(),
            None => return Some(rescan_error_envelope(
                "",
                "missing_project_code",
                "argument `project_code` is required",
            )),
        };
        let full = args
            .get("full")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mode_label = if full { "full" } else { "delta" };

        // Step 1 — resolve project_path via soll.ProjectCodeRegistry.
        // Inline (instead of touching workflow_project.rs) so the file
        // allocation contract for sub-B2 is respected.
        let project_path = match self.lookup_project_path_for_rescan(&project_code) {
            Some(path) => path,
            None => return Some(rescan_error_envelope(
                &project_code,
                "unknown_project_code",
                &format!(
                    "project_code `{}` is not present in soll.ProjectCodeRegistry — register it via axon_init_project first",
                    project_code
                ),
            )),
        };

        // Step 2 — when full=true, wipe IndexedFile rows under the
        // project_path so the scanner cannot skip files via cached
        // content_hash. Best-effort : a failure here is logged in the
        // returned envelope's `cache_invalidation` field but does not
        // abort the rescan trigger (degraded path still beats nothing).
        let cache_invalidation = if full {
            self.rescan_wipe_indexed_files(&project_path)
        } else {
            "skipped (delta mode)".to_string()
        };

        // Step 3 — enumerate files on disk to compute
        // `files_scheduled` for the caller. The scanner applies the
        // same .gitignore / .axonignore / supported-extension filters
        // the indexer would, so the count matches what will actually
        // be queued by A1.
        let files_scheduled = self.rescan_enumerate_file_count(&project_path, &project_code);

        // Step 4 — emit NOTIFY on the existing registry channel. The
        // listener (when ingestion is enabled — see runtime_boot.rs)
        // converts the payload into an `IngressSource::Scan` subtree
        // hint with priority 100 via record_subtree_hint, identical to
        // what axon_init_project triggers. If the indexer is not
        // running, the NOTIFY is silently dropped (PG semantics) ; the
        // caller still gets a structured envelope so the operator
        // sees the work was requested.
        let notify_outcome = self.rescan_emit_subtree_notify(&project_code, &project_path, full);

        // Step 5 — projection ETA. Heuristic : ~30 ms/file end-to-end
        // through A1+A2+A3 (CPU graph + chunks) ; B1/B2/B3 (GPU embed)
        // overlaps with A so we don't double-count. This is a coarse
        // lower bound for operator UX — actual throughput depends on
        // file size, parser, GPU saturation.
        const ETA_MS_PER_FILE: usize = 30;
        let projection_eta_ms = files_scheduled.saturating_mul(ETA_MS_PER_FILE);

        let report = format!(
            "### Rescan Project\n\n\
             **project_code:** `{project_code}`\n\
             **project_path:** `{project_path_display}`\n\
             **mode:** {mode_label} (full={full})\n\
             **files_scheduled:** {files_scheduled}\n\
             **projection_eta_ms:** {projection_eta_ms}\n\
             **cache_invalidation:** {cache_invalidation}\n\
             **notify_outcome:** {notify_outcome}\n\n\
             Re-scan triggered via `axon_registry_changed` NOTIFY ; \
             the indexer's `record_subtree_hint` consumer (REQ-AXO-901675) \
             will pick the work up asynchronously. If the indexer is not \
             running, start it via `./scripts/axon-{{live,dev}} start \
             --indexer-graph` and the next boot will replay IndexedFile from \
             PG before scanning.",
            project_path_display = project_path,
        );
        let structured = json!({
            "status": "ok",
            "project_code": project_code,
            "project_path": project_path,
            "mode": mode_label,
            "full": full,
            "files_scheduled": files_scheduled,
            "projection_eta_ms": projection_eta_ms,
            "cache_invalidation": cache_invalidation,
            "notify_outcome": notify_outcome,
        });
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "structuredContent": structured,
        }))
    }

    /// Internal helper — registry lookup duplicated from
    /// `workflow_project::lookup_project_path_by_code` so sub-B2 file
    /// allocation stays tight (see PR contract). Returns the absolute
    /// project_path string on hit, `None` otherwise.
    fn lookup_project_path_for_rescan(&self, project_code: &str) -> Option<String> {
        let escaped = project_code.replace('\'', "''");
        let raw = self
            .graph_store
            .query_json(&format!(
                "SELECT project_path FROM {} WHERE project_code = '{}'",
                self.graph_store.soll_table("ProjectCodeRegistry"),
                escaped
            ))
            .ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).ok()?;
        let path = rows.into_iter().next()?.into_iter().next()?;
        if path.trim().is_empty() {
            None
        } else {
            Some(path)
        }
    }

    /// Internal helper — wipe IndexedFile rows under `project_path`
    /// so the scanner is forced to re-parse every file regardless of
    /// the cached `content_hash`. Returns a human-readable status
    /// string for the envelope. Failure is non-fatal : the NOTIFY
    /// still fires and the indexer will at minimum re-touch
    /// `last_seen_ms` on next pass.
    fn rescan_wipe_indexed_files(&self, project_path: &str) -> String {
        let escaped = project_path.replace('\'', "''");
        let sql = format!(
            "DELETE FROM public.IndexedFile WHERE path LIKE '{}/%'",
            escaped
        );
        match self.graph_store.execute_raw_sql_gateway(&sql) {
            Ok(_) => "wiped (full mode)".to_string(),
            Err(err) => format!("wipe_failed: {err}"),
        }
    }

    /// Internal helper — enumerate files on disk for the project,
    /// honoring the same .gitignore / .axonignore / supported-extension
    /// rules as the indexer's scan pass.
    fn rescan_enumerate_file_count(&self, project_path: &str, project_code: &str) -> usize {
        let scanner = crate::scanner::Scanner::new(project_path, project_code);
        scanner.enumerate_files().len()
    }

    /// Internal helper — emit `pg_notify('axon_registry_changed', ...)`
    /// with the same payload shape as `db/ddl/07_registry_notify.sql`
    /// so the existing listener (`registry_notify_listener.rs`)
    /// converts it into an `IngressSource::Scan` subtree hint.
    /// `op` is set to `"rescan"` so operators can distinguish an
    /// operator-driven rescan from a registry insert/update in
    /// downstream telemetry. (The listener treats any non-empty
    /// `project_path` as a scan trigger so the new op string is
    /// forward-compatible.)
    fn rescan_emit_subtree_notify(
        &self,
        project_code: &str,
        project_path: &str,
        full: bool,
    ) -> String {
        let payload = json!({
            "op": "rescan",
            "project_code": project_code,
            "project_path": project_path,
            "full": full,
        });
        let payload_str = payload.to_string().replace('\'', "''");
        let sql = format!(
            "SELECT pg_notify('axon_registry_changed', '{}')",
            payload_str
        );
        match self.graph_store.execute_raw_sql_gateway(&sql) {
            Ok(_) => "notified".to_string(),
            Err(err) => format!("notify_failed: {err}"),
        }
    }
}

/// Build a standard MCP error envelope for rescan_project failures.
/// Mirrors the shape used by other tools (`{content, structuredContent,
/// isError}`) so callers parsing the envelope schema get a uniform
/// signal regardless of which tool failed.
fn rescan_error_envelope(project_code: &str, code: &str, message: &str) -> Value {
    let text = format!(
        "### Rescan Project — error\n\n\
         **status:** error\n\
         **code:** {code}\n\
         **project_code:** `{project_code}`\n\
         **message:** {message}"
    );
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": {
            "status": "error",
            "code": code,
            "project_code": project_code,
            "message": message,
        },
        "isError": true,
    })
}

