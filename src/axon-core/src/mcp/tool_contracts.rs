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
    /// Output verbosity (default: brief/terse). `brief` returns the ranked
    /// results + `next` links only. `verbose` (alias `full`) ADDS the graph r=1
    /// neighbour expansion (`data.context.related_symbols_via_graph`) and lifts
    /// the text cap.
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

/// REQ-AXO-901949 — single-source interaction-graph record for a tool.
///
/// The "exchange is a graph" the operator asked for: instead of the same tool's
/// edges and metadata being scattered across five hand-maintained `match` arms
/// in mcp.rs (`default_follow_up_tools_for`, `primary_goal_for`,
/// `workflow_stage_for`, `token_efficiency_hint_for`, `follow_up_reason_for`),
/// every routing fact for a tool lives here, co-located with its input schema.
/// The mcp.rs functions delegate to `tool_routing(name)` for the tracer-bullet
/// set and fall back to their literal arms for the other 64 (slice-2 rollout).
pub(crate) struct ToolRouting {
    /// Valid next tools (the graph edges out of this node).
    pub follow_ups: &'static [&'static str],
    /// Why an agent would call this tool.
    pub goal: &'static str,
    /// Workflow stage this tool belongs to.
    pub stage: &'static str,
    /// Token-economy guidance for this tool.
    pub token_hint: &'static str,
    /// When a peer tool should route TO this tool (its inbound-edge reason).
    pub use_when: &'static str,
}

/// Routing record for a tracer-bullet tool, or `None` for tools still served by
/// the hand-written mcp.rs match arms. Values mirror the pre-refactor arms
/// exactly — this is a co-location, not a behaviour change.
pub(crate) fn tool_routing(name: &str) -> Option<ToolRouting> {
    Some(match name {
        "sql" => ToolRouting {
            follow_ups: &["schema_overview", "query_examples"],
            goal: "move to the next highest-signal MCP step",
            stage: "general_mcp_operation",
            token_hint:
                "Follow the server-provided next step before composing additional exploratory calls.",
            use_when: "use when it is the next highest-signal MCP move",
        },
        "query" => ToolRouting {
            follow_ups: &["inspect", "retrieve_context", "impact"],
            goal: "discover plausible targets with broad recall",
            stage: "target_discovery",
            token_hint:
                "Use `query` to widen recall only when the target anchor is still ambiguous; switch to `inspect` quickly once a candidate exists.",
            use_when: "use when recall is too narrow and you need broader candidate discovery",
        },
        "soll_manager" => ToolRouting {
            follow_ups: &["soll_validate", "soll_query_context"],
            goal: "perform an exact SOLL create/update/link operation",
            stage: "intent_governance",
            token_hint:
                "Follow the server-provided next step before composing additional exploratory calls.",
            use_when: "use when the next step is an exact canonical mutation",
        },
        _ => return None,
    })
}

/// REQ-AXO-901949 inv.5 — the "terse default" decision for read tools, in one
/// place so the rollout to the other reads reuses the same rule. `verbose` is
/// opt-in (`mode=verbose`, case-insensitive); everything else — including the
/// omitted/`brief` default — is terse. A normal-sized result is identical under
/// brief and verbose UNTIL a tool gates a *detail surface* on this (e.g. `query`
/// skips the graph r=1 expansion in brief), which is what makes the knob real
/// rather than a no-op for the common case.
pub(crate) fn read_mode_is_verbose(mode: Option<&str>) -> bool {
    // `verbose` is the canonical token (QueryInput schema), but AC5's own
    // language is "detail=full" — an LLM will reasonably guess `full`/`detail`.
    // Be liberal in what we accept (Postel) so the natural guess works instead
    // of silently degrading to terse.
    mode.is_some_and(|m| {
        let m = m.trim();
        m.eq_ignore_ascii_case("verbose")
            || m.eq_ignore_ascii_case("full")
            || m.eq_ignore_ascii_case("detail")
    })
}

/// REQ-AXO-901949 inv.5 — the "auto-continue" property: every tracer-tool
/// response carries its valid next moves, sourced from the SAME `tool_routing`
/// record that feeds the routing tests (single source, no drift). `None` for
/// non-tracer tools so callers can inject unconditionally without a per-tool
/// branch. Returns `{tools, reason}` rather than a bare list so the agent gets
/// the token-economy rationale inline (why follow the link before fanning out).
pub(crate) fn next_links(name: &str) -> Option<Value> {
    let routing = tool_routing(name)?;
    Some(serde_json::json!({
        "tools": routing.follow_ups,
        "reason": routing.token_hint,
    }))
}

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
    let Ok(re) = regex::Regex::new(r"(?i)\b(?:from|join)\s+([a-z_][a-z0-9_]*)\.([a-z_][a-z0-9_]*)")
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

