// Copyright (c) Didier Stadelmann. All rights reserved.

use duckdb::Connection;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
struct Args {
    ist_db: PathBuf,
    soll_db: PathBuf,
    out_root: PathBuf,
    publication_id: String,
    project_code: String,
    all_projects: bool,
    retain_successful: usize,
}

fn usage() -> String {
    [
        "usage: memgraph-publication [--ist-db PATH] [--soll-db PATH] [--out-dir DIR] [--publication-id ID] [--project-code CODE] [--all-projects] [--project-only] [--retain-successful N]",
        "",
        "Publishes a human-only IST/SOLL graph projection as versioned Parquet.",
        "By default, publication covers all projects. Use --project-only for a diagnostic single-project export.",
        "The command reads only Axon-controlled reader/snapshot databases and writes a disposable publication directory.",
        "LLM clients must continue to use Axon MCP, not Memgraph, as their source of truth.",
    ]
    .join("\n")
}

fn default_ist_db() -> PathBuf {
    PathBuf::from(".axon/graph_v2/ist-reader.db")
}

fn default_soll_db() -> PathBuf {
    let canonical = PathBuf::from(".axon/graph_v2/soll.db");
    if canonical.exists() {
        return canonical;
    }
    PathBuf::from(".axon/graph_v2/soll-mirror.db")
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn parse_args() -> Result<Args, String> {
    let mut args = env::args().skip(1);
    let generated_id = format!("pub-{}", now_unix_ms());
    let mut parsed = Args {
        ist_db: default_ist_db(),
        soll_db: default_soll_db(),
        out_root: PathBuf::from(".axon/memgraph/publications"),
        publication_id: generated_id,
        project_code: "AXO".to_string(),
        all_projects: true,
        retain_successful: 2,
    };

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ist-db" => {
                parsed.ist_db = PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--ist-db requires a path".to_string())?,
                );
            }
            "--soll-db" => {
                parsed.soll_db = PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--soll-db requires a path".to_string())?,
                );
            }
            "--out-dir" => {
                parsed.out_root = PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--out-dir requires a directory".to_string())?,
                );
            }
            "--publication-id" => {
                parsed.publication_id = args
                    .next()
                    .ok_or_else(|| "--publication-id requires an id".to_string())?;
            }
            "--project-code" => {
                parsed.project_code = args
                    .next()
                    .ok_or_else(|| "--project-code requires a code".to_string())?;
            }
            "--all-projects" => {
                parsed.all_projects = true;
            }
            "--project-only" => {
                parsed.all_projects = false;
            }
            "--retain-successful" => {
                let raw = args
                    .next()
                    .ok_or_else(|| "--retain-successful requires a number".to_string())?;
                parsed.retain_successful = raw
                    .parse::<usize>()
                    .map_err(|err| format!("invalid --retain-successful value {raw}: {err}"))?;
            }
            "--help" | "-h" => return Err(usage()),
            other => return Err(format!("unknown option: {other}\n\n{}", usage())),
        }
    }

    if parsed.publication_id.trim().is_empty() {
        return Err("--publication-id must not be empty".to_string());
    }

    Ok(parsed)
}

fn sql_string(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "''"))
}

fn sql_path(path: &Path) -> String {
    sql_string(&path.display().to_string())
}

fn execute(connection: &Connection, sql: &str) -> Result<(), String> {
    connection
        .execute_batch(sql)
        .map_err(|err| format!("failed SQL:\n{sql}\n\n{err}"))
}

fn probe(connection: &Connection, sql: &str) -> bool {
    connection.prepare(sql).is_ok()
}

fn source_exists(connection: &Connection, catalog: &str, table: &str) -> bool {
    probe(
        connection,
        &format!("SELECT 1 FROM {catalog}.{table} LIMIT 0"),
    )
}

fn column_exists(connection: &Connection, catalog: &str, table: &str, column: &str) -> bool {
    probe(
        connection,
        &format!("SELECT {column} FROM {catalog}.{table} LIMIT 0"),
    )
}

fn first_existing_column<'a>(
    connection: &Connection,
    catalog: &str,
    table: &str,
    candidates: &'a [&'a str],
) -> Option<&'a str> {
    candidates
        .iter()
        .copied()
        .find(|column| column_exists(connection, catalog, table, column))
}

