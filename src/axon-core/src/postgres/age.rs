//! MIL-AXO-015 option B.2 — AGE Cypher helpers for writer + reader
//! migration.
//!
//! AGE accepts Cypher only via `SELECT * FROM cypher('graph', $$...$$ )`.
//! The Cypher source is a heredoc string (no parameter binding because
//! we route SQL through `pg_execute` which doesn't bind parameters
//! either — same constraint as the rest of the postgres helpers in
//! this crate).
//!
//! Properties are JSON-encoded with strict escape rules so the heredoc
//! cannot terminate the surrounding `$$ … $$`. Identifiers (graph
//! name, label name, vertex/edge id) are validated against the
//! `[a-zA-Z0-9_]+` shape used by [`crate::postgres::ddl::schema_name_for`]
//! so they're safe to inline.
//!
//! ## Insert pattern
//!
//! ```ignore
//! let sql = cypher_merge_edge(
//!     "axon_graph",
//!     "File", "F::src/main.rs",
//!     "CONTAINS", &serde_json::json!({"project_code": "AXO"}),
//!     "Symbol", "S::AXO::main",
//! );
//! graph_store.execute(&sql)?;
//! ```
//!
//! ## Query pattern
//!
//! ```ignore
//! let sql = cypher_query(
//!     "axon_graph",
//!     "MATCH (f:File)-[:CONTAINS]->(s:Symbol) \
//!      WHERE s.project_code = 'AXO' \
//!      RETURN f.path, s.name",
//!     &["path", "name"],
//! );
//! let rows = graph_store.query_json(&sql)?;
//! ```

use anyhow::{anyhow, Result};

/// Parse an agtype list-of-strings rendered by `pg_query_json` for
/// readers that project simple string lists. Accepts the canonical
/// JSON form (`["a", "b"]`) and forms with a trailing `::ident`
/// suffix that some AGE versions append (e.g. `::path`, `::list`).
/// Returns `None` on any unexpected shape so the caller can fall
/// back to SQL instead of misinterpreting the result.
pub fn parse_agtype_string_list(raw: &str) -> Option<Vec<String>> {
    let trimmed = raw.trim();
    let cleaned = strip_trailing_type_suffix(trimmed);
    let parsed: serde_json::Value = serde_json::from_str(cleaned).ok()?;
    let arr = parsed.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item.as_str()?;
        out.push(s.to_string());
    }
    Some(out)
}

/// Parse an agtype list-of-vertices and extract one property from
/// each. AGE returns vertex lists from `nodes(path)` as
/// `[{"id":<int>, "label":<str>, "properties":{…}}::vertex, …]`.
/// We strip every `::vertex` (and other `::ident`) suffix, parse the
/// cleaned JSON, and for each element pull `properties.<prop>` as a
/// string. Returns `None` on unexpected shape so callers fall back
/// to SQL.
pub fn parse_agtype_vertex_list_property(raw: &str, prop: &str) -> Option<Vec<String>> {
    let cleaned = strip_agtype_value_suffixes(raw.trim());
    let parsed: serde_json::Value = serde_json::from_str(&cleaned).ok()?;
    let arr = parsed.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let value = item
            .get("properties")
            .and_then(|p| p.get(prop))
            .and_then(|v| v.as_str())?;
        out.push(value.to_string());
    }
    Some(out)
}

/// Remove a trailing `::ident` suffix from an agtype scalar/list
/// rendering (e.g. `"x"::string`, `["a","b"]::path`). No-op if the
/// suffix is absent or the segment after `::` contains non-ident
/// characters.
fn strip_trailing_type_suffix(s: &str) -> &str {
    if let Some(idx) = s.rfind("::") {
        let suffix = &s[idx + 2..];
        if !suffix.is_empty()
            && suffix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return &s[..idx];
        }
    }
    s
}

