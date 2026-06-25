//! Advisory TechnologyMigration residue detector (REQ-AXO-902051).
//!
//! `detect_remnants` scans the IST for code-anchored residue of known
//! migrations and (re)creates idempotent `HAS_REMNANT` edges so the delivered
//! N3/N4 surfaces (`tech_debt_inventory` + pre-flight advisory + work-plan)
//! report them. ADVISORY only — never a hard gate (DEC-AXO-901641 gates the
//! *blocking* lane). Runs OFF the ingestion hot-path (explicit action only,
//! PIL-AXO-007).
//!
//! Precision discipline (4-agent review, REQ-902051 §1): anchor on
//! `ist.Symbol.name` (comment-free by construction) so documentation markers of
//! a *completed* migration ("DuckDB-era", "AGE retirement") are NOT counted as
//! residue — without this, `/duckdb/i` returns ~50 hits of which ~44 are
//! comments, inverting a "proof of closure ~0" into a false "regression ~50".
//! String-literal residue that is not a symbol name (a `nvidia-smi` shell call)
//! is matched against COMMENT-STRIPPED chunk content.
//!
//! Rulesets deliberately EXCLUDE sanctioned-permanent tokens: never `pipeline`
//! / the `_v2` suffix (canonical name, CPT-AXO-054 / GUI-PRO-108 case-2), never
//! `WITH RECURSIVE` (the mandated PG fallback invariant, PIL-AXO-9002).

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

use super::storage::escape_sql;
use crate::mcp::McpServer;

/// One code-anchored detection ruleset, keyed by the TMG node's
/// `metadata.detect_key`. Migrations are seeded as `TechnologyMigration`
/// (TMG-…) nodes carrying that key; the detector binds rule→node by it.
struct RemnantRuleset {
    detect_key: &'static str,
    /// PG POSIX regex matched case-insensitively (`~*`) against
    /// `ist.Symbol.name` — comment-free by construction. `None` = no symbol scan.
    symbol_name_regex: Option<&'static str>,
    /// Rust regex matched against COMMENT-STRIPPED `ist.Chunk.content` — for
    /// string-literal residue that is not a symbol name. `None` = no chunk scan.
    chunk_content_regex: Option<&'static str>,
}

/// The seeded migrations (TMG-AXO-001..004). Pipeline v1→v2 + nvidia-smi→NVML
/// carry live residue; DuckDB→PG + AGE are proof-of-closure (expect ~0 once
/// comments are excluded).
const RULESETS: &[RemnantRuleset] = &[
    RemnantRuleset {
        detect_key: "pipeline_v1_to_v2",
        // Dead `_v1` leaves only (e.g. `compose_dashboard_state_v1`); the
        // char-class after `_v1` avoids `_v12…` and never matches `_v2`.
        symbol_name_regex: Some("_v1([^0-9a-z]|$)"),
        chunk_content_regex: None,
    },
    RemnantRuleset {
        detect_key: "duckdb_to_pg",
        symbol_name_regex: Some("duckdb"),
        chunk_content_regex: None,
    },
    RemnantRuleset {
        detect_key: "age_to_pg",
        // AGE-specific API tokens ONLY. NOT the bare word "age" (→ page/storage/…)
        // and NOT "cypher" — Cypher is the legitimate CURRENT query language of
        // the Memgraph projection (build_cypherl, send_cypher, …), not AGE residue.
        symbol_name_regex: Some("ag_catalog|agtype"),
        chunk_content_regex: None,
    },
    RemnantRuleset {
        detect_key: "nvidia_smi_to_nvml",
        symbol_name_regex: None,
        chunk_content_regex: Some(r"nvidia[_-]smi"),
    },
];

/// Code symbol kinds (ist.Symbol.kind). EXCLUDES `section` (markdown/doc
/// headings — 4894 of them in AXO), `element` (markup), `TODO` (comment), and
/// config/db kinds: a doc heading like "Axon DuckDB Migration Plan" DOCUMENTS a
/// migration, it is not its residue (the same comment/doc-noise the 4-agent
/// review flagged, manifested through doc-derived symbols). Anchoring the scan
/// to real code kinds is what makes "proof of closure ~0" hold.
const CODE_SYMBOL_KINDS: &[&str] = &[
    "function",
    "method",
    "module",
    "struct",
    "impl",
    "enum",
    "class",
    "variable",
    "type_alias",
    "macro",
    "interface",
];