fn text_expr(
    connection: &Connection,
    catalog: &str,
    table: &str,
    candidates: &[&str],
    fallback: &str,
) -> String {
    first_existing_column(connection, catalog, table, candidates)
        .map(|column| format!("CAST({column} AS VARCHAR)"))
        .unwrap_or_else(|| sql_string(fallback))
}

fn project_filter(
    connection: &Connection,
    catalog: &str,
    table: &str,
    project_code: &str,
    all_projects: bool,
) -> String {
    if all_projects || !column_exists(connection, catalog, table, "project_code") {
        return String::new();
    }
    format!(
        " AND CAST(project_code AS VARCHAR) = {}",
        sql_string(project_code)
    )
}

fn bool_expr(
    connection: &Connection,
    catalog: &str,
    table: &str,
    candidates: &[&str],
    fallback: bool,
) -> String {
    first_existing_column(connection, catalog, table, candidates)
        .map(|column| format!("COALESCE(CAST({column} AS BOOLEAN), {fallback})"))
        .unwrap_or_else(|| fallback.to_string())
}

fn number_expr(
    connection: &Connection,
    catalog: &str,
    table: &str,
    candidates: &[&str],
    fallback: i64,
) -> String {
    first_existing_column(connection, catalog, table, candidates)
        .map(|column| format!("COALESCE(CAST({column} AS BIGINT), {fallback})"))
        .unwrap_or_else(|| fallback.to_string())
}

fn count_rows(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap_or(0)
}

fn file_freshness(path: &Path) -> Value {
    let metadata = fs::metadata(path);
    let Ok(metadata) = metadata else {
        return json!({
            "path": path.display().to_string(),
            "exists": false,
        });
    };
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64);
    json!({
        "path": path.display().to_string(),
        "exists": true,
        "size_bytes": metadata.len(),
        "modified_at_ms": modified_ms,
    })
}

fn create_project_nodes(
    connection: &Connection,
    generated_at_ms: i64,
    project_code: &str,
    all_projects: bool,
) -> Result<(), String> {
    if source_exists(connection, "ist", "Project") {
        let id = text_expr(
            connection,
            "ist",
            "Project",
            &["code", "project_code", "id", "name"],
            project_code,
        );
        let code = text_expr(
            connection,
            "ist",
            "Project",
            &["project_code", "code", "id", "name"],
            project_code,
        );
        let title = text_expr(
            connection,
            "ist",
            "Project",
            &["name", "title", "project_code", "code"],
            project_code,
        );
        let where_clause = if all_projects {
            String::new()
        } else {
            format!(" WHERE {code} = {}", sql_string(project_code))
        };
        execute(
            connection,
            &format!(
                "CREATE OR REPLACE TEMP TABLE node_project AS
                 SELECT 'project:' || {id} AS id,
                        'Project' AS label,
                        {code} AS project_code,
                        {title} AS title,
                        'IST' AS source,
                        {generated_at_ms} AS publication_generated_at_ms,
                        NULL::VARCHAR AS path,
                        NULL::VARCHAR AS kind,
                        NULL::VARCHAR AS status,
                        NULL::BOOLEAN AS graph_ready,
                        NULL::BOOLEAN AS vector_ready,
                        NULL::BIGINT AS size_bytes,
                        NULL::VARCHAR AS description,
                        NULL::VARCHAR AS metadata
                 FROM ist.Project{where_clause};"
            ),
        )
    } else {
        execute(
            connection,
            &format!(
                "CREATE OR REPLACE TEMP TABLE node_project AS
                 SELECT 'project:' || {code} AS id,
                        'Project' AS label,
                        {code} AS project_code,
                        {code} AS title,
                        'fallback' AS source,
                        {generated_at_ms} AS publication_generated_at_ms,
                        NULL::VARCHAR AS path,
                        NULL::VARCHAR AS kind,
                        NULL::VARCHAR AS status,
                        NULL::BOOLEAN AS graph_ready,
                        NULL::BOOLEAN AS vector_ready,
                        NULL::BIGINT AS size_bytes,
                        NULL::VARCHAR AS description,
                        NULL::VARCHAR AS metadata;",
                code = sql_string(project_code)
            ),
        )
    }
}

