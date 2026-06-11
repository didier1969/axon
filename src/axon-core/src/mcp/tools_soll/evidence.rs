use super::*;

/// REQ-AXO-901938 — a copy-pasteable minimal valid evidence artifact built from
/// the entity's accepted schema, embedded inline in rejection messages so the
/// LLM corrects the call in a single round-trip instead of guessing field
/// names/values across retries.
fn minimal_evidence_example(accepted_schema: &[String]) -> String {
    let artifact_type = accepted_schema
        .first()
        .map(String::as_str)
        .unwrap_or("document");
    format!(
        "{{\"artifact_type\": \"{artifact_type}\", \"artifact_ref\": \"<path-or-id>\", \"note\": \"<optional>\"}}"
    )
}

impl McpServer {
    fn normalize_file_artifact_ref(
        &self,
        entity_id: &str,
        raw_ref: &str,
    ) -> (Option<String>, Vec<String>) {
        let raw = raw_ref.trim();
        if raw.is_empty() {
            return (None, vec!["empty_path".to_string()]);
        }

        let mut diagnostics = Vec::new();
        let raw_path = Path::new(raw);
        let project_root = self.canonical_project_root_for_entity(entity_id);
        let mut candidates = Vec::new();
        candidates.push(raw.to_string());

        if raw_path.is_absolute() {
            candidates.push(raw_path.to_string_lossy().into_owned());
        } else if let Some(root) = project_root.as_ref() {
            candidates.push(root.join(raw_path).to_string_lossy().into_owned());
        }

        if let Some(root) = project_root.as_ref() {
            let candidate_absolute = if raw_path.is_absolute() {
                raw_path.to_path_buf()
            } else {
                root.join(raw_path)
            };

            if let Ok(relative) = candidate_absolute.strip_prefix(root) {
                candidates.push(relative.to_string_lossy().into_owned());
                diagnostics.push("normalized_relative_project_path".to_string());
            }
        }

        candidates.sort();
        candidates.dedup();

        // REQ-AXO-901653 slice-5d — public.File retired ; canonical file
        // presence probe now reads ist.IndexedFile (3-col pipeline_v2 pivot).
        for candidate in &candidates {
            let query = format!(
                "SELECT path FROM ist.IndexedFile WHERE path = '{}' LIMIT 1",
                escape_sql(candidate)
            );
            if let Ok(paths) = self.query_single_column(&query) {
                if let Some(path) = paths.into_iter().next() {
                    diagnostics.push("matched_indexed_file".to_string());
                    return (Some(path), diagnostics);
                }
            }
        }

        let preferred = if let Some(root) = project_root.as_ref() {
            let absolute = if raw_path.is_absolute() {
                raw_path.to_path_buf()
            } else {
                root.join(raw_path)
            };
            if absolute.exists() {
                if let Ok(relative) = absolute.strip_prefix(root) {
                    diagnostics.push("resolved_existing_project_file".to_string());
                    Some(relative.to_string_lossy().into_owned())
                } else {
                    diagnostics.push("resolved_existing_absolute_file".to_string());
                    Some(absolute.to_string_lossy().into_owned())
                }
            } else {
                None
            }
        } else if raw_path.exists() {
            diagnostics.push("resolved_existing_absolute_file".to_string());
            Some(raw_path.to_string_lossy().into_owned())
        } else {
            None
        };

        if preferred.is_none() {
            diagnostics.push("path_not_resolvable".to_string());
        }

        (preferred, diagnostics)
    }

