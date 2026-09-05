use serde_json::{json, Value};

use super::format::{format_standard_contract, format_table_from_json};
use super::tools_system_debug;
use super::McpServer;
use crate::pipeline::orchestrator::PipelineAWorkerCounts;
use crate::runtime_mode::AxonRuntimeMode;

/// REQ-AXO-902621 (suite) — plafond de RESTITUTION de `sql`, en caractères.
///
/// 60 000 : au-dessus de la quasi-totalité des appels mesurés, et très en dessous
/// du seuil auquel le client REFUSE la réponse (une sortie de 343 165 caractères
/// a été rejetée le 2026-09-05 avant d'être lue). Le chiffre est un compromis
/// choisi, pas hérité : plus haut, on paie des réponses que personne ne lira ;
/// plus bas, on force une pagination à des lectures qui passaient très bien.
const SEUIL_RENDU_SQL_CHARS: usize = 60_000;
use crate::runtime_topology::{current_runtime_process_role, AxonProcessRole};

/// CPT-AXO-90052 — normalize a SQL query to a content-free SHAPE for the
/// `axon.sql_shape_stat` rollup: string/number literals → `?`, whitespace
/// collapsed, lowercased. Identifiers embedding digits (e.g. `pipeline`) are
/// preserved — a digit becomes `?` only when NOT preceded by an identifier char.
/// Never stores literal values (PIL-AXO-9003 commercial privacy).
fn normalize_sql_shape(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\'' {
            out.push('?');
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\'' {
                    // doubled '' is an escaped quote inside the literal
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else if b.is_ascii_digit()
            && !(i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'))
        {
            out.push('?');
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
        } else if (b as char).is_ascii_whitespace() {
            if !out.ends_with(' ') {
                out.push(' ');
            }
            i += 1;
        } else {
            out.push((b as char).to_ascii_lowercase());
            i += 1;
        }
    }
    out.trim().to_string()
}

/// CPT-AXO-90052 — stable 16-hex hash of a normalized shape (rollup PK key).
fn sql_shape_hash(shape: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    shape.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// REQ-AXO-902345 residual — does a resolved GPU embed provider run on a non-GPU
/// worker? `effective_provider` is the OUTPUT of `query_embed_effective_provider`,
/// which already folds in GPU presence + provider-lib availability, so `cuda` /
/// `tensorrt` mean GPU was intended AND resolvable. If the worker's observed
/// compute is nonetheless not GPU, the execution provider failed to load and the
/// embedder fell back to CPU silently — the 2026-08-17 defect where compute=CPU
/// was reported under a HEALTHY banner. `cpu`→CPU is consistent (no mismatch);
/// a non-GPU provider running on GPU is not a defect either. Pure → unit-testable.
fn embed_provider_compute_mismatch(effective_provider: &str, observed_compute: &str) -> bool {
    let provider_intends_gpu = matches!(effective_provider, "cuda" | "tensorrt" | "gpu");
    provider_intends_gpu && !observed_compute.eq_ignore_ascii_case("GPU")
}

fn semantic_lane_is_blocked(pending_chunks: i64, active_workers: Option<i64>) -> bool {
    pending_chunks > 0 && active_workers.is_some_and(|active| active == 0)
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
        // REQ-AXO-902329 — the schema set is DERIVED (exclude the system schemas), not
        // enumerated. It used to be a hardcoded `IN ('ist', 'soll')`, duplicated in the
        // two queries below, and it hid **36 of the product's 61 tables** — every one of
        // `axon.*` (31) and `pgmq.*` (5).
        //
        // That is not a cosmetic gap. This tool is what the `sql` contract tells a caller
        // to consult BEFORE writing a query ("Use only after `schema_overview` or
        // `query_examples`"). Meanwhile `mcp_friction_report` prints
        // "_Table: `axon.mcp_friction`._" and its own published description names that
        // table as the backing store — so the product pointed an LLM at a table its own
        // inventory reported as non-existent. Measured cost, on this very defect: the
        // author of REQ-AXO-902325 abandoned an audit of `axon.practice` ("verification
        // tentée et BLOQUÉE") and stopped at a hypothesis that turned out to be wrong;
        // the real cause was one query away.
        //
        // An allow-list also fails silently forward: every schema added after it was
        // written is invisible until somebody notices. Excluding the two system schemas
        // inverts that — a new product schema shows up on its own.
        const PRODUCT_SCHEMAS: &str =
            "table_schema NOT IN ('pg_catalog', 'information_schema') \
             AND table_schema NOT LIKE 'pg_toast%' AND table_schema NOT LIKE 'pg_temp%'";
        let tables = self
            .graph_store
            .query_json(&format!(
                "SELECT table_schema, table_name \
                 FROM information_schema.tables \
                 WHERE table_type = 'BASE TABLE' AND {PRODUCT_SCHEMAS} \
                 ORDER BY table_schema, table_name"
            ))
            .unwrap_or_else(|_| "[]".to_string());
        let columns = self
            .graph_store
            .query_json(&format!(
                "SELECT c.table_schema, c.table_name, COUNT(*) \
                 FROM information_schema.columns c \
                 JOIN information_schema.tables t \
                   ON t.table_schema = c.table_schema AND t.table_name = c.table_name \
                 WHERE t.table_type = 'BASE TABLE' AND c.{PRODUCT_SCHEMAS} \
                 GROUP BY 1,2 \
                 ORDER BY 1,2"
            ))
            .unwrap_or_else(|_| "[]".to_string());

        // Publish the totals. A caller who knows the inventory holds N tables across M
        // schemas can SEE a future hole as a number; a bare list makes an omission look
        // exactly like a complete answer — which is how 36 missing tables went unnoticed.
        let (table_count, schema_list) = serde_json::from_str::<Vec<Vec<Value>>>(&tables)
            .map(|rows| {
                let mut schemas: Vec<String> = rows
                    .iter()
                    .filter_map(|r| r.first().and_then(Value::as_str).map(str::to_string))
                    .collect();
                schemas.sort();
                schemas.dedup();
                (rows.len(), schemas.join(", "))
            })
            .unwrap_or((0, String::new()));

        let report = format!(
            "## 🧭 Axon Schema Overview\n\n\
             **{table_count} base table(s) across {} schema(s): {schema_list}** \
             (every non-system schema — REQ-AXO-902329)\n\n\
             **Tables:**\n{}\n\n\
             **Column count by table:**\n{}\n",
            schema_list.split(", ").filter(|s| !s.is_empty()).count(),
            format_table_from_json(&tables, &["Schema", "Table"]),
            format_table_from_json(&columns, &["Schema", "Table", "Columns"])
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {"status":"ok","table_count":table_count,"schemas":schema_list}
        }))
    }

    pub(crate) fn axon_query_examples(&self, _args: &Value) -> Option<Value> {
        // REQ-AXO-901653 slice-5d — examples migrated from public.File to
        // pipeline canonical (IndexedFile + Chunk + ChunkEmbedding).
        let examples = r#"## 📚 Query Examples (SQL gateway / cypher tool)

1) Workspace size (canonical pipeline)
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

    /// REQ-AXO-902234 — runtime arm/disarm of the GPU idle-drop watchdog WITHOUT
    /// a restart. Writes the DESIRED state to `axon.EmbedderControl`; the PG
    /// trigger notifies `embedder_control`, and the indexer's listener flips the
    /// atomics the watchdog re-reads each tick (~5 s).
    ///
    /// Why a PG row and not an in-process atomic like `embed_provider`: the
    /// watchdog lives in `axon-indexer`, this tool in `axon-brain` — two
    /// processes. The row ALSO makes the setting durable, which fixes the original
    /// defect (an env var read once at boot, so an activation was lost on every
    /// restart).
    pub(crate) fn axon_idle_drop(&self, args: &Value) -> Option<Value> {
        const ROLE: &str = "indexer";
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("get");

        // Desired state as currently stored (also the `get` answer).
        let read_row = || -> Option<(bool, i64, String)> {
            let raw = self
                .graph_store
                .execute_raw_sql_gateway(&format!(
                    "SELECT idle_drop_enabled, idle_seconds, COALESCE(updated_by,'') \
                     FROM axon.EmbedderControl WHERE process_role = '{ROLE}'"
                ))
                .ok()?;
            let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).ok()?;
            let row = rows.first()?;
            // The SQL gateway renders every column as a string (see
            // tools_system_debug::json_i64) — tolerate both shapes.
            let enabled = row.first().map(|v| v == "true" || v == true).unwrap_or(false);
            let seconds = row
                .get(1)
                .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse::<i64>().ok()))
                .unwrap_or(0);
            let by = row
                .get(2)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            Some((enabled, seconds, by))
        };

        if action == "set" {
            let Some(enabled) = args.get("enabled").and_then(|v| v.as_bool()) else {
                // GUI-AXO-1026 inv.5 — repair AS DATA, at the recency edge.
                return Some(json!({
                    "content": [{ "type": "text", "text":
                        "idle_drop action=set requires `enabled` (true = reclaim idle VRAM, false = keep the session resident)." }],
                    "isError": true,
                    "data": { "status": "input_invalid", "parameter_repair": {
                        "invalid_field": "enabled",
                        "accepted_values": [true, false],
                        "corrected_call": { "name": "idle_drop", "arguments": { "action": "set", "enabled": true } }
                    } }
                }));
            };
            let seconds = args.get("seconds").and_then(|v| v.as_u64()).map(|s| s.max(1));
            let now_ms = chrono::Utc::now().timestamp_millis();
            // `seconds` omitted ⇒ keep the stored threshold (documented in the
            // input contract) rather than silently resetting it to the default.
            let seconds_update = match seconds {
                Some(_) => "EXCLUDED.idle_seconds",
                None => "axon.EmbedderControl.idle_seconds",
            };
            let seconds_insert = seconds.unwrap_or(20);
            let sql = format!(
                "INSERT INTO axon.EmbedderControl \
                 (process_role, idle_drop_enabled, idle_seconds, updated_ms, updated_by) \
                 VALUES ('{ROLE}', {enabled}, {seconds_insert}, {now_ms}, 'mcp:idle_drop') \
                 ON CONFLICT (process_role) DO UPDATE SET \
                   idle_drop_enabled = EXCLUDED.idle_drop_enabled, \
                   idle_seconds = {seconds_update}, \
                   updated_ms = EXCLUDED.updated_ms, \
                   updated_by = EXCLUDED.updated_by"
            );
            return match self.graph_store.execute_raw_sql_gateway(&sql) {
                Ok(_) => {
                    let (_, stored_seconds, _) = read_row().unwrap_or((enabled, seconds_insert as i64, String::new()));
                    Some(json!({
                        "content": [{ "type": "text", "text": format!(
                            "GPU idle-drop {} (t_idle={} s). The indexer applies it within ~5 s via LISTEN embedder_control — no restart, no GPU teardown. Durable: survives restarts and reboots.",
                            if enabled { "ARMED" } else { "DISARMED" },
                            stored_seconds
                        ) }],
                        "data": {
                            "status": "ok",
                            "idle_drop_enabled": enabled,
                            "idle_seconds": stored_seconds,
                            "applies_in": "<=5s (watchdog tick)",
                            "next_action": { "kind": "continue_with_follow_up_tool", "tool": "embedding_status", "when": "after_5s" }
                        }
                    }))
                }
                Err(e) => Some(json!({
                    "content": [{ "type": "text", "text": format!("idle_drop set failed: {e}") }],
                    "isError": true,
                    "data": { "status": "error" }
                })),
            };
        }

        match read_row() {
            Some((enabled, seconds, by)) => Some(json!({
                "content": [{ "type": "text", "text": format!(
                    "GPU idle-drop desired state: {} (t_idle={} s, set by `{}`). Flip it with action=set, enabled=true|false — no restart. Observe the effect via `embedding_status` (lifecycle_sleep_count).",
                    if enabled { "ARMED" } else { "DISARMED" }, seconds,
                    if by.is_empty() { "unknown" } else { &by }
                ) }],
                "data": { "status": "ok", "idle_drop_enabled": enabled, "idle_seconds": seconds, "updated_by": by }
            })),
            // No row yet = the indexer has not booted since this feature landed;
            // it seeds the row from the env on its next boot (REQ-AXO-902234 D1).
            None => Some(json!({
                "content": [{ "type": "text", "text":
                    "No idle-drop control row yet — the indexer seeds it from AXON_EMBEDDER_IDLE_DROP on its next boot. `action=set` creates it now." }],
                "data": { "status": "ok", "idle_drop_enabled": null, "seeded": false }
            })),
        }
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
                        chunks_total, chunks_embedded, chunks_pending, edges, \
                        chunks_failed \
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
        // REQ-AXO-902382 — read the two states SEPARATELY from the view instead of
        // deriving one by subtraction.
        //
        // `pending_chunks` was `total - embedded`, which merges an ACTIVE queue with
        // a TERMINAL population: nothing in the runtime ever retries `failed` (the
        // sorted-drain reservoir only SELECTs `embed_status='pending'`). Measured
        // 2026-08-21 on PRP: the view says `chunks_pending = 0` while the
        // subtraction yielded 25 194 — every one of them dead, none waiting.
        //
        // VPC read the aggregate as a queue that was not being served, and went
        // looking for a service mechanism that does not exist (inbox 11935/12086).
        // The view already computed both counts correctly; this code skipped column
        // 6 to recompute a worse answer.
        let mut pending_chunks = 0i64;
        let mut failed_chunks = 0i64;
        for row in &view_rows {
            indexed_files += col_i64(row, 1);
            files_chunked += col_i64(row, 2);
            symbols += col_i64(row, 3);
            total_chunks += col_i64(row, 4);
            embedded_chunks += col_i64(row, 5);
            pending_chunks += col_i64(row, 6);
            edges += col_i64(row, 7);
            failed_chunks += col_i64(row, 8);
        }
        let projects = view_rows.len() as i64;
        let coverage_pct = if total_chunks > 0 {
            (embedded_chunks as f64 / total_chunks as f64) * 100.0
        } else {
            0.0
        };

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
                    Some(json!({
                        "project_code": code,
                        "files_total": ft,
                        "files_chunked": fc,
                        "indexed_files": ft,
                        "chunks": ch,
                        "embeddings": emb,
                        // REQ-AXO-902382 — per-project too: this breakdown is what an
                        // operator reads to decide WHERE to act, and "11 projects below
                        // 25% coverage" means something different when the shortfall is
                        // dead rather than queued.
                        "pending": col_i64(row, 6),
                        "failed": col_i64(row, 8),
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
        // REQ-AXO-902550 — expose effective live-safe concurrency. Reporting
        // the stale raw override after the runtime clamps it sends operators in
        // exactly the wrong direction during a customer-availability incident.
        let pipeline_a = PipelineAWorkerCounts::from_env();
        let a1 = pipeline_a.a1;
        let a2 = pipeline_a.a2;
        let a3 = pipeline_a.a3;
        let a3_batch = env_usize("AXON_A3_BATCH_SIZE", 32);
        let a3_timeout = env_u64("AXON_A3_BATCH_TIMEOUT_MS", 10);
        // B1 retired (REQ-AXO-901975) — no fetch-by-id worker pool ; the sorted
        // drain feeds B2 directly. `AXON_B1_WORKERS` is a dead knob, not surfaced.
        let b2 = env_usize("AXON_B2_WORKERS", 1);
        let b3 = env_usize("AXON_B3_WORKERS", 2);
        let b2_batch = env_usize(
            "AXON_B2_BATCH_SIZE",
            crate::pipeline::channels::B2_BATCH_SIZE_DEFAULT,
        );
        let b2_timeout = env_u64(
            "AXON_B2_BATCH_TIMEOUT_MS",
            crate::pipeline::channels::B2_BATCH_TIMEOUT_MS_DEFAULT,
        );
        let b3_batch = env_usize(
            "AXON_B3_BATCH_SIZE",
            crate::pipeline::channels::B3_BATCH_SIZE_DEFAULT,
        );
        let b3_timeout = env_u64(
            "AXON_B3_BATCH_TIMEOUT_MS",
            crate::pipeline::channels::B3_BATCH_TIMEOUT_MS_DEFAULT,
        );
        // REQ-AXO-901678 — surface drain saturation knobs + counters so
        // the operator can spot A1 back-pressure without trawling
        // journalctl. Defaults mirror `PipelineChannelCaps` so an
        // unconfigured env still reports the canonical 512.
        // REQ-AXO-901893 (LEGACY FEED PURGE) — the ingress drain + periodic
        // sweep telemetry was ripped with the ingress_buffer. Watchman feeds
        // pipeline A directly; DBQ-A drains the backlog (stock_a below).

        // REQ-AXO-90009 Slice 2 / REQ-AXO-902044 — best-effort in-memory pending
        // heartbeat. `runtime_pending` is what THIS process's `EmbedderRuntimeState`
        // tracks (A3 mark_pending → B3 mark_embedded); `pending_chunks` above is the
        // DB-derived GROUND TRUTH and is authoritative. The two no longer auto-
        // reconcile: the wholesale self-healing loop was retired (REQ-AXO-902036),
        // so `runtime_pending` can drift inflated (chunks pending-marked by A3 but
        // dedup-skipped on the B lane never clear). Treat it as a coarse liveness
        // hint only; `compute_pipeline_status` below reads pending_chunks for the
        // idle verdict so the drift can never mask a genuinely-drained pipeline.
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
        let indexer_runtime_truth = self
            .graph_store
            .latest_indexer_runtime_truth("indexer")
            .ok()
            .flatten()
            .filter(|row| (now_ms - row.heartbeat_ms).max(0) <= HEARTBEAT_FRESHNESS_MS);

        // REQ-AXO-902597 — a fresh process heartbeat proves only that the
        // indexer process lives. It does NOT prove pipeline B has a worker.
        // Project the owner-published admission truth so `embedding_status`
        // cannot call a 0-worker durable backlog active again.
        let semantic_lane_blocked = semantic_lane_is_blocked(
            pending_chunks,
            indexer_runtime_truth
                .as_ref()
                .map(|truth| truth.vector_workers_active_current),
        );

        // Slice 3 SOTA — single source of truth for pipeline status +
        // blocked_reason. Same function dashboard_state.rs uses so the
        // operator sees identical strings across MCP + dashboard.
        let (pipeline_status, blocked_reason) = if semantic_lane_blocked {
            (
                "indexer_idle_blocked",
                Some("semantic_vector_workers_not_running"),
            )
        } else {
            crate::dashboard_state::compute_pipeline_status(
                AxonRuntimeMode::from_env().as_str(),
                runtime_pending_empty,
                pending_chunks,
                None,
                indexer_heartbeat.is_some(),
            )
        };
        let semantic_lane_line = match indexer_runtime_truth.as_ref() {
            Some(truth) => format!(
                "- Semantic admission (indexer-owned): mode={} enabled={} configured={} active={} started_total={} reason={} allowed_gpu_workers={}",
                truth.runtime_mode,
                truth.semantic_workers_enabled,
                truth.vector_workers_configured,
                truth.vector_workers_active_current,
                truth.vector_workers_started_total,
                truth.vector_worker_admission_reason,
                truth.allowed_gpu_workers,
            ),
            None => "- Semantic admission (indexer-owned): unavailable — a process heartbeat alone does not prove vector workers are running".to_string(),
        };

        // PIL-AXO-007 (REQ-AXO-901916) — the pipeline-A claim feeder and the
        // status='discovered' work queue were retired. Pipeline A is fed directly by the
        // scanner/Watchman walk into a bounded in-process channel, so there is no DB
        // 'discovered' stock and no A feeder metrics: stock_a=0, replenish_a=null.
        // DEC-AXO-901631 — pipeline B is fed by the flat sorted-drain (no demand_pull
        // feeder, no (s,Q) metrics); the B backlog is already surfaced as the top-level
        // `pending_chunks` field, so replenish_b=null.
        //
        // REQ-AXO-902260 — the paragraph that used to sit here described `stock_a` as a
        // live discovered-backlog count read through a "canonical helper", ten lines above
        // `let stock_a: i64 = 0;`. That helper is now deleted; the description of a feed
        // that no longer exists is what sent the LLL investigation down the wrong path
        // (REQ-AXO-902253). Coverage truth is chunk presence (`diagnose_indexing`).
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
        // REQ-AXO-902345 residual — a RESOLVED GPU provider whose worker actually
        // runs on CPU is a SILENT fallback. The 2026-08-17 migration left the CUDA
        // EP unloadable (libcuda.so.1 off LD_LIBRARY_PATH); the worker fell to CPU
        // and embedding_status reported compute=CPU while the runtime read HEALTHY.
        // `query_embed_effective_provider` already folds in GPU presence + provider
        // lib availability, so an effective `cuda`/`tensorrt` means GPU was INTENDED
        // and resolvable — if the observed compute is nonetheless not GPU, the EP
        // died at load time. Make that mismatch a FIRST-CLASS field instead of a
        // two-field cross-reference nobody was making.
        let effective_embed_provider = crate::embedder::query_embed_effective_provider();
        let provider_compute_mismatch =
            embed_provider_compute_mismatch(&effective_embed_provider, &observed_compute);
        let provider_mismatch_line = if provider_compute_mismatch {
            format!(
                "\n             - ⚠️ PROVIDER/COMPUTE MISMATCH: resolved embed provider is `{effective_embed_provider}` (GPU intended + resolvable) but the worker runs on {observed_compute} — the GPU execution provider failed to load and the embedder fell back to CPU SILENTLY. This is NOT healthy: query / why / retrieve_context embed at ~seconds, not ~ms. Check LD_LIBRARY_PATH (libcuda.so.1) and the brain log for 'CUDA init failed' (REQ-AXO-902345)."
            )
        } else {
            String::new()
        };

        let indexer_build_id = indexer_heartbeat
            .as_ref()
            .and_then(|row| row.build_id.clone());
        // REQ-AXO-902047 slice 1b — B3 (embedding persist) health, published by
        // the indexer in the same heartbeat row. Surfaces the REAL PG error
        // (root message + SQLSTATE, deduped) + a systemic-failure verdict so an
        // LLM diagnoses a wedged embed writer (the REQ-AXO-902046 incident) in
        // ONE call instead of gdb + 4 h. Only the indexer runs B3 — when no
        // fresh indexer heartbeat exists the verdict is HEALTHY/unknown, never a
        // brain-local fabrication.
        use crate::pipeline::stage_health::B3_SYSTEMIC_FAILURE_THRESHOLD;
        let (
            b3_consecutive_failures,
            b3_total_failures,
            b3_total_successes,
            b3_last_error,
            b3_last_error_count,
            b3_last_error_last_seen_ms,
        ) = match indexer_heartbeat.as_ref() {
            Some(row) => (
                row.b3_consecutive_failures,
                row.b3_total_failures,
                row.b3_total_successes,
                row.b3_last_error.clone(),
                row.b3_last_error_count,
                row.b3_last_error_last_seen_ms,
            ),
            None => (0, 0, 0, None, 0, 0),
        };
        let b3_total_attempts = b3_total_failures + b3_total_successes;
        let b3_error_rate = if b3_total_attempts > 0 {
            b3_total_failures as f64 / b3_total_attempts as f64
        } else {
            0.0
        };
        let b3_systemically_failing =
            b3_consecutive_failures >= B3_SYSTEMIC_FAILURE_THRESHOLD as i64;
        let b3_verdict = if b3_systemically_failing {
            "DEGRADED"
        } else {
            "HEALTHY"
        };
        // Recovery hint only when DEGRADED — a healthy stage needs no advice.
        let b3_recovery_hint = if b3_systemically_failing {
            Some(format!(
                "B3 embedding-persist has failed {b3_consecutive_failures} batches in a row; the sorted-drain has backed off to protect CPU. Last error: {}. This signature (e.g. `missing chunk … toast … XX001`) points at corrupt ist.ChunkEmbedding rows poisoning the HNSW index — see reference_hnsw_toast_corruption_remediation (scan ctid + pg_surgery.heap_force_kill + REINDEX CONCURRENTLY). Recovery is automatic once the DB is repaired (a probe batch runs after each backoff sleep).",
                b3_last_error.as_deref().unwrap_or("<none captured>")
            ))
        } else {
            None
        };
        let heartbeat_age_suffix = lifecycle_heartbeat_age_ms
            .map(|age| format!(", heartbeat_age_ms={age}"))
            .unwrap_or_default();
        // REQ-AXO-902047 slice 1b — one human-readable B3 health line for the
        // text report (the structured block carries the machine fields).
        let b3_health_line = if b3_systemically_failing {
            format!(
                "- B3 health: ⚠️ DEGRADED — {b3_consecutive_failures} consecutive failures, error_rate={:.1}%, last_error={} (×{b3_last_error_count}). Run `diagnose_indexing`; see recovery_hint.",
                b3_error_rate * 100.0,
                b3_last_error.as_deref().unwrap_or("<none>")
            )
        } else if b3_total_failures > 0 {
            format!(
                "- B3 health: HEALTHY (recovered) — error_rate={:.2}%, last_error={} (×{b3_last_error_count}), {b3_total_successes} successes",
                b3_error_rate * 100.0,
                b3_last_error.as_deref().unwrap_or("<none>")
            )
        } else {
            "- B3 health: HEALTHY — no persist failure observed".to_string()
        };
        // REQ-AXO-902387 — B2 (GPU embed) VRAM pressure, published by the indexer
        // in the same heartbeat row. Answers the question no existing signal could
        // on 2026-08-20: "is the GPU lane actually serving, or is every batch
        // silently taking the CPU path at 1/100th the throughput?". The ratio is
        // over a rolling window of recent batches — a cumulative count cannot
        // express a regime, and a total since boot drowns a fresh incident.
        let b2_observed = indexer_heartbeat
            .as_ref()
            .map(|row| row.b2_window_observed)
            .unwrap_or(0);
        let b2_cpu_fallbacks = indexer_heartbeat
            .as_ref()
            .map(|row| row.b2_window_cpu_fallbacks)
            .unwrap_or(0);
        let b2_batch_cap = indexer_heartbeat
            .as_ref()
            .map(|row| row.b2_gpu_batch_cap)
            .unwrap_or(0);
        let b2_resizes = indexer_heartbeat
            .as_ref()
            .map(|row| row.b2_resizes)
            .unwrap_or(0);
        let b2_recycles = indexer_heartbeat
            .as_ref()
            .map(|row| row.b2_session_recycles)
            .unwrap_or(0);
        // An empty window is NOT zero pressure. Rendering it as 0.0 would be the
        // vacuous verdict this codebase keeps paying for (REQ-AXO-902384), so the
        // ratio stays absent and the verdict says NOT ARMED.
        let b2_ratio = (b2_observed > 0).then(|| b2_cpu_fallbacks as f64 / b2_observed as f64);
        let b2_verdict = match b2_ratio {
            None => "not_armed",
            Some(r) if r >= crate::pipeline::embed_pressure::CRITICAL_RATIO => "critical",
            Some(r) if r >= crate::pipeline::embed_pressure::DEGRADED_RATIO => "degraded",
            Some(_) => "healthy",
        };
        let b2_cap_suffix = if b2_batch_cap > 0 {
            format!(
                ", lot GPU plafonné à {b2_batch_cap} ({b2_resizes} retaillages, {b2_recycles} recyclages de session)"
            )
        } else {
            String::new()
        };
        let b2_health_line = match b2_ratio {
            None => "- B2 VRAM pressure: NON ARMÉ — aucun lot d'embed observé depuis le \
                     démarrage de l'indexeur. Ce n'est PAS « pas de pression » : rien n'a \
                     été mesuré."
                .to_string(),
            Some(ratio) => format!(
                "- B2 VRAM pressure: {} — b2_cpu_fallback_ratio={:.2} ({b2_cpu_fallbacks}/{b2_observed} lots récents recalculés sur CPU faute de VRAM){b2_cap_suffix}.{}",
                match b2_verdict {
                    "critical" => "⚠️ CRITIQUE",
                    "degraded" => "⚠️ DÉGRADÉ",
                    _ => "SAIN",
                },
                ratio,
                if b2_verdict == "healthy" {
                    ""
                } else {
                    " Le débit d'embed est effondré SANS qu'aucun lot n'échoue — libérez de la VRAM, augmentez AXON_GPU_RESERVE_MB, ou baissez AXON_B2_BATCH_SIZE."
                }
            ),
        };
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
             {semantic_lane_line}\n\
             - Lifecycle phase: {lifecycle_phase}  (wake_count={lifecycle_wake_count}, sleep_count={lifecycle_sleep_count}, source={lifecycle_source}{heartbeat_age_suffix})\n\
             - Compute (observed): {observed_compute}  (source={observed_compute_source}) — DEC-AXO-901626, same canonical signal as status.embedder_runtime + dashboard{provider_mismatch_line}\n\
             {b3_health_line}\n\
             {b2_health_line}\n\n\
             ### File source — Watchman + DBQ-A (REQ-AXO-901893 / REQ-AXO-901897)\n\
             - Feed: Watchman clock/cursor deltas → pipeline A input_tx (legacy ingress drain + periodic sweep RIPPED)\n\
             - Backlog drainer: DBQ-A claim feeder (discovered stock below)\n\n\
             Sustained backlog > 0 with NOTIFY listener up = indexer disconnected or B2 starved; run `diagnose_indexing` for triage. Worker counts shown are env-resolved by the responding process (brain or indexer).",
        );

        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "structuredContent": {
                "project": project,
                "symbols": symbols,
                "total_chunks": total_chunks,
                "embedded_chunks": embedded_chunks,
                "pending_chunks": pending_chunks,
                // REQ-AXO-902382 — TERMINAL, and nothing retries it. Rendered next to
                // `pending` precisely so the two can never be read as one number again.
                "failed_chunks": failed_chunks,
                "failed_is_terminal": true,
                "failed_recovery": if failed_chunks > 0 {
                    Some("`failed` chunks are NEVER retried by the runtime — the drain only reads `pending`. Requeue with `bash scripts/maintenance/reset_failed_embeddings.sh --execute` (paced, autovacuum-guarded). Chunks above the model window stay failed until REQ-AXO-902364.")
                } else {
                    None
                },
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
                "notify_channel": crate::pipeline::channels::CHUNK_PENDING_NOTIFY_CHANNEL,
                "runtime_pending_count": runtime_pending,
                "runtime_idle": runtime_pending_empty,
                // Slice 3 SOTA — surface pipeline_status + blocked_reason
                // explicitly so an operator never has to guess between
                // "no indexer paired" vs "indexer up but stuck". Single
                // source of truth = `dashboard_state::compute_pipeline_status`
                // so MCP + dashboard agree.
                "pipeline_status": pipeline_status,
                "blocked_reason": blocked_reason,
                "semantic_lane": indexer_runtime_truth.as_ref().map(|truth| json!({
                    "source": "indexer_runtime_truth",
                    "runtime_mode": truth.runtime_mode,
                    "semantic_workers_enabled": truth.semantic_workers_enabled,
                    "vector_workers_configured": truth.vector_workers_configured,
                    "vector_workers_active_current": truth.vector_workers_active_current,
                    "vector_workers_started_total": truth.vector_workers_started_total,
                    "vector_worker_admission_reason": truth.vector_worker_admission_reason,
                    "allowed_gpu_workers": truth.allowed_gpu_workers,
                    "heartbeat_age_ms": (now_ms - truth.heartbeat_ms).max(0),
                })),
                "lifecycle_phase": lifecycle_phase,
                "lifecycle_last_used_ms": lifecycle_last_used_ms,
                "lifecycle_wake_count": lifecycle_wake_count,
                "lifecycle_sleep_count": lifecycle_sleep_count,
                "lifecycle_source": lifecycle_source,
                "lifecycle_heartbeat_age_ms": lifecycle_heartbeat_age_ms,
                // REQ-AXO-902047 slice 1b — B3 (embedding persist) health. One
                // call gives an LLM the systemic-failure verdict, the REAL PG
                // error (root + SQLSTATE), the error rate, and a recovery hint —
                // no log access, no gdb (the REQ-AXO-902046 incident).
                "b2_pressure": {
                    "verdict": b2_verdict,
                    "cpu_fallback_ratio": b2_ratio,
                    "window_observed": b2_observed,
                    "window_cpu_fallbacks": b2_cpu_fallbacks,
                    "gpu_batch_cap": (b2_batch_cap > 0).then_some(b2_batch_cap),
                    "resizes": b2_resizes,
                    "session_recycles": b2_recycles,
                    "degraded_threshold": crate::pipeline::embed_pressure::DEGRADED_RATIO,
                    "critical_threshold": crate::pipeline::embed_pressure::CRITICAL_RATIO,
                },
                "b3_health": {
                    "verdict": b3_verdict,
                    "systemically_failing": b3_systemically_failing,
                    "consecutive_failures": b3_consecutive_failures,
                    "systemic_threshold": B3_SYSTEMIC_FAILURE_THRESHOLD,
                    "total_failures": b3_total_failures,
                    "total_successes": b3_total_successes,
                    "error_rate": b3_error_rate,
                    "last_error": b3_last_error,
                    "last_error_count": b3_last_error_count,
                    "last_error_last_seen_ms": b3_last_error_last_seen_ms,
                    "recovery_hint": b3_recovery_hint,
                },
                // DEC-AXO-901626 — observed compute verdict (canonical, same
                // source as status.embedder_runtime + the dashboard).
                "compute": observed_compute,
                "compute_source": observed_compute_source,
                // REQ-AXO-902345 residual — silent GPU→CPU fallback made loud.
                "effective_embed_provider": effective_embed_provider,
                "provider_compute_mismatch": provider_compute_mismatch,
                "indexer_build_id": indexer_build_id,
                // REQ-AXO-902198 residual — process-global count of rows dropped by the
                // bulk-writer's poison-row bisection (chunks/symbols/edges/indexed_files/
                // chunk_embeddings) since process start. A drop is a SILENT recovery (the
                // batch lands, the drain never freezes) — this is the operator-visible
                // counterpart to the `log::warn!` line each bisection emits.
                "poison_rows_dropped": crate::postgres::bulk_writer::poison_rows_dropped_count(),
                // REQ-AXO-901893 (LEGACY FEED PURGE) — `pipeline_drain` +
                // `periodic_sweep` telemetry blocks were ripped with the
                // ingress_buffer. The Watchman file source feeds pipeline A
                // directly (no buffer to meter); DBQ-A is the backlog drainer
                // (see `stock_a` / discovered-backlog above).
            }
        }))
    }

    /// CPT-AXO-90052 — upsert the normalized shape of a `sql` query into the
    /// hourly rollup. Best-effort + content-free: a telemetry write failure
    /// (e.g. table not yet created on an old runtime) never affects the response.
    fn record_sql_shape(&self, sql: &str, status: &str, latency_ms: i64) {
        let shape: String = normalize_sql_shape(sql).chars().take(2000).collect();
        if shape.is_empty() {
            return;
        }
        let hash = sql_shape_hash(&shape);
        let lm = latency_ms.max(0);
        let _ = self.graph_store.execute_param(
            "INSERT INTO axon.sql_shape_stat (shape_hash, shape, project_code, status, bucket_hour, call_count, latency_sum_ms, latency_max_ms)
             VALUES (?, ?, '', ?, date_trunc('hour', now()), 1, ?, ?)
             ON CONFLICT (shape_hash, status, bucket_hour)
             DO UPDATE SET call_count = axon.sql_shape_stat.call_count + 1,
                           latency_sum_ms = axon.sql_shape_stat.latency_sum_ms + EXCLUDED.latency_sum_ms,
                           latency_max_ms = greatest(axon.sql_shape_stat.latency_max_ms, EXCLUDED.latency_max_ms)",
            &json!([hash, shape, status, lm, lm]),
        );
    }

    /// REQ-AXO-902621 (suite) — BORNER `sql`, la surface la plus lourde de tout le
    /// serveur, et de deux ordres de grandeur.
    ///
    /// Mesure du 2026-09-05 sur `axon.mcp_call_stat` : **515 Mo rendus sur 135 507
    /// appels, dont UN de 13,7 Mo**. `status` vient loin derrière (17 Mo), et
    /// `batch` — que le plan désignait comme le prochain chantier — pèse 38 Ko en
    /// tout. La thèse « borner `batch` » est réfutée par la mesure ; c'est `sql`
    /// qu'il fallait borner.
    ///
    /// Une réponse de 13,7 Mo est REFUSÉE par le client avant d'être lue :
    /// l'appelant paie le calcul, le transport et l'attente, et n'obtient rien.
    /// Borner domine strictement.
    ///
    /// Ce que cette fonction ne fait PAS : effacer en silence. Le nombre de lignes
    /// TOTAL reste `row_count` — il est compté sur la sortie entière, pas sur ce qui
    /// est rendu — et le texte dit combien de lignes il porte et comment obtenir les
    /// suivantes. `REQ-AXO-902409` interdit l'inverse.
    ///
    /// Retour : `(texte, lignes_rendues, tronqué)`. `lignes_rendues` vaut `Some(n)`
    /// UNIQUEMENT quand la coupe a porté sur des lignes délimitées ; il vaut `None`
    /// quand la borne n'a pas mordu ET quand elle a mordu sur une sortie qu'on n'a
    /// pas su découper. Ces deux `None` se distinguent par le troisième membre — un
    /// entier ne le pourrait pas, et `0` y voudrait dire deux choses opposées.
    pub(crate) fn borner_lignes_sql(
        resultat: &str,
        seuil: usize,
    ) -> (String, Option<usize>, bool) {
        if resultat.chars().count() <= seuil {
            return (resultat.to_string(), None, false);
        }
        // `RawValue` DÉLIMITE les lignes sans désérialiser leur contenu : la coupe
        // se fait sur des lignes entières, jamais au milieu d'un objet JSON.
        let Ok(lignes) = serde_json::from_str::<Vec<&serde_json::value::RawValue>>(resultat) else {
            // Sortie non délimitable. La borner aux caractères produirait du JSON
            // invalide, ce qui est pire qu'un texte long : on rend donc un texte
            // PLAT, annoncé comme tel, plutôt qu'un tableau cassé.
            let tete: String = resultat.chars().take(seuil).collect();
            return (
                format!(
                    "{tete}\n\n… sortie tronquée à {seuil} caractères (elle n'a pas pu être \
                     délimitée en lignes, donc elle est rendue TELLE QUELLE et coupée ; le JSON \
                     ci-dessus est incomplet). Réduisez la requête — `LIMIT`, moins de colonnes."
                ),
                // `None`, jamais `0` : sur ce chemin on N'A PAS su compter les lignes,
                // et un `rows_rendered: 0` se lirait comme « aucune ligne rendue » alors
                // que du texte EST rendu. Deux sens dans un même entier, c'est la
                // confusion que `ok_empty` existe déjà pour éviter (REQ-AXO-902583).
                None,
                true,
            );
        };
        let total = lignes.len();
        let mut rendues = 0usize;
        let mut poids = 2usize; // les crochets
        for ligne in &lignes {
            let taille = ligne.get().chars().count() + 1; // + la virgule
            if rendues > 0 && poids + taille > seuil {
                break;
            }
            poids += taille;
            rendues += 1;
        }
        // Au moins UNE ligne, même énorme : rendre zéro ligne se lirait comme un
        // résultat vide, et c'est exactement la confusion que `ok_empty` existe pour
        // éviter.
        let rendues = rendues.max(1).min(total);
        let corps: Vec<&serde_json::value::RawValue> = lignes.into_iter().take(rendues).collect();
        let json_rendu = serde_json::to_string(&corps).unwrap_or_else(|_| "[]".to_string());
        (
            format!(
                "{json_rendu}\n\nStatus: ok_truncated — {rendues} ligne(s) rendue(s) sur \
                 {total}. La requête a tourné ENTIÈREMENT ; c'est la RESTITUTION qui est bornée \
                 à {seuil} caractères, parce qu'une réponse plus longue est refusée par le \
                 client avant d'être lue. Pour la suite : ajoutez `LIMIT {rendues} OFFSET \
                 {rendues}`, ou sélectionnez moins de colonnes. `row_count` ci-dessous porte \
                 le total, pas le rendu."
            ),
            Some(rendues),
            true,
        )
    }

    /// REQ-AXO-902621 (suite) — le STATUT que le client lit, en fonction de la coupe.
    ///
    /// Sortie ici plutôt qu'en ligne dans `axon_sql` : ce mapping est la seule chose
    /// qui rende `rows_rendered` interprétable, et `axon_sql` n'est pas exerçable
    /// sans base. Le triplet que le client reçoit doit se lire sans ambiguïté :
    ///
    /// | `status` | `row_count` | `rows_rendered` | sens |
    /// |---|---|---|---|
    /// | `ok` / `ok_empty` / `ok_uncounted` | total ou `null` | `null` | rien n'a été coupé |
    /// | `ok_truncated` | total | `n` | `n` lignes rendues sur le total |
    /// | `ok_truncated_undelimited` | `null` | `null` | coupé à plat, non comptable |
    pub(crate) fn statut_apres_borne<'a>(
        status: &'a str,
        lignes_rendues: Option<usize>,
        tronque: bool,
    ) -> &'a str {
        match (tronque, lignes_rendues) {
            (false, _) => status,
            (true, Some(_)) => "ok_truncated",
            (true, None) => "ok_truncated_undelimited",
        }
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

        // CPT-AXO-90052 — record the NORMALIZED query shape (no literal values)
        // so recurring raw-SQL patterns can be mined and promoted to commands.
        // Best-effort: a telemetry write never affects the tool response.
        let _sql_t0 = std::time::Instant::now();
        let outcome = self.graph_store.query_json(q);
        let _sql_latency_ms = _sql_t0.elapsed().as_millis() as i64;
        self.record_sql_shape(
            q,
            if outcome.is_ok() { "ok" } else { "error" },
            _sql_latency_ms,
        );
        match outcome {
            Ok(result) => {
                // REQ-AXO-901949 inv.5 — auto-continue: surface the valid next
                // moves from the single-source tool_routing record.
                let next = super::tool_contracts::next_links("sql");
                // REQ-AXO-902583 — COMPTER, au lieu de rendre une enveloppe qu'on
                // peut lire comme « vide ». Avant, zéro ligne rendait le texte `[]`
                // et un `data` qui ne portait que `next` : un appelant programmatique
                // ne pouvait pas distinguer « la requête a tourné et n'a rien trouvé »
                // de « il ne s'est rien passé », et le silence le désignait comme
                // fautif — il reformulait, et payait deux fois (NEX, CSAT 2026-08-31).
                //
                // COÛT, dit franchement : un second passage sur la sortie déjà
                // sérialisée. Il est borné par `RawValue`, qui DÉLIMITE les lignes
                // sans désérialiser leur contenu — pas d'allocation par cellule. Le
                // cas vide court-circuite même ce passage. `sql` est une surface
                // « advanced read », jamais un chemin chaud RAM-first.
                let row_count: Option<usize> = if result.trim() == "[]" {
                    Some(0)
                } else {
                    serde_json::from_str::<Vec<&serde_json::value::RawValue>>(&result)
                        .ok()
                        .map(|rows| rows.len())
                };
                // Une sortie qu'on n'a pas su délimiter ne se compte PAS à zéro :
                // fabriquer un 0 la rendrait identique au résultat vide.
                let status = match row_count {
                    Some(0) => "ok_empty",
                    Some(_) => "ok",
                    None => "ok_uncounted",
                };
                let texte = if result.trim() == "[]" && ql.contains("match") {
                    "[]\n\nStatus: ok_empty — 0 ligne. La requête a bien tourné.\nHint: Cypher-style query detected. `sql` is read-only SQL over canonical tables; multi-hop CALLS traversal is NOT done in SQL (REQ-AXO-901952 retired the `ist.path` PG functions — graph traversal is RAM-only now). Use the structural tools `path`, `impact`, `bidi_trace` or `query` instead.".to_string()
                } else if row_count == Some(0) {
                    // Le vide se DIT dans le texte aussi : beaucoup de clients ne
                    // rendent que `content[0].text` (REQ-AXO-901949 inv.2).
                    "[]\n\nStatus: ok_empty — 0 ligne. La requête a bien tourné ; \
                     c'est le prédicat qui ne ramène rien, pas l'outil qui s'est tu."
                        .to_string()
                } else {
                    result
                };
                // REQ-AXO-902621 (suite) — la borne, posée APRÈS le comptage :
                // `row_count` reste le total réel, et le texte dit ce qu'il porte.
                let (texte, lignes_rendues, tronque) =
                    Self::borner_lignes_sql(&texte, SEUIL_RENDU_SQL_CHARS);
                // Deux coupes, deux statuts. Sur une sortie qu'on n'a pas su délimiter,
                // le texte rendu est PLAT et incomplet : l'annoncer `ok_truncated` avec
                // `rows_rendered: 0` ferait lire « bornée à zéro ligne » là où il faut
                // lire « bornée, et pas comptable ». `row_count` y vaut déjà `null`.
                let status = Self::statut_apres_borne(status, lignes_rendues, tronque);
                Some(json!({
                    "content": [{ "type": "text", "text": texte }],
                    "data": {
                        "next": next,
                        "row_count": row_count,
                        "status": status,
                        // Dits SEULEMENT quand la borne a mordu : les poser toujours
                        // ferait payer deux champs à 135 000 appels pour rien.
                        "rows_rendered": match lignes_rendues {
                            Some(n) => json!(n),
                            None => Value::Null,
                        },
                        "truncated": if tronque { json!(true) } else { Value::Null }
                    }
                }))
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
                // REQ-AXO-902323 — `pg_error_repair` has ALREADY classified this
                // error precisely (`undefined_column` / `undefined_table`), and the
                // class is rendered in the text above. Hardcoding `input_invalid`
                // here threw that class away for the one consumer that needs it: the
                // friction signature keys on `(project, tool, problem_class, field)`,
                // and `sql` never carries a `field`. So EVERY input error on the
                // most-called tool collapsed into ONE signature — a wrong table name,
                // a wrong column name and a genuine contract defect all bumping the
                // same counter, and any caller typo "regressing" a signature that was
                // legitimately resolved. Observed 2026-08-15 on signature #3187.
                //
                // Computed, rendered, then discarded before the surface that decides
                // rollout priorities. Same shape as REQ-AXO-902244.
                let problem_class = repair
                    .as_ref()
                    .and_then(|r| r.get("problem_class"))
                    .and_then(Value::as_str)
                    .unwrap_or("input_invalid");
                Some(json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "operator_guidance": {
                            "problem_class": problem_class,
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
            // REQ-AXO-902434 — ce site ne rabotait QUE `axon_` : un nom en
            // `mcp_axon_` marchait par la voie directe et echouait par `batch`.
            let normalized_tool_name = super::catalog::canonical_tool_name(tool_name);
            let tool_args = call.get("args").cloned().unwrap_or_else(|| json!({}));

            // REQ-AXO-901925 — route through the canonical dispatcher so EVERY
            // tool is reachable from batch, not just query/inspect/impact. The
            // old hardcoded 3-tool match returned `_ => None`, silently dropping
            // every other tool and yielding `[]` (e.g. status + embedding_status).
            // REQ-AXO-902583 — le diagnostic `unknown_tool` était INATTEIGNABLE par
            // sa cause déclarée, et faux quand il tirait : un nom d'outil inconnu
            // rend `Some({"Tool not found", isError: true})`, jamais `None`. Un
            // `None` ne peut donc venir que d'un handler RECONNU. Dire à l'appelant
            // que son outil n'existe pas l'envoie corriger un nom correct — la forme
            // REQ-AXO-902584, une affirmation positive fausse.
            let res = self
                .execute_tool_direct(normalized_tool_name, &tool_args)
                .unwrap_or_else(|| {
                    json!({
                        "content": [{ "type": "text", "text": format!(
                            "`{tool_name}` n'a rendu aucune enveloppe. Le NOM de l'outil \
                             est correct — l'outil a refusé ces arguments, ou il n'est pas \
                             servi dans ce mode runtime. `help` liste la surface réellement \
                             servie et le contrat de cet outil."
                        ) }],
                        "isError": true,
                        "data": { "status": "handler_returned_no_envelope", "tool": tool_name }
                    })
                });
            all_results.push(json!({
                "name": tool_name,
                "result": res
            }));
        }

        // REQ-AXO-902479 (doléance OPV #259) — dédupliquer AU NIVEAU DU LOT.
        //
        // Mesuré : **58 Ko pour 25 mutations triviales**, au-delà du cap de sortie —
        // donc deux allers-retours de PLUS qu'une simple boucle d'appels unitaires.
        // Un outil censé économiser des appels en coûtait davantage.
        //
        // La cause n'est pas le nombre d'appels, c'est la RÉPÉTITION : chaque mutation
        // SOLL rend un `mutation_feedback` dont `remaining_blockers` porte 49 ids —
        // les MÊMES 49 à chaque fois. Sur 25 appels, ces ids sont écrits 1 225 fois
        // pour une information qui vaut d'être écrite une fois.
        //
        // ⚠️ Rien n'est tronqué : la valeur est DÉPLACÉE en tête du lot, pas supprimée
        // (troncage silencieux interdit — REQ-AXO-902409). Et elle n'est factorisée
        // que si elle est STRICTEMENT identique partout : deux lots différents ne
        // peuvent pas être confondus.
        let commun = Self::facteur_commun_du_lot(&mut all_results);

        // REQ-AXO-902583 — RENDRE UN VERDICT, et le rendre dans les DEUX canaux.
        //
        // `axon_batch` ne posait jamais de clé `data`. Conséquence mécanique dans
        // `attach_default_tool_guidance` : `structuredContent` valait `{}` sur TOUTE
        // réponse, quel que soit le nombre de résultats. Pour un client qui ne lit
        // que ce champ — le champ canonique du protocole — un lot se lisait comme
        // une enveloppe vide, sans erreur. C'est le défaut rapporté par NEX
        // (« 4 appels soll_get sans résultat ni erreur, les mêmes un par un
        // fonctionnent »), et c'est la classe que REQ-AXO-902560 a fermée sur `sql`,
        // `fs_read` et `diagnose_indexing` sans jamais passer par ici.
        //
        // On pose `data`, JAMAIS `structuredContent` : la branche `None` du
        // dispatcher le recopie, et comme `results` n'est pas une clé de guidance
        // aucun miroir `rendered_text` n'est ajouté. La charge est donc écrite
        // exactement deux fois. La poser soi-même l'écrirait trois fois.
        let echecs: Vec<String> = all_results
            .iter()
            .filter(|r| {
                r.get("error").is_some()
                    || r.pointer("/result/isError") == Some(&Value::Bool(true))
            })
            .filter_map(|r| r.get("name").and_then(Value::as_str).map(str::to_string))
            .collect();
        let statut = match (all_results.len(), echecs.len()) {
            (0, _) => "ok_empty",
            (n, e) if e == n => "error",
            (_, 0) => "ok",
            _ => "partial",
        };

        // `results` est TOUJOURS un tableau. Avant, le texte était un tableau nu ou
        // un objet selon que la déduplication ci-dessus avait mordu : la forme rendue
        // dépendait des données, si bien qu'un lot de lectures et un lot de mutations
        // ne se lisaient pas pareil.
        let mut charge = serde_json::Map::new();
        charge.insert("status".into(), json!(statut));
        charge.insert("call_count".into(), json!(all_results.len()));
        charge.insert("failed_count".into(), json!(echecs.len()));
        if !echecs.is_empty() {
            charge.insert("failed_calls".into(), json!(echecs));
        }
        if commun != json!({}) {
            charge.insert("contexte_commun_du_lot".into(), commun);
            charge.insert(
                "note".into(),
                json!("ces champs étaient IDENTIQUES dans tous les résultats : \
                       écrits une fois ici, retirés de chaque résultat. Rien n'est tronqué."),
            );
        }
        charge.insert("results".into(), json!(all_results));
        let charge = Value::Object(charge);

        Some(json!({
            "content": [{ "type": "text", "text": serde_json::to_string(&charge).unwrap_or_default() }],
            "data": charge
        }))
    }

    /// REQ-AXO-902479 — sort du lot les champs de contexte identiques partout.
    ///
    /// Fermé volontairement à une liste de clés : factoriser TOUT champ identique
    /// ferait disparaître des valeurs qui se trouvent coïncider (deux `status: "ok"`),
    /// et un lecteur ne saurait plus si un résultat porte la valeur ou l'a héritée.
    /// Ces quatre clés-là sont du contexte de PROJET, pas du résultat d'appel : elles
    /// décrivent l'état du monde après la mutation, identique par construction.
    fn facteur_commun_du_lot(resultats: &mut [Value]) -> Value {
        const CLES: [&str; 4] = [
            "remaining_blockers",
            "next_best_actions",
            "completeness_before",
            "completeness_after",
        ];
        // Il faut au moins deux résultats pour qu'une répétition existe.
        if resultats.len() < 2 {
            return json!({});
        }
        let feedbacks = |v: &Value| -> Option<Value> {
            v.get("result")
                .and_then(|r| r.get("data"))
                .and_then(|d| d.get("mutation_feedback"))
                .cloned()
        };
        let mut commun = serde_json::Map::new();
        for cle in CLES {
            let mut valeurs = resultats.iter().map(|r| {
                feedbacks(r)
                    .and_then(|f| f.get(cle).cloned())
                    .unwrap_or(Value::Null)
            });
            let Some(premiere) = valeurs.next() else {
                continue;
            };
            // Un `null` partout n'est pas un facteur commun, c'est une absence.
            if premiere.is_null() || !valeurs.all(|v| v == premiere) {
                continue;
            }
            commun.insert(cle.to_string(), premiere);
        }
        if commun.is_empty() {
            return json!({});
        }
        for r in resultats.iter_mut() {
            if let Some(f) = r
                .get_mut("result")
                .and_then(|r| r.get_mut("data"))
                .and_then(|d| d.get_mut("mutation_feedback"))
                .and_then(|f| f.as_object_mut())
            {
                for cle in CLES {
                    if commun.contains_key(cle) {
                        f.remove(cle);
                    }
                }
                f.insert(
                    "voir".to_string(),
                    Value::from("contexte_commun_du_lot (champs identiques factorisés)"),
                );
            }
        }
        Value::Object(commun)
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
        let project_path = match self.lookup_project_path(&project_code) {
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
        let (cache_invalidation, invalidated_rows) = if full {
            self.rescan_wipe_indexed_files(&project_code, &project_path)
        } else {
            ("skipped (delta mode)".to_string(), Some(0))
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
             **invalidated_rows:** {invalidated_rows_display}\n\
             **cache_invalidation:** {cache_invalidation}\n\
             **notify_outcome:** {notify_outcome}\n\n\
             Re-scan triggered via `axon_registry_changed` NOTIFY ; \
             the indexer's `record_subtree_hint` consumer (REQ-AXO-901675) \
             will pick the work up asynchronously. If the indexer is not \
             running, start it via `./scripts/axon-{{live,dev}} start \
             --indexer-graph` and the next boot will replay IndexedFile from \
             PG before scanning.",
            project_path_display = project_path,
            invalidated_rows_display = invalidated_rows
                .map(|count| count.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
        );
        let structured = json!({
            "status": "ok",
            "project_code": project_code,
            "project_path": project_path,
            "mode": mode_label,
            "full": full,
            "files_scheduled": files_scheduled,
            "projection_eta_ms": projection_eta_ms,
            "invalidated_rows": invalidated_rows,
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
    /// Resolve a project's canonical absolute path from `soll.ProjectCodeRegistry`.
    /// Shared by `rescan_project` and `data_catalog` (REQ-AXO-902017). Returns
    /// `None` when the code is unknown or its registry path is empty.
    pub(crate) fn lookup_project_path(&self, project_code: &str) -> Option<String> {
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

    /// Internal helper — wipe only IndexedFile rows owned by `project_code`
    /// so a broad registry path cannot invalidate a more-specific tenant.
    /// The path remains the cache-invalidation signal consumed by the indexer.
    /// Returns a human-readable status and the exact deleted-row count. Failure is non-fatal : the NOTIFY
    /// still fires and the indexer will at minimum re-touch
    /// `last_seen_ms` on next pass.
    /// REQ-AXO-902262 — wipe the PG rows AND tell the indexer to forget them.
    ///
    /// Wiping PG alone was never enough, and the old `cache_invalidation: "wiped"` said
    /// otherwise. The cache that decides whether a file is re-read is `IndexedFileCache`,
    /// a DashMap in the INDEXER's RAM hydrated once at boot; this tool runs in the BRAIN.
    /// So `full=true` deleted a project's chunks, the walk re-enrolled the rows with the
    /// same on-disk mtime/size, A1 asked the untouched RAM cache, got "unchanged", and
    /// skipped — permanently. Measured on LLL: 434/434 files chunked → 2/438, with the
    /// 15-minute reconciliation walk replaying the same skip forever and `status: ok`
    /// throughout. The only recovery was an indexer restart (a GPU teardown), which the
    /// tool never mentioned.
    ///
    /// `pg_notify` crosses the process boundary; the indexer's `ist_cache_invalidate`
    /// listener calls `forget_prefix`. Same mechanism as REQ-AXO-902234's idle-drop
    /// control, and no restart is required.
    ///
    /// The NOTIFY is best-effort AND its outcome is REPORTED: if the cache could not be
    /// signalled, the caller is told the re-index will not happen on its own rather than
    /// being handed a success that means nothing.
    fn rescan_wipe_indexed_files(
        &self,
        project_code: &str,
        project_path: &str,
    ) -> (String, Option<usize>) {
        let escaped_code = project_code.replace('\'', "''");
        let escaped = project_path.replace('\'', "''");
        let sql = format!(
            "WITH deleted AS (\
                 DELETE FROM ist.IndexedFile \
                 WHERE project_code = '{}' \
                 RETURNING 1\
             ) SELECT count(*) FROM deleted",
            escaped_code
        );
        let invalidated_rows = match self.graph_store.execute_raw_sql_gateway(&sql) {
            Ok(raw) => serde_json::from_str::<Vec<Vec<Value>>>(&raw)
                .ok()
                .and_then(|rows| rows.first().and_then(|row| row.first()).cloned())
                .and_then(|value| {
                    value
                        .as_u64()
                        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
                })
                .map(|count| count as usize),
            Err(err) => return (format!("wipe_failed: {err}"), None),
        };
        let notify_sql = format!(
            "SELECT pg_notify('{}', '{}')",
            crate::pipeline::cache_invalidate_listener::LISTEN_CHANNEL,
            escaped
        );
        match self.graph_store.execute_raw_sql_gateway(&notify_sql) {
            Ok(_) => (
                "wiped by project_code (full mode) + indexer dedup-cache invalidated".to_string(),
                invalidated_rows,
            ),
            Err(err) => (
                format!(
                    "wiped (full mode) BUT cache-invalidate NOTIFY failed ({err}) — the indexer \
                     will keep skipping these files as unchanged; restart axon-indexer to force \
                     the re-index (REQ-AXO-902262)"
                ),
                invalidated_rows,
            ),
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
    /// ist.IndexedFile with status='discovered'.
    ///
    /// REQ-AXO-902262 — the previous sentence here claimed "the DBQ-A claim feeder
    /// (REQ-AXO-901897) drains those rows into pipeline A **by construction** — no indexer
    /// restart". That feeder DOES NOT EXIST: REQ-AXO-901916 (PIL-AXO-007) "replaces the
    /// claim-feeder + status='discovered' machine ENTIRELY" with a direct-streaming walk
    /// that pushes paths into pipeline A's input_tx. Nothing anywhere SELECTs
    /// status='discovered' (verified). So the column is dead debt, and this tool's
    /// re-index relied on a component that had been removed — which is exactly why
    /// full=true destroyed LLL's chunks without rebuilding them. The re-index now happens
    /// because the indexer's reconciliation walk re-reads the files, and it re-reads them
    /// because `rescan_wipe_indexed_files` invalidates the RAM dedup cache. `full` is informational here
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

    /// REQ-AXO-165 (+ absorbs REQ-AXO-161 writer-lock / build-info drift) —
    /// read-only filesystem health for an instance: the IST/SOLL writer locks
    /// (ORPHAN detection — a lock whose owner pid is dead blocks the next writer)
    /// and build-info drift (binary newer than its recorded identity). Scope is
    /// deliberately the launcher-AGNOSTIC artifacts: pid-file and socket paths
    /// diverge between the axonctl layout and the live process-compose runtime
    /// (HTTP :44129, `.axon/live-run/*.pid`), so process liveness stays with
    /// `status` / `mcp_surface_diagnostics`. Writer-lock + build-info are the
    /// artifacts an operator can't read at a glance — exactly REQ-AXO-161's intent.
    pub(crate) fn axon_runtime_filesystem_health(&self, args: &Value) -> Option<Value> {
        use std::path::{Path, PathBuf};
        let instance = args.get("instance").and_then(|v| v.as_str()).unwrap_or("live");
        let role = args.get("role").and_then(|v| v.as_str()).unwrap_or("brain");
        let instance_dir = if instance == "dev" { ".axon-dev" } else { ".axon" };
        let binary = if role == "indexer" { "axon-indexer" } else { "axon-brain" };
        let root = std::env::var("AXON_PROJECT_ROOT")
            .ok()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let db_root = root.join(instance_dir).join("graph_v2");

        let pid_alive = |pid: i64| Path::new(&format!("/proc/{pid}")).exists();
        // Writer locks use the runtime_writer_guard.rs format:
        // "owner=<identity>;pid=<N>" on the owner line (cf. axonctl::parse_lock_file_pid).
        let lock_owner = |p: &Path| -> Option<(String, i64)> {
            let content = std::fs::read_to_string(p).ok()?;
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("owner=") {
                    if let Some(pid_part) = rest.split(";pid=").nth(1) {
                        if let Ok(pid) = pid_part.trim().parse::<i64>() {
                            let identity = rest.split(";pid=").next().unwrap_or("").to_string();
                            return Some((identity, pid));
                        }
                    }
                }
            }
            None
        };

        let mut artifacts: Vec<Value> = Vec::new();
        let mut issues = 0usize;

        // IST + SOLL writer locks — orphan detection.
        for (label, lock) in [
            ("ist_writer_lock", db_root.join(".axon-ist.writer.lock")),
            ("soll_writer_lock", db_root.join(".axon-soll.writer.lock")),
        ] {
            let (present, status, detail) = if lock.exists() {
                match lock_owner(&lock) {
                    Some((id, pid)) if pid_alive(pid) => {
                        (true, "ok", format!("held by live owner {id} (pid {pid})"))
                    }
                    Some((id, pid)) => {
                        issues += 1;
                        (true, "stale", format!("ORPHAN lock — owner {id} (pid {pid}) is dead; next writer will block"))
                    }
                    None => {
                        issues += 1;
                        (true, "unknown", "lock present but owner/pid not parseable".to_string())
                    }
                }
            } else {
                (false, "absent", "no writer lock held (writer idle or not this instance)".to_string())
            };
            artifacts.push(json!({
                "artifact": label, "path": lock.to_string_lossy(),
                "present": present, "status": status, "detail": detail
            }));
        }

        // build-info drift (live binaries live in bin/; dev runs from cargo-target).
        let build_info = root.join("bin").join(format!("{binary}.build-info"));
        let binary_path = root.join("bin").join(binary);
        let (present, status, detail) = if build_info.exists() {
            let stale = matches!(
                (
                    std::fs::metadata(&build_info).and_then(|m| m.modified()),
                    std::fs::metadata(&binary_path).and_then(|m| m.modified())
                ),
                (Ok(bi), Ok(bin)) if bin > bi
            );
            if stale {
                issues += 1;
                (true, "stale", "binary mtime newer than build-info — identity drift".to_string())
            } else {
                (true, "ok", "build-info current".to_string())
            }
        } else {
            (false, "absent", format!("no bin/{binary}.build-info (dev binary lives in cargo-target)"))
        };
        artifacts.push(json!({
            "artifact": "build_info", "path": build_info.to_string_lossy(),
            "present": present, "status": status, "detail": detail
        }));

        // REQ-AXO-902378 — the text used to report `{issues} issue(s)` and leave the
        // NATURE of each one in `data.*`, which the Claude Code client does not
        // expose. APS finished a full diagnostic session without ever learning what
        // the single reported filesystem issue WAS (inbox 11933). A count that says
        // "there is something to know" and withholds what it is, is worse than
        // silence: it is a dead end (PIL-AXO-002).
        let issue_lines: Vec<String> = artifacts
            .iter()
            .filter(|a| {
                !matches!(
                    a.get("status").and_then(|v| v.as_str()),
                    Some("ok") | Some("absent")
                )
            })
            .map(|a| {
                format!(
                    "  - **{}** ({}): {}\n    `{}`",
                    a.get("artifact").and_then(|v| v.as_str()).unwrap_or("?"),
                    a.get("status").and_then(|v| v.as_str()).unwrap_or("?"),
                    a.get("detail").and_then(|v| v.as_str()).unwrap_or("(no detail)"),
                    a.get("path").and_then(|v| v.as_str()).unwrap_or("?"),
                )
            })
            .collect();
        let text = if issue_lines.is_empty() {
            format!(
                "Filesystem health {instance}/{role}: {} artefact(s) inspecté(s), **aucun problème**.",
                artifacts.len()
            )
        } else {
            format!(
                "Filesystem health {instance}/{role}: {} artefact(s) inspecté(s), **{issues} problème(s)** :\n\n{}",
                artifacts.len(),
                issue_lines.join("\n")
            )
        };

        Some(json!({
            "content": [{ "type": "text", "text": text }],
            "data": {
                "status": "ok",
                "instance": instance,
                "role": role,
                "issues": issues,
                // The denominator travels with the count (REQ-AXO-902384).
                "artifacts_inspected": artifacts.len(),
                "artifacts": artifacts,
                "scope_note": "launcher-agnostic FS artifacts only (writer locks + build-info); process/socket liveness via status / mcp_surface_diagnostics",
                "follow_up_tools": ["status", "truth_check"]
            }
        }))
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

#[cfg(test)]
mod provider_compute_mismatch_tests {
    //! REQ-AXO-902345 residual — the silent GPU→CPU fallback detector. Cases
    //! anchored on the REAL 2026-08-17 defect: effective `cuda`, worker `CPU`.
    use super::embed_provider_compute_mismatch as m;

    #[test]
    fn resolved_gpu_provider_on_cpu_worker_is_a_mismatch() {
        // The 2026-08-17 defect: CUDA EP failed to load, worker fell to CPU,
        // compute=CPU reported under HEALTHY. This MUST flag.
        assert!(m("cuda", "CPU"));
        assert!(m("tensorrt", "CPU"));
        assert!(m("cuda", "unknown")); // not-GPU of any spelling flags
    }

    #[test]
    fn matching_states_are_not_a_mismatch() {
        assert!(!m("cuda", "GPU")); // healthy GPU path
        assert!(!m("cuda", "gpu")); // case-insensitive
        assert!(!m("cpu", "CPU")); // no GPU intended — consistent
    }

    #[test]
    fn a_cpu_provider_on_gpu_is_not_a_defect() {
        // The GPU being better than the resolved intent is not a silent fallback.
        assert!(!m("cpu", "GPU"));
    }
}

#[cfg(test)]
mod semantic_lane_status_tests {
    use super::semantic_lane_is_blocked;

    #[test]
    fn pending_backlog_and_owner_observed_zero_workers_blocks() {
        assert!(semantic_lane_is_blocked(29_492, Some(0)));
    }

    #[test]
    fn active_worker_or_empty_backlog_does_not_block() {
        assert!(!semantic_lane_is_blocked(29_492, Some(1)));
        assert!(!semantic_lane_is_blocked(0, Some(0)));
    }

    #[test]
    fn absent_owner_truth_is_unknown_not_a_fabricated_worker_failure() {
        assert!(!semantic_lane_is_blocked(29_492, None));
    }
}

#[cfg(test)]
mod sql_shape_tests {
    //! CPT-AXO-90052 — lock the SQL shape normalizer: literal values stripped to
    //! `?` (so structurally-identical queries collapse), identifiers preserved,
    //! never any literal content stored.
    use super::normalize_sql_shape;

    #[test]
    fn strips_string_and_number_literals() {
        let s = normalize_sql_shape(
            "SELECT id FROM ist.Symbol WHERE name='foo' AND project_code = 'AXO' LIMIT 10",
        );
        assert_eq!(
            s,
            "select id from ist.symbol where name=? and project_code = ? limit ?"
        );
    }

    #[test]
    fn same_structure_different_values_collapse_to_one_shape() {
        let a = normalize_sql_shape("SELECT x FROM t WHERE id='abc' AND n=5");
        let b = normalize_sql_shape("SELECT x FROM t WHERE id='zzzzz' AND n=999");
        assert_eq!(a, b);
        assert_eq!(a, "select x from t where id=? and n=?");
    }

    #[test]
    fn preserves_digit_inside_identifier() {
        let s = normalize_sql_shape("SELECT * FROM pipeline WHERE port=44129");
        assert_eq!(s, "select * from pipeline where port=?");
    }
}

#[cfg(test)]
mod facteur_commun_lot_tests {
    use super::McpServer;
    use serde_json::{json, Value};

    fn resultat(id: &str, blockers: Value) -> Value {
        json!({
            "name": "soll_manager",
            "result": { "data": {
                "id": id,
                "mutation_feedback": {
                    "remaining_blockers": blockers,
                    "topology_delta": { "edges": 1 },
                    "next_best_actions": ["rerun soll_work_plan"]
                }
            }}
        })
    }

    /// REQ-AXO-902479 — LA garde : ce qui est identique partout est écrit UNE fois.
    ///
    /// OPV a mesuré 58 Ko pour 25 mutations triviales, au-delà du cap de sortie —
    /// donc DEUX allers-retours de plus qu'une boucle d'appels unitaires. Un outil
    /// censé économiser des appels en coûtait davantage. La cause : 49 ids de
    /// blockers réécrits à chaque résultat.
    #[test]
    fn les_champs_identiques_partout_sortent_du_lot_et_rien_ne_disparait() {
        let blockers = json!(["REQ-A", "REQ-B", "REQ-C"]);
        let mut lot = vec![
            resultat("REQ-1", blockers.clone()),
            resultat("REQ-2", blockers.clone()),
            resultat("REQ-3", blockers.clone()),
        ];
        let commun = McpServer::facteur_commun_du_lot(&mut lot);

        assert_eq!(
            commun.get("remaining_blockers"),
            Some(&blockers),
            "la valeur doit être DÉPLACÉE en tête, pas perdue : {commun}"
        );
        for r in &lot {
            let f = &r["result"]["data"]["mutation_feedback"];
            assert!(
                f.get("remaining_blockers").is_none(),
                "elle ne doit plus être répétée dans chaque résultat : {f}"
            );
            // Ce qui est PROPRE à l'appel reste : c'est la moitié utile.
            assert!(f.get("topology_delta").is_some(), "topology_delta est par appel");
            assert!(f.get("voir").is_some(), "le résultat doit dire où trouver le reste");
        }
        // L'identité de chaque mutation survit intacte.
        assert_eq!(lot[1]["result"]["data"]["id"], json!("REQ-2"));
    }

    /// Contre-exemple : si les valeurs DIFFÈRENT, rien ne bouge. Sans ce test, une
    /// factorisation trop gourmande écraserait des résultats distincts — et le
    /// lecteur ne saurait plus si un résultat porte sa valeur ou l'a héritée.
    #[test]
    fn des_valeurs_differentes_ne_sont_jamais_factorisees() {
        let mut lot = vec![
            resultat("REQ-1", json!(["REQ-A"])),
            resultat("REQ-2", json!(["REQ-B"])),
        ];
        let commun = McpServer::facteur_commun_du_lot(&mut lot);
        assert!(
            commun.get("remaining_blockers").is_none(),
            "deux lots différents ne doivent pas être confondus : {commun}"
        );
        assert_eq!(
            lot[0]["result"]["data"]["mutation_feedback"]["remaining_blockers"],
            json!(["REQ-A"]),
            "chaque résultat garde sa propre valeur"
        );
    }

    /// Un lot d'UN seul appel n'a aucune répétition à factoriser : le coût de la
    /// factorisation doit être nul quand il n'y a rien à gagner.
    #[test]
    fn un_lot_d_un_seul_appel_reste_inchange() {
        let mut lot = vec![resultat("REQ-1", json!(["REQ-A"]))];
        let commun = McpServer::facteur_commun_du_lot(&mut lot);
        assert_eq!(commun, json!({}));
        assert!(lot[0]["result"]["data"]["mutation_feedback"]["remaining_blockers"].is_array());
    }
}
