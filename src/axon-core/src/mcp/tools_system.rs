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
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    /// DEC-AXO-086 slice 2 — read-only embedding backlog snapshot.
    ///
    /// Surfaces the three numbers a caller needs to gauge embedding
    /// freshness without dispatching `diagnose_indexing`: total chunks,
    /// embedded chunks, pending chunks (= total − embedded). Plus the
    /// static pipeline configuration: NOTIFY channel name + cold-start
    /// poll interval (CPT-AXO-054 contract).
    ///
    /// `project` arg optional: when set, scopes the counts to that
    /// `project_code`; `*` (default) is global.
    pub(crate) fn axon_embedding_status(&self, args: &Value) -> Option<Value> {
        let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let (where_chunk, where_emb) = if project == "*" {
            (String::new(), String::new())
        } else {
            let safe = project.replace('\'', "''");
            (
                format!(" WHERE project_code = '{}'", safe),
                format!(" WHERE project_code = '{}'", safe),
            )
        };

        let scalar = |query: &str| -> i64 {
            self.graph_store
                .execute_raw_sql_gateway(query)
                .ok()
                .as_deref()
                .and_then(tools_system_debug::parse_scalar_count_row)
                .unwrap_or(0)
        };

        let total_chunks = scalar(&format!(
            "SELECT count(*) FROM public.Chunk{}",
            where_chunk
        ));
        let embedded_chunks = scalar(&format!(
            "SELECT count(*) FROM public.ChunkEmbedding{}",
            where_emb
        ));
        let pending_chunks = (total_chunks - embedded_chunks).max(0);
        let coverage_pct = if total_chunks > 0 {
            (embedded_chunks as f64 / total_chunks as f64) * 100.0
        } else {
            0.0
        };

        let report = format!(
            "## Embedding Status (project={project})\n\n\
             - Total chunks:    {total_chunks}\n\
             - Embedded chunks: {embedded_chunks}\n\
             - Pending chunks:  {pending_chunks}\n\
             - Coverage:        {coverage_pct:.2}%\n\n\
             ### Pipeline B configuration (DEC-AXO-086 / CPT-AXO-054)\n\
             - NOTIFY channel:        chunk_pending_embed\n\
             - Cold-start poll:       every 30s (LEFT JOIN safety net)\n\
             - A3→B1 try_send buffer: 10 000 (drops rattrapés par cold-start poll)\n\
             - B2 batch size:         64 chunks\n\
             - B2 batch timeout:      200 ms\n\n\
             A backlog > 0 with NOTIFY listener up clears within minutes; \
             a sustained backlog suggests the indexer is stopped or the \
             listener is disconnected — run `diagnose_indexing` for triage."
        );

        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "structuredContent": {
                "project": project,
                "total_chunks": total_chunks,
                "embedded_chunks": embedded_chunks,
                "pending_chunks": pending_chunks,
                "coverage_pct": coverage_pct,
                "notify_channel": "chunk_pending_embed",
                "coldstart_poll_interval_secs": 30,
            }
        }))
    }

    pub(crate) fn axon_sql(&self, args: &Value) -> Option<Value> {
        let sql = args.get("sql")?.as_str()?;
        let q = sql.trim();
        let ql = q.to_ascii_lowercase();

        // REQ-AXO-251: under PG age-only-relations, the SQL relation tables
        // (CALLS / CALLS_NIF) are empty/dropped — bypass the SQL translation
        // and let the AGE Cypher path handle the query natively.
        let skip_legacy_relations = self.graph_store.skip_legacy_relations();
        // Minimal robust support for common multi-hop CALLS checks.
        if !skip_legacy_relations
            && ql.contains("match")
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
                    let calls_count = if skip_legacy_relations {
                        0
                    } else {
                        self.graph_store
                            .query_count("SELECT count(*) FROM CALLS")
                            .unwrap_or(0)
                    };
                    let note = format!(
                        "[]\n\nStatus: warn_empty_result\nHint: Cypher-style query detected. Backend accepts SQL first; for multi-hop CALLS, use `MATCH ... [:CALLS*1..3] ... RETURN count(*)` or `query_examples`.\nCALLS_count={}",
                        calls_count
                    );
                    Some(json!({ "content": [{ "type": "text", "text": note }] }))
                } else {
                    Some(json!({ "content": [{ "type": "text", "text": result }] }))
                }
            }
            Err(e) => {
                let raw = e.to_string();
                // REQ-AXO-139 — universal parameter_repair contract slice for
                // cypher binder errors. DuckDB already emits the candidate
                // column list inside its error text (`Candidate bindings:
                // "X", "Y"`). Surface it as structured `data.parameter_repair`
                // + `data.next_action` so the LLM can fix the column name in
                // one round-trip instead of guessing or re-running schema_overview.
                if let Some((missing_col, candidates)) = parse_duckdb_binder_error(&raw) {
                    let candidates_csv = candidates.join(", ");
                    return Some(json!({
                        "content": [{ "type": "text", "text": format!(
                            "Cypher binder error: column '{}' not found. Candidates: [{}]",
                            missing_col, candidates_csv
                        )}],
                        "isError": true,
                        "data": {
                            "status": "input_invalid",
                            "next_action": {
                                "kind": "fix_column_then_retry",
                                "tool": "sql",
                                "when": "after_replacing_invalid_column"
                            },
                            "operator_guidance": {
                                "problem_class": "input_invalid",
                                "follow_up_tools": ["schema_overview", "list_labels_tables", "query_examples"],
                            },
                            "parameter_repair": {
                                "invalid_field": "sql",
                                "missing_column": missing_col,
                                "available_columns": candidates,
                                "hint": format!(
                                    "Replace '{}' with one of [{}], or run `schema_overview` for the full column list.",
                                    missing_col, candidates_csv
                                )
                            },
                            "diagnostic_excerpt": raw.chars().take(240).collect::<String>()
                        }
                    }));
                }
                Some(json!({
                    "content": [{ "type": "text", "text": format!("Cypher Error: {}", raw) }],
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

/// Parse a DuckDB binder error message into (missing_column, candidate_columns).
///
/// DuckDB's binder errors follow the pattern:
///   `Binder Error: Referenced column "X" not found in FROM clause!  Candidate bindings: "A", "B", "C"`
/// Returns None when the error doesn't match this pattern (REQ-AXO-139 slice).
pub(crate) fn parse_duckdb_binder_error(raw: &str) -> Option<(String, Vec<String>)> {
    let referenced_marker = "Referenced column \"";
    let candidate_marker = "Candidate bindings: ";
    let ref_start = raw.find(referenced_marker)? + referenced_marker.len();
    let ref_end = raw[ref_start..].find('"')?;
    let missing = raw[ref_start..ref_start + ref_end].to_string();
    let cand_start = raw.find(candidate_marker)? + candidate_marker.len();
    // DuckDB appends `LINE N: ...` location markers AFTER the candidate list.
    // Terminate the block at the first newline to avoid swallowing the
    // location pointer into the last candidate (REQ-AXO-139 follow-up:
    // single-candidate edge case where there's no comma to split on).
    let cand_tail = &raw[cand_start..];
    let cand_block = match cand_tail.find('\n') {
        Some(nl) => &cand_tail[..nl],
        None => cand_tail,
    };
    // Split candidates on commas, trim quotes/whitespace.
    let candidates: Vec<String> = cand_block
        .split(',')
        .filter_map(|seg| {
            let trimmed = seg.trim();
            let no_punct = trimmed.trim_end_matches('.').trim();
            let inner = no_punct.trim_matches('"').trim();
            if inner.is_empty() {
                None
            } else {
                Some(inner.to_string())
            }
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    Some((missing, candidates))
}

#[cfg(test)]
mod parse_duckdb_binder_error_tests {
    use super::parse_duckdb_binder_error;

    #[test]
    fn parses_canonical_binder_error() {
        let raw = "DuckDB plugin error: prepare: Binder Error: Referenced column \"callee\" not found in FROM clause!\nCandidate bindings: \"target_id\", \"source_id\", \"project_code\"";
        let (missing, candidates) = parse_duckdb_binder_error(raw).expect("must parse");
        assert_eq!(missing, "callee");
        assert_eq!(candidates, vec!["target_id", "source_id", "project_code"]);
    }

    #[test]
    fn returns_none_for_non_binder_errors() {
        let raw = "DuckDB plugin error: Catalog Error: Table 'Nonexistent' does not exist";
        assert!(parse_duckdb_binder_error(raw).is_none());
    }

    #[test]
    fn handles_single_candidate() {
        let raw = "Binder Error: Referenced column \"foo\" not found in FROM clause! Candidate bindings: \"bar\"";
        let (missing, candidates) = parse_duckdb_binder_error(raw).expect("must parse");
        assert_eq!(missing, "foo");
        assert_eq!(candidates, vec!["bar"]);
    }

    #[test]
    fn ignores_duckdb_line_marker_after_candidates() {
        // DuckDB appends `LINE N: ... \n  ^` location pointers after the
        // candidate list. Earlier parser swallowed the marker into the
        // last candidate when there was no comma to split on.
        let raw = "Binder Error: Referenced column \"callee\" not found in FROM clause!\nCandidate bindings: \"target_id\"\n\nLINE 1: SELECT callee FROM main.CALLS LIMIT 1\n               ^";
        let (missing, candidates) = parse_duckdb_binder_error(raw).expect("must parse");
        assert_eq!(missing, "callee");
        assert_eq!(
            candidates,
            vec!["target_id"],
            "LINE marker must NOT contaminate the candidate"
        );
    }
}