/// REQ-AXO-901949 inv.2 — render a `pg_error_repair` envelope into the inline
/// text channel.
///
/// The repair object is already attached to `data.parameter_repair`, but most
/// MCP client renderings (including the bare HTTP/curl path) surface only
/// `content[0].text`. Without this the agent reads `SQL Error: column "x" does
/// not exist` and has to probe `schema_overview` anyway — the exact second
/// round-trip AC4 promised to remove. Folding the real columns into the text
/// makes the corrected call self-sufficient in the same response. Pure (takes
/// the already-built repair `Value`) so it is unit-testable without a DB.
pub(crate) fn render_pg_repair_text(repair: &Value) -> String {
    let problem = repair
        .get("problem_class")
        .and_then(Value::as_str)
        .unwrap_or("input_invalid");
    let mut out = format!("\n\nRepair ({problem}):");

    match repair.get("referenced_relations").and_then(Value::as_array) {
        Some(rels) if !rels.is_empty() => {
            for rel in rels {
                let name = rel.get("relation").and_then(Value::as_str).unwrap_or("?");
                let cols: Vec<&str> = rel
                    .get("real_columns")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let exists = rel
                    .get("exists")
                    .and_then(Value::as_bool)
                    .unwrap_or(!cols.is_empty());
                if exists && !cols.is_empty() {
                    out.push_str(&format!("\n  {name} columns: {}", cols.join(", ")));
                } else {
                    out.push_str(&format!(
                        "\n  {name} does not exist — run `schema_overview` for the table list"
                    ));
                }
            }
        }
        _ => out.push_str(
            "\n  (no schema-qualified relation parsed — run `schema_overview` for the table list)",
        ),
    }

    if let Some(hint) = repair.get("hint").and_then(Value::as_str) {
        out.push_str(&format!("\n  Hint: {hint}"));
    }
    out
}

/// Derived JSON Schema for a tracer-bullet tool, or `None` for any tool still
/// served by the hand-written catalog literal (slice-2 rollout).
pub(crate) fn derived_input_schema(name: &str) -> Option<Value> {
    // REQ-AXO-901949 inv.7 — inline enum sub-schemas so the agent never resolves
    // a `$ref`/`$defs`: `query.mode`, `soll_manager.action`/`entity` render as
    // inline `{type, enum}`. A `$ref` to chase is friction for an LLM.
    fn generator() -> schemars::SchemaGenerator {
        schemars::generate::SchemaSettings::default()
            .with(|s| s.inline_subschemas = true)
            .into_generator()
    }
    let schema = match name {
        "sql" => generator().into_root_schema_for::<SqlInput>(),
        "query" => generator().into_root_schema_for::<QueryInput>(),
        "soll_manager" => generator().into_root_schema_for::<SollManagerInput>(),
        _ => return None,
    };
    let mut value = serde_json::to_value(schema).ok()?;
    if let Some(obj) = value.as_object_mut() {
        // Strip cosmetic top-level keys; with inlining there should be no
        // $defs/definitions left, but drop them defensively.
        obj.remove("$schema");
        obj.remove("title");
        obj.remove("$defs");
        obj.remove("definitions");
    }
    // REQ-AXO-901949 inv.2 — correct-by-construction: express soll_manager's
    // per-action requiredness in the schema itself (if/then) so a wrong-shaped
    // call is structurally invalid, not merely flagged in prose.
    if name == "soll_manager" {
        augment_soll_manager_conditionals(&mut value);
    }
    Some(value)
}

