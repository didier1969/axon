//! Derived MCP tool input contracts (REQ-AXO-901949, tracer-bullet for
//! REQ-AXO-901947).
//!
//! Single source of truth: the Rust struct IS the schema, the documentation,
//! and (slice 2) the validator target. `schemars` derives the JSON Schema the
//! agent sees, so it can never drift from the type the handler reads — the
//! "auto-descriptive" property of an optimal-for-LLM surface.
//!
//! Scope: `sql`, `soll_manager`, `query` (the three tools that bit the LLM in
//! session 75). Rollout to the remaining 64 catalog literals is slice 2.

// The struct fields below are consumed by the `schemars` derive macro (and, in
// slice 2, by `serde::Deserialize` for server-side validation). They are not
// read via field access at runtime yet, so dead-code analysis flags them; the
// allow is scoped to this contract-definition module only.
#![allow(dead_code)]

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

/// `sql` — raw read-only SQL against the PG backend (post-MIL-AXO-017).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SqlInput {
    /// Read-only SQL statement (PG dialect). Use `schema_overview` /
    /// `query_examples` first to discover tables and columns.
    pub sql: String,
}

/// Output verbosity for read tools.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum QueryMode {
    Brief,
    Verbose,
}

/// `query` — search symbols by name / natural-language fragment.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct QueryInput {
    /// Symbol name or natural-language fragment to search for.
    pub query: String,
    /// Project code scope (e.g. "AXO"). Auto-resolved from cwd when omitted.
    #[serde(default)]
    pub project: Option<String>,
    /// Output verbosity (default: brief).
    #[serde(default)]
    pub mode: Option<QueryMode>,
}

/// `soll_manager` operation.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SollAction {
    Create,
    Update,
    Link,
    Unlink,
}

/// `soll_manager` target entity type (derives `soll.Node.type`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SollEntity {
    Vision,
    Pillar,
    Requirement,
    Concept,
    Milestone,
    Decision,
    Stakeholder,
    Validation,
    Guideline,
    Skill,
    PromptTemplate,
}

/// `soll_manager.data` payload. Which fields are required depends on `action`
/// (see the tool description); all are optional at the schema level and
/// validated server-side per-action. Extra metadata-routed fields (goal,
/// rationale, acceptance_criteria, owner, context, evidence_refs) are accepted
/// — the schema does not forbid additional properties.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SollManagerData {
    /// Canonical project code (create). e.g. "AXO".
    #[serde(default)]
    pub project_code: Option<String>,
    /// Parent node id to attach to (create, non-vision). e.g. "PIL-AXO-002".
    #[serde(default)]
    pub attach_to: Option<String>,
    /// Canonical edge type (create attach / link). e.g. "BELONGS_TO",
    /// "REFINES", "SOLVES", "EPITOMIZES", "SUPERSEDES".
    #[serde(default)]
    pub relation_type: Option<String>,
    /// Node id (update target). DB-allocated on create — forbidden there.
    #[serde(default)]
    pub id: Option<String>,
    /// Edge source id (link / unlink).
    #[serde(default)]
    pub source_id: Option<String>,
    /// Edge target id (link / unlink).
    #[serde(default)]
    pub target_id: Option<String>,
    /// Human-readable title.
    #[serde(default)]
    pub title: Option<String>,
    /// Body / description (canonical column).
    #[serde(default)]
    pub description: Option<String>,
    /// Lifecycle status (e.g. "planned", "current", "delivered").
    #[serde(default)]
    pub status: Option<String>,
    /// Priority bucket P0..P3 (metadata-routed; consumed by soll_work_plan).
    #[serde(default)]
    pub priority: Option<String>,
    /// Tag list (metadata-routed; consumed by soll_query_context filter).
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

/// `soll_manager` — create / update / link / unlink intent entities.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SollManagerInput {
    /// Operation to perform.
    pub action: SollAction,
    /// Target entity type.
    pub entity: SollEntity,
    /// Operation payload (fields depend on `action`).
    pub data: SollManagerData,
}

/// Tools currently served by a derived schema (tracer-bullet set).
pub(crate) const DERIVED_TOOLS: &[&str] = &["sql", "query", "soll_manager"];

/// REQ-AXO-901949 — classify a PG execution error as undefined-column (42703)
/// or undefined-table/relation (42P01) so `sql` can answer with the real
/// columns/tables instead of the opaque passthrough string. Returns `None` for
/// any other error (the raw `SQL Error` text already carries those).
pub(crate) fn classify_pg_undefined(raw: &str) -> Option<&'static str> {
    let lower = raw.to_ascii_lowercase();
    if !lower.contains("does not exist") {
        return None;
    }
    if lower.contains("42703") || lower.contains("column") {
        return Some("undefined_column");
    }
    if lower.contains("42p01") || lower.contains("relation") || lower.contains("table") {
        return Some("undefined_table");
    }
    None
}

/// REQ-AXO-901949 — extract the `schema.table` relations named in a `FROM` /
/// `JOIN` clause so the repair can inline each one's real columns. De-duplicated,
/// lower-cased, capped at 4. Pure (no DB) so the parsing is unit-testable.
pub(crate) fn extract_sql_relations(sql: &str) -> Vec<(String, String)> {
    let Ok(re) =
        regex::Regex::new(r"(?i)\b(?:from|join)\s+([a-z_][a-z0-9_]*)\.([a-z_][a-z0-9_]*)")
    else {
        return Vec::new();
    };
    let mut relations: Vec<(String, String)> = Vec::new();
    for cap in re.captures_iter(sql) {
        let schema = cap[1].to_ascii_lowercase();
        let table = cap[2].to_ascii_lowercase();
        if !relations.iter().any(|(s, t)| s == &schema && t == &table) {
            relations.push((schema, table));
        }
        if relations.len() >= 4 {
            break;
        }
    }
    relations
}