/// PG `IN ('a','b',…)` list of the code kinds, for the symbol scan filter.
fn code_kinds_in_clause() -> String {
    CODE_SYMBOL_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(",")
}

fn block_comment_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)/\*.*?\*/").expect("valid block-comment regex"))
}

/// Best-effort, language-agnostic comment stripping: remove `/* … */` block
/// comments, then cut each line at the first line-comment marker (`//`, `#`,
/// `--`). Conservative by design — over-stripping only RISKS missing a residue
/// (a false negative), never inventing one (PIL: an advisory must not cry wolf).
fn strip_comments(src: &str) -> String {
    let no_block = block_comment_re().replace_all(src, " ");
    no_block
        .lines()
        .map(|line| {
            let mut cut = line.len();
            for marker in ["//", "#", "--"] {
                if let Some(pos) = line.find(marker) {
                    if pos < cut {
                        cut = pos;
                    }
                }
            }
            &line[..cut]
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// True if `re` matches the chunk's content once comments are stripped — i.e.
/// the token appears in real code, not only in a comment.
fn chunk_has_non_comment_match(content: &str, re: &Regex) -> bool {
    re.is_match(&strip_comments(content))
}

impl McpServer {
    /// REQ-AXO-902051 — scan the IST for migration residue and (re)create
    /// idempotent HAS_REMNANT edges. Args: optional `project_code` (default
    /// AXO), optional `detect_key` (scope to one ruleset). Read-mostly: the
    /// only writes are idempotent edge inserts + a one-time baseline metadata
    /// set on first run.
    pub(crate) fn axon_detect_remnants(&self, args: &Value) -> Option<Value> {
        let project_code = args
            .get("project_code")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "AXO".to_string());
        let only_key = args
            .get("detect_key")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty());
        // Re-record baseline_remnants to the current count even if one exists —
        // use after cleaning residue (or after a ruleset precision fix) so
        // progress is measured against an honest baseline.
        let reset_baseline = args
            .get("reset_baseline")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut summaries: Vec<Value> = Vec::new();
        let mut total_remnants = 0usize;

        for rule in RULESETS {
            if let Some(k) = only_key {
                if k != rule.detect_key {
                    continue;
                }
            }
            let Some((tmg_id, has_baseline)) =
                self.find_tmg_by_detect_key(&project_code, rule.detect_key)
            else {
                summaries.push(json!({
                    "detect_key": rule.detect_key,
                    "status": "not_seeded",
                    "hint": format!(
                        "No TechnologyMigration carries detect_key='{}'. Seed via `soll_manager action=create entity=technology_migration data={{project_code, title, attach_to, relation_type:'BELONGS_TO', metadata:{{detect_key:'{}', from_tech, to_tech}}}}`.",
                        rule.detect_key, rule.detect_key
                    )
                }));
                continue;
            };

            // Collect (target_id, target_kind) matches.
            let mut targets: Vec<(String, &'static str)> = Vec::new();
            if let Some(pat) = rule.symbol_name_regex {
                for id in self.scan_symbol_names(&project_code, pat) {
                    targets.push((id, "ist:symbol"));
                }
            }
            if let Some(pat) = rule.chunk_content_regex {
                if let Ok(re) = Regex::new(&format!("(?i){pat}")) {
                    for (id, content) in self.scan_chunk_candidates(&project_code, pat) {
                        if chunk_has_non_comment_match(&content, &re) {
                            targets.push((id, "ist:chunk"));
                        }
                    }
                }
            }

            // Reconcile the edge set to the current scan: add new matches, prune
            // edges whose target no longer matches (a fixed pattern or cleaned
            // residue) so the inventory converges instead of accumulating stale
            // edges.
            let current_ids: std::collections::HashSet<String> =
                targets.iter().map(|(id, _)| id.clone()).collect();
            let existing = self.existing_remnant_targets(&tmg_id);
            let mut created = 0usize;
            for (target_id, kind) in &targets {
                if !existing.contains(target_id)
                    && self.insert_remnant_edge_idempotent(&tmg_id, target_id, kind, &project_code)
                {
                    created += 1;
                }
            }
            let stale: Vec<String> = existing
                .into_iter()
                .filter(|e| !current_ids.contains(e))
                .collect();
            let removed = self.prune_remnant_edges(&tmg_id, &stale);
            let found = targets.len();
            total_remnants += found;

            // Record the baseline once so a later 0 reads as "scanned-and-clean",
            // not "never scanned" (REQ-902051 §4).
            let baseline_set = if !has_baseline || reset_baseline {
                self.set_tmg_baseline(&tmg_id, found);
                true
            } else {
                false
            };

            summaries.push(json!({
                "detect_key": rule.detect_key,
                "migration_id": tmg_id,
                "remnants_found": found,
                "edges_created": created,
                "edges_removed": removed,
                "baseline_set": baseline_set,
            }));
        }

        // REQ-AXO-902067 — global no-phantom sweep. Prune every HAS_REMNANT edge
        // whose target vanished from ALL canonical graphs (ist.Symbol ∪ ist.Chunk
        // ∪ soll.Node), independent of which rulesets re-scanned. The s89 incident
        // (TMG→gone chunks) was a transient RAM-snapshot lag, but a stale edge
        // could otherwise linger if a target disappears without its ruleset being
        // re-run; this sweep makes PIL-AXO-9002 no-phantom hold by construction.
        let orphaned_pruned = self.prune_orphaned_remnant_edges(&project_code);

        let lines: Vec<String> = summaries
            .iter()
            .map(|s| {
                let key = s.get("detect_key").and_then(|v| v.as_str()).unwrap_or("?");
                if s.get("status").and_then(|v| v.as_str()) == Some("not_seeded") {
                    format!("- {key}: not seeded (see hint)")
                } else {
                    format!(
                        "- {key} ({}): {} remnant(s), +{} / -{} edge(s){}",
                        s.get("migration_id").and_then(|v| v.as_str()).unwrap_or("?"),
                        s.get("remnants_found").and_then(|v| v.as_u64()).unwrap_or(0),
                        s.get("edges_created").and_then(|v| v.as_u64()).unwrap_or(0),
                        s.get("edges_removed").and_then(|v| v.as_u64()).unwrap_or(0),
                        if s.get("baseline_set").and_then(|v| v.as_bool()) == Some(true) {
                            " [baseline recorded]"
                        } else {
                            ""
                        }
                    )
                }
            })
            .collect();
        let report = format!(
            "## detect_remnants — project {project_code}\n\nAdvisory residue scan (code-anchored; comments excluded). Surfaces via `tech_debt_inventory` + pre-flight + work-plan.\n\n{}\n\nTotal remnants linked: {total_remnants}. No-phantom sweep: {orphaned_pruned} orphaned edge(s) pruned.",
            lines.join("\n")
        );

        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "structuredContent": {
                "project_code": project_code,
                "total_remnants": total_remnants,
                "orphaned_pruned": orphaned_pruned,
                "migrations": summaries,
            }
        }))
    }

    /// Find the TMG node bound to a ruleset by `metadata.detect_key`. Returns
    /// `(tmg_id, has_baseline)`.
    fn find_tmg_by_detect_key(&self, project_code: &str, detect_key: &str) -> Option<(String, bool)> {
        let sql = format!(
            "SELECT id, (metadata ? 'baseline_remnants') FROM soll.Node \
             WHERE type = 'TechnologyMigration' AND project_code = '{}' \
             AND metadata->>'detect_key' = '{}' ORDER BY id LIMIT 1",
            escape_sql(project_code),
            escape_sql(detect_key),
        );
        let raw = self.graph_store.query_json(&sql).ok()?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let row = rows.into_iter().next()?;
        let id = row.first()?.as_str()?.to_string();
        let has_baseline = row
            .get(1)
            .map(|v| v.as_bool().unwrap_or(false) || v.as_str() == Some("true"))
            .unwrap_or(false);
        Some((id, has_baseline))
    }

    /// `ist.Symbol.id`s whose `name` matches `pattern` (PG POSIX `~*`,
    /// case-insensitive), restricted to CODE kinds so doc/section headings
    /// (which document a migration, not its residue) are excluded.
    fn scan_symbol_names(&self, project_code: &str, pattern: &str) -> Vec<String> {
        let sql = format!(
            "SELECT id FROM ist.Symbol WHERE project_code = '{}' AND kind IN ({}) AND name ~* '{}'",
            escape_sql(project_code),
            code_kinds_in_clause(),
            escape_sql(pattern),
        );
        let Ok(raw) = self.graph_store.query_json(&sql) else {
            return Vec::new();
        };
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| r.into_iter().next().and_then(|v| v.as_str().map(str::to_string)))
            .collect()
    }

    /// Candidate `(chunk_id, content)` whose raw content matches `pattern`. The
    /// caller re-checks against comment-stripped content before linking.
    fn scan_chunk_candidates(&self, project_code: &str, pattern: &str) -> Vec<(String, String)> {
        // Exclude documentation files (.md/.txt/.rst and any /docs/ path): prose
        // discussing a migration is not its code residue. Comment-stripping then
        // removes in-code comments from the remaining code/script chunks.
        let sql = format!(
            "SELECT id, content FROM ist.Chunk \
             WHERE project_code = '{}' AND content ~* '{}' \
             AND coalesce(file_path, '') !~* '\\.(md|markdown|txt|rst)$' \
             AND coalesce(file_path, '') !~* '/docs/'",
            escape_sql(project_code),
            escape_sql(pattern),
        );
        let Ok(raw) = self.graph_store.query_json(&sql) else {
            return Vec::new();
        };
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                let mut it = r.into_iter();
                let id = it.next()?.as_str()?.to_string();
                let content = it.next()?.as_str().unwrap_or("").to_string();
                Some((id, content))
            })
            .collect()
    }

    /// Idempotent HAS_REMNANT edge insert (mirrors `link_has_remnant`'s
    /// ON CONFLICT DO NOTHING semantics; the kind is already resolved by the
    /// scan, so no IST re-probe). Returns true if a new edge was created.
    fn insert_remnant_edge_idempotent(
        &self,
        tmg_id: &str,
        target_id: &str,
        target_kind: &str,
        project_code: &str,
    ) -> bool {
        let already = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE source_id = '{}' AND target_id = '{}' AND relation_type = 'HAS_REMNANT'",
                escape_sql(tmg_id),
                escape_sql(target_id),
            ))
            .unwrap_or(0);
        if already > 0 {
            return false;
        }
        let metadata = json!({ "target_kind": target_kind }).to_string();
        self.graph_store
            .execute_param(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata, project_code) \
                 VALUES (?, ?, 'HAS_REMNANT', ?::jsonb, ?) ON CONFLICT DO NOTHING",
                &json!([tmg_id, target_id, metadata, project_code]),
            )
            .is_ok()
    }

    /// The set of `target_id`s this TMG currently has HAS_REMNANT edges to.
    fn existing_remnant_targets(&self, tmg_id: &str) -> std::collections::HashSet<String> {
        let sql = format!(
            "SELECT target_id FROM soll.Edge WHERE source_id = '{}' AND relation_type = 'HAS_REMNANT'",
            escape_sql(tmg_id),
        );
        let Ok(raw) = self.graph_store.query_json(&sql) else {
            return std::collections::HashSet::new();
        };
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| r.into_iter().next().and_then(|v| v.as_str().map(str::to_string)))
            .collect()
    }

    /// Delete HAS_REMNANT edges from `tmg_id` to targets that no longer match
    /// (reconciliation). HAS_REMNANT is a detector-managed projection, not
    /// intent, so pruning a stale edge is safe (never touches SOLL intent
    /// nodes). Returns the count removed.
    fn prune_remnant_edges(&self, tmg_id: &str, stale: &[String]) -> usize {
        if stale.is_empty() {
            return 0;
        }
        let in_list = stale
            .iter()
            .map(|t| format!("'{}'", escape_sql(t)))
            .collect::<Vec<_>>()
            .join(",");
        let ok = self
            .graph_store
            .execute_param(
                &format!(
                    "DELETE FROM soll.Edge WHERE source_id = '{}' AND relation_type = 'HAS_REMNANT' AND target_id IN ({})",
                    escape_sql(tmg_id),
                    in_list
                ),
                &json!([]),
            )
            .is_ok();
        if ok {
            stale.len()
        } else {
            0
        }
    }

    /// REQ-AXO-902067 — prune fully-orphaned HAS_REMNANT edges (target absent
    /// from ist.Symbol ∪ ist.Chunk ∪ soll.Node) for `project_code`. Returns the
    /// count removed. HAS_REMNANT is a detector-managed projection (never intent),
    /// so deleting a phantom edge is safe and upholds PIL-AXO-9002 no-phantom by
    /// construction.
    fn prune_orphaned_remnant_edges(&self, project_code: &str) -> usize {
        let n = self
            .graph_store
            .query_count(&orphaned_remnant_count_sql(project_code))
            .unwrap_or(0) as usize;
        if n > 0 {
            let _ = self
                .graph_store
                .execute(&orphaned_remnant_prune_sql(project_code));
        }
        n
    }

    /// Set `metadata.baseline_remnants` once (first run) so `tech_debt_inventory`
    /// can compute honest progress and a later 0 means "scanned-and-clean".
    fn set_tmg_baseline(&self, tmg_id: &str, baseline: usize) {
        let _ = self.graph_store.execute_param(
            "UPDATE soll.Node \
             SET metadata = jsonb_set(coalesce(metadata, '{}'::jsonb), '{baseline_remnants}', ?::jsonb) \
             WHERE id = ?",
            &json!([baseline.to_string(), tmg_id]),
        );
    }
}

