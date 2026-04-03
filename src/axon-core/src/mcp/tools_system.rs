use serde_json::{json, Value};

use super::format::format_table_from_json;
use super::McpServer;
use crate::ingress_buffer::ingress_metrics_snapshot;
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

impl McpServer {
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
        let file_count = self
            .graph_store
            .query_count("SELECT count(*) FROM File")
            .unwrap_or(0);
        let pending_count = self
            .graph_store
            .query_count("SELECT count(*) FROM File WHERE status = 'pending'")
            .unwrap_or(0);
        let indexing_count = self
            .graph_store
            .query_count("SELECT count(*) FROM File WHERE status = 'indexing'")
            .unwrap_or(0);
        let degraded_count = self
            .graph_store
            .query_count("SELECT count(*) FROM File WHERE status = 'indexed_degraded'")
            .unwrap_or(0);
        let oversized_count = self
            .graph_store
            .query_count("SELECT count(*) FROM File WHERE status = 'oversized_for_current_budget'")
            .unwrap_or(0);
        let skipped_count = self
            .graph_store
            .query_count("SELECT count(*) FROM File WHERE status = 'skipped'")
            .unwrap_or(0);
        let graph_ready_count = self
            .graph_store
            .query_count("SELECT count(*) FROM File WHERE graph_ready = TRUE")
            .unwrap_or(0);
        let vector_ready_count = self
            .graph_store
            .query_count("SELECT count(*) FROM File WHERE vector_ready = TRUE")
            .unwrap_or(0);
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
        let completed_count = (file_count - pending_count - indexing_count).max(0);
        let completion_rate = if file_count > 0 {
            (completed_count as f64 / file_count as f64) * 100.0
        } else {
            0.0
        };
        let symbol_count = self
            .graph_store
            .query_count("SELECT count(*) FROM Symbol")
            .unwrap_or(0);
        let edge_count = self
            .graph_store
            .query_count(
                "SELECT (SELECT count(*) FROM CONTAINS) + (SELECT count(*) FROM CALLS) + (SELECT count(*) FROM CALLS_NIF)",
            )
            .unwrap_or(0);
        let memory = process_memory_snapshot();
        let storage = duckdb_storage_snapshot(&self.graph_store);
        let duckdb_memory = duckdb_memory_snapshot(&self.graph_store);
        let ingress = ingress_metrics_snapshot();
        let stage_rows = self
            .graph_store
            .query_json(
                "SELECT COALESCE(file_stage, 'unknown'), count(*) \
                 FROM File \
                 GROUP BY 1 \
                 ORDER BY count(*) DESC, 1 ASC \
                 LIMIT 6",
            )
            .unwrap_or_else(|_| "[]".to_string());
        let stage_counts = parse_reason_count_rows(&stage_rows);
        let backlog_reason_rows = self
            .graph_store
            .query_json(
                "SELECT COALESCE(status_reason, 'unknown'), count(*) \
                 FROM File \
                 WHERE status IN ('pending', 'indexing') \
                 GROUP BY 1 \
                 ORDER BY count(*) DESC, 1 ASC \
                 LIMIT 5",
            )
            .unwrap_or_else(|_| "[]".to_string());
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

        let report = format!(
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
            ingress.collapsed_total,
            ingress.flush_count,
            ingress.last_flush_duration_ms,
            ingress.last_promoted_count,
        );
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_cypher(&self, args: &Value) -> Option<Value> {
        let cypher = args.get("cypher")?.as_str()?;
        match self.graph_store.query_json(cypher) {
            Ok(result) => Some(json!({ "content": [{ "type": "text", "text": result }] })),
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
            let tool_args = call.get("args")?;

            let res = match tool_name {
                "axon_query" => self.axon_query(tool_args),
                "axon_inspect" => self.axon_inspect(tool_args),
                "axon_impact" => self.axon_impact(tool_args),
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
