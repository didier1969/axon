use serde_json::{json, Value};

use super::format::{format_standard_contract, format_table_from_json};
use super::tools_system_debug;
use super::McpServer;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_topology::{current_runtime_process_role, AxonProcessRole};

// ── Filesystem counters (background-refreshed, NEVER on the hot path) ──
// `disk_files` + `eligible_files` are slow-moving totals over
// `AXON_WATCH_DIR`. The walk is O(tree): on the operator host
// AXON_WATCH_DIR=/home/dstadel/projects spans ~1.7 M files (~0.8 M of them
// node_modules/.git/target/_build noise). The previous design walked
// synchronously on the 1 Hz telemetry loop on every TTL miss, blocking the
// loop — and therefore the runtime heartbeat — for 17-35 s, tripping the
// watchdog (`no_telemetry_window_exceeded`) and making MCP + dashboard
// appear dead (the stale "~284 k files / ~1 s walk" assumption was off by
// an order of magnitude).
//
// Fix (responsiveness): `cached_fs_counters()` is now a PURE, non-blocking
// read of a snapshot recomputed off-runtime by `spawn_fs_counter_refresher`
// (spawn_blocking, started at brain boot). The walk also prunes build/dep/
// VCS noise directories, so it is fast AND `disk_files` reflects the real
// source tree instead of node_modules churn.
// Long-term canonical target: per-project incremental counters fed by
// watcher/PG (REQ-AXO-901749).
const FS_COUNTER_REFRESH_SECS: u64 = 60;

// Directories never worth descending into for source-file counts. Pruning
// them keeps the walk cheap and keeps `disk_files` honest.
const FS_COUNTER_PRUNE_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "cargo-target",
    "_build",
    "deps",
    ".axon",
    ".axon-dev",
    ".venv",
    "__pycache__",
    ".elixir_ls",
    ".mypy_cache",
];

struct FsCounterSnapshot {
    disk_files: i64,
    eligible_files: i64,
}

static FS_COUNTER_CACHE: std::sync::Mutex<Option<FsCounterSnapshot>> = std::sync::Mutex::new(None);

/// Walk `watch_root` once (pruning build/dep/VCS noise dirs) and return
/// `(disk_files, eligible_files)`. CPU/IO-bound — callers MUST run this off
/// the async runtime (see [`spawn_fs_counter_refresher`]).
fn compute_fs_counters(watch_root: &str) -> (i64, i64) {
    let scanner = crate::scanner::Scanner::new(watch_root, "");
    let walker = ignore::WalkBuilder::new(watch_root)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .filter_entry(|entry| {
            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    return !FS_COUNTER_PRUNE_DIRS.contains(&name);
                }
            }
            true
        })
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
    (disk, eligible)
}

/// Returns the latest `(disk_files, eligible_files)` snapshot — a PURE,
/// non-blocking read. Returns `(-1, -1)` when `AXON_WATCH_DIR` is unset or
/// no snapshot has been computed yet. NEVER walks the filesystem here; the
/// walk runs off-runtime in [`spawn_fs_counter_refresher`].
///
/// REQ-AXO-901806 — exposed `pub(crate)` so `dashboard_state.rs` reads the
/// same snapshot in the 1 Hz event composition.
pub(crate) fn cached_fs_counters() -> (i64, i64) {
    match std::env::var("AXON_WATCH_DIR") {
        Ok(v) if !v.trim().is_empty() => {}
        _ => return (-1, -1),
    }
    if let Ok(guard) = FS_COUNTER_CACHE.lock() {
        if let Some(ref snap) = *guard {
            return (snap.disk_files, snap.eligible_files);
        }
    }
    (-1, -1)
}