fn create_file_nodes(
    connection: &Connection,
    project_code: &str,
    all_projects: bool,
) -> Result<(), String> {
    if !source_exists(connection, "ist", "File") {
        return execute(
            connection,
            "CREATE OR REPLACE TEMP TABLE node_file AS SELECT * FROM node_project WHERE false;",
        );
    }
    let path = text_expr(connection, "ist", "File", &["path", "id"], "");
    let project = text_expr(connection, "ist", "File", &["project_code"], project_code);
    let status = text_expr(connection, "ist", "File", &["status"], "unknown");
    let graph_ready = bool_expr(connection, "ist", "File", &["graph_ready"], false);
    let vector_ready = bool_expr(connection, "ist", "File", &["vector_ready"], false);
    let size = number_expr(connection, "ist", "File", &["size", "size_bytes"], 0);
    let filter = project_filter(connection, "ist", "File", project_code, all_projects);
    execute(
        connection,
        &format!(
            "CREATE OR REPLACE TEMP TABLE node_file AS
             SELECT 'file:' || {path} AS id,
                    'File' AS label,
                    {project} AS project_code,
                    {path} AS title,
                    'IST' AS source,
                    NULL::BIGINT AS publication_generated_at_ms,
                    {path} AS path,
                    NULL::VARCHAR AS kind,
                    {status} AS status,
                    {graph_ready} AS graph_ready,
                    {vector_ready} AS vector_ready,
                    {size} AS size_bytes,
                    NULL::VARCHAR AS description,
                    NULL::VARCHAR AS metadata
             FROM ist.File
             WHERE {path} <> ''{filter};"
        ),
    )
}

fn create_symbol_nodes(
    connection: &Connection,
    project_code: &str,
    all_projects: bool,
) -> Result<(), String> {
    if !source_exists(connection, "ist", "Symbol") {
        return execute(
            connection,
            "CREATE OR REPLACE TEMP TABLE node_symbol AS SELECT * FROM node_project WHERE false;",
        );
    }
    let raw_id = text_expr(connection, "ist", "Symbol", &["id", "name"], "");
    let project = text_expr(connection, "ist", "Symbol", &["project_code"], project_code);
    let name = text_expr(connection, "ist", "Symbol", &["name", "id"], "");
    let kind = text_expr(
        connection,
        "ist",
        "Symbol",
        &["kind", "symbol_kind"],
        "symbol",
    );
    let status = text_expr(connection, "ist", "Symbol", &["status"], "known");
    let filter = project_filter(connection, "ist", "Symbol", project_code, all_projects);
    execute(
        connection,
        &format!(
            "CREATE OR REPLACE TEMP TABLE node_symbol AS
             SELECT 'symbol:' || {raw_id} AS id,
                    'Symbol' AS label,
                    {project} AS project_code,
                    {name} AS title,
                    'IST' AS source,
                    NULL::BIGINT AS publication_generated_at_ms,
                    NULL::VARCHAR AS path,
                    {kind} AS kind,
                    {status} AS status,
                    NULL::BOOLEAN AS graph_ready,
                    NULL::BOOLEAN AS vector_ready,
                    NULL::BIGINT AS size_bytes,
                    NULL::VARCHAR AS description,
                    NULL::VARCHAR AS metadata
             FROM ist.Symbol
             WHERE {raw_id} <> ''{filter};"
        ),
    )
}

fn create_chunk_nodes(
    connection: &Connection,
    project_code: &str,
    all_projects: bool,
) -> Result<(), String> {
    if !source_exists(connection, "ist", "Chunk") {
        return execute(
            connection,
            "CREATE OR REPLACE TEMP TABLE node_chunk AS SELECT * FROM node_project WHERE false;",
        );
    }
    let raw_id = text_expr(connection, "ist", "Chunk", &["id", "chunk_id"], "");
    let project = text_expr(connection, "ist", "Chunk", &["project_code"], project_code);
    let file_path = text_expr(connection, "ist", "Chunk", &["file_path", "path"], "");
    let text = text_expr(connection, "ist", "Chunk", &["text", "content", "body"], "");
    let filter = project_filter(connection, "ist", "Chunk", project_code, all_projects);
    execute(
        connection,
        &format!(
            "CREATE OR REPLACE TEMP TABLE node_chunk AS
             SELECT 'chunk:' || {raw_id} AS id,
                    'Chunk' AS label,
                    {project} AS project_code,
                    {raw_id} AS title,
                    'IST' AS source,
                    NULL::BIGINT AS publication_generated_at_ms,
                    {file_path} AS path,
                    NULL::VARCHAR AS kind,
                    NULL::VARCHAR AS status,
                    NULL::BOOLEAN AS graph_ready,
                    NULL::BOOLEAN AS vector_ready,
                    NULL::BIGINT AS size_bytes,
                    substr({text}, 1, 512) AS description,
                    NULL::VARCHAR AS metadata
             FROM ist.Chunk
             WHERE {raw_id} <> ''{filter};"
        ),
    )
}

