//! Data-artifact catalog surface (REQ-AXO-902017 slice 1, PIL-AXO-9003).
//!
//! On a DATA-CENTRIC project (finance / ML / ETL), ~half of an agent's
//! environment-understanding questions are about DATA artifacts (CSV lakes,
//! fixtures, manifests) — not code. The IST indexes code + SOLL but not the
//! data, so an LLM had to shell out (`ls`/`head`/`wc`) to learn "how many
//! assets, which period, what's wired vs not" — a comprehension hole in the
//! shared runtime truth (PIL-AXO-001), not just a token cost.
//!
//! This slice ingests the project's **normalized data catalog** — the pivot
//! format `data/CATALOG.json` (OPV REQ-OPV-215), already derived from the raw
//! manifests — and answers it through MCP, so the data inventory is one call
//! away instead of a shell dredge. Indexing raw CSV headers / cross-referencing
//! which code reads which lake is a deliberate later slice; ingesting the
//! normalized catalog first de-risks it (REQ body: "ingestion d'un catalogue
//! JSON normalisé > parsing hétérogène").
//!
//! Read-only: the catalog is small (KBs) and read on demand; no IST persistence
//! yet, no new table.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Value};

use super::McpServer;

/// Default catalog location relative to a project's root (the pivot format).
const DEFAULT_CATALOG_REL_PATH: &str = "data/CATALOG.json";

/// One artifact entry as it appears in `data/CATALOG.json`. Every field is
/// optional: a heterogeneous real-world catalog may omit any of them, and a
/// missing field must degrade gracefully (never fail the whole parse).
#[derive(Debug, Clone, Deserialize, Default)]
struct CatalogArtifact {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    bytes: Option<i64>,
    #[serde(default)]
    rows: Option<i64>,
    #[serde(default)]
    n_cols: Option<i64>,
    #[serde(default)]
    date_range: Option<Value>,
    #[serde(default)]
    manifest: Option<String>,
    // `sha256` / other provenance fields present in the catalog are ignored by
    // serde here — content-hash cross-reference is a later slice (REQ body).
    #[serde(default)]
    source: Option<String>,
}

/// Top-level catalog shape: `{ "artifacts": { "<id>": { … }, … } }`.
#[derive(Debug, Clone, Deserialize)]
struct RawCatalog {
    #[serde(default)]
    artifacts: BTreeMap<String, CatalogArtifact>,
}

/// Aggregated, LLM-ready summary of a project's data catalog.
#[derive(Debug, Clone, PartialEq)]
struct DataCatalogSummary {
    total_artifacts: usize,
    total_rows: i64,
    total_bytes: i64,
    /// artifact count per `kind` (e.g. lake → 12), sorted by kind.
    by_kind: BTreeMap<String, usize>,
    with_manifest: usize,
    /// artifact ids that declare no manifest — the data-provenance gaps.
    missing_manifest: Vec<String>,
    /// flattened rows, sorted by id, for the structured listing.
    artifacts: Vec<ArtifactRow>,
}

#[derive(Debug, Clone, PartialEq)]
struct ArtifactRow {
    id: String,
    name: Option<String>,
    kind: Option<String>,
    rows: Option<i64>,
    n_cols: Option<i64>,
    bytes: Option<i64>,
    has_manifest: bool,
    /// Raw manifest path (REQ-AXO-902017 ingest persists it); `has_manifest` is
    /// the derived "non-empty" flag the read summary surfaces.
    manifest: Option<String>,
    date_range: Option<Value>,
    source: Option<String>,
}

