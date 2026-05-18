use serde_json::{json, Value};

use super::format::{format_standard_contract, format_table_from_json};
use super::tools_system_debug;
use super::McpServer;
use crate::graph_query::ReadFreshness;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_topology::{current_runtime_process_role, AxonProcessRole};

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
                        "**Lattice Refiner executed successfully.**\n\nDiscovered and linked **{} FFI bridges (Rustler NIFs)** between Elixir and Rust.\n\n{}",
                        count,
                        format_table_from_json(&res, &["NIF Name", "Elixir File", "Rust File"])
                    )
                } else {
                    "**Lattice Refiner executed.**\nNo new unlinked FFI bridge (Rustler NIF) detected in the graph.".to_string()
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
        // REQ-AXO-251: under PG age-only-relations, the SQL relation tables
        // (CALLS / CALLS_NIF / CONTAINS) are empty/dropped — skip those
        // checks so drift is reported only on canonical surfaces.
        let skip_legacy_relations = self.graph_store.skip_legacy_relations();
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

        let mut checks: Vec<(&str, &str)> = vec![
            ("File", "SELECT count(*) FROM File"),
            ("Symbol", "SELECT count(*) FROM Symbol"),
        ];
        if !skip_legacy_relations {
            checks.extend([
                ("CALLS", "SELECT count(*) FROM CALLS"),
                ("CALLS_NIF", "SELECT count(*) FROM CALLS_NIF"),
                ("CONTAINS", "SELECT count(*) FROM CONTAINS"),
            ]);
        }

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
        let a3_to_b1_cap = env_usize(
            "AXON_A3_TO_B1_BUFFER",
            crate::pipeline_v2::channels::A3_TO_B1_BUFFER_CAP_DEFAULT,
        );

        // REQ-AXO-90009 Slice 2 — in-memory pending set heartbeat.
        // `runtime_pending` reflects what THIS process's
        // `EmbedderRuntimeState` is tracking ; `pending_chunks` above
        // is the DB-derived ground truth. The two should converge
        // within `reconcile_interval` ; a wide divergence flags a
        // NOTIFY listener drop or a missed mark_embedded.
        let runtime_pending = crate::embedder::lifecycle::process_state().pending_count();
        let runtime_pending_empty = runtime_pending == 0;

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
        let report = format!(
            "## Axon Status (project={project})\n\n\
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
             **Runtime pending set** : {runtime_pending} (in-memory ; syncé via NOTIFY + reconcile)\n\n\
             ### Pipeline A — CPU (graph + chunks + FTS)\n\
             - Workers:           a1={a1}  a2={a2}  a3={a3}\n\
             - A3 batch:          {a3_batch} chunks, timeout {a3_timeout} ms\n\n\
             ### Pipeline B — GPU embedding\n\
             - Workers:           b1={b1}  b2={b2}  b3={b3}\n\
             - B2 batch:          {b2_batch} chunks, timeout {b2_timeout} ms\n\
             - B3 batch:          {b3_batch} chunks, timeout {b3_timeout} ms\n\
             - A3→B1 try_send:    cap {a3_to_b1_cap} (drops rattrapés par cold-start poll)\n\
             - NOTIFY channel:    chunk_pending_embed\n\
             - Cold-start poll:   every 30 s, batch {coldstart_batch}\n\
             - Runtime idle (pending=0): {runtime_pending_empty}\n\
             - Lifecycle phase: {lifecycle_phase}  (wake_count={lifecycle_wake_count}, sleep_count={lifecycle_sleep_count}, source={lifecycle_source}{heartbeat_age_suffix})\n\n\
             Sustained backlog > 0 with NOTIFY listener up = indexer disconnected or B2 starved; run `diagnose_indexing` for triage. Worker counts shown are env-resolved by the responding process (brain or indexer)."
        );

        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "structuredContent": {
                "project": project,
                "symbols": symbols,
                "total_chunks": total_chunks,
                "embedded_chunks": embedded_chunks,
                "pending_chunks": pending_chunks,
                "coverage_pct": coverage_pct,
                "edges": edges,
                "indexed_files": indexed_files,
                "projects": projects,
                "pipeline_a": { "a1": a1, "a2": a2, "a3": a3, "a3_batch_size": a3_batch, "a3_batch_timeout_ms": a3_timeout },
                "pipeline_b": { "b1": b1, "b2": b2, "b3": b3, "b2_batch_size": b2_batch, "b2_batch_timeout_ms": b2_timeout, "b3_batch_size": b3_batch, "b3_batch_timeout_ms": b3_timeout, "a3_to_b1_buffer_cap": a3_to_b1_cap, "coldstart_batch_size": coldstart_batch },
                "notify_channel": "chunk_pending_embed",
                "coldstart_poll_interval_secs": 30,
                "runtime_pending_count": runtime_pending,
                "runtime_idle": runtime_pending_empty,
                "lifecycle_phase": lifecycle_phase,
                "lifecycle_last_used_ms": lifecycle_last_used_ms,
                "lifecycle_wake_count": lifecycle_wake_count,
                "lifecycle_sleep_count": lifecycle_sleep_count,
                "lifecycle_source": lifecycle_source,
                "lifecycle_heartbeat_age_ms": lifecycle_heartbeat_age_ms,
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
}