fn create_intent_nodes(
    connection: &Connection,
    project_code: &str,
    all_projects: bool,
) -> Result<(), String> {
    if !source_exists(connection, "soll", "Node") {
        return execute(
            connection,
            "CREATE OR REPLACE TEMP TABLE node_intent AS SELECT * FROM node_project WHERE false;",
        );
    }
    let raw_id = text_expr(connection, "soll", "Node", &["id", "node_id"], "");
    let project = text_expr(connection, "soll", "Node", &["project_code"], project_code);
    let intent_type = text_expr(
        connection,
        "soll",
        "Node",
        &["intent_type", "node_type", "type", "kind"],
        "Intent",
    );
    let title = text_expr(
        connection,
        "soll",
        "Node",
        &["title", "name", "label", "id"],
        "",
    );
    let description = text_expr(
        connection,
        "soll",
        "Node",
        &["description", "body", "summary"],
        "",
    );
    let status = text_expr(connection, "soll", "Node", &["status", "state"], "unknown");
    let metadata = text_expr(
        connection,
        "soll",
        "Node",
        &["metadata", "data", "payload"],
        "{}",
    );
    let filter = project_filter(connection, "soll", "Node", project_code, all_projects);
    execute(
        connection,
        &format!(
            "CREATE OR REPLACE TEMP TABLE node_intent AS
             SELECT 'intent:' || {raw_id} AS id,
                    {intent_type} AS label,
                    {project} AS project_code,
                    {title} AS title,
                    'SOLL' AS source,
                    NULL::BIGINT AS publication_generated_at_ms,
                    NULL::VARCHAR AS path,
                    {intent_type} AS kind,
                    {status} AS status,
                    NULL::BOOLEAN AS graph_ready,
                    NULL::BOOLEAN AS vector_ready,
                    NULL::BIGINT AS size_bytes,
                    {description} AS description,
                    {metadata} AS metadata
             FROM soll.Node
             WHERE {raw_id} <> ''{filter};"
        ),
    )
}

fn create_evidence_nodes(
    connection: &Connection,
    project_code: &str,
    all_projects: bool,
) -> Result<(), String> {
    if !source_exists(connection, "soll", "Traceability") {
        return execute(
            connection,
            "CREATE OR REPLACE TEMP TABLE node_evidence AS SELECT * FROM node_project WHERE false;",
        );
    }
    let entity_id = text_expr(
        connection,
        "soll",
        "Traceability",
        &["soll_entity_id", "entity_id", "source_id"],
        "",
    );
    let artifact_type = text_expr(
        connection,
        "soll",
        "Traceability",
        &["artifact_type", "target_type", "evidence_type"],
        "artifact",
    );
    let artifact_ref = text_expr(
        connection,
        "soll",
        "Traceability",
        &["artifact_ref", "target_id", "evidence_ref", "path"],
        "",
    );
    let project = text_expr(
        connection,
        "soll",
        "Traceability",
        &["project_code"],
        project_code,
    );
    let metadata = text_expr(
        connection,
        "soll",
        "Traceability",
        &["metadata", "data", "payload"],
        "{}",
    );
    let filter = project_filter(
        connection,
        "soll",
        "Traceability",
        project_code,
        all_projects,
    );
    execute(
        connection,
        &format!(
            "CREATE OR REPLACE TEMP TABLE node_evidence AS
             SELECT 'evidence:' || {entity_id} || ':' || {artifact_ref} AS id,
                    'Evidence' AS label,
                    {project} AS project_code,
                    {artifact_ref} AS title,
                    'SOLL' AS source,
                    NULL::BIGINT AS publication_generated_at_ms,
                    {artifact_ref} AS path,
                    {artifact_type} AS kind,
                    NULL::VARCHAR AS status,
                    NULL::BOOLEAN AS graph_ready,
                    NULL::BOOLEAN AS vector_ready,
                    NULL::BIGINT AS size_bytes,
                    NULL::VARCHAR AS description,
                    {metadata} AS metadata
             FROM soll.Traceability
             WHERE ({entity_id} <> '' OR {artifact_ref} <> ''){filter};"
        ),
    )
}