/// Pure parser — maps the raw catalog JSON to an aggregated summary. Side-effect
/// free so it is unit-testable without a project path or a live server.
fn parse_data_catalog(raw: &str) -> anyhow::Result<DataCatalogSummary> {
    let parsed: RawCatalog = serde_json::from_str(raw)?;
    let mut total_rows = 0i64;
    let mut total_bytes = 0i64;
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut with_manifest = 0usize;
    let mut missing_manifest = Vec::new();
    let mut artifacts = Vec::with_capacity(parsed.artifacts.len());

    for (id, a) in parsed.artifacts {
        total_rows = total_rows.saturating_add(a.rows.unwrap_or(0));
        total_bytes = total_bytes.saturating_add(a.bytes.unwrap_or(0));
        let kind = a.kind.clone().unwrap_or_else(|| "unknown".to_string());
        *by_kind.entry(kind).or_insert(0) += 1;
        let has_manifest = a
            .manifest
            .as_deref()
            .map(|m| !m.trim().is_empty())
            .unwrap_or(false);
        if has_manifest {
            with_manifest += 1;
        } else {
            missing_manifest.push(id.clone());
        }
        artifacts.push(ArtifactRow {
            id,
            name: a.name,
            kind: a.kind,
            rows: a.rows,
            n_cols: a.n_cols,
            bytes: a.bytes,
            has_manifest,
            manifest: a.manifest,
            date_range: a.date_range,
            source: a.source,
        });
    }
    // BTreeMap already iterates ids in sorted order; keep it deterministic.
    artifacts.sort_by(|l, r| l.id.cmp(&r.id));
    missing_manifest.sort();

    Ok(DataCatalogSummary {
        total_artifacts: artifacts.len(),
        total_rows,
        total_bytes,
        by_kind,
        with_manifest,
        missing_manifest,
        artifacts,
    })
}

impl McpServer {
    /// REQ-AXO-902017 slice 1 — read a project's normalized data catalog
    /// (`data/CATALOG.json`) and answer its inventory through MCP.
    pub(crate) fn axon_data_catalog(&self, args: &Value) -> Option<Value> {
        let project_code = args
            .get("project_code")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "AXO".to_string());