/// REQ-AXO-902067 — count HAS_REMNANT edges for `project_code` whose target is
/// absent from every canonical graph (the no-phantom orphan set).
fn orphaned_remnant_count_sql(project_code: &str) -> String {
    let p = escape_sql(project_code);
    format!(
        "SELECT count(*) FROM soll.Edge e WHERE e.relation_type = 'HAS_REMNANT' \
         AND e.project_code = '{p}' \
         AND NOT EXISTS (SELECT 1 FROM ist.Symbol s WHERE s.id = e.target_id) \
         AND NOT EXISTS (SELECT 1 FROM ist.Chunk c WHERE c.id = e.target_id) \
         AND NOT EXISTS (SELECT 1 FROM soll.Node n WHERE n.id = e.target_id)"
    )
}

/// REQ-AXO-902067 — delete the orphan set computed by [`orphaned_remnant_count_sql`].
/// HAS_REMNANT is a detector-managed projection (never intent), so the delete is
/// safe; it upholds PIL-AXO-9002 no-phantom by construction at every run.
fn orphaned_remnant_prune_sql(project_code: &str) -> String {
    let p = escape_sql(project_code);
    format!(
        "DELETE FROM soll.Edge AS e WHERE e.relation_type = 'HAS_REMNANT' \
         AND e.project_code = '{p}' \
         AND NOT EXISTS (SELECT 1 FROM ist.Symbol s WHERE s.id = e.target_id) \
         AND NOT EXISTS (SELECT 1 FROM ist.Chunk c WHERE c.id = e.target_id) \
         AND NOT EXISTS (SELECT 1 FROM soll.Node n WHERE n.id = e.target_id)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orphaned_prune_sql_targets_all_three_graphs() {
        let sql = orphaned_remnant_prune_sql("AXO");
        assert!(sql.starts_with("DELETE FROM soll.Edge"));
        assert!(sql.contains("e.relation_type = 'HAS_REMNANT'"));
        assert!(sql.contains("e.project_code = 'AXO'"));
        assert!(sql.contains("ist.Symbol"));
        assert!(sql.contains("ist.Chunk"));
        assert!(sql.contains("soll.Node"));
    }

    #[test]
    fn orphaned_count_and_prune_share_the_same_predicate() {
        // Both must scope identically so the reported count matches the delete.
        let c = orphaned_remnant_count_sql("AXO");
        let d = orphaned_remnant_prune_sql("AXO");
        for needle in ["ist.Symbol", "ist.Chunk", "soll.Node", "HAS_REMNANT"] {
            assert!(c.contains(needle) && d.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn orphaned_prune_sql_escapes_project() {
        let sql = orphaned_remnant_prune_sql("A'X");
        assert!(sql.contains("'A''X'"));
    }

    #[test]
    fn strip_comments_removes_line_markers() {
        let src = "let x = nvidia_smi();  // legacy nvidia-smi note\n# nvidia-smi in a hash comment\nactual_code();";
        let out = strip_comments(src);
        assert!(out.contains("nvidia_smi()"), "code kept");
        assert!(!out.contains("legacy nvidia-smi note"), "// comment stripped");
        assert!(!out.contains("hash comment"), "# comment stripped");
    }

    #[test]
    fn strip_comments_removes_block_comments() {
        let src = "code_a();\n/* a block\n mentioning nvidia-smi */\ncode_b();";
        let out = strip_comments(src);
        assert!(out.contains("code_a()") && out.contains("code_b()"));
        assert!(!out.contains("mentioning"), "block comment stripped");
    }

    #[test]
    fn non_comment_match_true_for_real_code_call() {
        let re = Regex::new(r"(?i)nvidia[_-]smi").unwrap();
        let code = r#"Command::new("nvidia-smi").arg("--query");"#;
        assert!(chunk_has_non_comment_match(code, &re));
    }

    #[test]
    fn non_comment_match_false_when_only_in_comment() {
        let re = Regex::new(r"(?i)nvidia[_-]smi").unwrap();
        let only_comment = "// historical: replaced nvidia-smi with NVML\nlet gpu = nvml_probe();";
        assert!(
            !chunk_has_non_comment_match(only_comment, &re),
            "a token only in a comment must NOT be flagged as residue"
        );
    }

    #[test]
    fn pipeline_v1_pattern_matches_dead_leaf_not_v2() {
        // Mirror the PG `~*` rule with the Rust engine for a sanity check.
        let re = Regex::new(r"(?i)_v1([^0-9a-z]|$)").unwrap();
        assert!(re.is_match("compose_dashboard_state_v1"));
        assert!(re.is_match("tensorrt_ready_vector_pipeline_v1"));
        assert!(!re.is_match("pipeline"), "never flag the canonical v2 name");
        assert!(!re.is_match("schema_v12"), "_v12 is not a v1 leaf");
    }

    #[test]
    fn age_pattern_matches_age_api_not_bare_words_nor_memgraph_cypher() {
        let re = Regex::new(r"(?i)ag_catalog|agtype").unwrap();
        assert!(!re.is_match("page_count"));
        assert!(!re.is_match("message_storage"));
        assert!(re.is_match("ag_catalog"));
        assert!(re.is_match("agtype_value"));
        // `cypher` is dropped: it is the CURRENT Memgraph projection language,
        // not AGE residue (build_cypherl / send_cypher must NOT be flagged).
        assert!(!re.is_match("build_cypherl"));
        assert!(!re.is_match("send_cypher"));
    }

    #[test]
    fn code_kinds_clause_excludes_doc_section_kind() {
        let clause = code_kinds_in_clause();
        assert!(clause.contains("'function'") && clause.contains("'struct'"));
        // The doc/markup/comment kinds that caused false positives must be absent.
        for excluded in ["'section'", "'element'", "'TODO'", "'config_key'", "'table'"] {
            assert!(
                !clause.contains(excluded),
                "code-kind allowlist must exclude {excluded}"
            );
        }
    }
}