/// REQ-AXO-901949 inv.2 — per-action `required` constraints for `soll_manager`,
/// co-located with the struct (single source). create needs attach_to +
/// relation_type ; link/unlink need source_id + target_id + relation_type ;
/// update needs id. Encoded as JSON-Schema `allOf` if/then so a validator (inv.3)
/// rejects the malformed call at dispatch instead of the handler discovering it
/// late.
fn augment_soll_manager_conditionals(schema: &mut Value) {
    let conditionals = serde_json::json!([
        { "if": { "properties": { "action": { "const": "create" } } },
          "then": { "properties": { "data": { "required": ["attach_to", "relation_type"] } } } },
        { "if": { "properties": { "action": { "const": "link" } } },
          "then": { "properties": { "data": { "required": ["source_id", "target_id", "relation_type"] } } } },
        { "if": { "properties": { "action": { "const": "unlink" } } },
          "then": { "properties": { "data": { "required": ["source_id", "target_id", "relation_type"] } } } },
        { "if": { "properties": { "action": { "const": "update" } } },
          "then": { "properties": { "data": { "required": ["id"] } } } }
    ]);
    if let Some(obj) = schema.as_object_mut() {
        obj.insert("allOf".to_string(), conditionals);
    }
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
            assert!(
                required.iter().any(|v| v == field),
                "missing required {field}"
            );
        }
        // `data` resolves (directly or via $ref/$defs) to a schema carrying the
        // canonical field names — the win over the prose blob.
        let rendered = serde_json::to_string(&schema).unwrap();
        for field in [
            "project_code",
            "attach_to",
            "relation_type",
            "source_id",
            "target_id",
        ] {
            assert!(rendered.contains(field), "data schema must mention {field}");
        }
    }

    #[test]
    fn unknown_tool_has_no_derived_schema() {
        assert!(derived_input_schema("status").is_none());
        assert!(derived_input_schema("impact").is_none());
    }

    #[test]
    fn derived_schemas_have_no_ref_or_defs() {
        // REQ-AXO-901949 inv.7 — enums inlined; an agent never resolves a $ref.
        for name in DERIVED_TOOLS {
            let rendered = serde_json::to_string(&derived_input_schema(name).unwrap()).unwrap();
            assert!(
                !rendered.contains("$ref") && !rendered.contains("$defs") && !rendered.contains("definitions"),
                "{name} schema must inline subschemas (no $ref/$defs): {rendered}"
            );
        }
        // The enum actually rendered inline as {type,enum}.
        let q = derived_input_schema("query").unwrap();
        let mode = &q["properties"]["mode"];
        let rendered = serde_json::to_string(mode).unwrap();
        assert!(
            rendered.contains("brief") && rendered.contains("verbose"),
            "query.mode enum must be inline: {rendered}"
        );
    }

    #[test]
    fn soll_manager_schema_encodes_per_action_required() {
        // REQ-AXO-901949 inv.2 — correct-by-construction: create requires
        // attach_to+relation_type, link/unlink require source_id+target_id, etc.
        let schema = derived_input_schema("soll_manager").unwrap();
        let all_of = schema["allOf"].as_array().expect("allOf conditionals");
        // create branch
        let create = all_of
            .iter()
            .find(|c| c["if"]["properties"]["action"]["const"] == "create")
            .expect("create conditional");
        let create_req: Vec<&str> = create["then"]["properties"]["data"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(create_req.contains(&"attach_to") && create_req.contains(&"relation_type"));
        // link branch
        let link = all_of
            .iter()
            .find(|c| c["if"]["properties"]["action"]["const"] == "link")
            .expect("link conditional");
        let link_req: Vec<&str> = link["then"]["properties"]["data"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(link_req.contains(&"source_id") && link_req.contains(&"target_id"));
    }

    #[test]
    fn classify_pg_undefined_column_and_table() {
        // The exact PG error the LLM hit in session 75.
        assert_eq!(
            classify_pg_undefined(r#"db error — column "kind" does not exist [SQLSTATE 42703]"#),
            Some("undefined_column")
        );
        assert_eq!(
            classify_pg_undefined(r#"relation "soll.foo" does not exist [SQLSTATE 42P01]"#),
            Some("undefined_table")
        );
        // Unrelated errors are left to the raw passthrough.
        assert_eq!(
            classify_pg_undefined("syntax error at or near \"SELEC\""),
            None
        );
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
            rels.iter()
                .filter(|(s, t)| s == "soll" && t == "node")
                .count(),
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
    fn read_mode_verbose_is_opt_in_and_case_insensitive() {
        // Terse is the default: omitted / brief / anything-not-verbose → false.
        assert!(!read_mode_is_verbose(None));
        assert!(!read_mode_is_verbose(Some("brief")));
        assert!(!read_mode_is_verbose(Some("")));
        assert!(!read_mode_is_verbose(Some("terse")));
        // Detail opt-in: canonical `verbose` + the AC5-natural synonyms
        // `full`/`detail`, case-insensitive, trimmed.
        assert!(read_mode_is_verbose(Some("verbose")));
        assert!(read_mode_is_verbose(Some("VERBOSE")));
        assert!(read_mode_is_verbose(Some("Verbose")));
        assert!(read_mode_is_verbose(Some("full")));
        assert!(read_mode_is_verbose(Some(" detail ")));
    }

    #[test]
    fn next_links_single_source_for_tracer_tools() {
        // The `next` envelope is derived from the SAME tool_routing record the
        // routing test asserts — proving single-source (no second list to drift).
        let sql = next_links("sql").expect("sql next");
        assert_eq!(
            sql["tools"].as_array().unwrap(),
            &[
                serde_json::json!("schema_overview"),
                serde_json::json!("query_examples")
            ]
        );
        assert!(sql["reason"].as_str().unwrap().len() > 10);

        let query = next_links("query").expect("query next");
        assert_eq!(
            query["tools"].as_array().unwrap(),
            &[
                serde_json::json!("inspect"),
                serde_json::json!("retrieve_context"),
                serde_json::json!("impact")
            ]
        );

        let soll = next_links("soll_manager").expect("soll_manager next");
        assert_eq!(
            soll["tools"].as_array().unwrap(),
            &[
                serde_json::json!("soll_validate"),
                serde_json::json!("soll_query_context")
            ]
        );

        // Non-tracer tools yield None → callers inject unconditionally, no-op.
        assert!(next_links("impact").is_none());
        assert!(next_links("status").is_none());
    }

    #[test]
    fn render_repair_inlines_real_columns() {
        // The repair built by `pg_error_repair` for the exact session-75 friction:
        // `SELECT ... priority ... FROM soll.Node` → 42703. The rendered text MUST
        // carry the real columns so the agent self-corrects in one shot, never
        // forced into a second `schema_overview` probe.
        let repair = serde_json::json!({
            "problem_class": "undefined_column",
            "referenced_relations": [{
                "relation": "soll.node",
                "real_columns": ["id", "title", "type", "status", "description"],
                "exists": true
            }],
            "hint": "Use only `real_columns` for each relation; re-run `sql`."
        });
        let text = render_pg_repair_text(&repair);
        assert!(text.contains("undefined_column"));
        assert!(
            text.contains("soll.node columns: id, title, type, status, description"),
            "real columns must be inline in the text channel, got: {text}"
        );
        // The bad guess (`priority`) is absent from the real columns — the agent
        // can see that directly.
        assert!(!text.contains("priority"));
        assert!(text.contains("Hint:"));
    }

    #[test]
    fn render_repair_flags_missing_relation_and_empty_set() {
        // A schema-qualified relation that does not exist → explicit pointer.
        let missing = serde_json::json!({
            "problem_class": "undefined_table",
            "referenced_relations": [{ "relation": "soll.foo", "real_columns": [], "exists": false }]
        });
        let text = render_pg_repair_text(&missing);
        assert!(text.contains("soll.foo does not exist"));
        assert!(text.contains("schema_overview"));

        // No schema-qualified relation parsed (bare table) → fallback pointer.
        let bare = serde_json::json!({
            "problem_class": "undefined_column",
            "referenced_relations": []
        });
        let text = render_pg_repair_text(&bare);
        assert!(text.contains("no schema-qualified relation parsed"));
        assert!(text.contains("schema_overview"));
    }

    #[test]
    fn routing_single_source_for_tracer_tools() {
        // The interaction-graph edges + metadata for each tracer tool live in
        // exactly one place. Values mirror the pre-refactor mcp.rs arms.
        let sql = tool_routing("sql").expect("sql routing");
        assert_eq!(sql.follow_ups, &["schema_overview", "query_examples"]);
        assert_eq!(sql.stage, "general_mcp_operation");

        let query = tool_routing("query").expect("query routing");
        assert_eq!(query.follow_ups, &["inspect", "retrieve_context", "impact"]);
        assert_eq!(query.goal, "discover plausible targets with broad recall");
        assert_eq!(query.stage, "target_discovery");

        let soll = tool_routing("soll_manager").expect("soll_manager routing");
        assert_eq!(soll.follow_ups, &["soll_validate", "soll_query_context"]);
        assert_eq!(soll.stage, "intent_governance");

        // Tools not in the tracer set keep their hand-written arms (no routing
        // record yet) — slice-2 rollout.
        assert!(tool_routing("impact").is_none());
        assert!(tool_routing("status").is_none());
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
        assert!(
            rendered.contains("relation_type"),
            "data fields must be advertised"
        );
    }
}
