use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use super::McpServer;

fn token_is_code_on_line(line: &str, token_start: usize) -> bool {
    let bytes = line.as_bytes();
    let mut index = 0usize;
    let mut quote = None;
    let mut escaped = false;
    while index < token_start {
        let byte = bytes[index];
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == delimiter {
                quote = None;
            }
            index += 1;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            return false;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            // Conservatively reject the rest of a line after a block-comment
            // opener.  A false negative here is safer than inventing a target.
            return false;
        }
        if byte == b'#' && line[..index].trim().is_empty() {
            return false;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
        }
        index += 1;
    }
    quote.is_none()
}

fn contains_declaration_like_identifier(content: &str, needle: &str, extension: &str) -> bool {
    let is_identifier = |ch: char| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$';
    let declaration_keywords = [
        "class",
        "def",
        "enum",
        "fn",
        "func",
        "function",
        "interface",
        "module",
        "record",
        "struct",
        "trait",
        "type",
    ];
    let typed_callable_extensions = [
        "c", "cc", "cpp", "cs", "java", "kt", "kts", "php", "scala", "swift",
    ];
    let non_type_prefixes = [
        "await", "break", "case", "else", "if", "match", "new", "return", "throw", "while", "yield",
    ];

    content.lines().any(|line| {
        line.match_indices(needle).any(|(start, _)| {
            if !token_is_code_on_line(line, start) {
                return false;
            }
            let before_char = line[..start].chars().next_back();
            let after = &line[start + needle.len()..];
            let after_char = after.chars().next();
            if before_char.is_some_and(is_identifier) || after_char.is_some_and(is_identifier) {
                return false;
            }

            let prefix = line[..start].trim_end();
            let previous_word = prefix
                .rsplit(|ch: char| !is_identifier(ch))
                .find(|part| !part.is_empty())
                .unwrap_or("");
            if declaration_keywords.contains(&previous_word) {
                return true;
            }

            // Java/C-family methods have no declaration keyword.  Require a
            // type-like word immediately before the identifier, a call shape
            // immediately after it, and no assignment/call punctuation in the
            // current statement segment.  This rejects strings, JSON values,
            // member calls and ordinary invocations.
            if !typed_callable_extensions.contains(&extension)
                || !before_char.is_some_and(char::is_whitespace)
                || !after.trim_start().starts_with('(')
                || previous_word.is_empty()
                || non_type_prefixes.contains(&previous_word)
            {
                return false;
            }
            let statement_prefix = prefix
                .rsplit(['{', '}', ';'])
                .next()
                .unwrap_or(prefix)
                .trim();
            !statement_prefix.contains('=')
                && !statement_prefix.ends_with('.')
                && !statement_prefix.ends_with(')')
                && !statement_prefix.ends_with(']')
        })
    })
}

#[cfg(test)]
mod workspace_declaration_evidence_tests {
    use super::contains_declaration_like_identifier;