/// Strip every embedded `::ident` suffix that AGE injects into the
/// JSON rendering of compound values. Used by
/// `parse_agtype_vertex_list_property` to clean the
/// `[{...}::vertex, {...}::vertex]` shape into parseable JSON.
fn strip_agtype_value_suffixes(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        // Skip ::ident segments that appear after a `}` or `]` or `"`
        // (i.e. directly after a JSON value's closing token).
        if c == ':' && chars.peek() == Some(&':') {
            // Peek the previous emitted char to decide if this is a
            // type suffix or a normal substring (e.g. `AXO::main` in a
            // string value, which would already be JSON-quoted and
            // shouldn't reach this branch).
            if matches!(out.chars().last(), Some('}') | Some(']') | Some('"')) {
                chars.next(); // consume second ':'
                while let Some(&peek) = chars.peek() {
                    if peek.is_ascii_alphanumeric() || peek == '_' {
                        chars.next();
                    } else {
                        break;
                    }
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Read-once env knob that gates the option B.3 AGE reader transition.
///
/// Default: **ON** (post 2026-05-08 session 4 phase 11, after all 6
/// graph-traversal readers shipped + smoke-test validated against
/// `axon-test/age-pgvector:pg17`).
///
/// Behaviour:
/// - Default ON: MCP graph-traversal readers (`path`, `impact`,
///   `bidi_trace`, `anomalies`, `architectural_drift` call-graph
///   section) try the AGE Cypher MATCH first under PG. On empty
///   result or error they fall back to the legacy SQL relation-table
///   read — zero regression for PG installs without dual-write
///   populated.
/// - Set `AXON_AGE_READ=0` (or `false` / `no` / `off`) to force the
///   legacy SQL path explicitly. Useful for benchmarking AGE vs SQL
///   parity or recovering from an AGE schema regression.
/// - Once B.4 drops the SQL relation tables (REQ-AXO-216), the gate
///   disappears entirely and AGE becomes the sole read path.
pub fn age_read_enabled() -> bool {
    std::env::var("AXON_AGE_READ")
        .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"))
        .unwrap_or(true)
}

/// Validate that an identifier (graph / label / vertex id) is safe to
/// inline in a Cypher heredoc. Accepts ASCII alphanumerics, underscore,
/// `:` and `-` (the chunk_id format used elsewhere in Axon contains
/// these). Anything that could terminate the heredoc or sneak special
/// Cypher syntax is rejected.
pub fn validate_identifier(value: &str, kind: &'static str) -> Result<()> {
    if value.is_empty() {
        return Err(anyhow!("AGE identifier ({kind}) is empty"));
    }
    if value.len() > 256 {
        return Err(anyhow!("AGE identifier ({kind}) too long: {}", value.len()));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-' | '.' | '/'))
    {
        return Err(anyhow!(
            "AGE identifier ({kind}) contains characters outside [a-zA-Z0-9_:./-]: {}",
            value
        ));
    }
    Ok(())
}

/// Render a JSON value as a Cypher property map literal — `{k: v, …}`.
/// String values are double-quoted with `\` and `"` escaped. Numbers,
/// booleans, and nulls pass through literally. Nested objects /
/// arrays are flattened to JSON strings (rare in our schema).
pub fn cypher_props_literal(props: &serde_json::Value) -> Result<String> {
    let map = props.as_object().ok_or_else(|| {
        anyhow!("cypher_props_literal expects a JSON object, got {props:?}")
    })?;
    let mut buf = String::with_capacity(64);
    buf.push('{');
    let mut first = true;
    for (key, value) in map {
        if !first {
            buf.push_str(", ");
        }
        first = false;
        // Property key: same shape rule as identifiers.
        validate_identifier(key, "property key")?;
        buf.push_str(key);
        buf.push_str(": ");
        match value {
            serde_json::Value::Null => buf.push_str("null"),
            serde_json::Value::Bool(b) => buf.push_str(if *b { "true" } else { "false" }),
            serde_json::Value::Number(n) => buf.push_str(&n.to_string()),
            serde_json::Value::String(s) => {
                buf.push('"');
                for ch in s.chars() {
                    match ch {
                        '"' => buf.push_str("\\\""),
                        '\\' => buf.push_str("\\\\"),
                        '\n' => buf.push_str("\\n"),
                        '\r' => buf.push_str("\\r"),
                        '\t' => buf.push_str("\\t"),
                        c if c.is_control() => {
                            return Err(anyhow!(
                                "Cypher property string contains a control character (U+{:04X})",
                                c as u32
                            ));
                        }
                        c => buf.push(c),
                    }
                }
                buf.push('"');
            }
            // Nested arrays/objects → encode as JSON string (rare).
            other => {
                buf.push('"');
                let nested = serde_json::to_string(other)
                    .map_err(|e| anyhow!("cannot serialise nested property: {e}"))?;
                for ch in nested.chars() {
                    match ch {
                        '"' => buf.push_str("\\\""),
                        '\\' => buf.push_str("\\\\"),
                        c => buf.push(c),
                    }
                }
                buf.push('"');
            }
        }
    }
    buf.push('}');
    Ok(buf)
}

/// Build the SQL wrapper that AGE requires around a Cypher fragment
/// that returns nothing (CREATE / MERGE without RETURN).
fn cypher_void_wrapper(graph: &str, body: &str) -> String {
    // AGE requires a RETURN expression in the column list of the
    // outer SELECT, even for write-only Cypher. We return the literal
    // `1` to satisfy the wrapper.
    format!(
        "SELECT * FROM cypher('{graph}', $$\n\
         {body}\n\
         RETURN 1\n\
         $$) AS (_ag_void agtype)"
    )
}

/// Idempotently MERGE a vertex with the given id and properties.
/// `id_property` defaults to `id` per the writer convention; pass a
/// different one for tables whose PK is named differently (e.g.
/// `path` for File).
pub fn cypher_merge_vertex(
    graph: &str,
    label: &str,
    id_property: &str,
    id_value: &str,
    props: &serde_json::Value,
) -> Result<String> {
    validate_identifier(graph, "graph")?;
    validate_identifier(label, "vertex label")?;
    validate_identifier(id_property, "id property")?;
    validate_identifier(id_value, "vertex id value")?;
    let props_lit = cypher_props_literal(props)?;
    let body = format!(
        "MERGE (n:{label} {{{id_property}: \"{id_value}\"}}) SET n += {props_lit}"
    );
    Ok(cypher_void_wrapper(graph, &body))
}

/// Idempotently MERGE an edge from `(src_label, src_id_value)` to
/// `(dst_label, dst_id_value)` with the given edge label and
/// properties. The endpoints are MERGEd as well so the edge insert
/// is self-contained — callers don't need to pre-create vertices.
///
/// `src_id_property` and `dst_id_property` default to `id` for Symbol
/// / Chunk; use `path` for File endpoints.
#[allow(clippy::too_many_arguments)]
pub fn cypher_merge_edge(
    graph: &str,
    src_label: &str,
    src_id_property: &str,
    src_id_value: &str,
    edge_label: &str,
    edge_props: &serde_json::Value,
    dst_label: &str,
    dst_id_property: &str,
    dst_id_value: &str,
) -> Result<String> {
    validate_identifier(graph, "graph")?;
    validate_identifier(src_label, "source label")?;
    validate_identifier(src_id_property, "source id property")?;
    validate_identifier(src_id_value, "source id value")?;
    validate_identifier(edge_label, "edge label")?;
    validate_identifier(dst_label, "destination label")?;
    validate_identifier(dst_id_property, "destination id property")?;
    validate_identifier(dst_id_value, "destination id value")?;
    let edge_props_lit = cypher_props_literal(edge_props)?;
    let body = format!(
        "MERGE (a:{src_label} {{{src_id_property}: \"{src_id_value}\"}}) \
         MERGE (b:{dst_label} {{{dst_id_property}: \"{dst_id_value}\"}}) \
         MERGE (a)-[r:{edge_label}]->(b) \
         SET r += {edge_props_lit}"
    );
    Ok(cypher_void_wrapper(graph, &body))
}

/// Batch helper for option B.2 wire-up: emit one Cypher MERGE
/// statement per edge triple. Each edge is materialised as a
/// separate Cypher statement (one heredoc per edge) so failures
/// stay isolated and the SQL row-per-edge semantics of the
/// existing relation tables are preserved one-for-one. Empty
/// `edges` returns `Ok(vec![])` rather than erroring — callers
/// can compose this with empty batches without special-casing.
///
/// Identifier validation runs once per fixed argument (graph /
/// label / property) and once per edge endpoint (src_id /
/// dst_id). Property maps are validated and serialised by
/// [`cypher_props_literal`] inside [`cypher_merge_edge`].
pub fn cypher_merge_edges_batch(
    graph: &str,
    src_label: &str,
    src_id_property: &str,
    edge_label: &str,
    dst_label: &str,
    dst_id_property: &str,
    edges: &[(String, String, serde_json::Value)],
) -> Result<Vec<String>> {
    if edges.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(edges.len());
    for (src_id, dst_id, props) in edges {
        let sql = cypher_merge_edge(
            graph,
            src_label,
            src_id_property,
            src_id,
            edge_label,
            props,
            dst_label,
            dst_id_property,
            dst_id,
        )?;
        out.push(sql);
    }
    Ok(out)
}

/// Batch helper for option B.2 vertex enrichment: emit one Cypher
/// MERGE statement per (id, props) pair so the AGE graph carries
/// the same searchable fields as the SQL `Symbol` / `File` tables
/// (name, kind, is_nif, project_code …). Vertices with the same id
/// are deduplicated by AGE itself via the MERGE semantics; the
/// helper does not pre-dedup.
///
/// Empty `vertices` returns `Ok(vec![])`. Identifier validation
/// errors propagate from `cypher_merge_vertex`.
pub fn cypher_merge_vertices_batch(
    graph: &str,
    label: &str,
    id_property: &str,
    vertices: &[(String, serde_json::Value)],
) -> Result<Vec<String>> {
    if vertices.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(vertices.len());
    for (id_value, props) in vertices {
        let sql = cypher_merge_vertex(graph, label, id_property, id_value, props)?;
        out.push(sql);
    }
    Ok(out)
}

/// Compose a SQL query that wraps a read-side Cypher MATCH. The
/// caller passes the Cypher RETURN column names (in order) so the
/// AS clause receives the right `(name agtype, …)` declarations.
/// Empty `return_cols` is rejected — Cypher MATCH must return at
/// least one column.
pub fn cypher_query(graph: &str, cypher: &str, return_cols: &[&str]) -> Result<String> {
    validate_identifier(graph, "graph")?;
    if return_cols.is_empty() {
        return Err(anyhow!("cypher_query requires at least one RETURN column"));
    }
    if cypher.contains("$$") {
        return Err(anyhow!(
            "Cypher body cannot contain `$$` (terminates the heredoc)"
        ));
    }
    for col in return_cols {
        validate_identifier(col, "return column alias")?;
    }
    let cols = return_cols
        .iter()
        .map(|c| format!("{c} agtype"))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "SELECT * FROM cypher('{graph}', $$\n\
         {cypher}\n\
         $$) AS ({cols})"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_identifier_accepts_axon_id_shapes() {
        for ok in [
            "AXO::McpServer",
            "F::src/axon-core/src/mcp.rs",
            "axon_graph",
            "Symbol",
            "CONTAINS",
            "chunk-1234.5",
        ] {
            assert!(
                validate_identifier(ok, "test").is_ok(),
                "expected {ok} to validate"
            );
        }
    }

    #[test]
    fn validate_identifier_rejects_injection_attempts() {
        for bad in [
            "",
            "abc; DROP TABLE",
            "abc'", // single-quote
            "abc\"", // double-quote
            "abc\nMATCH", // newline
            "abc$$rest", // heredoc terminator
            "abc{}", // brace
            "abc(",
        ] {
            assert!(
                validate_identifier(bad, "test").is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn props_literal_serialises_basic_types() {
        let props = serde_json::json!({
            "project_code": "AXO",
            "kind": "struct",
            "is_public": true,
            "size": 1024,
            "missing": null
        });
        let out = cypher_props_literal(&props).unwrap();
        assert!(out.starts_with('{'));
        assert!(out.ends_with('}'));
        assert!(out.contains("project_code: \"AXO\""));
        assert!(out.contains("kind: \"struct\""));
        assert!(out.contains("is_public: true"));
        assert!(out.contains("size: 1024"));
        assert!(out.contains("missing: null"));
    }

    #[test]
    fn props_literal_escapes_double_quotes_and_backslashes() {
        let props = serde_json::json!({
            "name": "He said \"hi\\bye\""
        });
        let out = cypher_props_literal(&props).unwrap();
        // Source value: He said "hi\bye"
        // Cypher escape: He said \"hi\\bye\"
        assert!(out.contains("name: \"He said \\\"hi\\\\bye\\\"\""));
    }

    #[test]
    fn props_literal_rejects_control_chars() {
        let props = serde_json::json!({
            "bad": "x\u{0007}y"
        });
        assert!(cypher_props_literal(&props).is_err());
    }

    #[test]
    fn props_literal_rejects_non_object_root() {
        let props = serde_json::json!([1, 2, 3]);
        assert!(cypher_props_literal(&props).is_err());
    }

    #[test]
    fn merge_vertex_emits_merge_with_label_and_id() {
        let sql = cypher_merge_vertex(
            "axon_graph",
            "Symbol",
            "id",
            "AXO::McpServer",
            &serde_json::json!({"name": "McpServer", "kind": "struct", "project_code": "AXO"}),
        )
        .unwrap();
        assert!(sql.contains("SELECT * FROM cypher('axon_graph'"));
        assert!(sql.contains("MERGE (n:Symbol {id: \"AXO::McpServer\"})"));
        assert!(sql.contains("SET n +="));
        assert!(sql.contains("name: \"McpServer\""));
        assert!(sql.contains("project_code: \"AXO\""));
        assert!(sql.contains("RETURN 1"));
        assert!(sql.contains("AS (_ag_void agtype)"));
    }

    #[test]
    fn merge_edge_emits_three_merges_and_sets_props() {
        let sql = cypher_merge_edge(
            "axon_graph",
            "File",
            "path",
            "src/main.rs",
            "CONTAINS",
            &serde_json::json!({"project_code": "AXO"}),
            "Symbol",
            "id",
            "AXO::main",
        )
        .unwrap();
        // Three MERGEs (vertices + edge), one SET on the edge.
        assert_eq!(sql.matches("MERGE ").count(), 3, "expected 3 MERGE clauses");
        assert!(sql.contains("MERGE (a:File {path: \"src/main.rs\"})"));
        assert!(sql.contains("MERGE (b:Symbol {id: \"AXO::main\"})"));
        assert!(sql.contains("MERGE (a)-[r:CONTAINS]->(b)"));
        assert!(sql.contains("SET r +="));
        assert!(sql.contains("project_code: \"AXO\""));
    }

    #[test]
    fn merge_edge_rejects_invalid_label() {
        let bad = cypher_merge_edge(
            "axon_graph",
            "File",
            "path",
            "p",
            "CONTAINS;DROP TABLE",
            &serde_json::json!({}),
            "Symbol",
            "id",
            "s",
        );
        assert!(bad.is_err());
    }

    #[test]
    fn cypher_query_wraps_match_with_typed_columns() {
        let sql = cypher_query(
            "axon_graph",
            "MATCH (f:File)-[:CONTAINS]->(s:Symbol) WHERE s.project_code = 'AXO' \
             RETURN f.path, s.name",
            &["fpath", "sname"],
        )
        .unwrap();
        assert!(sql.contains("SELECT * FROM cypher('axon_graph'"));
        assert!(sql.contains("MATCH (f:File)-[:CONTAINS]->(s:Symbol)"));
        assert!(sql.contains("AS (fpath agtype, sname agtype)"));
    }

    #[test]
    fn cypher_query_rejects_heredoc_terminator() {
        let bad = cypher_query("axon_graph", "MATCH (n) $$ RETURN n", &["n"]);
        assert!(bad.is_err());
    }

    #[test]
    fn cypher_query_rejects_empty_return_cols() {
        let bad = cypher_query("axon_graph", "MATCH (n) RETURN n", &[]);
        assert!(bad.is_err());
    }

    #[test]
    fn merge_edges_batch_empty_returns_empty_vec() {
        let out = cypher_merge_edges_batch(
            "axon_graph",
            "File",
            "path",
            "CONTAINS",
            "Symbol",
            "id",
            &[],
        )
        .unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn merge_edges_batch_emits_one_merge_per_edge() {
        let edges = vec![
            (
                "src/main.rs".to_string(),
                "AXO::main".to_string(),
                serde_json::json!({"project_code": "AXO"}),
            ),
            (
                "src/lib.rs".to_string(),
                "AXO::lib".to_string(),
                serde_json::json!({"project_code": "AXO"}),
            ),
            (
                "src/util.rs".to_string(),
                "AXO::util".to_string(),
                serde_json::json!({"project_code": "AXO"}),
            ),
        ];
        let out = cypher_merge_edges_batch(
            "axon_graph",
            "File",
            "path",
            "CONTAINS",
            "Symbol",
            "id",
            &edges,
        )
        .unwrap();
        assert_eq!(out.len(), 3);
        for (i, sql) in out.iter().enumerate() {
            let (src, dst, _) = &edges[i];
            assert!(sql.contains(&format!("MERGE (a:File {{path: \"{src}\"}})")));
            assert!(sql.contains(&format!("MERGE (b:Symbol {{id: \"{dst}\"}})")));
            assert!(sql.contains("MERGE (a)-[r:CONTAINS]->(b)"));
            assert!(sql.contains("project_code: \"AXO\""));
        }
    }

    #[test]
    fn parse_agtype_string_list_canonical_json() {
        let raw = r#"["AXO::main", "AXO::lib", "AXO::util"]"#;
        let out = parse_agtype_string_list(raw).unwrap();
        assert_eq!(out, vec!["AXO::main", "AXO::lib", "AXO::util"]);
    }

    #[test]
    fn parse_agtype_string_list_with_suffix_strip() {
        // AGE may append `::path` or similar.
        let raw = r#"["a", "b"]::path"#;
        let out = parse_agtype_string_list(raw).unwrap();
        assert_eq!(out, vec!["a", "b"]);
    }

    #[test]
    fn parse_agtype_string_list_empty_array() {
        let out = parse_agtype_string_list("[]").unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn parse_agtype_string_list_rejects_non_string_items() {
        let out = parse_agtype_string_list(r#"[1, 2, 3]"#);
        assert!(out.is_none());
    }

    #[test]
    fn parse_agtype_string_list_rejects_garbage() {
        assert!(parse_agtype_string_list("garbage").is_none());
        assert!(parse_agtype_string_list("").is_none());
    }

    #[test]
    fn parse_agtype_vertex_list_property_extracts_name() {
        // Real AGE output from `RETURN nodes(path)` (validated against
        // axon-test/age-pgvector:pg17 container 2026-05-08).
        let raw = r#"[{"id": 844424930131969, "label": "Symbol", "properties": {"id": "AXO::main", "kind": "fn", "name": "main", "is_nif": false, "project_code": "AXO"}}::vertex, {"id": 844424930131970, "label": "Symbol", "properties": {"id": "AXO::lib", "kind": "mod", "name": "lib", "is_nif": false, "project_code": "AXO"}}::vertex, {"id": 844424930131969, "label": "Symbol", "properties": {"id": "AXO::main", "kind": "fn", "name": "main", "is_nif": false, "project_code": "AXO"}}::vertex]"#;
        let names = parse_agtype_vertex_list_property(raw, "name").unwrap();
        assert_eq!(names, vec!["main", "lib", "main"]);
    }

    #[test]
    fn parse_agtype_vertex_list_property_extracts_id() {
        let raw = r#"[{"id": 1, "label": "Symbol", "properties": {"id": "AXO::a"}}::vertex, {"id": 2, "label": "Symbol", "properties": {"id": "AXO::b"}}::vertex]"#;
        let ids = parse_agtype_vertex_list_property(raw, "id").unwrap();
        assert_eq!(ids, vec!["AXO::a", "AXO::b"]);
    }

    #[test]
    fn parse_agtype_vertex_list_property_missing_field_returns_none() {
        let raw = r#"[{"id": 1, "label": "Symbol", "properties": {"id": "AXO::a"}}::vertex]"#;
        assert!(parse_agtype_vertex_list_property(raw, "nonexistent").is_none());
    }

    #[test]
    fn parse_agtype_vertex_list_property_empty_array() {
        let out = parse_agtype_vertex_list_property("[]", "name").unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn strip_agtype_value_suffixes_removes_vertex_markers() {
        let cleaned = strip_agtype_value_suffixes(r#"[{"x":1}::vertex, {"y":2}::vertex]"#);
        assert_eq!(cleaned, r#"[{"x":1}, {"y":2}]"#);
    }

    #[test]
    fn strip_agtype_value_suffixes_preserves_string_double_colons() {
        // Double-colon inside a JSON string literal must NOT be
        // stripped (e.g. our symbol_id format `AXO::main`).
        let cleaned = strip_agtype_value_suffixes(r#"{"id": "AXO::main"}::vertex"#);
        assert_eq!(cleaned, r#"{"id": "AXO::main"}"#);
    }

    #[test]
    fn merge_vertices_batch_empty_returns_empty_vec() {
        let out = cypher_merge_vertices_batch("axon_graph", "Symbol", "id", &[]).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn merge_vertices_batch_emits_one_merge_per_vertex() {
        let vertices = vec![
            (
                "AXO::main".to_string(),
                serde_json::json!({"name": "main", "kind": "fn", "project_code": "AXO"}),
            ),
            (
                "AXO::lib".to_string(),
                serde_json::json!({"name": "lib", "kind": "mod", "project_code": "AXO"}),
            ),
        ];
        let out =
            cypher_merge_vertices_batch("axon_graph", "Symbol", "id", &vertices).unwrap();
        assert_eq!(out.len(), 2);
        assert!(out[0].contains("MERGE (n:Symbol {id: \"AXO::main\"})"));
        assert!(out[0].contains("name: \"main\""));
        assert!(out[0].contains("kind: \"fn\""));
        assert!(out[1].contains("MERGE (n:Symbol {id: \"AXO::lib\"})"));
        assert!(out[1].contains("name: \"lib\""));
    }

    #[test]
    fn merge_edges_batch_propagates_validation_error() {
        let edges = vec![
            (
                "ok-src".to_string(),
                "ok-dst".to_string(),
                serde_json::json!({}),
            ),
        ];
        let bad = cypher_merge_edges_batch(
            "axon_graph",
            "File",
            "path",
            "CONTAINS;DROP TABLE",
            "Symbol",
            "id",
            &edges,
        );
        assert!(bad.is_err());
    }
}