/// Spawn the background filesystem-counter refresher (brain boot). Recomputes
/// the snapshot every `FS_COUNTER_REFRESH_SECS` via `spawn_blocking`, so the
/// multi-second walk over a large `AXON_WATCH_DIR` NEVER blocks the async
/// runtime / 1 Hz telemetry loop / runtime heartbeat. No-op when
/// `AXON_WATCH_DIR` is unset.
pub(crate) fn spawn_fs_counter_refresher() {
    let watch_root = match std::env::var("AXON_WATCH_DIR") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return,
    };
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(FS_COUNTER_REFRESH_SECS));
        loop {
            interval.tick().await;
            let root = watch_root.clone();
            if let Ok((disk, eligible)) =
                tokio::task::spawn_blocking(move || compute_fs_counters(&root)).await
            {
                if let Ok(mut guard) = FS_COUNTER_CACHE.lock() {
                    *guard = Some(FsCounterSnapshot {
                        disk_files: disk,
                        eligible_files: eligible,
                    });
                }
            }
        }
    });
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
        // REQ-AXO-901956 — expose the IST schema (`ist.*`: Symbol / Edge /
        // IndexedFile / Chunk / ChunkEmbedding), not just SOLL intent. When the
        // DX tools (impact/inspect/bidi_trace) return hollow results, the `sql`
        // gateway is the canonical structured fallback for the code graph — but
        // only if its schema is discoverable here. ('main' was the retired
        // DuckDB schema, gone post-MIL-AXO-017.)
        let tables = self
            .graph_store
            .query_json(
                "SELECT table_schema, table_name \
                 FROM information_schema.tables \
                 WHERE table_schema IN ('ist', 'soll') \
                 ORDER BY table_schema, table_name",
            )
            .unwrap_or_else(|_| "[]".to_string());
        let columns = self
            .graph_store
            .query_json(
                "SELECT table_schema, table_name, COUNT(*) \
                 FROM information_schema.columns \
                 WHERE table_schema IN ('ist', 'soll') \
                 GROUP BY 1,2 \
                 ORDER BY 1,2",
            )
            .unwrap_or_else(|_| "[]".to_string());

        let report = format!(
            "## 🧭 Axon Schema Overview\n\n\
             **Tables (ist + soll):**\n{}\n\n\
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
`SELECT count(*) AS indexed_files FROM ist.IndexedFile;`

2) Project health (Chunk = canonical per-file per-project pivot)
`SELECT project_code, count(DISTINCT file_path) AS files, count(*) AS chunks FROM ist.Chunk GROUP BY project_code ORDER BY chunks DESC;`

3) Vector embedding coverage
`SELECT c.project_code, count(DISTINCT c.file_path) AS files_with_embeddings FROM ist.Chunk c JOIN ist.ChunkEmbedding e ON e.chunk_id = c.id GROUP BY c.project_code ORDER BY 2 DESC;`

4) Per-file chunk distribution
`SELECT file_path, count(*) AS chunks FROM ist.Chunk GROUP BY file_path ORDER BY chunks DESC LIMIT 20;`

5) Inter-language bridge visibility (Edge canonical)
`SELECT relation_type, count(*) FROM ist.Edge GROUP BY relation_type ORDER BY 2 DESC;`