    #[test]
    fn declarations_are_evidence_but_literals_and_invocations_are_not() {
        let target = "cs_target_fn";
        assert!(contains_declaration_like_identifier(
            "public void cs_target_fn() {}",
            target,
            "java"
        ));
        assert!(contains_declaration_like_identifier(
            "fn cs_target_fn() {}",
            target,
            "rs"
        ));
        assert!(!contains_declaration_like_identifier(
            r#"{"target": "cs_target_fn"}"#,
            target,
            "rs"
        ));
        assert!(!contains_declaration_like_identifier(
            "const TARGET: &str = \"cs_target_fn\";",
            target,
            "rs"
        ));
        assert!(!contains_declaration_like_identifier(
            "assert!(contains(\"fn cs_target_fn() {}\"));",
            target,
            "rs"
        ));
        assert!(!contains_declaration_like_identifier(
            "service.cs_target_fn();",
            target,
            "java"
        ));
        assert!(!contains_declaration_like_identifier(
            include_str!("tools_framework_validation.rs"),
            target,
            "rs"
        ));
        assert!(!contains_declaration_like_identifier(
            include_str!("tests/context_and_analysis.rs"),
            target,
            "rs"
        ));
    }
}

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
    fn workspace_file_is_confirmed_unindexed(
        &self,
        project: &str,
        project_root: &Path,
        path: &Path,
    ) -> bool {
        let absolute = path.to_string_lossy().replace('\'', "''");
        let relative = path
            .strip_prefix(project_root)
            .ok()
            .map(|value| value.to_string_lossy().replace('\'', "''"));
        let escaped_project = project.replace('\'', "''");
        let mut candidates = vec![format!("path = '{absolute}'")];
        if let Some(relative) = relative.filter(|value| !value.is_empty()) {
            candidates.push(format!("path = '{relative}'"));
            candidates.push(format!("path = './{relative}'"));
        }
        let query = format!(
            "SELECT count(*) FROM ist.IndexedFile \
             WHERE project_code = '{escaped_project}' AND ({})",
            candidates.join(" OR ")
        );

        // A storage/query failure is not evidence that a file is absent from
        // the IST.  Fail closed instead of manufacturing an unindexed claim.
        self.graph_store
            .query_count(&query)
            .ok()
            .is_some_and(|count| count == 0)
    }

    /// REQ-AXO-902608 — absence from the IST is not evidence of absence from
    /// the workspace.  This deliberately bounded fallback is only called after
    /// canonical symbol resolution failed.  It identifies a file containing
    /// the requested identifier; it does not attempt to reconstruct a symbol
    /// or infer test coverage from source text.
    pub(super) fn unindexed_workspace_target(&self, project: &str, target: &str) -> Option<Value> {
        const MAX_FILES: usize = 10_000;
        const MAX_BYTES: u64 = 32 * 1024 * 1024;
        const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
        const MAX_WALL: Duration = Duration::from_millis(250);

        let project_root = self.lookup_project_path(project)?;
        let root = Path::new(&project_root);
        let needle = target
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'))
            .filter(|part| !part.is_empty())
            .next_back()?;
        if needle.len() < 3 {
            return None;
        }

        let started = Instant::now();
        let mut scanned_files = 0usize;
        let mut scanned_bytes = 0u64;
        let source_extensions = [
            "c", "cc", "cpp", "cs", "ex", "exs", "go", "java", "js", "jsx", "kt", "kts", "php",
            "py", "rb", "rs", "scala", "swift", "ts", "tsx",
        ];
        let walker = ignore::WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            .build();

        for entry in walker.filter_map(Result::ok) {
            if started.elapsed() >= MAX_WALL
                || scanned_files >= MAX_FILES
                || scanned_bytes >= MAX_BYTES
            {
                break;
            }
            let Some(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
            if !source_extensions.contains(&extension) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() > MAX_FILE_BYTES {
                continue;
            }
            scanned_files += 1;
            scanned_bytes = scanned_bytes.saturating_add(metadata.len());

            let filename_match = path
                .file_stem()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == needle);
            let content_match = !filename_match
                && fs::read_to_string(path).ok().is_some_and(|content| {
                    contains_declaration_like_identifier(&content, needle, extension)
                });
            if (filename_match || content_match)
                && self.workspace_file_is_confirmed_unindexed(project, root, path)
            {
                return Some(json!({
                    "state": "target_unindexed_workspace_file",
                    "target": target,
                    "matched_identifier": needle,
                    "matching_rule": if filename_match { "filename_stem" } else { "declaration_like" },
                    "file_path": path.to_string_lossy(),
                    "coverage_truth": "unknown",
                    "indexed_symbol_found": false,
                    "scan": {
                        "bounded": true,
                        "max_files": MAX_FILES,
                        "max_bytes": MAX_BYTES,
                        "max_wall_ms": MAX_WALL.as_millis(),
                        "scanned_files": scanned_files,
                        "scanned_bytes": scanned_bytes,
                    },
                    "next_action": {
                        "tool": "rescan_project",
                        "arguments": {"project_code": project}
                    }
                }));
            }
        }
        None
    }

    pub(super) fn symbol_validation_signals(&self, project: &str, symbol_name: &str) -> Value {
        let escaped_project = project.replace('\'', "''");
        let escaped_name = symbol_name.replace('\'', "''");
        // REQ-AXO-902452 — surface interne : ces signaux alimentent
        // `change_safety`, qui rend DEJA la note d'ambiguite au lecteur. Ici on
        // ne garde que l'id, en le disant.
        let resolved_symbol_id = if project == "*" {
            self.resolve_scoped_symbol(symbol_name, None)
        } else {
            self.resolve_scoped_symbol(symbol_name, Some(project))
        }
        .map(|resolved| resolved.id);
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
