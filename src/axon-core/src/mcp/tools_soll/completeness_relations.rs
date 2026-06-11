use super::*;
use crate::soll_snapshot::SollSnapshot;

impl McpServer {
    /// DEC-AXO-091 / REQ-AXO-322 (v2) — snapshot-aware endpoint
    /// classifier. When the snapshot contains the id we skip the SQL
    /// existence check entirely (saves ~1ms per endpoint × thousands
    /// per `collect_relation_policy_violations`). Falls back to the
    /// legacy SQL classifier for non-SOLL ids (File/Symbol/Chunk
    /// artifacts) and for SOLL ids that are absent from the snapshot
    /// (cross-project edges, dangling references, or snapshot=None).
    pub(crate) fn classify_endpoint_fast(
        &self,
        id: &str,
        snapshot: Option<&SollSnapshot>,
    ) -> anyhow::Result<LinkEndpointKind> {
        if let Some(snap) = snapshot {
            if snap.nodes.contains_key(id) {
                let prefix = id.split('-').next().unwrap_or("");
                let canonical_prefix = match prefix {
                    "VIS" => "VIS",
                    "PIL" => "PIL",
                    "REQ" => "REQ",
                    "CPT" => "CPT",
                    "DEC" => "DEC",
                    "MIL" => "MIL",
                    "VAL" => "VAL",
                    "STK" => "STK",
                    "GUI" => "GUI",
                    _ => {
                        return self.classify_existing_link_endpoint(id);
                    }
                };
                return Ok(LinkEndpointKind::Soll(canonical_prefix));
            }
        }
        self.classify_existing_link_endpoint(id)
    }

    pub(crate) fn classify_existing_link_endpoint(
        &self,
        id: &str,
    ) -> anyhow::Result<LinkEndpointKind> {
        let prefix = id.split('-').next().unwrap_or("");
        if let Some(table_name) = soll_entity_table_name(prefix) {
            let exists = self.graph_store.query_count(&format!(
                "SELECT count(*) FROM {} WHERE id = '{}'",
                table_name,
                escape_sql(id)
            ))?;
            if exists == 0 {
                return Err(anyhow!("ID `{}` not found", id));
            }
            let canonical_prefix = match prefix {
                "VIS" => "VIS",
                "PIL" => "PIL",
                "REQ" => "REQ",
                "CPT" => "CPT",
                "DEC" => "DEC",
                "MIL" => "MIL",
                "VAL" => "VAL",
                "STK" => "STK",
                "GUI" => "GUI",
                "SKI" => "SKI", // REQ-AXO-91578
                "PRT" => "PRT", // REQ-AXO-91579
                _ => return Err(anyhow!("Unsupported SOLL prefix `{}`", prefix)),
            };
            return Ok(LinkEndpointKind::Soll(canonical_prefix));
        }

        for table_name in ["File", "Symbol", "Chunk"] {
            let column = if table_name == "File" { "path" } else { "id" };
            let exists = self.graph_store.query_count(&format!(
                "SELECT count(*) FROM {} WHERE {} = '{}'",
                table_name,
                column,
                escape_sql(id)
            ))?;
            if exists > 0 {
                return Ok(LinkEndpointKind::Artifact);
            }
        }

        Err(anyhow!("ID `{}` not found", id))
    }

    pub(crate) fn select_relation_type_for_link(
        &self,
        source_id: &str,
        target_id: &str,
        explicit_relation_type: Option<&str>,
    ) -> anyhow::Result<(&'static str, RelationPolicy)> {
        let source_kind = self.classify_existing_link_endpoint(source_id)?;
        let target_kind = self.classify_existing_link_endpoint(target_id)?;
        let policy = relation_policy_for_pair(source_kind.label(), target_kind.label())
            .ok_or_else(|| {
                anyhow!(
                    "{}",
                    json!({
                        "error": "forbidden_relation",
                        "attempted": format!("{} -> {}", source_kind.label(), target_kind.label()),
                        "reason": if relation_policy_for_pair(target_kind.label(), source_kind.label()).is_some() {
                            "canonical direction exists in the reverse direction"
                        } else {
                            "no canonical relation policy exists for this pair"
                        },
                        "did_you_mean": reverse_relation_hint_payload(source_kind.label(), target_kind.label())
                    })
                    .to_string()
                )
            })?;

        let selected = if let Some(relation_type) = explicit_relation_type {
            let normalized = relation_type.to_uppercase();
            if !policy.allowed.iter().any(|allowed| *allowed == normalized) {
                return Err(anyhow!(
                    "Relation `{}` forbidden for {} -> {}. Allowed: {}. Default: {}",
                    normalized,
                    source_kind.label(),
                    target_kind.label(),
                    policy.allowed.join(", "),
                    policy.default.unwrap_or("none")
                ));
            }
            normalized
        } else if let Some(default_relation) = policy.default {
            default_relation.to_string()
        } else {
            return Err(anyhow!(
                "Explicit relation required for {} -> {}. Allowed: {}",
                source_kind.label(),
                target_kind.label(),
                policy.allowed.join(", ")
            ));
        };

        let selected_static = policy
            .allowed
            .iter()
            .find(|allowed| **allowed == selected)
            .copied()
            .ok_or_else(|| anyhow!("Relation `{}` not found in canonical policy", selected))?;

        Ok((selected_static, policy))
    }

