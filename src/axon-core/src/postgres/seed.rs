//! MIL-AXO-015 P5 — SOLL bootstrap seed loader.
//!
//! When a fresh PostgreSQL backend boots with an empty `soll.Node`,
//! callers can load a JSON-encoded snapshot of the canonical SOLL
//! layer (Visions, Pillars, Decisions, Concepts, Requirements,
//! Milestones, Validations, Stakeholders, Guidelines, Edges,
//! Revisions, Traceability) so the brain comes up populated rather
//! than empty. The export side (DuckDB → JSON) is operator-owned;
//! this module owns the import (JSON → PostgreSQL via the unified
//! `GraphStore::execute` path).
//!
//! Format (`SeedDocument`):
//! ```json
//! {
//!   "version": 1,
//!   "generated_at_ms": 1714999999000,
//!   "nodes": [{"id": "VIS-AXO-001", "type": "Vision", "project_code": "AXO", ...}],
//!   "edges": [{"source_id": "REQ-AXO-198", "target_id": "MIL-AXO-015", "relation_type": "BELONGS_TO", "project_code": "AXO"}],
//!   "registry": [{"project_code": "AXON_GLOBAL", "id": "AXON_GLOBAL", "last_vis": 0, ...}]
//! }
//! ```
//!
//! All sections except `version` are optional; an empty document is a
//! valid no-op. The loader returns the total number of inserted rows.
//!
//! Idempotence: the loader checks `soll.Node` is empty before doing
//! anything. A non-empty SOLL means the seed has already been applied
//! (or the operator is upgrading an existing deployment) — in both
//! cases the loader returns Ok(0) without touching the database.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::graph::GraphStore;

#[derive(Debug, serde::Serialize, Deserialize)]
pub struct SeedDocument {
    pub version: u32,
    #[serde(default)]
    pub generated_at_ms: Option<i64>,
    #[serde(default)]
    pub nodes: Vec<SeedNode>,
    #[serde(default)]
    pub edges: Vec<SeedEdge>,
    #[serde(default)]
    pub registry: Vec<SeedRegistryRow>,
    #[serde(default)]
    pub revisions: Vec<SeedRevision>,
    #[serde(default)]
    pub revision_changes: Vec<SeedRevisionChange>,
    #[serde(default)]
    pub traceability: Vec<SeedTraceability>,
}

#[derive(Debug, serde::Serialize, Deserialize)]
pub struct SeedNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub project_code: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, serde::Serialize, Deserialize)]
pub struct SeedEdge {
    pub source_id: String,
    pub target_id: String,
    pub relation_type: String,
    pub project_code: String,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, serde::Serialize, Deserialize)]
pub struct SeedRegistryRow {
    pub project_code: String,
    pub id: String,
    #[serde(default)]
    pub last_vis: i64,
    #[serde(default)]
    pub last_pil: i64,
    #[serde(default)]
    pub last_req: i64,
    #[serde(default)]
    pub last_cpt: i64,
    #[serde(default)]
    pub last_dec: i64,
    #[serde(default)]
    pub last_mil: i64,
    #[serde(default)]
    pub last_val: i64,
    #[serde(default)]
    pub last_stk: i64,
    #[serde(default)]
    pub last_gui: i64,
    #[serde(default)]
    pub last_prv: i64,
    #[serde(default)]
    pub last_rev: i64,
}

#[derive(Debug, serde::Serialize, Deserialize)]
pub struct SeedRevision {
    pub revision_id: String,
    pub project_code: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub created_at: Option<i64>,
    #[serde(default)]
    pub committed_at: Option<i64>,
}

#[derive(Debug, serde::Serialize, Deserialize)]
pub struct SeedRevisionChange {
    pub revision_id: String,
    pub entity_type: String,
    pub entity_id: String,
    pub project_code: String,
    pub action: String,
    #[serde(default)]
    pub before_json: Option<serde_json::Value>,
    #[serde(default)]
    pub after_json: Option<serde_json::Value>,
    #[serde(default)]
    pub created_at: Option<i64>,
}

#[derive(Debug, serde::Serialize, Deserialize)]
pub struct SeedTraceability {
    pub id: String,
    pub soll_entity_type: String,
    pub soll_entity_id: String,
    pub artifact_type: String,
    pub artifact_ref: String,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub created_at: Option<i64>,
}

/// Idempotent entry point: load the seed at `path` ONLY if `soll.Node`
/// is empty. Returns the number of rows inserted across all SOLL
/// tables. When the file is missing, returns Ok(0) without error so
/// production deployments can ship the loader wired in even before
/// the operator generates a seed file.
pub fn load_seed_if_needed(store: &GraphStore, path: &Path) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let existing = store
        .query_count("SELECT count(*)::BIGINT FROM soll.Node")
        .context("count soll.Node before seed load")?;
    if existing > 0 {
        return Ok(0);
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read seed file {}", path.display()))?;
    let doc: SeedDocument = serde_json::from_str(&raw)
        .with_context(|| format!("parse seed JSON at {}", path.display()))?;
    apply_seed(store, &doc)
}