fn create_nodes(
    connection: &Connection,
    generated_at_ms: i64,
    project_code: &str,
    all_projects: bool,
) -> Result<(), String> {
    create_project_nodes(connection, generated_at_ms, project_code, all_projects)?;
    create_file_nodes(connection, project_code, all_projects)?;
    create_symbol_nodes(connection, project_code, all_projects)?;
    create_chunk_nodes(connection, project_code, all_projects)?;
    create_intent_nodes(connection, project_code, all_projects)?;
    create_evidence_nodes(connection, project_code, all_projects)?;
    execute(
        connection,
        "CREATE OR REPLACE TEMP TABLE nodes AS
         SELECT * FROM node_project
         UNION ALL SELECT * FROM node_file
         UNION ALL SELECT * FROM node_symbol
         UNION ALL SELECT * FROM node_chunk
         UNION ALL SELECT * FROM node_intent
         UNION ALL SELECT * FROM node_evidence;",
    )
}

fn create_empty_edges(connection: &Connection, table: &str) -> Result<(), String> {
    execute(
        connection,
        &format!(
            "CREATE OR REPLACE TEMP TABLE {table} (
                id VARCHAR,
                from_id VARCHAR,
                to_id VARCHAR,
                relation_type VARCHAR,
                source VARCHAR,
                project_code VARCHAR,
                metadata VARCHAR
             );"
        ),
    )
}

fn create_ist_binary_edge(
    connection: &Connection,
    source_table: &str,
    target_table: &str,
    relation_type: &str,
    from_prefix: &str,
    to_prefix: &str,
    project_code: &str,
    all_projects: bool,
) -> Result<(), String> {
    if !source_exists(connection, "ist", source_table) {
        return create_empty_edges(connection, target_table);
    }
    let from = text_expr(
        connection,
        "ist",
        source_table,
        &["source_id", "from_id", "src_id", "source"],
        "",
    );
    let to = text_expr(
        connection,
        "ist",
        source_table,
        &["target_id", "to_id", "dst_id", "target"],
        "",
    );
    let project = text_expr(
        connection,
        "ist",
        source_table,
        &["project_code"],
        project_code,
    );
    let filter = project_filter(connection, "ist", source_table, project_code, all_projects);
    execute(
        connection,
        &format!(
            "CREATE OR REPLACE TEMP TABLE {target_table} AS
             SELECT {relation_lit} || ':' || {from_prefix_lit} || {from} || '->' || {to_prefix_lit} || {to} AS id,
                    {from_prefix_lit} || {from} AS from_id,
                    {to_prefix_lit} || {to} AS to_id,
                    {relation_lit} AS relation_type,
                    'IST' AS source,
                    {project} AS project_code,
                    NULL::VARCHAR AS metadata
             FROM ist.{source_table}
            WHERE {from} <> '' AND {to} <> ''{filter};",
            from_prefix_lit = sql_string(from_prefix),
            relation_lit = sql_string(relation_type),
            to_prefix_lit = sql_string(to_prefix)
        ),
    )
}

fn create_soll_edges(
    connection: &Connection,
    project_code: &str,
    all_projects: bool,
) -> Result<(), String> {
    if !source_exists(connection, "soll", "Edge") {
        return create_empty_edges(connection, "edge_soll");
    }
    let from = text_expr(
        connection,
        "soll",
        "Edge",
        &["source_id", "from_id", "src_id"],
        "",
    );
    let to = text_expr(
        connection,
        "soll",
        "Edge",
        &["target_id", "to_id", "dst_id"],
        "",
    );
    let relation = text_expr(
        connection,
        "soll",
        "Edge",
        &["relation_type", "type", "kind"],
        "SOLL_EDGE",
    );
    let project = text_expr(connection, "soll", "Edge", &["project_code"], project_code);
    let metadata = text_expr(
        connection,
        "soll",
        "Edge",
        &["metadata", "data", "payload"],
        "{}",
    );
    let filter = project_filter(connection, "soll", "Edge", project_code, all_projects);
    execute(
        connection,
        &format!(
            "CREATE OR REPLACE TEMP TABLE edge_soll AS
             SELECT upper({relation}) || ':' || {from} || '->' || {to} AS id,
                    'intent:' || {from} AS from_id,
                    'intent:' || {to} AS to_id,
                    upper({relation}) AS relation_type,
                    'SOLL' AS source,
                    {project} AS project_code,
                    {metadata} AS metadata
             FROM soll.Edge
             WHERE {from} <> '' AND {to} <> ''{filter};"
        ),
    )
}

