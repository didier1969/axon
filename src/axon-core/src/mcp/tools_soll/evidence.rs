use super::*;

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

        for candidate in &candidates {
            let query = format!(
                "SELECT path FROM File WHERE path = '{}' LIMIT 1",
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
                artifact_diagnostics.push(json!({
                    "index": idx,
                    "input": art,
                    "status": "rejected",
                    "normalized_artifact_type": normalized_artifact_type,
                    "normalized_artifact_ref": artifact_ref,
                    "reasons": diagnostic_reasons
                }));
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

        Some(json!({
            "content": [{"type":"text","text": format!("Attached {} evidence item(s) to {}:{}", attached, entity_type, entity_id)}],
            "data": {
                "attached": attached,
                "normalized_entity_type": normalize_traceability_entity_type(entity_type),
                "accepted_artifact_schema": accepted_schema,
                "artifact_diagnostics": artifact_diagnostics,
                "fallback_guidance": fallback_guidance
            }
        }))
    }
}
