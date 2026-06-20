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
