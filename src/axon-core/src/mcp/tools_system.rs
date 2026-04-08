use serde_json::{json, Value};

use super::format::{evidence_by_mode, format_standard_contract, format_table_from_json};
use super::McpServer;
use crate::ingress_buffer::ingress_metrics_snapshot;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_observability::{
    duckdb_memory_snapshot, duckdb_storage_snapshot, process_memory_snapshot,
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
        let canonical_json = |query: &str| -> String {
            self.graph_store
                .execute_raw_sql_gateway(query)
                .unwrap_or_else(|_| "[]".to_string())
        };

        let file_count = canonical_count("SELECT count(*) FROM File");
        let pending_count = canonical_count("SELECT count(*) FROM File WHERE status = 'pending'");
        let indexing_count = canonical_count("SELECT count(*) FROM File WHERE status = 'indexing'");
        let degraded_count =
            canonical_count("SELECT count(*) FROM File WHERE status = 'indexed_degraded'");
        let oversized_count =
            canonical_count("SELECT count(*) FROM File WHERE status = 'oversized_for_current_budget'");
        let skipped_count = canonical_count("SELECT count(*) FROM File WHERE status = 'skipped'");
        let graph_ready_count = canonical_count("SELECT count(*) FROM File WHERE graph_ready = TRUE");
        let vector_ready_count = canonical_count(
            "WITH pending_vector_chunks AS ( \
               SELECT co.source_id AS file_path \
               FROM Chunk c \
               JOIN CONTAINS co ON co.target_id = c.source_id \
               LEFT JOIN ChunkEmbedding ce \
                 ON ce.chunk_id = c.id \
                AND ce.model_id = 'chunk-bge-small-en-v1.5-384' \
                AND ce.source_hash = c.content_hash \
               WHERE ce.chunk_id IS NULL OR ce.source_hash IS DISTINCT FROM c.content_hash \
               GROUP BY 1 \
             ) \
             SELECT COUNT(*) \
             FROM File f \
             LEFT JOIN pending_vector_chunks pvc ON pvc.file_path = f.path \
             WHERE f.graph_ready = TRUE AND pvc.file_path IS NULL",
        );
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
        let reader_file_count = self
            .graph_store
            .query_count("SELECT count(*) FROM File")
            .unwrap_or(0);
        let truth_drift_files = (file_count - reader_file_count).abs();
        let completed_count = (file_count - pending_count - indexing_count).max(0);
        let completion_rate = if file_count > 0 {
            (completed_count as f64 / file_count as f64) * 100.0
        } else {
            0.0
        };
        let symbol_count = canonical_count("SELECT count(*) FROM Symbol");
        let edge_count = canonical_count(
            "SELECT (SELECT count(*) FROM CONTAINS) + (SELECT count(*) FROM CALLS) + (SELECT count(*) FROM CALLS_NIF)",
        );
        let memory = process_memory_snapshot();
        let storage = duckdb_storage_snapshot(&self.graph_store);
        let duckdb_memory = duckdb_memory_snapshot(&self.graph_store);
        let ingress = ingress_metrics_snapshot();
        let stage_rows = canonical_json(
            "SELECT COALESCE(file_stage, 'unknown'), count(*) \
             FROM File \
             GROUP BY 1 \
             ORDER BY count(*) DESC, 1 ASC \
             LIMIT 6",
        );
        let stage_counts = parse_reason_count_rows(&stage_rows);
        let backlog_reason_rows = canonical_json(
            "SELECT COALESCE(status_reason, 'unknown'), count(*) \
             FROM File \
             WHERE status IN ('pending', 'indexing') \
             GROUP BY 1 \
             ORDER BY count(*) DESC, 1 ASC \
             LIMIT 5",
        );
        let backlog_reasons = parse_reason_count_rows(&backlog_reason_rows);
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
        );
        if reader_snapshot_age_ms == u64::MAX {
            evidence
                .push_str("\n**Reader Snapshot:** indisponible (mode mémoire ou non initialisé)\n");
        } else {
            evidence.push_str(&format!(
                "\n**Reader Snapshot:** age={} ms, refresh_failures_total={}\n",
                reader_snapshot_age_ms, reader_refresh_failures_total
            ));
        }
        evidence.push_str(&format!(
            "**Truth Drift (File count):** canonical={} vs reader={} (delta={})\n",
            file_count, reader_file_count, truth_drift_files
        ));
        let report = format!(
            "## 🤖 Axon Debug\n\n{}",
            format_standard_contract(
                "ok",
                "runtime diagnostics collected",
                "workspace:*",
                &evidence_by_mode(&evidence, mode),
                &["run `truth_check` to inspect canonical vs reader drift", "run `health` for project-level view"],
                "high",
            )
        );
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_schema_overview(&self, _args: &Value) -> Option<Value> {
        let tables = self
            .graph_store
            .execute_raw_sql_gateway(
                "SELECT table_schema, table_name \
                 FROM information_schema.tables \
                 WHERE table_schema IN ('main', 'soll') \
                 ORDER BY table_schema, table_name",
            )
            .unwrap_or_else(|_| "[]".to_string());
        let columns = self
            .graph_store
            .execute_raw_sql_gateway(
                "SELECT table_schema, table_name, COUNT(*) \
                 FROM information_schema.columns \
                 WHERE table_schema IN ('main', 'soll') \
                 GROUP BY 1,2 \
                 ORDER BY 1,2",
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
            .execute_raw_sql_gateway(
                "SELECT table_name \
                 FROM information_schema.tables \
                 WHERE table_schema = 'main' \
                   AND table_name IN ('File','Symbol','Chunk','CONTAINS','CALLS','CALLS_NIF','IMPACTS','SUBSTANTIATES','GraphEmbedding','GraphProjection','GraphProjectionQueue','FileVectorizationQueue') \
                 ORDER BY table_name",
            )
            .unwrap_or_else(|_| "[]".to_string());
        let cols = self
            .graph_store
            .execute_raw_sql_gateway(
                "SELECT table_name, column_name, data_type \
                 FROM information_schema.columns \
                 WHERE table_schema = 'main' \
                   AND table_name IN ('File','Symbol','CALLS','CALLS_NIF','CONTAINS','GraphEmbedding') \
                 ORDER BY table_name, ordinal_position",
            )
            .unwrap_or_else(|_| "[]".to_string());
        let report = format!(
            "## 🗂️ Labels / Tables Discovery\n\n\
             **Core tables:**\n{}\n\n\
             **Key columns:**\n{}\n",
            format_table_from_json(&rels, &["Table"]),
            format_table_from_json(&cols, &["Table", "Column", "Type"])
        );
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_query_examples(&self, _args: &Value) -> Option<Value> {
        let examples = r#"## 📚 Query Examples (SQL gateway / cypher tool)

1) Workspace status
`SELECT status, count(*) FROM File GROUP BY 1 ORDER BY 2 DESC;`

2) Project health
`SELECT project_slug, count(*) AS known, SUM(CASE WHEN status IN ('indexed','indexed_degraded','skipped','deleted') THEN 1 ELSE 0 END) AS completed FROM File GROUP BY 1 ORDER BY known DESC;`

3) Top backlog reasons
`SELECT COALESCE(status_reason,'unknown'), count(*) FROM File WHERE status IN ('pending','indexing') GROUP BY 1 ORDER BY 2 DESC LIMIT 10;`

