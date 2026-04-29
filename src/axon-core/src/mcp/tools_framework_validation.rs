use serde_json::{json, Value};
use std::collections::HashMap;

use super::McpServer;

pub(super) fn linked_validations_from_intentions(intentions: &[Value]) -> Vec<Value> {
    intentions
        .iter()
        .filter(|entity| {
            entity
                .get("type")
                .and_then(|value| value.as_str())
                .map(|kind| kind.eq_ignore_ascii_case("Validation"))
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

impl McpServer {
    pub(super) fn symbol_validation_signals(&self, project: &str, symbol_name: &str) -> Value {
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

    pub(super) fn batch_symbol_validation_signals(
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

    pub(super) fn intent_validation_signals(&self, project: &str, entity_id: &str) -> Value {
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

    pub(super) fn batch_intent_validation_signals(
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
}