fn create_traceability_edges(
    connection: &Connection,
    project_code: &str,
    all_projects: bool,
) -> Result<(), String> {
    if !source_exists(connection, "soll", "Traceability") {
        return create_empty_edges(connection, "edge_traceability");
    }
    let entity_id = text_expr(
        connection,
        "soll",
        "Traceability",
        &["soll_entity_id", "entity_id", "source_id"],
        "",
    );
    let artifact_ref = text_expr(
        connection,
        "soll",
        "Traceability",
        &["artifact_ref", "target_id", "evidence_ref", "path"],
        "",
    );
    let project = text_expr(
        connection,
        "soll",
        "Traceability",
        &["project_code"],
        project_code,
    );
    let filter = project_filter(
        connection,
        "soll",
        "Traceability",
        project_code,
        all_projects,
    );
    execute(
        connection,
        &format!(
            "CREATE OR REPLACE TEMP TABLE edge_traceability AS
             SELECT 'TRACEABLE_TO:' || {entity_id} || '->' || {artifact_ref} AS id,
                    'intent:' || {entity_id} AS from_id,
                    'evidence:' || {entity_id} || ':' || {artifact_ref} AS to_id,
                    'TRACEABLE_TO' AS relation_type,
                    'SOLL' AS source,
                    {project} AS project_code,
                    NULL::VARCHAR AS metadata
             FROM soll.Traceability
             WHERE {entity_id} <> '' AND {artifact_ref} <> ''{filter};"
        ),
    )
}

fn create_edges(
    connection: &Connection,
    project_code: &str,
    all_projects: bool,
) -> Result<(), String> {
    create_ist_binary_edge(
        connection,
        "CONTAINS",
        "edge_contains",
        "CONTAINS",
        "file:",
        "symbol:",
        project_code,
        all_projects,
    )?;
    create_ist_binary_edge(
        connection,
        "CALLS",
        "edge_calls",
        "CALLS",
        "symbol:",
        "symbol:",
        project_code,
        all_projects,
    )?;
    create_ist_binary_edge(
        connection,
        "IMPACTS",
        "edge_impacts",
        "IMPACTS",
        "symbol:",
        "symbol:",
        project_code,
        all_projects,
    )?;
    create_ist_binary_edge(
        connection,
        "SUBSTANTIATES",
        "edge_substantiates",
        "SUBSTANTIATES",
        "symbol:",
        "symbol:",
        project_code,
        all_projects,
    )?;
    create_soll_edges(connection, project_code, all_projects)?;
    create_traceability_edges(connection, project_code, all_projects)?;
    execute(
        connection,
        "CREATE OR REPLACE TEMP TABLE edges AS
         SELECT * FROM edge_contains
         UNION ALL SELECT * FROM edge_calls
         UNION ALL SELECT * FROM edge_impacts
         UNION ALL SELECT * FROM edge_substantiates
         UNION ALL SELECT * FROM edge_soll
         UNION ALL SELECT * FROM edge_traceability;",
    )
}

fn add_unresolved_endpoint_nodes(connection: &Connection) -> Result<(), String> {
    execute(
        connection,
        "CREATE OR REPLACE TEMP TABLE node_unresolved_endpoint AS
         WITH endpoints AS (
             SELECT from_id AS endpoint_id, source, project_code FROM edges WHERE from_id <> ''
             UNION ALL
             SELECT to_id AS endpoint_id, source, project_code FROM edges WHERE to_id <> ''
         )
         SELECT DISTINCT
                endpoint_id AS id,
                'UnresolvedEndpoint' AS label,
                e.project_code AS project_code,
                endpoint_id AS title,
                e.source AS source,
                NULL::BIGINT AS publication_generated_at_ms,
                NULL::VARCHAR AS path,
                'edge_endpoint' AS kind,
                'unresolved' AS status,
                NULL::BOOLEAN AS graph_ready,
                NULL::BOOLEAN AS vector_ready,
                NULL::BIGINT AS size_bytes,
                'Endpoint referenced by an edge but not materialized as a canonical node in the publication source.' AS description,
                '{\"generated_by\":\"memgraph_publication\",\"reason\":\"edge_endpoint_missing_from_nodes\"}' AS metadata
         FROM endpoints e
         LEFT JOIN nodes n ON n.id = e.endpoint_id
         WHERE n.id IS NULL;",
    )?;
    execute(
        connection,
        "CREATE OR REPLACE TEMP TABLE nodes AS
         SELECT * FROM nodes
         UNION ALL SELECT * FROM node_unresolved_endpoint;",
    )?;
    execute(
        connection,
        "CREATE OR REPLACE TEMP TABLE nodes_dedup AS
         SELECT id,
                label,
                project_code,
                title,
                source,
                publication_generated_at_ms,
                path,
                kind,
                status,
                graph_ready,
                vector_ready,
                size_bytes,
                description,
                metadata
         FROM (
             SELECT *,
                    row_number() OVER (
                        PARTITION BY id
                        ORDER BY CASE WHEN label = 'UnresolvedEndpoint' THEN 9 ELSE 0 END,
                                 source,
                                 label
                    ) AS axon_row_rank
             FROM nodes
         )
         WHERE axon_row_rank = 1;",
    )?;
    execute(
        connection,
        "CREATE OR REPLACE TEMP TABLE nodes AS
         SELECT * FROM nodes_dedup;",
    )
}