/// Lower-level entry point: apply an already-parsed seed document.
/// Caller is responsible for the soll.Node empty-check.
pub fn apply_seed(store: &GraphStore, doc: &SeedDocument) -> Result<usize> {
    let mut total = 0usize;
    for r in &doc.registry {
        store.execute(&insert_registry_sql(r))
            .with_context(|| format!("insert registry row {}", r.id))?;
        total += 1;
    }
    for n in &doc.nodes {
        store.execute(&insert_node_sql(n))
            .with_context(|| format!("insert node {}", n.id))?;
        total += 1;
    }
    for e in &doc.edges {
        store.execute(&insert_edge_sql(e))
            .with_context(|| format!("insert edge {} -> {}", e.source_id, e.target_id))?;
        total += 1;
    }
    for r in &doc.revisions {
        store.execute(&insert_revision_sql(r))
            .with_context(|| format!("insert revision {}", r.revision_id))?;
        total += 1;
    }
    for rc in &doc.revision_changes {
        store.execute(&insert_revision_change_sql(rc))
            .with_context(|| format!("insert revision_change {}/{}", rc.revision_id, rc.entity_id))?;
        total += 1;
    }
    for t in &doc.traceability {
        store.execute(&insert_traceability_sql(t))
            .with_context(|| format!("insert traceability {}", t.id))?;
        total += 1;
    }
    Ok(total)
}

/// SQL-quote a string by escaping single quotes. Caller-controlled
/// JSON inputs only — we do not parameter-bind because GraphStore's
/// uniform FFI surface treats SQL as opaque text. Acceptable here
/// because seed content is operator-curated.
fn sql_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn sql_quote_opt(o: &Option<String>) -> String {
    match o {
        Some(s) => sql_quote(s),
        None => "NULL".to_string(),
    }
}

fn jsonb_or_null(v: &Option<serde_json::Value>) -> String {
    match v {
        Some(value) => {
            // REQ-AXO-249 / soll-export-seed gap: DuckDB's to_json(t)
            // emits VARCHAR metadata columns as JSON-encoded STRINGS
            // (e.g. "metadata": "{\"acceptance_criteria\":\"...\"}").
            // Round-tripping that into PG verbatim casts a JSONB SCALAR
            // STRING, breaking `metadata->>'key'` lookups (returns
            // NULL instead of the value). Detect that case here and
            // parse the inner JSON so the column lands as a JSONB
            // OBJECT — what every consumer expects.
            let canonical = match value {
                serde_json::Value::String(s) => match serde_json::from_str::<serde_json::Value>(s) {
                    Ok(parsed) if !parsed.is_string() => parsed,
                    _ => value.clone(),
                },
                _ => value.clone(),
            };
            format!("{}::jsonb", sql_quote(&canonical.to_string()))
        }
        None => "NULL".to_string(),
    }
}

fn insert_node_sql(n: &SeedNode) -> String {
    format!(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
         VALUES ({}, {}, {}, {}, {}, {}, {}) ON CONFLICT (id) DO NOTHING",
        sql_quote(&n.id),
        sql_quote(&n.node_type),
        sql_quote(&n.project_code),
        sql_quote_opt(&n.title),
        sql_quote_opt(&n.description),
        sql_quote_opt(&n.status),
        jsonb_or_null(&n.metadata),
    )
}

fn insert_edge_sql(e: &SeedEdge) -> String {
    format!(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code, metadata) \
         VALUES ({}, {}, {}, {}, {}) ON CONFLICT (source_id, target_id, relation_type) DO NOTHING",
        sql_quote(&e.source_id),
        sql_quote(&e.target_id),
        sql_quote(&e.relation_type),
        sql_quote(&e.project_code),
        jsonb_or_null(&e.metadata),
    )
}

fn insert_registry_sql(r: &SeedRegistryRow) -> String {
    format!(
        "INSERT INTO soll.Registry (project_code, id, last_vis, last_pil, last_req, last_cpt, \
         last_dec, last_mil, last_val, last_stk, last_gui, last_prv, last_rev) \
         VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}) \
         ON CONFLICT (project_code) DO NOTHING",
        sql_quote(&r.project_code),
        sql_quote(&r.id),
        r.last_vis,
        r.last_pil,
        r.last_req,
        r.last_cpt,
        r.last_dec,
        r.last_mil,
        r.last_val,
        r.last_stk,
        r.last_gui,
        r.last_prv,
        r.last_rev,
    )
}

fn insert_revision_sql(r: &SeedRevision) -> String {
    format!(
        "INSERT INTO soll.Revision (revision_id, project_code, author, source, summary, status, created_at, committed_at) \
         VALUES ({}, {}, {}, {}, {}, {}, {}, {}) ON CONFLICT (revision_id) DO NOTHING",
        sql_quote(&r.revision_id),
        sql_quote(&r.project_code),
        sql_quote_opt(&r.author),
        sql_quote_opt(&r.source),
        sql_quote_opt(&r.summary),
        sql_quote_opt(&r.status),
        r.created_at.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string()),
        r.committed_at.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string()),
    )
}