        // Resolve the project root from the registry (shared with rescan).
        let project_path = match self.lookup_project_path(&project_code) {
            Some(p) => p,
            None => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!(
                        "Unknown project_code `{project_code}` — no registry path. List known codes with `project_registry_lookup`."
                    )}],
                    "isError": true,
                    "data": {
                        "status": "input_not_found",
                        "parameter_repair": {
                            "field_in_error": "project_code",
                            "follow_up_tool": "project_registry_lookup"
                        }
                    }
                }));
            }
        };

        // The catalog path: an explicit override, else the pivot default.
        let catalog_path: PathBuf = match args.get("catalog_path").and_then(|v| v.as_str()) {
            Some(rel) if !rel.trim().is_empty() => {
                let p = PathBuf::from(rel);
                if p.is_absolute() {
                    p
                } else {
                    PathBuf::from(&project_path).join(p)
                }
            }
            _ => PathBuf::from(&project_path).join(DEFAULT_CATALOG_REL_PATH),
        };

        let raw = match std::fs::read_to_string(&catalog_path) {
            Ok(text) => text,
            Err(err) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!(
                        "No data catalog at `{}` ({err}). The pivot format is a normalized JSON catalog `{{\"artifacts\": {{\"<id>\": {{name, kind, rows, n_cols, bytes, manifest, sha256, source}}}}}}` at `{DEFAULT_CATALOG_REL_PATH}`. A data-centric project produces it from its manifests (e.g. OPV REQ-OPV-215); pass `catalog_path` to point elsewhere.",
                        catalog_path.display()
                    )}],
                    "isError": true,
                    "data": {
                        "status": "input_not_found",
                        "expected_path": catalog_path.display().to_string(),
                        "parameter_repair": { "field_in_error": "catalog_path" }
                    }
                }));
            }
        };

        let summary = match parse_data_catalog(&raw) {
            Ok(s) => s,
            Err(err) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!(
                        "Data catalog at `{}` is not valid JSON of the expected shape ({err}). Expected `{{\"artifacts\": {{\"<id>\": {{…}}}}}}`.",
                        catalog_path.display()
                    )}],
                    "isError": true,
                    "data": { "status": "input_invalid", "expected_path": catalog_path.display().to_string() }
                }));
            }
        };

        // REQ-AXO-902017 — action=index persists the catalog into the IST
        // (ist.Symbol kind='data_artifact' + ist.DataArtifact metadata) so
        // artifacts join the structural graph; action=read (default) just
        // summarizes on demand. Off the indexing hot-path (PIL-AXO-007).
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("read");
        if action == "index" {
            // CSV files (for header reads) resolve relative to the catalog's
            // own directory, not a hardcoded project/data/ — so a fixture
            // catalog anywhere stays self-contained.
            let catalog_dir = catalog_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(&project_path));
            return Some(self.ingest_data_artifacts_into_ist(&project_code, &catalog_dir, &summary));
        }

        let by_kind_text = summary
            .by_kind
            .iter()
            .map(|(k, n)| format!("{k}={n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let missing_note = if summary.missing_manifest.is_empty() {
            "all artifacts carry a manifest".to_string()
        } else {
            format!(
                "{} artifact(s) lack a manifest (provenance gap): {}",
                summary.missing_manifest.len(),
                summary.missing_manifest.join(", ")
            )
        };
        let report = format!(
            "## Data catalog — project {project_code}\n\n\
             Source: `{}`\n\n\
             - Artifacts: **{}**  (kinds: {})\n\
             - Total rows: {}\n\
             - Total bytes: {}\n\
             - Manifests: {}/{} present — {}\n",
            catalog_path.display(),
            summary.total_artifacts,
            if by_kind_text.is_empty() { "—".to_string() } else { by_kind_text },
            summary.total_rows,
            summary.total_bytes,
            summary.with_manifest,
            summary.total_artifacts,
            missing_note,
        );

        let artifacts_json: Vec<Value> = summary
            .artifacts
            .iter()
            .map(|a| {
                json!({
                    "id": a.id,
                    "name": a.name,
                    "kind": a.kind,
                    "rows": a.rows,
                    "n_cols": a.n_cols,
                    "bytes": a.bytes,
                    "has_manifest": a.has_manifest,
                    "date_range": a.date_range,
                    "source": a.source,
                })
            })
            .collect();
        let by_kind_json: BTreeMap<String, usize> = summary.by_kind.clone();

        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "structuredContent": {
                "project_code": project_code,
                "catalog_path": catalog_path.display().to_string(),
                "total_artifacts": summary.total_artifacts,
                "total_rows": summary.total_rows,
                "total_bytes": summary.total_bytes,
                "by_kind": by_kind_json,
                "manifests_present": summary.with_manifest,
                "missing_manifest": summary.missing_manifest,
                "artifacts": artifacts_json,
            }
        }))
    }

    /// REQ-AXO-902017 — persist the parsed catalog into the IST: every artifact
    /// becomes an `ist.Symbol` node (kind='data_artifact') plus an
    /// `ist.DataArtifact` metadata row keyed by the same id; stale
    /// data_artifact nodes for this project (no longer in the catalog) are
    /// pruned. Off the indexing hot-path — explicit `action=index` only
    /// (PIL-AXO-007). The code pipeline never indexes `.csv`, so these nodes are
    /// owned solely by this pass.
    fn ingest_data_artifacts_into_ist(
        &self,
        project_code: &str,
        catalog_dir: &std::path::Path,
        summary: &DataCatalogSummary,
    ) -> Value {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let mut ids: Vec<String> = Vec::with_capacity(summary.artifacts.len());
        let mut upserted = 0usize;
        let mut failed: Vec<String> = Vec::new();

        for a in &summary.artifacts {
            let id = data_artifact_node_id(project_code, &a.id);
            ids.push(id.clone());
            let name = a.name.clone().unwrap_or_else(|| a.id.clone());
            // Slice "CSV headers": when the named file exists next to the
            // catalog, read its header row for the real column names; else NULL.
            let columns = read_csv_header_columns(catalog_dir, name.as_str());

            let sym_sql = format!(
                "INSERT INTO ist.Symbol (id, name, kind, project_code) \
                 VALUES ('{id}','{name}','data_artifact','{proj}') \
                 ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, kind = 'data_artifact'",
                id = sql_str(&id),
                name = sql_str(&name),
                proj = sql_str(project_code),
            );
            let art_sql = format!(
                "INSERT INTO ist.DataArtifact \
                 (id, project_code, name, artifact_kind, file_path, rows_count, cols_count, \
                  bytes_size, manifest_path, source, columns, date_range, has_manifest, discovered_ms) \
                 VALUES ('{id}','{proj}','{name}',{kind},{fpath},{rows},{cols},{bytes},{manifest},{source},{columns},{date_range},{has_manifest},{now}) \
                 ON CONFLICT (id) DO UPDATE SET \
                   name = EXCLUDED.name, artifact_kind = EXCLUDED.artifact_kind, \
                   file_path = EXCLUDED.file_path, rows_count = EXCLUDED.rows_count, \
                   cols_count = EXCLUDED.cols_count, bytes_size = EXCLUDED.bytes_size, \
                   manifest_path = EXCLUDED.manifest_path, source = EXCLUDED.source, \
                   columns = EXCLUDED.columns, date_range = EXCLUDED.date_range, \
                   has_manifest = EXCLUDED.has_manifest",
                id = sql_str(&id),
                proj = sql_str(project_code),
                name = sql_str(&name),
                kind = sql_opt_str(a.kind.as_deref()),
                fpath = sql_opt_str(Some(name.as_str())),
                rows = sql_opt_i64(a.rows),
                cols = sql_opt_i64(a.n_cols),
                bytes = sql_opt_i64(a.bytes),
                manifest = sql_opt_str(a.manifest.as_deref().filter(|m| !m.trim().is_empty())),
                source = sql_opt_str(a.source.as_deref()),
                columns = sql_opt_jsonb(columns.as_ref().map(|c| json!(c))),
                date_range = sql_opt_jsonb(a.date_range.clone()),
                has_manifest = a.has_manifest,
                now = now_ms,
            );

            if self.graph_store.execute(&sym_sql).is_ok()
                && self.graph_store.execute(&art_sql).is_ok()
            {
                upserted += 1;
            } else {
                failed.push(id.clone());
            }
        }

        // Prune stale data_artifact nodes (present in IST but gone from the
        // catalog), scoped to this project + kind so code symbols are untouched.
        let not_in = if ids.is_empty() {
            "TRUE".to_string()
        } else {
            let list = ids
                .iter()
                .map(|i| format!("'{}'", sql_str(i)))
                .collect::<Vec<_>>()
                .join(",");
            format!("id NOT IN ({list})")
        };
        let _ = self.graph_store.execute(&format!(
            "DELETE FROM ist.DataArtifact WHERE project_code = '{}' AND {}",
            sql_str(project_code),
            not_in
        ));
        let _ = self.graph_store.execute(&format!(
            "DELETE FROM ist.Symbol WHERE project_code = '{}' AND kind = 'data_artifact' AND {}",
            sql_str(project_code),
            not_in
        ));

        // Cross-reference (slice 4): a code symbol whose indexed chunk content
        // names an artifact's file READS_ARTIFACT it. Rebuilt from scratch each
        // run (drop this project's READS_ARTIFACT edges, then re-derive) so the
        // set always reflects the current code + catalog. Heuristic v1: a
        // file-name substring match against ist.Chunk content.
        let _ = self.graph_store.execute(&format!(
            "DELETE FROM ist.Edge WHERE project_code = '{}' AND relation_type = 'READS_ARTIFACT'",
            sql_str(project_code)
        ));
        let mut cross_refs = 0usize;
        for a in &summary.artifacts {
            let id = data_artifact_node_id(project_code, &a.id);
            let needle = a.name.clone().unwrap_or_else(|| a.id.clone());
            // Skip short names that would match unrelated code noise.
            if needle.trim().len() < 4 {
                continue;
            }
            let xref_sql = format!(
                "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) \
                 SELECT DISTINCT c.source_id, '{tgt}', 'READS_ARTIFACT', '{proj}', {now} \
                 FROM ist.Chunk c \
                 WHERE c.project_code = '{proj}' AND c.source_type = 'symbol' \
                   AND c.source_id <> '{tgt}' AND c.content LIKE '%{needle}%' \
                 ON CONFLICT (source_id, target_id, relation_type, project_code) DO NOTHING",
                tgt = sql_str(&id),
                proj = sql_str(project_code),
                now = now_ms,
                needle = sql_str(&needle),
            );
            if self.graph_store.execute(&xref_sql).is_ok() {
                cross_refs += 1;
            }
        }

        let report = format!(
            "## Data catalog indexed → IST — project {project_code}\n\n\
             - Artifacts upserted as `data_artifact` nodes: **{upserted}** / {}\n\
             - Stale nodes pruned outside the current catalog set\n\
             - READS_ARTIFACT cross-reference rebuilt for {cross_refs} artifact(s) \
               (code symbols whose chunks name the file)\n",
            summary.artifacts.len(),
        );
        json!({
            "content": [{ "type": "text", "text": report }],
            "structuredContent": {
                "status": if failed.is_empty() { "ok" } else { "partial" },
                "action": "index",
                "project_code": project_code,
                "artifacts_total": summary.artifacts.len(),
                "artifacts_upserted": upserted,
                "cross_ref_artifacts": cross_refs,
                "failed_ids": failed,
                "surfaces_used": ["ist_symbol", "ist_data_artifact", "ist_edge"]
            }
        })
    }
}

