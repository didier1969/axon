use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_operational_profile::AxonRuntimeOperationalProfile;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::catalog::tools_catalog;
use super::format::{evidence_by_mode, format_standard_contract};
use super::McpServer;

type FrameworkCache = HashMap<String, (i64, Value)>;

static ANOMALIES_CACHE: OnceLock<Mutex<FrameworkCache>> = OnceLock::new();
static CONCEPTION_CACHE: OnceLock<Mutex<FrameworkCache>> = OnceLock::new();

const FRAMEWORK_CACHE_TTL_MS: i64 = 5_000;

impl McpServer {
    fn anomalies_cache() -> &'static Mutex<FrameworkCache> {
        ANOMALIES_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn conception_cache() -> &'static Mutex<FrameworkCache> {
        CONCEPTION_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
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

    fn canonical_sources_snapshot() -> Value {
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
                 LEFT JOIN CONTAINS rel ON rel.source_id = f.path
                 WHERE f.project_code = '{}'
                 GROUP BY 1
                 ORDER BY symbol_count DESC, f.path ASC
                 LIMIT 5",
                escaped_project
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
                 LEFT JOIN CONTAINS rel ON rel.target_id = s.id
                 LEFT JOIN File f ON f.path = rel.source_id
                 WHERE s.project_code = '{}'
                   AND s.kind = 'interface'
                 ORDER BY s.name ASC
                 LIMIT 5",
                escaped_project
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
                 LEFT JOIN CONTAINS rel ON rel.target_id = s.id
                 LEFT JOIN File f ON f.path = rel.source_id
                 WHERE s.project_code = '{}'
                   AND COALESCE(s.is_public, false) = true
                   AND s.kind IN ('interface', 'module', 'class', 'struct', 'function', 'method')
                 ORDER BY s.kind ASC, s.name ASC
                 LIMIT 5",
                escaped_project
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
                 JOIN CONTAINS src_rel ON src_rel.target_id = src.id
                 JOIN CONTAINS dst_rel ON dst_rel.target_id = dst.id
                 WHERE src.project_code = '{project}'
                   AND dst.project_code = '{project}'
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
                 JOIN CONTAINS src_rel ON src_rel.target_id = c.source_id
                 JOIN CONTAINS dst_rel ON dst_rel.target_id = c.target_id
                 JOIN Symbol src ON src.id = c.source_id
                 JOIN Symbol dst ON dst.id = c.target_id
                 WHERE src.project_code = '{project}'
                   AND dst.project_code = '{project}'
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
            FRAMEWORK_CACHE_TTL_MS,
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

    fn summarize_why_response(args: &Value, response: &mut Value) {
        let Some(data) = response
            .get_mut("data")
            .and_then(|value| value.as_object_mut())
        else {
            return;
        };
        let planner = data.get("planner").cloned().unwrap_or_else(|| json!({}));
        let packet = data.get("packet").cloned().unwrap_or_else(|| json!({}));
        let relevant_soll_entities = packet
            .get("relevant_soll_entities")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let direct_evidence = packet
            .get("direct_evidence")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let supporting_chunks = packet
            .get("supporting_chunks")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let structural_neighbors = packet
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
        let runtime_mode = AxonRuntimeMode::from_env();
        let runtime_profile = AxonRuntimeOperationalProfile::from_mode_and_strings(
            runtime_mode.as_str(),
            std::env::var("AXON_ENABLE_AUTONOMOUS_INGESTOR")
                .ok()
                .as_deref(),
        );
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
        let queued_files = vector_queue_statuses
            .iter()
            .filter_map(|row| {
                let status = row.get("status")?.as_str()?;
                let count = row.get("count")?.as_u64()?;
                (status == "queued" || status == "paused_for_interactive_priority").then_some(count)
            })
            .sum::<u64>();
        let inflight_files = vector_queue_statuses
            .iter()
            .filter_map(|row| {
                let status = row.get("status")?.as_str()?;
                let count = row.get("count")?.as_u64()?;
                (status == "inflight").then_some(count)
            })
            .sum::<u64>();
        let drain_state = debug_data
            .pointer("/embedding_contract/drain_state")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");

        let job_counts_raw = self
            .graph_store
            .execute_raw_sql_gateway(
                "SELECT status, count(*) FROM soll.McpJob GROUP BY 1 ORDER BY 2 DESC, 1 ASC",
            )
            .unwrap_or_else(|_| "[]".to_string());
        let job_rows: Vec<Vec<Value>> = serde_json::from_str(&job_counts_raw).unwrap_or_default();
        let job_counts = job_rows
            .iter()
            .filter_map(|row| {
                Some(json!({
                    "status": row.first()?.as_str()?.to_string(),
                    "count": row.get(1).and_then(|value| value.as_u64()).unwrap_or(0)
                }))
            })
            .collect::<Vec<_>>();

        let evidence = format!(
            "**Runtime mode:** `{}`\n\
**Runtime profile:** `{}`\n\
**Advanced indexed surfaces visible:** {}\n\
**Vector backlog:** queued={} inflight={}\n\
**Drain state:** `{}`\n\
**Public tools:** {}\n",
            runtime_mode.as_str(),
            runtime_profile.as_str(),
            if public_tool_names.iter().any(|name| *name == "impact") {
                "yes"
            } else {
                "no"
            },
            queued_files,
            inflight_files,
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
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "truth_status": if public_tool_names.iter().any(|name| *name == "impact") {
                    "canonical"
                } else {
                    "degraded"
                },
                "runtime_mode": runtime_mode.as_str(),
                "runtime_profile": runtime_profile.as_str(),
                "drain_state": drain_state,
                "availability": {
                    "advanced_indexed_surfaces_visible": public_tool_names.iter().any(|name| *name == "impact"),
                    "degraded_notes": if public_tool_names.iter().any(|name| *name == "impact") {
                        Vec::<String>::new()
                    } else {
                        vec!["advanced_indexed_surfaces_hidden_for_current_profile".to_string()]
                    }
                },
                "canonical_sources": Self::canonical_sources_snapshot(),
                "file_vectorization_queue": {
                    "queued": queued_files,
                    "inflight": inflight_files
                },
                "public_tools": public_tool_names,
                "job_counts": job_counts,
                "debug_snapshot": debug_data,
                "traceability": debug_data.get("traceability").cloned().unwrap_or_else(|| json!({}))
            }
        }))
    }

    pub(crate) fn axon_project_status(&self, args: &Value) -> Option<Value> {
        let mode = args.get("mode").and_then(|value| value.as_str());
        let project_code = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .unwrap_or("AXO");

        let status = self.axon_status(&json!({ "mode": mode.unwrap_or("brief") }))?;
        let status_data = status.get("data").cloned().unwrap_or_else(|| json!({}));
        let anomalies = self.axon_anomalies(&json!({
            "project": project_code,
            "mode": mode.unwrap_or("brief")
        }))?;
        let anomalies_data = anomalies.get("data").cloned().unwrap_or_else(|| json!({}));
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
            "top_k": args.get("top_k").cloned().unwrap_or_else(|| json!(if mode == "brief" { 4 } else { 6 })),
            "token_budget": args.get("token_budget").cloned().unwrap_or_else(|| json!(if mode == "brief" { 900 } else { 1400 })),
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
            FRAMEWORK_CACHE_TTL_MS,
        ) {
            return Some(cached);
        }

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
        let symbol_validation_map =
            self.batch_symbol_validation_signals(project, &symbol_signal_names);

        let intent_ids = orphan_intent_entities
            .iter()
            .map(|node| node.split(' ').next().unwrap_or(node).to_string())
            .collect::<Vec<_>>();
        let intent_validation_map = self.batch_intent_validation_signals(project, &intent_ids);

        let mut findings = Vec::new();
        for wrapper in &wrapper_entities {
            let source_symbol = wrapper.split(" -> ").next().unwrap_or(wrapper);
            let validation_signals = symbol_validation_map
                .get(source_symbol)
                .cloned()
                .unwrap_or_else(|| json!({"tested": false, "traceability_links": 0}));
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
                .unwrap_or_else(|| json!({"tested": false, "traceability_links": 0}));
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
                .unwrap_or_else(|| json!({"tested": false, "traceability_links": 0}));
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
                .unwrap_or_else(|| json!({"tested": false, "traceability_links": 0}));
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
                .unwrap_or_else(|| json!({"tested": false, "traceability_links": 0}));
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
        for node in &orphan_intent_entities {
            let node_id = node.split(' ').next().unwrap_or(node);
            let validation_signals =
                intent_validation_map
                    .get(node_id)
                    .cloned()
                    .unwrap_or_else(|| {
                        json!({
                            "traceability_links": 0,
                            "verifies_edges": 0,
                            "validation_nodes": 0
                        })
                    });
            let (estimated_effort, estimated_risk) =
                Self::recommend_effort_and_risk("orphan_intent", &validation_signals);
            findings.push(json!({
                "type": "orphan_intent",
                "entity": node,
                "scope": project,
                "severity": "high",
                "confidence": "medium",
                "provenance": "missing_traceability_evidence",
                "evidence_sources": ["soll.Node", "soll.Traceability", "soll.Edge"],
                "recommended_action": "attach implementation or validation evidence",
                "validation_signals": validation_signals,
                "estimated_effort": estimated_effort,
                "estimated_risk": estimated_risk,
                "safe_to_act": false,
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
                .and_then(|value| value.as_i64())
                .unwrap_or(0);
            let validation_signals = symbol_validation_map
                .get(name)
                .cloned()
                .unwrap_or_else(|| json!({"tested": false, "traceability_links": 0}));
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
        let orphan_intent_rate = if total_intent_nodes > 0 {
            ((orphan_intent.len() as f64 / total_intent_nodes as f64) * 100.0 * 10.0).round() / 10.0
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
**Orphan intent:** {}\n\
**Cycles:** {}\n\
**God objects:** {}\n",
            project,
            wrappers.len(),
            feature_envy.len(),
            detours.len(),
            abstraction_detours.len(),
            orphan_code.len(),
            orphan_intent.len(),
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
                    "orphan_intent_count": orphan_intent.len(),
                    "orphan_intent_rate": orphan_intent_rate,
                    "cycle_count": cycle_count,
                    "god_object_count": god_objects.len(),
                    "validation_coverage_score": validation_coverage_score,
                    "total_symbols": total_symbols,
                    "total_intent_nodes": total_intent_nodes
                },
                "snapshot": {
                    "generated_at": Self::now_unix_ms(),
                    "provenance": "aggregated_graph_analytics",
                    "confidence": "medium"
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
        let boundary_violations = if mode == "brief" {
            Vec::new()
        } else {
            self.axon_anomalies(&json!({ "project": project_code, "mode": "brief" }))
                .and_then(|value| value.get("data").cloned())
                .and_then(|data| data.get("findings").cloned())
                .and_then(|value| value.as_array().cloned())
                .unwrap_or_default()
                .into_iter()
                .filter(|finding| {
                    matches!(
                        finding.get("type").and_then(|value| value.as_str()),
                        Some("feature_envy" | "detour" | "abstraction_detour")
                    )
                })
                .collect::<Vec<_>>()
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
                "provenance": "aggregated",
                "confidence": confidence,
                "evidence_sources": ["Symbol", "soll.Traceability", "soll.Node", "soll.Edge"],
                "safe_to_act": safe_to_act,
                "needs_human_confirmation": needs_human_confirmation
            }
        }))
    }
}