6) Symbol lookup by project
`SELECT id, name, kind FROM ist.Symbol WHERE project_code = 'AXO' ORDER BY name LIMIT 50;`
"#;
        Some(json!({ "content": [{ "type": "text", "text": examples }] }))
    }

    /// REQ-AXO-901984 — runtime toggle of the query-embed provider WITHOUT a
    /// restart. `action=get` (default) reports the override + effective resolved
    /// provider + the worker's live compute. `action=set` with
    /// `provider=cpu|gpu|auto` flips it; the query worker rebuilds its model on
    /// the next request. Frees the GPU for Live (`cpu`) or re-grabs it (`gpu`).
    pub(crate) fn axon_embed_provider(&self, args: &Value) -> Option<Value> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("get");
        let current_override = crate::embedder::query_embed_provider_override_label();
        let worker_compute = crate::embedder::query_worker_compute_label().unwrap_or("unknown");
        if action == "set" {
            let Some(provider) = args.get("provider").and_then(|v| v.as_str()) else {
                return Some(json!({
                    "content": [{ "type": "text", "text": "embed_provider action=set requires `provider` = cpu | gpu | auto." }],
                    "isError": true,
                    "data": { "status": "input_invalid", "parameter_repair": { "invalid_field": "provider", "accepted_values": ["cpu", "gpu", "auto"] } }
                }));
            };
            return match crate::embedder::set_query_embed_provider_override(provider) {
                Ok(label) => {
                    let effective = crate::embedder::query_embed_effective_provider();
                    Some(json!({
                        "content": [{ "type": "text", "text": format!(
                            "Query-embed provider override set to `{}` (was `{}`). Effective lane provider now resolves to `{}`. The query worker rebuilds its model on the NEXT query — no restart. Use `cpu` to release the GPU for Live, `gpu` to re-grab it, `auto` for GPU-when-free.",
                            label, current_override, effective
                        ) }],
                        "data": { "status": "ok", "override": label, "effective_provider": effective, "reload": "lazy_on_next_query" }
                    }))
                }
                Err(e) => Some(json!({
                    "content": [{ "type": "text", "text": format!("embed_provider set failed: {}", e) }],
                    "isError": true,
                    "data": { "status": "input_invalid", "parameter_repair": { "invalid_field": "provider", "accepted_values": ["cpu", "gpu", "auto"] } }
                })),
            };
        }
        let effective = crate::embedder::query_embed_effective_provider();
        Some(json!({
            "content": [{ "type": "text", "text": format!(
                "Query-embed provider — override: `{}` ; effective (resolved): `{}` ; live worker compute: `{}`. Toggle with action=set, provider=cpu|gpu|auto (no restart; rebuilds on next query).",
                current_override, effective, worker_compute
            ) }],
            "data": { "status": "ok", "override": current_override, "effective_provider": effective, "worker_compute": worker_compute }
        }))
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
            ("IndexedFile", "SELECT count(*) FROM ist.IndexedFile"),
            ("Symbol", "SELECT count(*) FROM ist.Symbol"),
            ("Edge", "SELECT count(*) FROM ist.Edge"),
            ("Chunk", "SELECT count(*) FROM ist.Chunk"),
            ("ChunkEmbedding", "SELECT count(*) FROM ist.ChunkEmbedding"),
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

        // ── Canonical projection: ist.project_telemetry (the ONE source,
        // identical to the dashboard composite — REQ-AXO-901865). No more
        // ad-hoc per-table scalar counts / bespoke per-project rollup ; MCP
        // `embedding_status` and the dashboard now read the same view, so
        // their numbers cannot diverge. Coverage is REAL (files_chunked),
        // never the retired status column. ────────────────────────────────
        let view_rows: Vec<Vec<Value>> = self
            .graph_store
            .execute_raw_sql_gateway(&format!(
                "SELECT project_code, files_total, files_chunked, symbols, \
                        chunks_total, chunks_embedded, chunks_pending, edges \
                 FROM axon.project_telemetry{} ORDER BY chunks_total DESC",
                where_project
            ))
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
            .unwrap_or_default();

        // The SQL gateway returns numeric columns as JSON strings, so accept
        // both number and string-encoded integers (a bare as_i64() silently
        // yields 0 on "869").
        let col_i64 = |row: &[Value], idx: usize| -> i64 {
            row.get(idx)
                .and_then(|v| {
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
                })
                .unwrap_or(0)
        };
        let mut indexed_files = 0i64;
        let mut files_chunked = 0i64;
        let mut total_chunks = 0i64;
        let mut embedded_chunks = 0i64;
        let mut symbols = 0i64;
        let mut edges = 0i64;
        for row in &view_rows {
            indexed_files += col_i64(row, 1);
            files_chunked += col_i64(row, 2);
            symbols += col_i64(row, 3);
            total_chunks += col_i64(row, 4);
            embedded_chunks += col_i64(row, 5);
            edges += col_i64(row, 7);
        }
        let projects = view_rows.len() as i64;
        // pending = chunks − embedded (matches dashboard_totals exactly).
        let pending_chunks = (total_chunks - embedded_chunks).max(0);
        let coverage_pct = if total_chunks > 0 {
            (embedded_chunks as f64 / total_chunks as f64) * 100.0
        } else {
            0.0
        };

        // ── Filesystem scan counters (separate source — FS walk, not the
        // IST funnel ; refreshed off-runtime). Surfaced as a diagnostic. ──
        let (disk_files, eligible_files) = cached_fs_counters();

        // ── Per-project breakdown (global view only) — projected from the
        // same canonical view, so it reconciles with the totals above. ──
        let per_project_breakdown: Value = if project == "*" {
            let arr: Vec<Value> = view_rows
                .iter()
                .filter_map(|row| {
                    let code = row.first()?.as_str()?;
                    let ft = col_i64(row, 1);
                    let fc = col_i64(row, 2);
                    let ch = col_i64(row, 4);
                    let emb = col_i64(row, 5);
                    let cov = if ch > 0 {
                        (emb as f64 / ch as f64) * 100.0
                    } else {
                        0.0
                    };
                    // REQ-AXO-901749 — O(1) read of the per-project eligible
                    // file count from the incremental registry (populated by the
                    // scanner walk); -1 when this project has not been walked yet.
                    let fs_eligible = crate::project_file_counters::snapshot(code)
                        .map(|c| c.eligible_files)
                        .unwrap_or(-1);
                    Some(json!({
                        "project_code": code,
                        "files_total": ft,
                        "files_chunked": fc,
                        "indexed_files": ft,
                        "eligible_files": fs_eligible,
                        "chunks": ch,
                        "embeddings": emb,
                        "coverage_pct": (cov * 100.0).round() / 100.0,
                    }))
                })
                .collect();
            json!(arr)
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
        // B1 retired (REQ-AXO-901746) — no fetch-by-id worker pool ; demand_pull_b
        // feeds B2 directly. `AXON_B1_WORKERS` is a dead knob, not surfaced.
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
        // REQ-AXO-901678 — surface drain saturation knobs + counters so
        // the operator can spot A1 back-pressure without trawling
        // journalctl. Defaults mirror `PipelineChannelCaps` so an
        // unconfigured env still reports the canonical 512.
        // REQ-AXO-901893 (LEGACY FEED PURGE) — the ingress drain + periodic
        // sweep telemetry was ripped with the ingress_buffer. Watchman feeds
        // pipeline A directly; DBQ-A drains the backlog (stock_a below).

        // REQ-AXO-90009 Slice 2 — in-memory pending set heartbeat.
        // `runtime_pending` reflects what THIS process's
        // `EmbedderRuntimeState` is tracking ; `pending_chunks` above
        // is the DB-derived ground truth. The two should converge
        // within `reconcile_interval` ; a wide divergence flags a
        // NOTIFY listener drop or a missed mark_embedded.
        let runtime_pending = crate::embedder::lifecycle::process_state().pending_count();
        let runtime_pending_empty = runtime_pending == 0;

        // REQ-AXO-91572 option B / REQ-AXO-901854 — the indexer UPSERTs its
        // real lifecycle state to PG every ~5 s; a fresh row means the brain
        // is paired with a live indexer. Fetched here so pipeline_status can
        // distinguish a truly orphaned brain_only from one whose indexer is
        // draining, and reused below for the lifecycle phase telemetry.
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

        // Slice 3 SOTA — single source of truth for pipeline status +
        // blocked_reason. Same function dashboard_state.rs uses so the
        // operator sees identical strings across MCP + dashboard.
        let (pipeline_status, blocked_reason) = crate::dashboard_state::compute_pipeline_status(
            AxonRuntimeMode::from_env().as_str(),
            runtime_pending_empty,
            pending_chunks,
            None,
            indexer_heartbeat.is_some(),
        );

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
        //
        // REQ-AXO-901809 slice 2 — global stock uses the canonical
        // `pipeline_a_discovered_stock` helper (data layer owns the
        // SQL). The project-scoped variant still inlines its own
        // WHERE clause because the helper doesn't take a path filter
        // — adding one would be over-engineered for this single
        // caller. Both paths use the same retry-count cap (3) so the
        // numbers reconcile across surfaces.
        // PIL-AXO-007 (REQ-AXO-901916) — the pipeline-A claim feeder + the
        // status='discovered' work queue were retired. Pipeline A is now fed
        // directly by the scanner/Watchman walk into a bounded in-process
        // channel, so there is no DB 'discovered' stock and no A feeder metrics.
        // stock_a=0, replenish_a=null. DEC-AXO-901631 — pipeline B is now fed
        // by the flat sorted-drain (no demand_pull feeder, no (s,Q) metrics) ;
        // the B backlog is already surfaced as the top-level `pending_chunks`
        // field, so replenish_b=null.
        let stock_a: i64 = 0;
        let replenish_a = json!(null);
        let replenish_b = json!(null);

        // REQ-AXO-90009 Slice 3A — lifecycle phase telemetry. Surfaces the
        // sleep/wake state machine so operators see when the GPU session is
        // parked vs ready. Reuses `indexer_heartbeat` fetched above (the
        // indexer UPSERTs its real state every ~5 s); stale rows (> 30 s)
        // fall back to the brain-local singleton.
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
        // DEC-AXO-901626 — observed compute verdict from the SAME canonical
        // source the dashboard reads (indexer self-observation published to
        // the PG heartbeat). LLM callers get the GPU/CPU truth + how it was
        // determined, without a separate probe. Defaults CPU/unknown when no
        // fresh indexer heartbeat is present.
        // REQ-AXO-901979 — in brain_only there is no indexer heartbeat, so the
        // cross-process verdict is absent and this used to default `CPU` even
        // when the brain's OWN query worker ran on GPU (post-901978 B1). Fall
        // back to the worker's self-reported provider before defaulting CPU.
        let observed_compute = match indexer_heartbeat
            .as_ref()
            .and_then(|row| row.compute.as_deref())
        {
            Some(c) => c.to_string(),
            None => crate::embedder::query_worker_compute_label()
                .unwrap_or("CPU")
                .to_string(),
        };
        let observed_compute_source = match indexer_heartbeat
            .as_ref()
            .and_then(|row| row.compute_source.as_deref())
        {
            Some(s) => s.to_string(),
            None => crate::embedder::query_worker_compute_label()
                .map(|_| "brain_query_worker_self")
                .unwrap_or("unknown")
                .to_string(),
        };
        let indexer_build_id = indexer_heartbeat
            .as_ref()
            .and_then(|row| row.build_id.clone());
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
                        let code = entry
                            .get("project_code")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?");
                        let idx = entry
                            .get("indexed_files")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        let ch = entry.get("chunks").and_then(|v| v.as_i64()).unwrap_or(0);
                        let emb = entry
                            .get("embeddings")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        let cov = entry
                            .get("coverage_pct")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
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
             ### Filesystem (refreshed every {FS_COUNTER_REFRESH_SECS}s, off-runtime)\n\
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
             | ↳ chunked      | {files_chunked:>12} |\n\
             | Project        | {projects:>12} |\n\n\
             **Embedding coverage** : {embedded_chunks} / {total_chunks} = {coverage_pct:.2}%  (pending = {pending_chunks})\n\
             **Runtime pending set** : {runtime_pending} (in-memory ; syncé via NOTIFY + reconcile)\n\
             {breakdown_text}\n\
             ### Pipeline A — CPU (graph + chunks + FTS)\n\
             - Workers:           a1={a1}  a2={a2}  a3={a3}\n\
             - A3 batch:          {a3_batch} chunks, timeout {a3_timeout} ms\n\n\
             ### Pipeline B — GPU embedding (no B1 pool ; sorted-drain feeds B2)\n\
             - Workers:           b2={b2}  b3={b3}\n\
             - B2 batch:          {b2_batch} chunks, timeout {b2_timeout} ms\n\
             - B3 batch:          {b3_batch} chunks, timeout {b3_timeout} ms\n\
             - B fed via:        sorted-drain (ORDER BY token_count, reservoir + channel backpressure, 200ms→30s idle backoff) — DEC-AXO-901631\n\
             - Runtime idle (pending=0): {runtime_pending_empty}\n\
             - Lifecycle phase: {lifecycle_phase}  (wake_count={lifecycle_wake_count}, sleep_count={lifecycle_sleep_count}, source={lifecycle_source}{heartbeat_age_suffix})\n\
             - Compute (observed): {observed_compute}  (source={observed_compute_source}) — DEC-AXO-901626, same canonical signal as status.embedder_runtime + dashboard\n\n\
             ### File source — Watchman + DBQ-A (REQ-AXO-901893 / REQ-AXO-901897)\n\
             - Feed: Watchman clock/cursor deltas → pipeline A input_tx (legacy ingress drain + periodic sweep RIPPED)\n\
             - Backlog drainer: DBQ-A claim feeder (discovered stock below)\n\n\
             Sustained backlog > 0 with NOTIFY listener up = indexer disconnected or B2 starved; run `diagnose_indexing` for triage. Worker counts shown are env-resolved by the responding process (brain or indexer).",
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
                "files_chunked": files_chunked,
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
                    "b2": b2,
                    "b3": b3,
                    "b2_batch_size": b2_batch,
                    "b2_batch_timeout_ms": b2_timeout,
                    "b3_batch_size": b3_batch,
                    "b3_batch_timeout_ms": b3_timeout,
                    // REQ-AXO-901816 slice 6 — feeder counters only ; B backlog
                    // is already exposed as the top-level `pending_chunks` field.
                    "replenish": replenish_b
                },
                "notify_channel": crate::pipeline_v2::notify_listener::LISTEN_CHANNEL,
                "runtime_pending_count": runtime_pending,
                "runtime_idle": runtime_pending_empty,
                // Slice 3 SOTA — surface pipeline_status + blocked_reason
                // explicitly so an operator never has to guess between
                // "no indexer paired" vs "indexer up but stuck". Single
                // source of truth = `dashboard_state::compute_pipeline_status`
                // so MCP + dashboard agree.
                "pipeline_status": pipeline_status,
                "blocked_reason": blocked_reason,
                "lifecycle_phase": lifecycle_phase,
                "lifecycle_last_used_ms": lifecycle_last_used_ms,
                "lifecycle_wake_count": lifecycle_wake_count,
                "lifecycle_sleep_count": lifecycle_sleep_count,
                "lifecycle_source": lifecycle_source,
                "lifecycle_heartbeat_age_ms": lifecycle_heartbeat_age_ms,
                // DEC-AXO-901626 — observed compute verdict (canonical, same
                // source as status.embedder_runtime + the dashboard).
                "compute": observed_compute,
                "compute_source": observed_compute_source,
                "indexer_build_id": indexer_build_id,
                // REQ-AXO-901893 (LEGACY FEED PURGE) — `pipeline_drain` +
                // `periodic_sweep` telemetry blocks were ripped with the
                // ingress_buffer. The Watchman file source feeds pipeline A
                // directly (no buffer to meter); DBQ-A is the backlog drainer
                // (see `stock_a` / discovered-backlog above).
            }
        }))
    }

    pub(crate) fn axon_sql(&self, args: &Value) -> Option<Value> {
        let sql = args.get("sql")?.as_str()?;
        let q = sql.trim();
        let ql = q.to_ascii_lowercase();

        // REQ-AXO-901966 — the `sql` tool is READ-ONLY by contract. It runs on
        // the single writer-capable PG pool (query_json → query_json_on_writer),
        // so without this guard an INSERT/UPDATE/DELETE/DDL would mutate live
        // data. Reject mutations with a clear redirect instead of executing them.
        if !crate::graph_query::is_read_only_sql(q) {
            let next = super::tool_contracts::next_links("sql");
            return Some(json!({
                "content": [{ "type": "text", "text":
                    "Status: rejected_write\nThe `sql` tool is READ-ONLY (SELECT / WITH / EXPLAIN / SHOW / DESCRIBE / PRAGMA only); mutations are refused to protect live data.\n- Intent (vision / requirement / decision): use `soll_manager` or `document_intent`.\n- Runtime / index state: use the dedicated tools (status, rescan_project, …).\n- Report a problem / friction with a tool: use `mcp_feedback`." }],
                "data": { "rejected": true, "reason": "sql_tool_is_read_only", "next": next }
            }));
        }

        // REQ-AXO-271 slice 2d invariant : `skip_legacy_relations` is
        // always true under PG canonical (the SQL relation tables
        // CALLS / CALLS_NIF are dropped — `ist.Edge` + the
        // `WITH RECURSIVE` SQL graph functions handle traversal).
        // REQ-AXO-91501 vague 1d : the legacy `WITH RECURSIVE hops`
        // translation layer for `MATCH [:CALLS*1..3]` Cypher-style
        // queries is dead code under this invariant ; dropped. The
        // raw `query_json` path below handles every consumer.

        match self.graph_store.query_json(q) {
            Ok(result) => {
                // REQ-AXO-901949 inv.5 — auto-continue: surface the valid next
                // moves from the single-source tool_routing record.
                let next = super::tool_contracts::next_links("sql");
                if result.trim() == "[]" && ql.contains("match") {
                    let note =
                        "[]\n\nStatus: warn_empty_result\nHint: Cypher-style query detected. `sql` is read-only SQL over canonical tables; multi-hop CALLS traversal is NOT done in SQL (REQ-AXO-901952 retired the `ist.path` PG functions — graph traversal is RAM-only now). Use the structural tools `path`, `impact`, `bidi_trace` or `query` instead.";
                    Some(json!({ "content": [{ "type": "text", "text": note }], "data": { "next": next } }))
                } else {
                    Some(json!({ "content": [{ "type": "text", "text": result }], "data": { "next": next } }))
                }
            }
            Err(e) => {
                // REQ-AXO-901949 — repair-as-data for PG execution errors.
                // Pre-REQ-AXO-271 the DuckDB binder-error parser was retired and
                // PG errors fell back to a raw `column "x" does not exist` string
                // (the exact friction the LLM hit in session 75). We now inline
                // the *real* columns/tables of the referenced relations so the
                // agent can emit the corrected query without a second probe.
                let raw = e.to_string();
                let repair = self.pg_error_repair(q, &raw);
                // REQ-AXO-901949 inv.2 — fold the repair into the text channel so
                // the real columns are visible in the same response (clients that
                // render only `content[0].text`, incl. the HTTP/curl path, never
                // saw `data.parameter_repair`). The structured copy stays in `data`.
                let text = match &repair {
                    Some(r) => format!(
                        "SQL Error: {}{}",
                        raw,
                        super::tool_contracts::render_pg_repair_text(r)
                    ),
                    None => format!("SQL Error: {}", raw),
                };
                Some(json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "operator_guidance": {
                            "problem_class": "input_invalid",
                            "follow_up_tools": ["schema_overview", "query_examples"],
                        },
                        "diagnostic_excerpt": raw.chars().take(240).collect::<String>(),
                        "parameter_repair": repair,
                        "next_action": { "tool": "schema_overview", "arguments": {} }
                    }
                }))
            }
        }
    }

    /// REQ-AXO-901949 — turn an opaque PG execution error into repair-as-data.
    ///
    /// Detects undefined-column (42703) / undefined-table (42P01), extracts the
    /// `schema.table` relations named in the query, and inlines their real
    /// columns from `information_schema` so the agent self-corrects in one shot
    /// instead of guessing a second time. Returns `None` for unrelated errors
    /// (the raw `SQL Error` text already carries those).
    fn pg_error_repair(&self, sql: &str, raw: &str) -> Option<Value> {
        use super::tool_contracts::{classify_pg_undefined, extract_sql_relations};
        let problem_class = classify_pg_undefined(raw)?;
        let relations = extract_sql_relations(sql);

        let mut tables = Vec::new();
        for (schema, table) in &relations {
            let probe = format!(
                "SELECT column_name FROM information_schema.columns \
                 WHERE table_schema = '{}' AND lower(table_name) = '{}' \
                 ORDER BY ordinal_position",
                schema.replace('\'', "''"),
                table.replace('\'', "''")
            );
            let columns: Vec<String> = self
                .graph_store
                .query_json(&probe)
                .ok()
                .and_then(|json| serde_json::from_str::<Value>(&json).ok())
                .and_then(|v| v.as_array().cloned())
                .map(|rows| {
                    rows.iter()
                        .filter_map(|r| {
                            r.as_array()
                                .and_then(|c| c.first())
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .collect()
                })
                .unwrap_or_default();
            tables.push(json!({
                "relation": format!("{}.{}", schema, table),
                "real_columns": columns,
                "exists": !columns.is_empty()
            }));
        }

        Some(json!({
            "problem_class": problem_class,
            "referenced_relations": tables,
            "hint": "Use only `real_columns` for each relation; re-run `sql` with the corrected names. \
                     `schema_overview` lists every table if a relation is missing.",
            "follow_up_tools": ["schema_overview", "query_examples"]
        }))
    }

    pub(crate) fn axon_batch(&self, args: &Value) -> Option<Value> {
        let calls = args.get("calls")?.as_array()?;
        let mut all_results = Vec::new();

        for call in calls {
            // REQ-AXO-901925 — resilient per-call: a malformed entry yields a
            // per-call error instead of aborting the whole batch (the old `?`
            // short-circuited the entire call). One result per input call.
            let tool_name = call.get("tool").and_then(|v| v.as_str()).unwrap_or("");
            if tool_name.is_empty() {
                all_results.push(json!({ "name": "", "error": "missing `tool`" }));
                continue;
            }
            let normalized_tool_name = tool_name.strip_prefix("axon_").unwrap_or(tool_name);
            let tool_args = call.get("args").cloned().unwrap_or_else(|| json!({}));

            // REQ-AXO-901925 — route through the canonical dispatcher so EVERY
            // tool is reachable from batch, not just query/inspect/impact. The
            // old hardcoded 3-tool match returned `_ => None`, silently dropping
            // every other tool and yielding `[]` (e.g. status + embedding_status).
            let res = self
                .execute_tool_direct(normalized_tool_name, &tool_args)
                .unwrap_or_else(|| {
                    json!({
                        "status": "unknown_tool",
                        "tool": tool_name,
                        "hint": "tool not recognized by the canonical dispatcher; check `help`"
                    })
                });
            all_results.push(json!({
                "name": tool_name,
                "result": res
            }));
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
    ///  - `full=true` : wipes `ist.IndexedFile` rows whose `path`
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
            None => {
                return Some(rescan_error_envelope(
                    "",
                    "missing_project_code",
                    "argument `project_code` is required",
                ))
            }
        };
        let full = args.get("full").and_then(|v| v.as_bool()).unwrap_or(false);
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

        // Step 4 — REQ-AXO-901893 (LEGACY FEED PURGE): enrol the subtree
        // directly into the durable work queue. A scanner walk UPSERTs every
        // eligible file into ist.IndexedFile with status='discovered'; the DBQ-A
        // claim feeder (REQ-AXO-901897) drains those rows into pipeline A by
        // construction. This replaces the old pg_notify('axon_registry_changed')
        // → registry_notify_listener → ingress_buffer hop (both ripped).
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
            "DELETE FROM ist.IndexedFile WHERE path LIKE '{}/%'",
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

    /// Internal helper — enrol the project's files into the durable work queue.
    ///
    /// REQ-AXO-901893 (LEGACY FEED PURGE): the old path emitted
    /// `pg_notify('axon_registry_changed', ...)` for `registry_notify_listener.rs`
    /// to turn into an in-memory ingress subtree hint. Both the listener and the
    /// ingress_buffer were ripped, so the NOTIFY had no consumer. The tool now
    /// runs a direct scanner walk that UPSERTs every eligible file into
    /// ist.IndexedFile with status='discovered'; the DBQ-A claim feeder
    /// (REQ-AXO-901897) drains those rows into pipeline A by construction — no
    /// indexer restart, no live watcher dependency. `full` is informational here
    /// (the walk always re-enrols the whole subtree; the UPSERT is idempotent and
    /// only flips status back to 'discovered' for files whose mtime/size changed).
    fn rescan_emit_subtree_notify(
        &self,
        _project_code: &str,
        project_path: &str,
        _full: bool,
    ) -> String {
        let scanner = crate::scanner::Scanner::new(project_path, _project_code);
        let graph = self.graph_store.clone();
        let subtree = std::path::PathBuf::from(project_path);
        let enrolled = scanner.scan_subtree(graph, &subtree);
        format!("enrolled:{enrolled}")
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