/// Stable IST node id for a catalog artifact key (deterministic so re-indexing
/// upserts in place and READS_ARTIFACT edges resolve).
fn data_artifact_node_id(project_code: &str, catalog_key: &str) -> String {
    format!("{project_code}::artifact::{catalog_key}")
}

/// Single-quote escape for inline SQL string literals.
fn sql_str(s: &str) -> String {
    s.replace('\'', "''")
}

/// `'escaped'` or `NULL` for an optional text column.
fn sql_opt_str(s: Option<&str>) -> String {
    match s {
        Some(v) => format!("'{}'", sql_str(v)),
        None => "NULL".to_string(),
    }
}

/// Numeric literal or `NULL` for an optional integer column.
fn sql_opt_i64(n: Option<i64>) -> String {
    match n {
        Some(v) => v.to_string(),
        None => "NULL".to_string(),
    }
}

/// `'<json>'::jsonb` or `NULL` for an optional JSONB column.
fn sql_opt_jsonb(v: Option<Value>) -> String {
    match v {
        Some(value) => format!("'{}'::jsonb", sql_str(&value.to_string())),
        None => "NULL".to_string(),
    }
}

/// Best-effort CSV header read: when `<catalog_dir>/<name>` exists and looks
/// like a CSV, return its first-row column names. `None` on any failure
/// (missing file, not a CSV, unreadable) — never fails the ingest.
fn read_csv_header_columns(catalog_dir: &std::path::Path, name: &str) -> Option<Vec<String>> {
    if !name.to_ascii_lowercase().ends_with(".csv") {
        return None;
    }
    let path = catalog_dir.join(name);
    let content = std::fs::read_to_string(&path).ok()?;
    let header = content.lines().next()?;
    let cols: Vec<String> = header
        .split(',')
        .map(|c| c.trim().trim_matches('"').to_string())
        .filter(|c| !c.is_empty())
        .collect();
    if cols.is_empty() {
        None
    } else {
        Some(cols)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "artifacts": {
            "bis_lake.csv": {"name":"bis_lake.csv","kind":"lake","bytes":563739,"rows":24319,"n_cols":139,"manifest":"data/lakes/bis_lake_manifest.json","sha256":"ab","source":"BIS"},
            "book_rules.json": {"name":"book_rules.json","kind":"fixture","bytes":1200,"rows":40,"n_cols":3,"manifest":"","source":"manual"},
            "chain.csv": {"name":"chain.csv","kind":"lake","bytes":900,"rows":11,"date_range":["2020-01","2021-12"]}
        }
    }"#;

    #[test]
    fn parses_aggregates_and_kinds() {
        let s = parse_data_catalog(SAMPLE).expect("valid catalog");
        assert_eq!(s.total_artifacts, 3);
        assert_eq!(s.total_rows, 24319 + 40 + 11);
        assert_eq!(s.total_bytes, 563739 + 1200 + 900);
        assert_eq!(s.by_kind.get("lake"), Some(&2));
        assert_eq!(s.by_kind.get("fixture"), Some(&1));
    }

    // ── REQ-AXO-902017 ingest helpers (pure, IST persistence) ──────────────

    #[test]
    fn artifact_node_id_is_deterministic_and_scoped() {
        assert_eq!(
            data_artifact_node_id("AXO", "bis_lake.csv"),
            "AXO::artifact::bis_lake.csv"
        );
    }

    #[test]
    fn parse_now_exposes_manifest_for_ingest() {
        // The ingest path needs the raw manifest path, not just the has_manifest flag.
        let s = parse_data_catalog(SAMPLE).expect("valid catalog");
        let bis = s.artifacts.iter().find(|a| a.id == "bis_lake.csv").unwrap();
        assert_eq!(bis.manifest.as_deref(), Some("data/lakes/bis_lake_manifest.json"));
        assert!(bis.has_manifest);
    }

    #[test]
    fn sql_helpers_escape_and_null_correctly() {
        assert_eq!(sql_str("o'brien"), "o''brien");
        assert_eq!(sql_opt_str(Some("lake")), "'lake'");
        assert_eq!(sql_opt_str(None), "NULL");
        assert_eq!(sql_opt_i64(Some(42)), "42");
        assert_eq!(sql_opt_i64(None), "NULL");
        assert_eq!(sql_opt_jsonb(None), "NULL");
        assert_eq!(
            sql_opt_jsonb(Some(json!(["a", "b"]))),
            "'[\"a\",\"b\"]'::jsonb"
        );
        // A single quote inside JSON is doubled so the SQL literal stays valid.
        assert_eq!(
            sql_opt_jsonb(Some(json!(["o'x"]))),
            "'[\"o''x\"]'::jsonb"
        );
    }

    #[test]
    fn csv_header_reader_ignores_non_csv() {
        let dir = std::path::Path::new("/nonexistent");
        assert_eq!(read_csv_header_columns(dir, "book.json"), None);
        // Missing file degrades to None, never panics.
        assert_eq!(read_csv_header_columns(dir, "x.csv"), None);
    }

    #[test]
    fn csv_header_reader_returns_columns_for_real_fixture() {
        // REQ-AXO-902017 — the in-repo validation fixture's header round-trips.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/data_catalog");
        let cols = read_csv_header_columns(&dir, "axon_demo.csv").expect("fixture csv readable");
        assert_eq!(cols, vec!["metric", "value", "unit"]);
    }

    #[test]
    fn flags_missing_manifest_as_provenance_gap() {
        let s = parse_data_catalog(SAMPLE).expect("valid catalog");
        // `book_rules.json` has an empty manifest; `chain.csv` has none.
        assert_eq!(s.with_manifest, 1);
        assert_eq!(
            s.missing_manifest,
            vec!["book_rules.json".to_string(), "chain.csv".to_string()]
        );
    }

    #[test]
    fn missing_optional_fields_degrade_gracefully() {
        // No `kind`, no `rows`, no `bytes` → unknown kind, zero contributions.
        let raw = r#"{"artifacts":{"x":{"name":"x"}}}"#;
        let s = parse_data_catalog(raw).expect("valid");
        assert_eq!(s.total_artifacts, 1);
        assert_eq!(s.total_rows, 0);
        assert_eq!(s.by_kind.get("unknown"), Some(&1));
        assert_eq!(s.missing_manifest, vec!["x".to_string()]);
    }

    #[test]
    fn empty_catalog_is_valid() {
        let s = parse_data_catalog(r#"{"artifacts":{}}"#).expect("valid empty");
        assert_eq!(s.total_artifacts, 0);
        assert_eq!(s.total_rows, 0);
        assert!(s.by_kind.is_empty());
    }

    #[test]
    fn artifacts_listed_in_deterministic_id_order() {
        let raw = r#"{"artifacts":{"z":{"kind":"lake"},"a":{"kind":"lake"},"m":{"kind":"lake"}}}"#;
        let s = parse_data_catalog(raw).expect("valid");
        let ids: Vec<&str> = s.artifacts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "m", "z"]);
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        assert!(parse_data_catalog("not json").is_err());
        assert!(parse_data_catalog(r#"{"artifacts": []}"#).is_err());
    }
}