fn export_parquet(connection: &Connection, publication_dir: &Path) -> Result<(), String> {
    let nodes_path = publication_dir.join("nodes.parquet");
    let edges_path = publication_dir.join("edges.parquet");
    execute(
        connection,
        &format!(
            "COPY nodes TO {} (FORMAT PARQUET);
             COPY edges TO {} (FORMAT PARQUET);",
            sql_path(&nodes_path),
            sql_path(&edges_path)
        ),
    )
}

fn write_manifest(
    args: &Args,
    publication_dir: &Path,
    generated_at_ms: i64,
    counts: &BTreeMap<String, i64>,
) -> Result<Value, String> {
    let row_counts = counts
        .iter()
        .map(|(table, count)| (table.clone(), json!(count)))
        .collect::<Map<_, _>>();
    let manifest = json!({
        "schema_version": "memgraph_projection_v1",
        "publication_id": args.publication_id,
        "publication_kind": "memgraph_human_ist_soll_projection",
        "generated_at_ms": generated_at_ms,
        "project_code": args.project_code,
        "project_scope": {
            "mode": if args.all_projects { "all_projects" } else { "single_project" },
            "project_code": args.project_code,
        },
        "human_only": true,
        "canonical_role": "disposable_projection",
        "llm_contract": "use_axon_mcp_not_memgraph",
        "source_freshness": {
            "ist": file_freshness(&args.ist_db),
            "soll": file_freshness(&args.soll_db),
        },
        "paths": {
            "publication_dir": publication_dir.display().to_string(),
            "nodes_parquet": publication_dir.join("nodes.parquet").display().to_string(),
            "edges_parquet": publication_dir.join("edges.parquet").display().to_string(),
            "manifest": publication_dir.join("manifest.json").display().to_string(),
        },
        "row_counts": Value::Object(row_counts),
        "retention_contract": "retain current and previous successful publication; failed publications keep compact manifest/logs only",
        "promotion_contract": "blue_green_memgraph_import_required_before_serving",
        "incremental_refresh": {
            "status": "future_gated",
            "requires": ["stable_source_epochs", "tombstones", "replacement_semantics", "validation_checksums"]
        },
        "notes": [
            "Memgraph is a human visualization surface only.",
            "LLM clients must continue to use Axon MCP for information retrieval.",
            "This publication is not an IST/SOLL writer and is safe to discard/rebuild.",
            "Default publications cover all projects; --project-only is a diagnostic narrow export."
        ]
    });
    fs::write(
        publication_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)
            .map_err(|err| format!("failed to render manifest JSON: {err}"))?,
    )
    .map_err(|err| {
        format!(
            "failed to write {}: {err}",
            publication_dir.join("manifest.json").display()
        )
    })?;
    Ok(manifest)
}

fn manifest_generated_at(path: &Path) -> Option<i64> {
    let content = fs::read_to_string(path.join("manifest.json")).ok()?;
    let manifest = serde_json::from_str::<Value>(&content).ok()?;
    if manifest.get("publication_kind")?.as_str()? != "memgraph_human_ist_soll_projection" {
        return None;
    }
    manifest.get("generated_at_ms")?.as_i64()
}