/// Derived JSON Schema for a tracer-bullet tool, or `None` for any tool still
/// served by the hand-written catalog literal (slice-2 rollout).
pub(crate) fn derived_input_schema(name: &str) -> Option<Value> {
    let schema = match name {
        "sql" => schemars::schema_for!(SqlInput),
        "query" => schemars::schema_for!(QueryInput),
        "soll_manager" => schemars::schema_for!(SollManagerInput),
        _ => return None,
    };
    let mut value = serde_json::to_value(schema).ok()?;
    // Strip cosmetic top-level keys schemars emits ($schema / title); MCP wants
    // a plain object schema. `$defs` (enum sub-schemas) stay — valid JSON Schema.
    if let Some(obj) = value.as_object_mut() {
        obj.remove("$schema");
        obj.remove("title");
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_schema_present_for_tracer_tools() {
        for name in DERIVED_TOOLS {
            let schema = derived_input_schema(name)
                .unwrap_or_else(|| panic!("derived schema missing for {name}"));
            assert_eq!(schema["type"], "object", "{name} schema must be object");
            assert!(
                schema.get("$schema").is_none(),
                "{name} schema must strip cosmetic $schema key"
            );
            assert!(
                schema.get("properties").is_some(),
                "{name} schema must expose properties"
            );
        }
    }

    #[test]
    fn sql_schema_requires_sql_field() {
        let schema = derived_input_schema("sql").unwrap();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "sql"));
        assert!(schema["properties"].get("sql").is_some());
    }

    #[test]
    fn soll_manager_data_exposes_real_fields_not_prose() {
        // Acceptance #2: data is a real field schema, not a free-form object.
        let schema = derived_input_schema("soll_manager").unwrap();
        let required = schema["required"].as_array().unwrap();
        for field in ["action", "entity", "data"] {
            assert!(required.iter().any(|v| v == field), "missing required {field}");
        }
        // `data` resolves (directly or via $ref/$defs) to a schema carrying the
        // canonical field names — the win over the prose blob.
        let rendered = serde_json::to_string(&schema).unwrap();
        for field in ["project_code", "attach_to", "relation_type", "source_id", "target_id"] {
            assert!(rendered.contains(field), "data schema must mention {field}");
        }
    }

    #[test]
    fn unknown_tool_has_no_derived_schema() {
        assert!(derived_input_schema("status").is_none());
        assert!(derived_input_schema("impact").is_none());
    }

    #[test]
    fn classify_pg_undefined_column_and_table() {
        // The exact PG error the LLM hit in session 75.
        assert_eq!(
            classify_pg_undefined(
                r#"db error — column "kind" does not exist [SQLSTATE 42703]"#
            ),
            Some("undefined_column")
        );
        assert_eq!(
            classify_pg_undefined(r#"relation "soll.foo" does not exist [SQLSTATE 42P01]"#),
            Some("undefined_table")
        );
        // Unrelated errors are left to the raw passthrough.
        assert_eq!(classify_pg_undefined("syntax error at or near \"SELEC\""), None);
        assert_eq!(classify_pg_undefined("permission denied for table x"), None);
    }

    #[test]
    fn extract_relations_from_from_and_join() {
        let rels = extract_sql_relations(
            "SELECT n.id FROM soll.Node n LEFT JOIN soll.Edge e ON e.source_id = n.id",
        );
        assert!(rels.contains(&("soll".to_string(), "node".to_string())));
        assert!(rels.contains(&("soll".to_string(), "edge".to_string())));
        assert_eq!(rels.len(), 2, "deduplicated, both relations found");
    }

    #[test]
    fn extract_relations_dedup_and_cap() {
        let rels = extract_sql_relations(
            "SELECT 1 FROM soll.node a JOIN soll.node b ON true JOIN ist.symbol s ON true",
        );
        // soll.node appears twice → deduped to one.
        assert_eq!(
            rels.iter().filter(|(s, t)| s == "soll" && t == "node").count(),
            1
        );
        assert!(rels.contains(&("ist".to_string(), "symbol".to_string())));
    }

    #[test]
    fn extract_relations_handles_no_schema_qualified_tables() {
        // Bare table names (no schema prefix) are not extracted — the repair
        // falls back to schema_overview rather than guessing a schema.
        assert!(extract_sql_relations("SELECT 1 FROM mytable").is_empty());
    }

    #[test]
    fn catalog_serves_derived_schema_for_tracer_tools() {
        // Integration: the real tools/list catalog (pure fn, no DB) must carry
        // the schemars-derived schema for the tracer-bullet tools, proving the
        // override pass replaced the hand-written literal.
        let catalog = super::super::catalog::tools_catalog(true);
        let tools = catalog["tools"].as_array().expect("tools array");
        for name in DERIVED_TOOLS {
            let entry = tools
                .iter()
                .find(|t| t["name"].as_str() == Some(name))
                .unwrap_or_else(|| panic!("{name} absent from catalog"));
            let advertised = &entry["inputSchema"];
            let expected = derived_input_schema(name).unwrap();
            assert_eq!(
                advertised, &expected,
                "{name} catalog inputSchema must equal the derived schema"
            );
        }
        // soll_manager.data is now a real field schema, not the prose blob.
        let soll = tools
            .iter()
            .find(|t| t["name"].as_str() == Some("soll_manager"))
            .unwrap();
        let rendered = serde_json::to_string(&soll["inputSchema"]).unwrap();
        assert!(rendered.contains("relation_type"), "data fields must be advertised");
    }
}