fn insert_revision_change_sql(rc: &SeedRevisionChange) -> String {
    format!(
        "INSERT INTO soll.RevisionChange (revision_id, entity_type, entity_id, project_code, action, before_json, after_json, created_at) \
         VALUES ({}, {}, {}, {}, {}, {}, {}, {})",
        sql_quote(&rc.revision_id),
        sql_quote(&rc.entity_type),
        sql_quote(&rc.entity_id),
        sql_quote(&rc.project_code),
        sql_quote(&rc.action),
        jsonb_or_null(&rc.before_json),
        jsonb_or_null(&rc.after_json),
        rc.created_at.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string()),
    )
}

fn insert_traceability_sql(t: &SeedTraceability) -> String {
    format!(
        "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) \
         VALUES ({}, {}, {}, {}, {}, {}, {}, {}) ON CONFLICT (id) DO NOTHING",
        sql_quote(&t.id),
        sql_quote(&t.soll_entity_type),
        sql_quote(&t.soll_entity_id),
        sql_quote(&t.artifact_type),
        sql_quote(&t.artifact_ref),
        t.confidence.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string()),
        jsonb_or_null(&t.metadata),
        t.created_at.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_quote_escapes_single_quotes() {
        assert_eq!(sql_quote("foo"), "'foo'");
        assert_eq!(sql_quote("o'brien"), "'o''brien'");
        assert_eq!(sql_quote(""), "''");
    }

    #[test]
    fn sql_quote_opt_handles_none() {
        assert_eq!(sql_quote_opt(&None), "NULL");
        assert_eq!(sql_quote_opt(&Some("x".to_string())), "'x'");
    }

    #[test]
    fn jsonb_or_null_wraps_jsonb_cast() {
        let json = serde_json::json!({"k": "v"});
        let out = jsonb_or_null(&Some(json));
        assert!(out.contains("::jsonb"));
        assert!(out.contains("\"k\""));
    }

    #[test]
    fn jsonb_or_null_parses_json_encoded_string_into_object() {
        // REQ-AXO-249 — soll-export-seed emits metadata as a JSON-
        // encoded STRING (DuckDB to_json on VARCHAR). Without this
        // unwrap, ::jsonb casts a string scalar and metadata->>'key'
        // returns NULL, breaking soll_validate / completeness reads.
        let encoded =
            serde_json::Value::String(r#"{"acceptance_criteria":"items 1-3"}"#.to_string());
        let out = jsonb_or_null(&Some(encoded));
        assert!(out.contains("::jsonb"));
        assert!(out.contains("\"acceptance_criteria\""));
        assert!(out.contains("\"items 1-3\""));
        // Critical: the OUTER quotes around the object must be gone —
        // we want a JSONB object, not a JSONB string scalar.
        assert!(
            !out.contains(r#""{\"acceptance_criteria\""#),
            "unparsed string-of-json leaked through: {out}"
        );
    }

    #[test]
    fn jsonb_or_null_keeps_plain_strings_intact() {
        // A non-JSON string must NOT be unwrapped; metadata columns
        // sometimes hold plain text values.
        let plain = serde_json::Value::String("just a string".to_string());
        let out = jsonb_or_null(&Some(plain));
        assert!(out.contains("::jsonb"));
        assert!(out.contains("just a string"));
    }

    #[test]
    fn insert_node_sql_includes_on_conflict() {
        let n = SeedNode {
            id: "VIS-AXO-001".into(),
            node_type: "Vision".into(),
            project_code: "AXO".into(),
            title: Some("Axon Vision".into()),
            description: Some("desc".into()),
            status: Some("active".into()),
            metadata: Some(serde_json::json!({"tag": "core"})),
        };
        let sql = insert_node_sql(&n);
        assert!(sql.contains("ON CONFLICT (id) DO NOTHING"));
        assert!(sql.contains("'VIS-AXO-001'"));
        assert!(sql.contains("'Vision'"));
        assert!(sql.contains("'AXO'"));
        assert!(sql.contains("\"tag\""));
    }

    #[test]
    fn parse_minimal_seed() {
        let doc: SeedDocument = serde_json::from_str(
            r#"{"version": 1, "nodes": [{"id":"X","type":"T","project_code":"P"}]}"#,
        )
        .unwrap();
        assert_eq!(doc.version, 1);
        assert_eq!(doc.nodes.len(), 1);
        assert_eq!(doc.edges.len(), 0);
        assert_eq!(doc.nodes[0].id, "X");
    }

    #[test]
    fn parse_empty_seed_is_valid() {
        let doc: SeedDocument = serde_json::from_str(r#"{"version": 1}"#).unwrap();
        assert_eq!(doc.version, 1);
        assert!(doc.nodes.is_empty());
        assert!(doc.edges.is_empty());
        assert!(doc.registry.is_empty());
    }
}