4) Parser/ingestion failures
`SELECT COALESCE(last_error_reason,'unknown'), count(*) FROM File WHERE last_error_reason IS NOT NULL GROUP BY 1 ORDER BY 2 DESC;`

5) Inter-language bridge visibility
`SELECT COUNT(*) AS calls, (SELECT COUNT(*) FROM CALLS_NIF) AS calls_nif FROM CALLS;`

6) Symbol lookup by project
`SELECT id, name, kind FROM Symbol WHERE project_slug = 'BookingSystem' ORDER BY name LIMIT 50;`
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
        let reader_count = |query: &str| -> i64 { self.graph_store.query_count(query).unwrap_or(0) };

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
        let status = if drift_count == 0 { "aligned" } else { "drift_detected" };
        let report = format!(
            "## 🧪 Truth Contract Check\n\n\
             **Status:** {}\n\
             **Drifted counters:** {}\n\n\
             {}\n",
            status,
            drift_count,
            format_table_from_json(&table, &["Counter", "Canonical(writer)", "Reader-path", "Delta"])
        );
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_cypher(&self, args: &Value) -> Option<Value> {
        let cypher = args.get("cypher")?.as_str()?;
        let q = cypher.trim();
        let ql = q.to_ascii_lowercase();

        // Minimal robust support for common multi-hop CALLS checks.
        if ql.contains("match")
            && ql.contains("[:calls")
            && ql.contains("return count(*)")
        {
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