    pub(crate) fn relation_guidance_for_link(
        &self,
        source_id: &str,
        target_id: &str,
        explicit_relation_type: Option<&str>,
    ) -> Value {
        let requested_relation = explicit_relation_type.map(|value| value.to_ascii_uppercase());
        let source_kind = self.classify_existing_link_endpoint(source_id);
        let target_kind = self.classify_existing_link_endpoint(target_id);

        match (source_kind, target_kind) {
            (Ok(source_kind), Ok(target_kind)) => {
                let source_label = source_kind.label();
                let target_label = target_kind.label();
                let mut payload = relation_policy_payload(source_label, target_label);
                payload["source_id"] = json!(source_id);
                payload["target_id"] = json!(target_id);
                payload["requested_relation"] = requested_relation
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null);
                payload["allowed_target_kinds_from_source"] =
                    Value::Array(allowed_relation_targets_from_source(source_label));
                payload["recommended_incoming_links_to_source_kind"] =
                    Value::Array(incoming_relation_sources_for_target(source_label));
                payload["recommended_incoming_links_to_target_kind"] =
                    Value::Array(incoming_relation_sources_for_target(target_label));
                payload["source_graph_role"] = Value::from(graph_role_for_kind(source_label));
                payload["target_graph_role"] = Value::from(graph_role_for_kind(target_label));
                payload["canonical_examples"] = Value::Array(
                    payload
                        .get("allowed_relations")
                        .and_then(|value| value.as_array())
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|value| {
                            value.as_str().map(|relation| {
                                json!({
                                    "relation_type": relation,
                                    "example": relation_example_sentence(source_label, target_label, relation)
                                })
                            })
                        })
                        .collect(),
                );
                payload["suggested_next_actions"] = if payload
                    .get("pair_allowed")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
                {
                    let default_relation = payload
                        .get("default_relation")
                        .and_then(|value| value.as_str());
                    let mut actions = Vec::new();
                    if let Some(default_relation) = default_relation {
                        actions.push(format!(
                            "retry `soll_manager` link with relation_type `{}`",
                            default_relation
                        ));
                    }
                    actions.push(
                        "call `soll_relation_schema` with the same source/target ids".to_string(),
                    );
                    actions.push(
                        "if the graph is still incomplete, inspect `recommended_incoming_links_to_target_kind` for the target node".to_string(),
                    );
                    Value::Array(actions.into_iter().map(Value::from).collect())
                } else {
                    Value::Array(vec![
                        Value::from("call `soll_relation_schema` with `source_id` to inspect allowed target kinds"),
                        Value::from("choose a target id whose kind matches one of `allowed_target_kinds_from_source`"),
                        Value::from("inspect `recommended_incoming_links_to_target_kind` if the current target should be reached from another source kind"),
                    ])
                };
                payload
            }
            (source_result, target_result) => {
                let mut errors = Vec::new();
                if let Err(error) = source_result {
                    errors.push(format!("source lookup failed: {}", error));
                }
                if let Err(error) = target_result {
                    errors.push(format!("target lookup failed: {}", error));
                }
                json!({
                    "pair_allowed": false,
                    "source_id": source_id,
                    "target_id": target_id,
                    "requested_relation": requested_relation,
                    "lookup_errors": errors,
                    "suggested_next_actions": [
                        "verify that both ids exist and are canonical",
                        "call `soll_relation_schema` with the known ids or kinds before retrying"
                    ]
                })
            }
        }
    }

    pub(crate) fn insert_validated_relation(
        &self,
        relation_type: &str,
        source_id: &str,
        target_id: &str,
        policy: RelationPolicy,
    ) -> anyhow::Result<bool> {
        let same_relation_exists = self.graph_store.query_count(&format!(
            "SELECT count(*) FROM soll.Edge WHERE source_id = '{}' AND target_id = '{}' AND relation_type = '{}'",
            escape_sql(source_id),
            escape_sql(target_id),
            escape_sql(relation_type)
        ))?;
        if same_relation_exists > 0 {
            return Ok(false);
        }

        if !policy.allow_multiple_types {
            for other_relation in policy.allowed {
                if *other_relation == relation_type {
                    continue;
                }
                let count = self.graph_store.query_count(&format!(
                    "SELECT count(*) FROM soll.Edge WHERE source_id = '{}' AND target_id = '{}' AND relation_type = '{}'",
                    escape_sql(source_id),
                    escape_sql(target_id),
                    escape_sql(other_relation)
                ))?;
                if count > 0 {
                    return Err(anyhow::anyhow!(
                        "Cardinality conflict: `{}` already exists for `{}` -> `{}`; `{}` is exclusive on this pair",
                        other_relation,
                        source_id,
                        target_id,
                        relation_type
                    ));
                }
            }
        }

        // REQ-AXO-152: derive project_code from canonical ID prefix and write
        // it on INSERT. NULL project_code rows brick brain boot via WAL replay
        // / backfill PK conflict (observed 2026-05-03 promotion). Source first,
        // target as fallback (cross-project edges are rare; default to source's
        // tenant). 'AXO' fallback preserves pre-multi-tenant single-project
        // semantics for any caller that passes a non-canonical ID.
        let project_code = super::shared::project_code_from_canonical_entity_id(source_id)
            .or_else(|| super::shared::project_code_from_canonical_entity_id(target_id))
            .unwrap_or_else(|| "AXO".to_string());

        self.graph_store.execute_param(
            "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata, project_code) VALUES (?, ?, ?, '{}', ?) ON CONFLICT DO NOTHING",
            &serde_json::json!([source_id, target_id, relation_type, project_code]),
        )?;
        Ok(true)
    }

    pub(crate) fn collect_relation_policy_violations(
        &self,
        project_code: Option<&str>,
    ) -> anyhow::Result<Vec<String>> {
        let mut violations = Vec::new();
        let mut exclusive_pairs: std::collections::HashMap<
            (String, String),
            std::collections::HashSet<String>,
        > = std::collections::HashMap::new();

        // DEC-AXO-091 / REQ-AXO-322 (v2) — when the call is scoped to a
        // project, walk the in-memory snapshot's edges and classify
        // endpoints via the snapshot's nodes map. The previous SQL
        // path issued 2 `SELECT count(*)` per edge to verify endpoint
        // existence — at 789 AXO edges that's ~1,500 round-trips per
        // call, observed at ~1.6 s of `soll_verify_requirements`.
        // Workspace-wide (no project_code) keeps SQL since the
        // snapshot is per-project.
        let edge_rows: Vec<(String, String, String)> = if let Some(code) = project_code {
            let snapshot = self.soll_cache().snapshot(code)?;
            snapshot
                .edges
                .iter()
                .map(|e| {
                    (
                        e.source_id.clone(),
                        e.target_id.clone(),
                        e.relation_type.clone(),
                    )
                })
                .collect()
        } else {
            let rows_raw = self.graph_store.query_json(
                "SELECT source_id, target_id, relation_type FROM soll.Edge ORDER BY source_id, target_id",
            )?;
            let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();
            rows.into_iter()
                .filter_map(|r| {
                    if r.len() < 3 {
                        None
                    } else {
                        Some((r[0].clone(), r[1].clone(), r[2].clone()))
                    }
                })
                .collect()
        };

        // Cache snapshot for endpoint classification (fast-path lookup).
        let snapshot_opt = if let Some(code) = project_code {
            self.soll_cache().snapshot(code).ok()
        } else {
            None
        };

        for (source_id, target_id, relation_type) in edge_rows.iter() {
            let source_id: &str = source_id.as_str();
            let target_id: &str = target_id.as_str();
            let relation_type: &str = relation_type.as_str();
            if !relation_scope_matches(source_id, target_id, project_code) {
                continue;
            }

            let source_kind = match self.classify_endpoint_fast(source_id, snapshot_opt.as_deref())
            {
                Ok(kind) => kind,
                Err(e) => {
                    violations.push(format!(
                        "{}: {} -> {} ({})",
                        relation_type, source_id, target_id, e
                    ));
                    continue;
                }
            };
            let target_kind = match self.classify_endpoint_fast(target_id, snapshot_opt.as_deref())
            {
                Ok(kind) => kind,
                Err(e) => {
                    violations.push(format!(
                        "{}: {} -> {} ({})",
                        relation_type, source_id, target_id, e
                    ));
                    continue;
                }
            };

            let Some(policy) = relation_policy_for_pair(source_kind.label(), target_kind.label())
            else {
                violations.push(format!(
                    "{}: {} -> {} (pair {} -> {} forbidden)",
                    relation_type,
                    source_id,
                    target_id,
                    source_kind.label(),
                    target_kind.label()
                ));
                continue;
            };

            if !policy
                .allowed
                .iter()
                .any(|allowed| *allowed == relation_type)
            {
                violations.push(format!(
                    "{}: {} -> {} (not allowed for {} -> {}; allowed: {})",
                    relation_type,
                    source_id,
                    target_id,
                    source_kind.label(),
                    target_kind.label(),
                    policy.allowed.join(", ")
                ));
                continue;
            }

            if !policy.allow_multiple_types {
                exclusive_pairs
                    .entry((source_id.to_string(), target_id.to_string()))
                    .or_default()
                    .insert(relation_type.to_string());
            }
        }

        for ((source_id, target_id), relation_types) in exclusive_pairs {
            if relation_types.len() > 1 {
                let mut rels = relation_types.into_iter().collect::<Vec<_>>();
                rels.sort();
                violations.push(format!(
                    "{} -> {} (conflicting exclusive relations: {})",
                    source_id,
                    target_id,
                    rels.join(", ")
                ));
            }
        }

        violations.sort();
        violations.dedup();
        Ok(violations)
    }

    pub(crate) fn axon_soll_relation_schema(
        &self,
        args: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let source_type = args
            .get("source_type")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_uppercase());
        let target_type = args
            .get("target_type")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_uppercase());
        let source_id = args
            .get("source_id")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let target_id = args
            .get("target_id")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());

        if source_type.is_none()
            && target_type.is_none()
            && source_id.is_none()
            && target_id.is_none()
        {
            return Some(json!({
                "content": [{ "type": "text", "text": "`soll_relation_schema` attend au moins un de: `source_type`, `target_type`, `source_id`, `target_id`." }],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                    "parameter_repair": {
                        "invalid_field": "source_type|target_type|source_id|target_id",
                        "accepted_aliases": ["source_type", "target_type", "source_id", "target_id"],
                        "follow_up_tools": ["help"],
                        "hint": "supply at least one of `source_type` / `target_type` (entity type) or `source_id` / `target_id` (canonical id) to scope the relation lookup"
                    }
                }
            }));
        }

        let resolved_source_type = match (source_type, source_id) {
            (Some(kind), _) => Some(kind),
            (None, Some(id)) => match self.classify_existing_link_endpoint(id) {
                Ok(kind) => Some(kind.label().to_string()),
                Err(error) => {
                    return Some(json!({
                        "content": [{ "type": "text", "text": format!("Cannot resolve `source_id`. Discovery remains available via guidance fields: {}", error) }],
                        "data": {
                            "resolved": false,
                            "lookup_stage": "source_id",
                            "source_id": id,
                            "target_id": target_id,
                            "suggested_next_actions": [
                                "verify source_id is canonical",
                                "retry with `source_type` if known"
                            ]
                        }
                    }))
                }
            },
            (None, None) => None,
        };
        let resolved_target_type = match (target_type, target_id) {
            (Some(kind), _) => Some(kind),
            (None, Some(id)) => match self.classify_existing_link_endpoint(id) {
                Ok(kind) => Some(kind.label().to_string()),
                Err(error) => {
                    return Some(json!({
                        "content": [{ "type": "text", "text": format!("Cannot resolve `target_id`. Discovery remains available via guidance fields: {}", error) }],
                        "data": {
                            "resolved": false,
                            "lookup_stage": "target_id",
                            "source_id": source_id,
                            "target_id": id,
                            "suggested_next_actions": [
                                "verify target_id is canonical",
                                "retry with `target_type` if known"
                            ]
                        }
                    }))
                }
            },
            (None, None) => None,
        };

        let data = match (
            resolved_source_type.as_deref(),
            resolved_target_type.as_deref(),
        ) {
            (Some(source_kind), Some(target_kind)) => {
                let mut payload = relation_policy_payload(source_kind, target_kind);
                let reverse_hint = reverse_relation_hint_payload(source_kind, target_kind);
                payload["allowed_target_kinds_from_source"] =
                    Value::Array(allowed_relation_targets_from_source(source_kind));
                payload["allowed_targets"] =
                    Value::Array(allowed_relation_targets_from_source(source_kind));
                payload["forbidden_targets"] = relation_schema_summary_for_kind(source_kind)
                    .get("forbidden_targets")
                    .cloned()
                    .unwrap_or_else(|| Value::Array(vec![]));
                payload["recommended_incoming_links_to_source_kind"] =
                    Value::Array(incoming_relation_sources_for_target(source_kind));
                payload["recommended_incoming_links_to_target_kind"] =
                    Value::Array(incoming_relation_sources_for_target(target_kind));
                payload["source_graph_role"] = Value::from(graph_role_for_kind(source_kind));
                payload["target_graph_role"] = Value::from(graph_role_for_kind(target_kind));
                payload["source_type"] = Value::from(source_kind);
                payload["target_type"] = Value::from(target_kind);
                payload["direction"] = Value::from("source_to_target");
                payload["canonical_examples"] = Value::Array(
                    relation_policy_for_pair(source_kind, target_kind)
                        .map(|policy| {
                            policy
                                .allowed
                                .iter()
                                .map(|relation| {
                                    json!({
                                        "relation_type": relation,
                                        "example": relation_example_sentence(source_kind, target_kind, relation)
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                );
                payload["source_id"] = source_id.map(Value::from).unwrap_or(Value::Null);
                payload["target_id"] = target_id.map(Value::from).unwrap_or(Value::Null);
                if !payload["pair_allowed"].as_bool().unwrap_or(false) && !reverse_hint.is_null() {
                    payload["did_you_mean"] = reverse_hint.clone();
                }
                // REQ-AXO-91495 — surface 3 terse AC fields at the top
                // level so the LLM doesn't have to drill into nested
                // policy structures :
                //   * canonical_direction: "SRC -> TGT"
                //   * allowed_relation_types: flat list (subset of the
                //     `allowed` field already inside policy_payload)
                //   * reverse_canonical: when forbidden, the reverse
                //     direction that IS canonical (alias for did_you_mean)
                payload["canonical_direction"] =
                    Value::from(format!("{} -> {}", source_kind, target_kind));
                payload["allowed_relation_types"] = payload
                    .get("allowed_relations")
                    .cloned()
                    .unwrap_or_else(|| Value::Array(vec![]));
                payload["reverse_canonical"] = if payload["pair_allowed"].as_bool().unwrap_or(false)
                {
                    Value::Null
                } else {
                    reverse_hint
                };
                payload
            }
            (Some(source_kind), None) => relation_schema_summary_for_kind(source_kind),
            (None, Some(target_kind)) => json!({
                "resolved": true,
                "target_kind": target_kind,
                "graph_role": graph_role_for_kind(target_kind),
                "kind_projection": kind_projection_policy(target_kind).map(|policy| json!({
                    "breadcrumb_eligible": policy.breadcrumb_eligible,
                    "root_eligible": policy.root_eligible,
                    "tree_order_rank": policy.tree_order_rank
                })),
                "incoming_from_source_kinds": incoming_relation_sources_for_target(target_kind),
                "guidance_source": "derived_from_relation_policy"
            }),
            (None, None) => unreachable!(),
        };

        // REQ-AXO-91495 — surface canonical_direction + allowed
        // relation_types inline so the LLM-visible text matches the
        // promise made by the tool name (no more "resolved" claim
        // without the actual guidance).
        let visible_text = match (
            data.get("canonical_direction").and_then(Value::as_str),
            data.get("allowed_relation_types")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                }),
        ) {
            (Some(direction), Some(allowed)) if !allowed.is_empty() => format!(
                "Canonical SOLL relation: {direction} via [{}]",
                allowed.join(", ")
            ),
            (Some(direction), _) => {
                // REQ-AXO-901907 — inline the actual attach path in the
                // visible text instead of merely NAMING the `data` fields.
                // The tool's promise is "discover valid links without trial
                // and error"; an LLM optimises on the rendered text and won't
                // drill into the structured envelope, so the legal route must
                // be in the sentence (progressive-disclosure was inverted).
                let mut lines = vec![format!("Direction {direction} has no canonical relation.")];
                if let Some(rev) = data
                    .get("reverse_canonical")
                    .filter(|value| !value.is_null())
                {
                    if let (Some(sk), Some(tk), Some(rt)) = (
                        rev.get("source_kind").and_then(Value::as_str),
                        rev.get("target_kind").and_then(Value::as_str),
                        rev.get("relation_type").and_then(Value::as_str),
                    ) {
                        lines.push(format!("Legal inverse: {sk} -[{rt}]-> {tk}."));
                    }
                }
                let incoming: Vec<String> = data
                    .get("recommended_incoming_links_to_target_kind")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|entry| {
                                let sk = entry.get("source_kind").and_then(Value::as_str)?;
                                let rt = entry
                                    .get("default_relation")
                                    .and_then(Value::as_str)
                                    .or_else(|| {
                                        entry
                                            .get("allowed_relations")
                                            .and_then(Value::as_array)
                                            .and_then(|a| a.first())
                                            .and_then(Value::as_str)
                                    })?;
                                Some(format!("{sk} -[{rt}]->"))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if !incoming.is_empty() {
                    let target_kind = data
                        .get("target_type")
                        .and_then(Value::as_str)
                        .unwrap_or("target");
                    lines.push(format!(
                        "Source kinds that can legally reach {target_kind}: {}.",
                        incoming.join(", ")
                    ));
                }
                lines.join(" ")
            }
            _ => "Canonical SOLL relation policy resolved — inspect `data` for kind-scoped guidance.".to_string(),
        };
        Some(json!({
            "content": [{ "type": "text", "text": visible_text }],
            "data": data
        }))
    }
}

#[cfg(test)]
mod insert_validated_relation_tests {
    // REQ-AXO-152: every soll.Edge INSERT must populate `project_code`.
    // NULL `project_code` rows brick brain boot via DuckDB WAL replay /
    // backfill PK conflict (observed 2026-05-03 promotion). Regression test:
    // create a link via soll_manager, assert the new edge carries
    // project_code derived from the source/target canonical ID.
    use crate::mcp::JsonRpcRequest;
    use crate::test_support::ist_fixtures::{
        assert_ist_count, create_test_server_with_ist_seed, IstSeed, SollNodeFixture,
    };
    use serde_json::json;

    #[test]
    fn soll_manager_link_populates_project_code_on_new_edge() {
        let harness = create_test_server_with_ist_seed(
            IstSeed::new()
                .node(SollNodeFixture::new(
                    "REQ-AXO-9001",
                    "Requirement",
                    "AXO",
                    "REQ-AXO-152 fixture",
                ))
                .node(SollNodeFixture::new(
                    "PIL-AXO-9001",
                    "Pillar",
                    "AXO",
                    "REQ-AXO-152 pillar",
                )),
        )
        .unwrap();

        let response = harness
            .server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "soll_manager",
                    "arguments": {
                        "action": "link",
                        "entity": "requirement",
                        "data": {
                            "source_id": "REQ-AXO-9001",
                            "target_id": "PIL-AXO-9001",
                            "relation_type": "BELONGS_TO"
                        }
                    }
                })),
                id: Some(json!(15201)),
            })
            .expect("handle_request returned an envelope");
        let result = response.result.expect("link returned a result body");
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(
            text.contains("Link created"),
            "expected link to be created, got: {text}"
        );

        // The fix: the new edge must carry project_code='AXO' (derived from
        // source/target canonical ID prefix). Before REQ-AXO-152 fix this
        // count was 0; the row existed with project_code = NULL.
        assert_ist_count(
            &harness.store,
            "SELECT count(*) FROM soll.Edge WHERE source_id = 'REQ-AXO-9001' \
             AND target_id = 'PIL-AXO-9001' AND relation_type = 'BELONGS_TO' \
             AND project_code = 'AXO'",
            1,
        );
        // No NULL row leaked into the table from this insert.
        assert_ist_count(
            &harness.store,
            "SELECT count(*) FROM soll.Edge WHERE source_id = 'REQ-AXO-9001' \
             AND project_code IS NULL",
            0,
        );
    }
}