fn promote_current(
    out_root: &Path,
    publication_dir: &Path,
    manifest: &Value,
) -> Result<(), String> {
    fs::write(
        out_root.join("current.json"),
        serde_json::to_string_pretty(manifest)
            .map_err(|err| format!("failed to render current manifest JSON: {err}"))?,
    )
    .map_err(|err| {
        format!(
            "failed to write {}: {err}",
            out_root.join("current.json").display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs as unix_fs;
        let current_link = out_root.join("current");
        let _ = fs::remove_file(&current_link);
        let _ = fs::remove_dir(&current_link);
        unix_fs::symlink(publication_dir, &current_link).map_err(|err| {
            format!(
                "failed to update current publication symlink {}: {err}",
                current_link.display()
            )
        })?;
    }

    Ok(())
}

fn apply_retention(out_root: &Path, retain_successful: usize) -> Result<Vec<String>, String> {
    if retain_successful == 0 {
        return Ok(Vec::new());
    }
    let mut publications = Vec::new();
    for entry in fs::read_dir(out_root).map_err(|err| {
        format!(
            "failed to read publication root {}: {err}",
            out_root.display()
        )
    })? {
        let entry =
            entry.map_err(|err| format!("failed to read publication directory entry: {err}"))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(generated_at_ms) = manifest_generated_at(&path) {
            publications.push((generated_at_ms, path));
        }
    }
    publications.sort_by(|left, right| right.0.cmp(&left.0));

    let mut removed = Vec::new();
    for (_, path) in publications.into_iter().skip(retain_successful) {
        let display = path.display().to_string();
        fs::remove_dir_all(&path)
            .map_err(|err| format!("failed to remove old publication {display}: {err}"))?;
        removed.push(display);
    }
    Ok(removed)
}

fn run(args: Args) -> Result<(), String> {
    if !args.ist_db.exists() {
        return Err(format!(
            "IST database does not exist: {}",
            args.ist_db.display()
        ));
    }
    if !args.soll_db.exists() {
        return Err(format!(
            "SOLL database does not exist: {}",
            args.soll_db.display()
        ));
    }

    let publication_dir = args.out_root.join(&args.publication_id);
    fs::create_dir_all(&publication_dir).map_err(|err| {
        format!(
            "failed to create publication directory {}: {err}",
            publication_dir.display()
        )
    })?;

    let generated_at_ms = now_unix_ms();
    let connection = Connection::open_in_memory()
        .map_err(|err| format!("failed to open in-memory DuckDB publication builder: {err}"))?;
    execute(
        &connection,
        &format!(
            "ATTACH {} AS ist (READ_ONLY);
             ATTACH {} AS soll (READ_ONLY);",
            sql_path(&args.ist_db),
            sql_path(&args.soll_db)
        ),
    )?;

    create_nodes(
        &connection,
        generated_at_ms,
        &args.project_code,
        args.all_projects,
    )?;
    create_edges(&connection, &args.project_code, args.all_projects)?;
    add_unresolved_endpoint_nodes(&connection)?;
    export_parquet(&connection, &publication_dir)?;

    let counts = ["nodes", "edges"]
        .iter()
        .map(|table| ((*table).to_string(), count_rows(&connection, table)))
        .collect::<BTreeMap<_, _>>();
    let mut manifest = write_manifest(&args, &publication_dir, generated_at_ms, &counts)?;
    promote_current(&args.out_root, &publication_dir, &manifest)?;
    let retention_removed = apply_retention(&args.out_root, args.retain_successful)?;
    if let Value::Object(ref mut object) = manifest {
        object.insert(
            "retention".to_string(),
            json!({
                "retain_successful": args.retain_successful,
                "removed": retention_removed,
                "current_manifest": args.out_root.join("current.json").display().to_string(),
                "current_publication": args.out_root.join("current").display().to_string(),
            }),
        );
    }
    fs::write(
        publication_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)
            .map_err(|err| format!("failed to render manifest JSON: {err}"))?,
    )
    .map_err(|err| {
        format!(
            "failed to update {}: {err}",
            publication_dir.join("manifest.json").display()
        )
    })?;
    fs::write(
        args.out_root.join("current.json"),
        serde_json::to_string_pretty(&manifest)
            .map_err(|err| format!("failed to render current manifest JSON: {err}"))?,
    )
    .map_err(|err| {
        format!(
            "failed to update {}: {err}",
            args.out_root.join("current.json").display()
        )
    })?;

    println!(
        "{}",
        serde_json::to_string_pretty(&manifest)
            .map_err(|err| format!("failed to render manifest JSON: {err}"))?
    );
    Ok(())
}

fn main() {
    let exit = match parse_args().and_then(run) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("{message}");
            1
        }
    };
    std::process::exit(exit);
}