    pub(crate) fn axon_soll_attach_evidence(&self, args: &Value) -> Option<Value> {
        let entity_type = args.get("entity_type")?.as_str()?;
        let entity_id = args.get("entity_id")?.as_str()?;
        let artifacts = args.get("artifacts")?.as_array()?;
        let mut attached = 0usize;
        let now = now_unix_ms();
        let normalized_entity_type = normalize_traceability_entity_type(entity_type);
        let accepted_schema = accepted_evidence_artifact_schema(&normalized_entity_type)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut artifact_diagnostics = Vec::new();
        let mut fallback_guidance = Vec::new();

        // REQ-AXO-901619 — capture the canonical project root once so per-
        // artifact diagnostics can surface a concrete "did you mean
        // <absolute_path>" hint for path_not_resolvable rejections.
        let project_root_for_hint = self
            .canonical_project_root_for_entity(entity_id)
            .map(|p| p.to_string_lossy().into_owned());

        for (idx, art) in artifacts.iter().enumerate() {
            let raw_artifact_ref = art
                .get("artifact_ref")
                .or_else(|| art.get("path"))
                .or_else(|| art.get("file_path"))
                .or_else(|| art.get("uri"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let artifact_type = art
                .get("artifact_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let normalized_artifact_type =
                normalize_evidence_artifact_type(artifact_type, raw_artifact_ref);
            let mut diagnostic_reasons = Vec::new();
            let artifact_ref = if normalized_artifact_type == "File" {
                let (normalized, normalization_diagnostics) =
                    self.normalize_file_artifact_ref(entity_id, raw_artifact_ref);
                diagnostic_reasons.extend(normalization_diagnostics);
                normalized.unwrap_or_default()
            } else {
                raw_artifact_ref.trim().to_string()
            };

            if artifact_ref.is_empty() {
                diagnostic_reasons.push("missing_artifact_ref".to_string());
                // REQ-AXO-901619 — synthesise the "did you mean" suggested
                // path so rejected diagnostics carry a one-step recovery.
                let suggested_absolute = build_suggested_absolute_path(
                    raw_artifact_ref,
                    project_root_for_hint.as_deref(),
                );
                let mut diag = json!({
                    "index": idx,
                    "input": art,
                    "status": "rejected",
                    "normalized_artifact_type": normalized_artifact_type,
                    "normalized_artifact_ref": artifact_ref,
                    "reasons": diagnostic_reasons,
                });
                if let Some(suggestion) = suggested_absolute {
                    diag["suggested_absolute_path"] = json!(suggestion);
                }
                if let Some(root) = project_root_for_hint.as_ref() {
                    diag["project_root"] = json!(root);
                }
                artifact_diagnostics.push(diag);
                continue;
            }
            if !artifact_schema_accepts(&normalized_entity_type, &normalized_artifact_type) {
                diagnostic_reasons.push("artifact_type_not_allowed_for_entity".to_string());
                artifact_diagnostics.push(json!({
                    "index": idx,
                    "input": art,
                    "status": "rejected",
                    "normalized_artifact_type": normalized_artifact_type,
                    "normalized_artifact_ref": artifact_ref,
                    "reasons": diagnostic_reasons,
                    "accepted_artifact_schema": accepted_schema
                }));
                continue;
            }
            let confidence = art
                .get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.8);
            let metadata = art
                .get("metadata")
                .cloned()
                .unwrap_or(json!({}))
                .to_string();
            let trace_id = format!("TRC-{}-{}-{}", entity_id, now, idx);

            if self.graph_store.execute_param(
                "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                &json!([trace_id, normalized_entity_type, entity_id, normalized_artifact_type, artifact_ref, confidence, metadata, now]),
            ).is_ok() {
                attached += 1;
                diagnostic_reasons.push("traceability_inserted".to_string());
                artifact_diagnostics.push(json!({
                    "index": idx,
                    "input": art,
                    "status": "attached",
                    "normalized_artifact_type": normalized_artifact_type,
                    "normalized_artifact_ref": artifact_ref,
                    "reasons": diagnostic_reasons
                }));
            } else {
                diagnostic_reasons.push("traceability_insert_failed".to_string());
                artifact_diagnostics.push(json!({
                    "index": idx,
                    "input": art,
                    "status": "rejected",
                    "normalized_artifact_type": normalized_artifact_type,
                    "normalized_artifact_ref": artifact_ref,
                    "reasons": diagnostic_reasons
                }));
            }
        }

        if attached == 0 {
            if normalized_entity_type == "requirement" {
                fallback_guidance.push(
                    "If file evidence still fails, attach the proof to a validation node first and link that validation with `VERIFIES`.".to_string(),
                );
            }
            fallback_guidance.push(
                "Use `artifact_ref`, `path`, `file_path`, or `uri`; file artifacts are normalized against the canonical project root when possible."
                    .to_string(),
            );
        }

        // REQ-AXO-043 / REQ-AXO-026 — every public MCP tool must surface
        // recovery for empty/rejected results in the LLM-visible `content`
        // field, not buried in `data.artifact_diagnostics`. Previously the
        // tool returned "Attached 0" with no recovery hint when all artifacts
        // were rejected (observed 2026-05-01 by Claude Sonnet 4.7 session).
        let total = artifacts.len();
        let status_str = if total == 0 {
            "no_artifacts"
        } else if attached == total {
            "ok"
        } else if attached == 0 {
            "rejected_all"
        } else {
            "partial"
        };

        let primary_reason = artifact_diagnostics
            .iter()
            .filter_map(|d| d.get("reasons").and_then(|r| r.as_array()))
            .flatten()
            .filter_map(|r| r.as_str())
            .find(|r| {
                *r != "traceability_inserted"
                    && *r != "matched_indexed_file"
                    && *r != "normalized_relative_project_path"
                    && *r != "resolved_existing_project_file"
                    && *r != "resolved_existing_absolute_file"
            })
            .map(str::to_string);

        let next_action = match status_str {
            "ok" => None,
            "no_artifacts" => Some(
                "supply at least one artifact object in the `artifacts` array".to_string(),
            ),
            _ => match primary_reason.as_deref() {
                // REQ-AXO-901938 — every rejection renders {what's wrong, the
                // accepted schema, ONE minimal valid example} INLINE so the LLM
                // corrects in a single round-trip instead of guessing the field
                // names/values across 2-3 retries (observed: `{kind, ref}` →
                // reject → `{artifact_type:"commit", artifact_ref}` → reject).
                Some("missing_artifact_ref") => Some(format!(
                    "each artifact needs an `artifact_ref` (or `path` / `file_path` / `uri` alias). \
                     Accepted `artifact_type` for {}: {:?}. Example: {}",
                    normalized_entity_type,
                    accepted_schema,
                    minimal_evidence_example(&accepted_schema)
                )),
                Some("artifact_type_not_allowed_for_entity") => Some(format!(
                    "use one of the accepted `artifact_type` values for {}: {:?}. Example: {}",
                    normalized_entity_type,
                    accepted_schema,
                    minimal_evidence_example(&accepted_schema)
                )),
                // REQ-AXO-901619 — surface project root + did-you-mean hint
                // so the LLM can correct the relative path in one round-trip
                // without re-reading data.artifact_diagnostics.
                Some("path_not_resolvable") => Some({
                    let first_rejected_suggestion = artifact_diagnostics
                        .iter()
                        .find(|d| {
                            d.get("status").and_then(|v| v.as_str()) == Some("rejected")
                                && d.get("suggested_absolute_path").is_some()
                        })
                        .and_then(|d| {
                            let raw = d
                                .get("input")
                                .and_then(|v| v.get("artifact_ref")
                                    .or_else(|| v.get("path"))
                                    .or_else(|| v.get("file_path"))
                                    .or_else(|| v.get("uri")))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let suggested = d
                                .get("suggested_absolute_path")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let root = d
                                .get("project_root")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if suggested.is_empty() {
                                None
                            } else {
                                Some(format!(
                                    "artifact path `{}` does not exist relative to project root `{}` nor as an absolute path. Did you mean `{}`?",
                                    raw, root, suggested
                                ))
                            }
                        });
                    first_rejected_suggestion.unwrap_or_else(|| {
                        "artifact path does not resolve under the project root and is not absolute"
                            .to_string()
                    })
                }),
                Some("traceability_insert_failed") => Some(
                    "graph_store insert failed; check Traceability schema and DB availability"
                        .to_string(),
                ),
                Some(reason) => Some(format!(
                    "primary rejection reason: `{}`; see artifact_diagnostics for per-artifact detail",
                    reason
                )),
                None => Some(
                    "review `artifact_diagnostics` for per-artifact rejection reasons".to_string(),
                ),
            },
        };

        let problem_class = match status_str {
            "ok" => "ok",
            "no_artifacts" => "input_empty",
            "rejected_all" => "input_invalid",
            "partial" => "partial_input_invalid",
            _ => "unknown",
        };

        let next_best_actions: Vec<String> = match status_str {
            "ok" => Vec::new(),
            _ => match next_action.clone() {
                Some(action) => vec![action],
                None => Vec::new(),
            },
        };

        let operator_guidance = json!({
            "problem_class": problem_class,
            "likely_cause": primary_reason.clone().unwrap_or_else(|| "all_artifacts_accepted".to_string()),
            "next_best_actions": next_best_actions,
            "confidence": "high",
        });

        // REQ-AXO-139 slice — universal parameter_repair contract for
        // soll_attach_evidence. When an artifact is rejected, surface a
        // structured `parameter_repair` mirroring the cypher-binder slice so
        // the LLM can fix the input in one round-trip without re-reading the
        // per-artifact `artifact_diagnostics`.
        let parameter_repair = match status_str {
            "ok" => Value::Null,
            "no_artifacts" => json!({
                "invalid_field": "artifacts",
                "hint": "supply at least one artifact object in the `artifacts` array; \
                        each artifact needs `artifact_type` and one of `artifact_ref` / \
                        `path` / `file_path` / `uri`",
                "accepted_aliases": ["artifact_ref", "path", "file_path", "uri"],
                "accepted_artifact_schema": accepted_schema,
            }),
            _ => first_rejected_repair(&artifact_diagnostics, &accepted_schema).unwrap_or_else(
                || {
                    json!({
                        "invalid_field": "artifacts",
                        "hint": "review `artifact_diagnostics` for per-artifact rejection reasons",
                        "accepted_aliases": ["artifact_ref", "path", "file_path", "uri"],
                        "accepted_artifact_schema": accepted_schema,
                    })
                },
            ),
        };

        let content_text = match status_str {
            "ok" => format!(
                "Attached {} evidence item(s) to {}:{}",
                attached, entity_type, entity_id
            ),
            "no_artifacts" => format!(
                "Attached 0 evidence item(s) to {}:{} — `artifacts` array was empty. {}",
                entity_type,
                entity_id,
                next_action
                    .as_deref()
                    .unwrap_or("supply at least one artifact"),
            ),
            "rejected_all" => format!(
                "Attached 0 of {} evidence item(s) to {}:{} — all rejected. {}",
                total,
                entity_type,
                entity_id,
                next_action.as_deref().unwrap_or("see artifact_diagnostics"),
            ),
            "partial" => format!(
                "Attached {} of {} evidence item(s) to {}:{} — {} rejected. {}",
                attached,
                total,
                entity_type,
                entity_id,
                total - attached,
                next_action.as_deref().unwrap_or("see artifact_diagnostics"),
            ),
            _ => format!(
                "Attached {} evidence item(s) to {}:{}",
                attached, entity_type, entity_id
            ),
        };

        Some(json!({
            "content": [{"type":"text","text": content_text}],
            "data": {
                "status": status_str,
                "attached": attached,
                "total": total,
                "normalized_entity_type": normalize_traceability_entity_type(entity_type),
                "accepted_artifact_schema": accepted_schema,
                "artifact_diagnostics": artifact_diagnostics,
                "fallback_guidance": fallback_guidance,
                "next_action": next_action,
                "operator_guidance": operator_guidance,
                "parameter_repair": parameter_repair,
            }
        }))
    }
}

/// REQ-AXO-901619 — synthesise the "did you mean" suggested absolute path
/// for a relative `raw_ref` joined against the canonical project root.
/// Returns None when there is no project root, the path is already absolute,
/// or the raw_ref is empty. The path is returned even if the file does NOT
/// exist on disk so the LLM can spot the typo / wrong-prefix mistake.
fn build_suggested_absolute_path(raw_ref: &str, project_root: Option<&str>) -> Option<String> {
    let trimmed = raw_ref.trim();
    if trimmed.is_empty() {
        return None;
    }
    let p = Path::new(trimmed);
    if p.is_absolute() {
        return None;
    }
    let root = project_root?;
    let joined = Path::new(root).join(p);
    Some(joined.to_string_lossy().into_owned())
}

#[cfg(test)]
mod build_suggested_absolute_path_tests {
    use super::build_suggested_absolute_path;

    #[test]
    fn returns_joined_path_for_relative_ref() {
        let result =
            build_suggested_absolute_path("Cargo.toml", Some("/home/dstadel/projects/axon"));
        assert_eq!(
            result.as_deref(),
            Some("/home/dstadel/projects/axon/Cargo.toml")
        );
    }

    #[test]
    fn returns_none_for_absolute_path() {
        let result =
            build_suggested_absolute_path("/etc/hostname", Some("/home/dstadel/projects/axon"));
        assert_eq!(result, None);
    }

    #[test]
    fn returns_none_when_no_project_root() {
        let result = build_suggested_absolute_path("Cargo.toml", None);
        assert_eq!(result, None);
    }

    #[test]
    fn returns_none_for_empty_ref() {
        let result = build_suggested_absolute_path("", Some("/home/dstadel/projects/axon"));
        assert_eq!(result, None);
    }
}

// REQ-AXO-139 slice — extract structured parameter_repair from the first
// rejected artifact so an LLM can fix one input field per round-trip.
// Returns None when no rejection diagnostic is present (caller falls back to
// a generic shape).
fn first_rejected_repair(
    artifact_diagnostics: &[Value],
    accepted_schema: &[String],
) -> Option<Value> {
    for diag in artifact_diagnostics {
        let status = diag.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if status != "rejected" {
            continue;
        }
        let idx = diag.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
        let kind = diag
            .get("normalized_artifact_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let supplied = diag
            .get("input")
            .and_then(|v| v.get("artifact_type"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let raw_ref = diag
            .get("input")
            .and_then(|v| {
                v.get("artifact_ref")
                    .or_else(|| v.get("path"))
                    .or_else(|| v.get("file_path"))
                    .or_else(|| v.get("uri"))
            })
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let reasons: Vec<&str> = diag
            .get("reasons")
            .and_then(|r| r.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();
        let primary = reasons
            .iter()
            .find(|r| {
                **r != "traceability_inserted"
                    && **r != "matched_indexed_file"
                    && **r != "normalized_relative_project_path"
                    && **r != "resolved_existing_project_file"
                    && **r != "resolved_existing_absolute_file"
            })
            .copied()
            .unwrap_or("");

        let repair = match primary {
            "missing_artifact_ref" | "empty_path" => json!({
                "invalid_field": "artifact_ref",
                "rejected_artifact_index": idx,
                "rejected_artifact_kind": kind,
                "primary_reason": primary,
                "accepted_aliases": ["artifact_ref", "path", "file_path", "uri"],
                "required_field_hint": required_field_hint_for_artifact_kind(kind),
                "hint": format!(
                    "artifact #{idx} ({kind}) is missing a value: {}",
                    required_field_hint_for_artifact_kind(kind)
                ),
            }),
            "artifact_type_not_allowed_for_entity" => json!({
                "invalid_field": "artifact_type",
                "rejected_artifact_index": idx,
                "supplied_artifact_type": supplied,
                "accepted_artifact_schema": accepted_schema,
                "primary_reason": primary,
                "hint": format!(
                    "artifact #{idx} type `{supplied}` is not accepted; use one of {accepted_schema:?}"
                ),
            }),
            "path_not_resolvable" => {
                // REQ-AXO-901619 — forward suggested_absolute_path + project_root
                // so the LLM gets a concrete "did you mean" candidate in
                // parameter_repair (not buried in artifact_diagnostics).
                let suggested = diag
                    .get("suggested_absolute_path")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let root = diag
                    .get("project_root")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let mut repair = json!({
                    "invalid_field": "artifact_ref",
                    "rejected_artifact_index": idx,
                    "rejected_artifact_kind": kind,
                    "supplied_artifact_ref": raw_ref,
                    "primary_reason": primary,
                    "accepted_aliases": ["artifact_ref", "path", "file_path", "uri"],
                    "required_field_hint": required_field_hint_for_artifact_kind(kind),
                });
                if let Some(s) = &suggested {
                    repair["did_you_mean"] = json!(s);
                }
                if let Some(r) = &root {
                    repair["project_root"] = json!(r);
                }
                repair["hint"] = match (suggested.as_deref(), root.as_deref()) {
                    (Some(s), Some(r)) => json!(format!(
                        "artifact #{idx} path `{raw_ref}` does not exist relative to project root `{r}` nor as an absolute path. Did you mean `{s}`?"
                    )),
                    _ => json!(format!(
                        "artifact #{idx} path `{raw_ref}` does not resolve under the project root \
                         and is not absolute; {}",
                        required_field_hint_for_artifact_kind(kind)
                    )),
                };
                repair
            }
            "traceability_insert_failed" => json!({
                "invalid_field": "artifact_ref",
                "rejected_artifact_index": idx,
                "rejected_artifact_kind": kind,
                "primary_reason": primary,
                "hint": "graph_store insert failed; check Traceability schema and DB availability"
            }),
            other => json!({
                "invalid_field": "artifact_ref",
                "rejected_artifact_index": idx,
                "rejected_artifact_kind": kind,
                "primary_reason": other,
                "accepted_aliases": ["artifact_ref", "path", "file_path", "uri"],
                "required_field_hint": required_field_hint_for_artifact_kind(kind),
                "hint": format!(
                    "artifact #{idx} rejected: `{other}`; see artifact_diagnostics for full detail"
                ),
            }),
        };

        return Some(repair);
    }
    None
}

impl McpServer {
    /// REQ-AXO-254 — close MIL-AXO-015 wave G followup ("33 broken_file_evidence
    /// cleanup via soll_remove_evidence MCP verb"). Removes Traceability rows
    /// linking a SOLL entity to evidence artifacts. Two modes:
    ///
    /// 1. `broken_only=true` (default): removes only rows whose `artifact_ref`
    ///    no longer resolves to an existing file/document on disk, using the
    ///    same path-resolution logic as `broken_file_evidence_count_for_requirement`
    ///    (project-root-relative or absolute). Safe maintenance.
    ///
    /// 2. `broken_only=false`: removes the explicit `artifact_refs` regardless
    ///    of disk state. Intended for surgical correction (e.g. a renamed
    ///    file that still exists at the new path — operator wants to drop the
    ///    stale row without waiting for the file to actually disappear).
    ///
    /// Returns: `removed_count`, `removed[]` (id + artifact_ref), `kept[]`
    /// (entries that did NOT match the removal criterion, for audit).
    pub(crate) fn axon_soll_remove_evidence(&self, args: &Value) -> Option<Value> {
        let entity_id = match args.get("entity_id").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => {
                return Some(json!({
                    "content": [{"type":"text","text":"Missing required argument: entity_id"}],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "parameter_repair": {
                            "invalid_field": "entity_id",
                            "missing_required_fields": ["entity_id"],
                            "hint": "supply a canonical SOLL entity id (e.g. REQ-AXO-013)"
                        }
                    }
                }));
            }
        };
        let broken_only = args
            .get("broken_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let explicit_refs: Vec<String> = args
            .get("artifact_refs")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        // Resolve project root for relative-path checks (matches
        // `broken_file_evidence_count_for_requirement`).
        let project_root = self.canonical_project_root_for_entity(&entity_id);
        let escaped = escape_sql(&entity_id);
        let query = format!(
            "SELECT id, COALESCE(artifact_ref, ''), COALESCE(artifact_type, '') \
             FROM soll.Traceability \
             WHERE soll_entity_id = '{escaped}' \
               AND lower(artifact_type) IN ('file', 'document') \
             ORDER BY id"
        );
        let raw = match self.graph_store.query_json(&query) {
            Ok(s) => s,
            Err(e) => {
                return Some(json!({
                    "content": [{"type":"text","text": format!("soll_remove_evidence read error: {e}")}],
                    "isError": true,
                    "data": { "status": "internal_error" }
                }));
            }
        };
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();

        let mut removed: Vec<Value> = Vec::new();
        let mut kept: Vec<Value> = Vec::new();
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let id = row[0].clone();
            let artifact_ref = row[1].clone();
            let artifact_type = row.get(2).cloned().unwrap_or_default();
            let should_remove = if broken_only {
                let trimmed = artifact_ref.trim();
                if trimmed.is_empty() {
                    true
                } else {
                    let p = std::path::Path::new(trimmed);
                    let candidate = if p.is_absolute() {
                        p.to_path_buf()
                    } else if let Some(root) = project_root.as_ref() {
                        root.join(p)
                    } else {
                        p.to_path_buf()
                    };
                    !candidate.exists()
                }
            } else {
                explicit_refs.iter().any(|r| r == &artifact_ref)
            };
            if should_remove {
                if let Err(e) = self
                    .graph_store
                    .execute_param("DELETE FROM soll.Traceability WHERE id = ?", &json!([id]))
                {
                    kept.push(json!({
                        "id": id,
                        "artifact_ref": artifact_ref,
                        "artifact_type": artifact_type,
                        "status": "delete_failed",
                        "error": e.to_string()
                    }));
                } else {
                    removed.push(json!({
                        "id": id,
                        "artifact_ref": artifact_ref,
                        "artifact_type": artifact_type
                    }));
                }
            } else {
                kept.push(json!({
                    "id": id,
                    "artifact_ref": artifact_ref,
                    "artifact_type": artifact_type,
                    "status": "preserved"
                }));
            }
        }

        let removed_count = removed.len();
        let kept_count = kept.len();
        let mode_label = if broken_only {
            "broken_only"
        } else {
            "explicit_refs"
        };
        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "soll_remove_evidence({entity_id}, mode={mode_label}): removed {removed_count}, kept {kept_count}"
                )
            }],
            "data": {
                "entity_id": entity_id,
                "mode": mode_label,
                "removed_count": removed_count,
                "removed": removed,
                "kept": kept,
                "follow_up_tools": ["soll_verify_requirements", "soll_validate"]
            }
        }))
    }
}
