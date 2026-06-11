use super::*;

/// REQ-AXO-323 Fault 4 — `soll_manager.create` defaulted `status=''` when
/// the caller omitted the field, which violated the `soll_node_status_canonical`
/// CHECK and surfaced a cryptic DB error. This helper resolves the effective
/// status from the caller's `data` payload : empty / missing → `planned`
/// (canonical inception status per DEC-PRO-100). For `validation` entities,
/// the `result` field takes precedence over `status` (existing behavior preserved).
fn resolve_create_status<'a>(
    supplied_status: Option<&'a str>,
    entity: &str,
    validation_result: Option<&'a str>,
) -> &'a str {
    let supplied = supplied_status.unwrap_or("");
    let candidate = if entity == "validation" {
        validation_result.unwrap_or(supplied)
    } else {
        supplied
    };
    if candidate.is_empty() {
        "planned"
    } else {
        candidate
    }
}

#[cfg(test)]
fn id_exists_envelope(id: &str, entity_type: &str) -> serde_json::Value {
    serde_json::json!({
        "content": [{
            "type": "text",
            "text": format!(
                "Cannot create `{}`: id already exists. Use action=update to modify the existing node.",
                id
            )
        }],
        "isError": true,
        "data": {
            "status": "id_exists",
            "id": id,
            "entity_type": entity_type,
            "hint": "create is reserved for new IDs; use action=update for modifications.",
            "parameter_repair": {
                "tool": "soll_manager",
                "category": "id_exists",
                "invalid_field": "data.id",
                "supplied_value": id,
                "follow_up_tools": ["soll_query_context", "soll_manager"],
                "hint": format!(
                    "node `{}` already exists. Pick a different id, or call action=update with the same id to modify it.",
                    id
                ),
            }
        }
    })
}

/// REQ-AXO-901955 — single source of truth for the metadata-routed fields
/// of `soll_manager`. create and update previously each carried their own
/// copy of this list ; they drifted, and top-level `tags` was routed by
/// NEITHER (only when nested under `metadata`), silently dropping a field
/// the tool contract documents as metadata_routed. Folding every routed
/// field here kills the class : a new routed field is honoured by create
/// AND update from one edit (GUI-PRO-013 DRY, GUI-PRO-108 class-level fix).
/// Keep in sync with the `metadata_routed` list in the soll_manager schema.
fn apply_metadata_routed_fields(data: &serde_json::Value, meta: &mut serde_json::Value) {
    const METADATA_ROUTED_FIELDS: &[&str] = &[
        "goal",
        "priority",
        "owner",
        "acceptance_criteria",
        "evidence_refs",
        "rationale",
        "context",
        "supersedes_decision_id",
        "impact_scope",
        "role",
        "method",
        "result",
        "tags",
    ];
    for key in METADATA_ROUTED_FIELDS {
        if let Some(value) = data.get(*key) {
            meta[*key] = value.clone();
        }
    }
}

impl McpServer {
    /// REQ-AXO-125 — normalize writer errors so the LLM-visible text
    /// contains only the action kind, category, and a recovery hint —
    /// never the raw SQL or backend internals (which previously leaked
    /// the partially-substituted INSERT statement and bound metadata
    /// JSON to the caller). The full error is surfaced under
    /// `data.diagnostic_excerpt` (truncated to keep response small)
    /// for clients that explicitly want to inspect it.
    ///
    /// REQ-AXO-341 — backend hints retargeted to PostgreSQL canonical
    /// (`sql` tool + `soll.Node`) post-MIL-AXO-017 ; the prior
    /// `cypher` + `soll.main.Node` + `duckdb_writer` strings referenced
    /// retired backends.
    fn normalized_soll_writer_error(action: &'static str, e: anyhow::Error) -> serde_json::Value {
        let raw = format!("{}", e);
        let category = if raw.contains("Writer Error")
            || raw.contains("INSERT INTO")
            || raw.contains("duplicate key value")
            || raw.contains("violates")
            || raw.contains("db error:")
        {
            "writer_failed"
        } else if raw.contains("forbidden_relation")
            || raw.contains("No canonical relation allowed")
        {
            "forbidden_relation"
        } else if raw.contains("not found") {
            "target_not_found"
        } else if raw.contains("Unknown id kind") {
            "registry_unknown_id_kind"
        } else {
            "unknown"
        };
        let recovery = match category {
            "writer_failed" => {
                "Writer rejected the insert. Check column constraints: id collision, schema drift, missing project_code, or unique-constraint violation. Inspect `data.diagnostic_excerpt` for the PG error text."
            }
            "forbidden_relation" => {
                "Use a canonical relation type: REQ -BELONGS_TO-> PIL, CPT -EXPLAINS-> REQ, DEC -SOLVES/IMPACTS-> REQ, PIL -EPITOMIZES-> VIS. Run `soll_relation_schema` to discover allowed pairs."
            }
            "target_not_found" => {
                "Verify the target id exists via `sql SELECT id FROM soll.Node WHERE id = '<id>'`. If the id was just created, ensure it was committed."
            }
            "registry_unknown_id_kind" => {
                "Use one of the canonical entity types: vision, pillar, requirement, concept, decision, milestone, stakeholder, validation, guideline."
            }
            _ => "Inspect data.diagnostic_excerpt for the underlying writer error.",
        };
        let mut excerpt = raw.clone();
        if excerpt.len() > 240 {
            excerpt.truncate(240);
            excerpt.push_str("...");
        }
        // Strip newlines so the excerpt fits inline.
        let excerpt = excerpt.replace('\n', " ").replace("  ", " ");
        let kind = format!("{}_failed", action);
        let visible_text = format!("soll_manager {action} failed ({category}). {recovery}");
        // REQ-AXO-147 — surface canonical parameter_repair alongside the
        // existing kind/category fields so the LLM can route on a single
        // shape across all 5 slices of the universal contract.
        let repair_field = match category {
            "writer_failed" => "data.title|description",
            "forbidden_relation" => "relation_type",
            "target_not_found" => "target_id",
            "registry_unknown_id_kind" => "entity",
            _ => "data",
        };
        serde_json::json!({
            "content": [{ "type": "text", "text": visible_text }],
            "isError": true,
            "data": {
                "status": "input_invalid",
                "kind": kind,
                "category": category,
                "next_action": recovery,
                "diagnostic_excerpt": excerpt,
                "operator_guidance": {
                    "problem_class": kind,
                    "follow_up_tools": ["sql", "soll_relation_schema", "project_registry_lookup"],
                    "confidence": "high"
                },
                "parameter_repair": {
                    "invalid_field": repair_field,
                    "category": category,
                    "action": action,
                    "follow_up_tools": ["sql", "soll_relation_schema", "project_registry_lookup"],
                    "hint": recovery,
                }
            }
        })
    }

    /// REQ-AXO-043 / REQ-AXO-125 / REQ-AXO-341 — strip SQL leakage from
    /// link errors while preserving the existing flat
    /// `data.relation_guidance` shape that MCP callers depend on. The
    /// `link` path historically returned `format!("Link error: {}", e)`
    /// which exposed raw INSERT statements. Here we detect the writer
    /// pattern and substitute a classified message ; non-SQL errors
    /// (e.g. `Cardinality conflict`, `Relation X not found in canonical
    /// policy`) keep their human-readable form. Backend hint retargeted
    /// to PG canonical post-MIL-AXO-017 (`sql` / `soll.Node`).
    pub(super) fn sanitized_link_error_text(e: &anyhow::Error) -> String {
        let raw = format!("{}", e);
        if raw.contains("Writer Error")
            || raw.contains("INSERT INTO")
            || raw.contains("duplicate key value")
            || raw.contains("violates")
            || raw.contains("db error:")
        {
            return "Link error: writer rejected the edge insert. Verify both endpoints exist via `sql SELECT id FROM soll.Node WHERE id IN ('<src>','<tgt>')` and that the relation_type is allowed for the pair via `soll_relation_schema`.".to_string();
        }
        format!("Link error: {}", raw)
    }

    pub(crate) fn axon_soll_manager(&self, args: &Value) -> Option<Value> {
        let action = args.get("action")?.as_str()?;
        let entity = args.get("entity")?.as_str()?;
        let data = args.get("data")?;

        match action {
            "create" => {
                // REQ-AXO-092 / REQ-AXO-043 — validate entity BEFORE the registry
                // resolves an ID. Otherwise an unknown entity surfaces as a generic
                // "Registry error: Unknown id kind" which buries the schema mismatch.
                let entity_type_cap = match entity {
                    "vision" => "Vision",
                    "pillar" => "Pillar",
                    "requirement" => "Requirement",
                    "concept" => "Concept",
                    "decision" => "Decision",
                    "milestone" => "Milestone",
                    "stakeholder" => "Stakeholder",
                    "validation" => "Validation",
                    "guideline" => "Guideline",
                    "skill" => "Skill",                    // REQ-AXO-91578
                    "prompt_template" => "PromptTemplate", // REQ-AXO-91579
                    other => {
                        let accepted = [
                            "vision",
                            "pillar",
                            "requirement",
                            "concept",
                            "decision",
                            "milestone",
                            "stakeholder",
                            "validation",
                            "guideline",
                            "skill",
                            "prompt_template",
                        ];
                        return Some(json!({
                            "content": [{
                                "type": "text",
                                "text": format!(
                                    "Unknown entity: `{}`. Use one of: {}.",
                                    other,
                                    accepted.join(", "),
                                ),
                            }],
                            "isError": true,
                            "data": {
                                "status": "input_invalid",
                                "rejected_entity": other,
                                "accepted_entities": accepted,
                                "next_action": format!(
                                    "retry with one of: {}",
                                    accepted.join(", "),
                                ),
                                "operator_guidance": {
                                    "problem_class": "input_invalid",
                                    "likely_cause": "entity_not_in_schema_enum",
                                    "next_best_actions": [
                                        format!("retry with `entity` in: {}", accepted.join(", ")),
                                    ],
                                    "confidence": "high",
                                },
                                "parameter_repair": {
                                    "invalid_field": "entity",
                                    "supplied_value": other,
                                    "accepted_values": accepted,
                                    "follow_up_tools": ["help"],
                                    "hint": format!(
                                        "retry with one of the accepted entity types: {}",
                                        accepted.join(", "),
                                    ),
                                },
                            },
                        }));
                    }
                };

                // MIL-AXO-020 slice 2 (REQ-AXO-91542) — LLM contract scrub.
                // id is server-allocated via soll.allocate_node_id (slice 1).
                // Caller-provided `data.id` / `args.reserved_id` is rejected
                // BEFORE project_code resolution so the LLM gets a recoverable
                // envelope without burning a Registry counter bump. Test
                // fixtures may opt in to the legacy reserved_id flow via
                // `AXON_ALLOW_RESERVED_ID=1`; production never sets it.
                let allow_reserved_id =
                    std::env::var("AXON_ALLOW_RESERVED_ID").ok().as_deref() == Some("1");
                let caller_id = data
                    .get("id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty());
                let caller_reserved_id = args
                    .get("reserved_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty());
                if caller_id.is_some() || (caller_reserved_id.is_some() && !allow_reserved_id) {
                    let supplied = caller_id.or(caller_reserved_id).unwrap_or("");
                    let invalid_field = if caller_id.is_some() {
                        "data.id"
                    } else {
                        "reserved_id"
                    };
                    return Some(json!({
                        "content": [{
                            "type": "text",
                            "text": format!(
                                "Cannot create with caller-provided id `{}`. Server allocates canonical ids via soll.allocate_node_id; provide `type` + `project_code` only.",
                                supplied
                            )
                        }],
                        "isError": true,
                        "data": {
                            "status": "input_invalid",
                            "operator_guidance": {
                                "problem_class": "id_field_forbidden",
                                "likely_cause": "caller_provided_id",
                                "follow_up_tools": ["soll_manager"],
                                "confidence": "high",
                            },
                            "parameter_repair": {
                                "tool": "soll_manager",
                                "category": "id_field_forbidden",
                                "invalid_field": invalid_field,
                                "supplied_value": supplied,
                                "accepted_fields": [
                                    "project_code",
                                    "title",
                                    "description",
                                    "status",
                                    "metadata",
                                    "attach_to",
                                    "relation_type",
                                    "priority",
                                    "owner",
                                    "rationale",
                                    "acceptance_criteria",
                                    "evidence_refs",
                                    "context",
                                    "goal",
                                    "result"
                                ],
                                "hint": "remove the id field and retry; server returns the allocated canonical id in the response",
                                "follow_up_tools": ["soll_manager"],
                            },
                            "canonical_source": "MIL-AXO-020",
                        },
                    }));
                }

                // MIL-AXO-020 slice 3 (REQ-AXO-91543) — Vision creation is
                // reserved for axon_init_project. Reject here before any
                // Registry counter bump so the operator gets a clean
                // recovery hint without burning state.
                if entity == "vision" {
                    return Some(json!({
                        "content": [{
                            "type": "text",
                            "text": "soll_manager cannot create a Vision. Visions are seeded by axon_init_project at project registration."
                        }],
                        "isError": true,
                        "data": {
                            "status": "input_invalid",
                            "operator_guidance": {
                                "problem_class": "vision_creation_forbidden",
                                "follow_up_tools": ["axon_init_project"],
                                "confidence": "high",
                            },
                            "parameter_repair": {
                                "tool": "soll_manager",
                                "category": "vision_creation_forbidden",
                                "invalid_field": "entity",
                                "supplied_value": "vision",
                                "accepted_values": [
                                    "pillar", "requirement", "concept", "decision",
                                    "milestone", "validation", "stakeholder", "guideline", "skill",
                                    "prompt_template"
                                ],
                                "hint": "to register a new project with its Vision, call axon_init_project; for downstream entities, choose another entity type",
                                "follow_up_tools": ["axon_init_project"],
                            },
                            "canonical_source": "MIL-AXO-020",
                        },
                    }));
                }

                let project_code_raw = args
                    .get("project_code")
                    .and_then(|v| v.as_str())
                    .or_else(|| data.get("project_code").and_then(|v| v.as_str()))
                    .map(str::trim);
                let project_code = match self.require_registered_mutation_project_code(
                    project_code_raw,
                    "soll_manager create",
                ) {
                    Ok(code) => code,
                    Err(e) => {
                        let supplied = project_code_raw.unwrap_or("");
                        return Some(json!({
                            "content": [{ "type": "text", "text": format!("Canonical project error: {}", e) }],
                            "isError": true,
                            "data": {
                                "status": "wrong_project_scope",
                                "operator_guidance": {
                                    "problem_class": "wrong_project_scope",
                                    "follow_up_tools": ["project_registry_lookup", "axon_init_project"],
                                },
                                "parameter_repair": {
                                    "invalid_field": "project_code",
                                    "supplied_value": supplied,
                                    "follow_up_tools": ["project_registry_lookup", "axon_init_project"],
                                    "hint": "supply a registered project_code; call `project_registry_lookup` to list registered codes or `axon_init_project` to register the current project"
                                },
                                "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                            }
                        }));
                    }
                };
                let before_snapshot = self.soll_completeness_snapshot(Some(&project_code)).ok();
                let reserved_id = args.get("reserved_id").and_then(|value| value.as_str());
                let (_requested_code, canonical_code, formatted_id) = if let Some(reserved_id) =
                    reserved_id
                {
                    match self.resolve_canonical_project_identity_for_mutation(&project_code) {
                        Ok((canonical_code, project_code)) => {
                            (canonical_code, project_code, reserved_id.to_string())
                        }
                        Err(e) => {
                            return Some(json!({
                                "content": [{ "type": "text", "text": format!("Registry error: {}", e) }],
                                "isError": true,
                                "data": {
                                    "status": "internal_error",
                                    "operator_guidance": {
                                        "problem_class": "internal_error",
                                        "follow_up_tools": ["status", "project_registry_lookup"],
                                    },
                                    "parameter_repair": {
                                        "invalid_field": "project_code",
                                        "stage": "id_reservation",
                                        "follow_up_tools": ["status", "project_registry_lookup"],
                                        "hint": "soll.Registry id-reservation failed; verify runtime is healthy via `status` and the project_code is registered via `project_registry_lookup`"
                                    },
                                    "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                                }
                            }))
                        }
                    }
                } else {
                    match self.next_soll_numeric_id(&project_code, entity) {
                        Ok((canonical_code, project_code, prefix, next_num)) => (
                            canonical_code,
                            project_code.clone(),
                            format!("{}-{}-{:03}", prefix, project_code, next_num),
                        ),
                        Err(e) => {
                            return Some(json!({
                                "content": [{ "type": "text", "text": format!("Registry error: {}", e) }],
                                "isError": true,
                                "data": {
                                    "status": "internal_error",
                                    "operator_guidance": {
                                        "problem_class": "internal_error",
                                        "follow_up_tools": ["status", "project_registry_lookup"],
                                    },
                                    "parameter_repair": {
                                        "invalid_field": "project_code",
                                        "stage": "id_reservation",
                                        "follow_up_tools": ["status", "project_registry_lookup"],
                                        "hint": "soll.Registry id-reservation failed; verify runtime is healthy via `status` and the project_code is registered via `project_registry_lookup`"
                                    },
                                    "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                                }
                            }))
                        }
                    }
                };

                let mut meta = data.get("metadata").cloned().unwrap_or(json!({}));
                let title = data
                    .get("title")
                    .or_else(|| data.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let description = data
                    .get("description")
                    .or_else(|| data.get("explanation"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // REQ-AXO-323 Fault 4 — empty `status` previously bubbled
                // up to the DB CHECK constraint as a cryptic error. Resolve
                // the effective status via `resolve_create_status` : empty
                // / omitted → `planned`. Validation entities keep their
                // `result`-field precedence (handled inside the helper).
                let status = resolve_create_status(
                    data.get("status").and_then(|v| v.as_str()),
                    entity,
                    data.get("result").and_then(|v| v.as_str()),
                );

                // REQ-AXO-325 — server-side validation of `status` against canonical
                // vocabulary (DEC-PRO-100). Mirror the entity-rejection envelope
                // pattern above so the LLM gets a recoverable error BEFORE the
                // DB CHECK constraint surfaces a cryptic message.
                const ACCEPTED_STATUS: [&str; 5] =
                    ["current", "planned", "delivered", "superseded", "rejected"];
                if !status.is_empty() && !ACCEPTED_STATUS.contains(&status) {
                    let normalization_hint = match status {
                        "completed" | "done" | "passed" | "closed" | "archived" => "delivered",
                        "accepted" | "in_progress" | "active" | "open" | "partial" | "pending" => {
                            "current"
                        }
                        "proposed" | "draft" => "planned",
                        "failed" => "rejected",
                        _ => "current",
                    };
                    return Some(json!({
                        "content": [{
                            "type": "text",
                            "text": format!(
                                "Invalid status `{}`. Canonical vocabulary (DEC-PRO-100) = [{}]. Suggested: `{}`.",
                                status,
                                ACCEPTED_STATUS.join(", "),
                                normalization_hint,
                            ),
                        }],
                        "isError": true,
                        "data": {
                            "status": "input_invalid",
                            "operator_guidance": {
                                "problem_class": "input_invalid",
                                "likely_cause": "status_not_in_canonical_vocabulary",
                                "follow_up_tools": ["soll_manager", "soll_query_context"],
                                "confidence": "high",
                            },
                            "parameter_repair": {
                                "tool": "soll_manager",
                                "category": "status",
                                "invalid_field": "data.status",
                                "supplied_value": status,
                                "accepted_values": ACCEPTED_STATUS,
                                "normalization_hint": normalization_hint,
                                "canonical_source": "DEC-PRO-100",
                                "follow_up_tools": ["soll_manager", "soll_query_context"],
                                "hint": format!(
                                    "retry with status in {:?} ; default `current` for newly-owned nodes",
                                    ACCEPTED_STATUS,
                                ),
                            },
                            "example_valid_call": {
                                "action": "create",
                                "entity": entity,
                                "data": {
                                    "project_code": canonical_code,
                                    "title": "<title>",
                                    "description": "<body>",
                                    "status": normalization_hint,
                                },
                            },
                        },
                    }));
                }

                apply_metadata_routed_fields(data, &mut meta);

                meta["updated_at"] = json!(now_unix_ms());

                // MIL-AXO-020 slice 3 (REQ-AXO-91543) — atomic create+attach.
                // attach_to + relation_type are REQUIRED for every entity
                // (Vision was rejected above). The node + edge land in a
                // single CTE so neither survives in isolation on failure.
                let attach_to = data
                    .get("attach_to")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty());
                let relation_type_raw = data
                    .get("relation_type")
                    .and_then(|v| v.as_str())
                    .or_else(|| data.get("relation_hint").and_then(|v| v.as_str()))
                    .filter(|s| !s.trim().is_empty());

                let (attach_to, relation_type) = match (attach_to, relation_type_raw) {
                    (Some(a), Some(r)) => (a, r.to_uppercase()),
                    (missing_attach, _) => {
                        let missing_field = if missing_attach.is_none() {
                            "attach_to"
                        } else {
                            "relation_type"
                        };
                        return Some(json!({
                            "content": [{
                                "type": "text",
                                "text": format!(
                                    "soll_manager create requires `{}`. Every non-Vision node must attach to a canonical parent in the same transaction (MIL-AXO-020).",
                                    missing_field
                                )
                            }],
                            "isError": true,
                            "data": {
                                "status": "input_invalid",
                                "operator_guidance": {
                                    "problem_class": "attach_required",
                                    "follow_up_tools": ["soll_relation_schema", "soll_query_context"],
                                    "confidence": "high",
                                },
                                "parameter_repair": {
                                    "tool": "soll_manager",
                                    "category": "attach_required",
                                    "invalid_field": format!("data.{}", missing_field),
                                    "required_fields": ["attach_to", "relation_type"],
                                    "hint": "supply canonical parent id and the relation type (e.g. REQ→PIL=BELONGS_TO, CPT→REQ=EXPLAINS, DEC→REQ=SOLVES, MIL→REQ=TARGETS, VAL→REQ=VERIFIES, GUI→PIL=BELONGS_TO). Use soll_relation_schema source_id=<your_type>-... for the full table.",
                                    "follow_up_tools": ["soll_relation_schema"],
                                },
                                "canonical_source": "MIL-AXO-020",
                            },
                        }));
                    }
                };

                // Validate target existence first so the operator gets a
                // precise envelope instead of a downstream DB error.
                let target_count = self
                    .graph_store
                    .query_count_param(
                        "SELECT COUNT(*) FROM soll.Node WHERE id = ?",
                        &json!([attach_to]),
                    )
                    .unwrap_or(0);
                if target_count == 0 {
                    return Some(json!({
                        "content": [{
                            "type": "text",
                            "text": format!(
                                "attach_to `{}` does not exist. Cannot create `{}` without a valid parent.",
                                attach_to, entity_type_cap
                            )
                        }],
                        "isError": true,
                        "data": {
                            "status": "input_invalid",
                            "operator_guidance": {
                                "problem_class": "attach_target_not_found",
                                "follow_up_tools": ["soll_query_context", "query"],
                                "confidence": "high",
                            },
                            "parameter_repair": {
                                "tool": "soll_manager",
                                "category": "attach_target_not_found",
                                "invalid_field": "data.attach_to",
                                "supplied_value": attach_to,
                                "hint": "verify the parent id via `soll_query_context project_code=<code>` or `sql SELECT id FROM soll.Node WHERE id = '<id>'`",
                                "follow_up_tools": ["soll_query_context"],
                            },
                            "canonical_source": "MIL-AXO-020",
                        },
                    }));
                }

                // Validate (source_type, relation_type, target_type) per
                // the canonical relation_policy table.
                let source_prefix = match entity_type_cap {
                    "Vision" => "VIS",
                    "Pillar" => "PIL",
                    "Requirement" => "REQ",
                    "Concept" => "CPT",
                    "Decision" => "DEC",
                    "Milestone" => "MIL",
                    "Validation" => "VAL",
                    "Stakeholder" => "STK",
                    "Guideline" => "GUI",
                    "Skill" => "SKI",          // REQ-AXO-91578
                    "PromptTemplate" => "PRT", // REQ-AXO-91579
                    other => other,
                };
                let target_prefix: String = attach_to.split('-').next().unwrap_or("").to_string();
                let policy = relation_policy_for_pair(source_prefix, &target_prefix);
                match &policy {
                    Some(p)
                        if p.allowed
                            .iter()
                            .any(|allowed| *allowed == relation_type.as_str()) => {}
                    _ => {
                        let allowed: Vec<&str> = policy
                            .as_ref()
                            .map(|p| p.allowed.to_vec())
                            .unwrap_or_default();
                        return Some(json!({
                            "content": [{
                                "type": "text",
                                "text": format!(
                                    "relation_type `{}` is not canonical for {} → {}. Allowed: {:?}.",
                                    relation_type, source_prefix, target_prefix, allowed
                                )
                            }],
                            "isError": true,
                            "data": {
                                "status": "input_invalid",
                                "operator_guidance": {
                                    "problem_class": "forbidden_relation_for_type",
                                    "follow_up_tools": ["soll_relation_schema"],
                                    "confidence": "high",
                                },
                                "parameter_repair": {
                                    "tool": "soll_manager",
                                    "category": "forbidden_relation_for_type",
                                    "invalid_field": "data.relation_type",
                                    "supplied_value": relation_type,
                                    "accepted_values": allowed,
                                    "source_type": source_prefix,
                                    "target_type": target_prefix,
                                    "hint": "pick an allowed relation_type for this (source_type, target_type) pair, or change attach_to to a target whose type fits your relation",
                                    "follow_up_tools": ["soll_relation_schema"],
                                },
                                "canonical_source": "MIL-AXO-020",
                            },
                        }));
                    }
                }

                // Atomic INSERT node + INSERT edge via single CTE so the
                // node never survives a failed edge insert.
                let cte_sql = "WITH new_node AS (\
                    INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                    VALUES (?, ?, ?, ?, ?, ?, ?::JSONB) \
                    RETURNING id\
                ) \
                INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) \
                SELECT new_node.id, ?, ?, ? FROM new_node";

                let insert_res = self.graph_store.execute_param(
                    cte_sql,
                    &json!([
                        formatted_id,
                        entity_type_cap,
                        canonical_code,
                        title,
                        description,
                        status,
                        meta.to_string(),
                        attach_to,
                        relation_type,
                        canonical_code
                    ]),
                );

                match insert_res {
                    Ok(()) => {
                        let created_id = formatted_id.clone();
                        let report = format!(
                            "SOLL entity created: `{}`\nCanonical link applied: `{}` -> `{}` via `{}`",
                            created_id, created_id, attach_to, relation_type
                        );
                        let mut response_data = json!({
                            "created_id": created_id,
                            "entity_type": entity_type_cap,
                            "project_code": canonical_code,
                            "canonical_next_links": self.canonical_next_link_hints(entity_type_cap),
                            "attach_attempted": true,
                            "attached": true,
                            "attached_to": attach_to,
                            "applied_relation": relation_type,
                            "attach_status": "attached"
                        });

                        if let (Some(before), Ok(after)) = (
                            before_snapshot.as_ref(),
                            self.soll_completeness_snapshot(Some(&canonical_code)),
                        ) {
                            response_data["mutation_feedback"] = self.mutation_feedback_payload(
                                before,
                                &after,
                                vec![json!({
                                    "id": formatted_id,
                                    "change_kind": "created",
                                    "entity_type": entity_type_cap
                                })],
                                json!({
                                    "nodes_created": 1,
                                    "nodes_updated": 0,
                                    "edges_created": 1
                                }),
                            );
                        }

                        Some(json!({
                            "content": [{ "type": "text", "text": report }],
                            "data": response_data
                        }))
                    }
                    Err(e) => Some(Self::normalized_soll_writer_error("insert", e)),
                }
            }
            "update" => {
                let id = data.get("id")?.as_str()?;
                let project_code = project_code_from_canonical_entity_id(id);
                let before_snapshot = project_code
                    .as_deref()
                    .and_then(|code| self.soll_completeness_snapshot(Some(code)).ok());

                // REQ-AXO-325 — validate `status` if supplied. Mirrors the create
                // path validation above; pre-empts the DB CHECK constraint.
                if let Some(supplied_status) = data.get("status").and_then(|v| v.as_str()) {
                    const ACCEPTED_STATUS: [&str; 5] =
                        ["current", "planned", "delivered", "superseded", "rejected"];
                    if !supplied_status.is_empty() && !ACCEPTED_STATUS.contains(&supplied_status) {
                        let normalization_hint = match supplied_status {
                            "completed" | "done" | "passed" | "closed" | "archived" => "delivered",
                            "accepted" | "in_progress" | "active" | "open" | "partial"
                            | "pending" => "current",
                            "proposed" | "draft" => "planned",
                            "failed" => "rejected",
                            _ => "current",
                        };
                        return Some(json!({
                            "content": [{
                                "type": "text",
                                "text": format!(
                                    "Invalid status `{}`. Canonical vocabulary (DEC-PRO-100) = [{}]. Suggested: `{}`.",
                                    supplied_status,
                                    ACCEPTED_STATUS.join(", "),
                                    normalization_hint,
                                ),
                            }],
                            "isError": true,
                            "data": {
                                "status": "input_invalid",
                                "operator_guidance": {
                                    "problem_class": "input_invalid",
                                    "likely_cause": "status_not_in_canonical_vocabulary",
                                    "follow_up_tools": ["soll_manager", "soll_query_context"],
                                    "confidence": "high",
                                },
                                "parameter_repair": {
                                    "tool": "soll_manager",
                                    "category": "status",
                                    "invalid_field": "data.status",
                                    "supplied_value": supplied_status,
                                    "accepted_values": ACCEPTED_STATUS,
                                    "normalization_hint": normalization_hint,
                                    "canonical_source": "DEC-PRO-100",
                                    "follow_up_tools": ["soll_manager", "soll_query_context"],
                                    "hint": format!(
                                        "retry with status in {:?}",
                                        ACCEPTED_STATUS,
                                    ),
                                },
                                "example_valid_call": {
                                    "action": "update",
                                    "entity": entity,
                                    "data": {
                                        "id": id,
                                        "status": normalization_hint,
                                    },
                                },
                            },
                        }));
                    }
                }

                let update_res: anyhow::Result<()> = (|| {
                    let current = self.query_named_row(
                        &format!("SELECT title, description, status, metadata FROM soll.Node WHERE id = '{}'", escape_sql(id)),
                        4,
                    )?;
                    let mut meta: Value = serde_json::from_str(&current[3]).unwrap_or(json!({}));

                    let title = data
                        .get("title")
                        .or_else(|| data.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&current[0]);
                    let description = data
                        .get("description")
                        .or_else(|| data.get("explanation"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&current[1]);
                    let status = data
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&current[2]);

                    if let Some(m) = data.get("metadata") {
                        if let Some(obj) = m.as_object() {
                            for (k, v) in obj {
                                meta[k] = v.clone();
                            }
                        }
                    }
                    apply_metadata_routed_fields(data, &mut meta);

                    meta["updated_at"] = json!(now_unix_ms());

                    let q = "UPDATE soll.Node SET title = ?, description = ?, status = ?, metadata = ? WHERE id = ?";
                    self.graph_store.execute_param(
                        q,
                        &json!([title, description, status, meta.to_string(), id]),
                    )
                })();

                match update_res {
                    Ok(_) => {
                        let mut payload = json!({
                            "content": [{ "type": "text", "text": format!("Update succeeded for `{}`", id) }],
                            "data": {
                                "project_code": project_code.as_deref().unwrap_or(""),
                            }
                        });
                        if let (Some(code), Some(before), Ok(after)) = (
                            project_code.as_deref(),
                            before_snapshot.as_ref(),
                            project_code
                                .as_deref()
                                .ok_or_else(|| anyhow!("missing project"))
                                .and_then(|value| self.soll_completeness_snapshot(Some(value))),
                        ) {
                            let _ = code;
                            payload["data"]["mutation_feedback"] = self.mutation_feedback_payload(
                                before,
                                &after,
                                vec![json!({
                                    "id": id,
                                    "change_kind": "updated",
                                    "fields": ["title", "description", "status", "metadata"]
                                })],
                                json!({
                                    "nodes_created": 0,
                                    "nodes_updated": 1,
                                    "edges_created": 0
                                }),
                            );
                        }
                        Some(payload)
                    }
                    Err(e) => Some(Self::normalized_soll_writer_error("update", e)),
                }
            }
            "link" => {
                let src = data.get("source_id")?.as_str()?;
                let tgt = data.get("target_id")?.as_str()?;
                let explicit_rel = data.get("relation_type").and_then(|v| v.as_str());
                let project_code = project_code_from_canonical_entity_id(src)
                    .or_else(|| project_code_from_canonical_entity_id(tgt));
                let before_snapshot = project_code
                    .as_deref()
                    .and_then(|code| self.soll_completeness_snapshot(Some(code)).ok());
                match self.select_relation_type_for_link(src, tgt, explicit_rel) {
                    Ok((relation_type, policy)) => {
                        let rel_table = relation_table_name(relation_type).unwrap_or(relation_type);

                        // MIL-AXO-020 slice 4 (REQ-AXO-91544) + REQ-AXO-901593 —
                        // cycle pre-check on the structural-backbone relations
                        // (DEC-AXO-098). Filiation is traversed as ONE mixed DAG
                        // (its 6 relations form a single hierarchy). Each
                        // non-filiation guarded relation (INHERITS_FROM, USES,
                        // …) is checked within its OWN type, so a legitimate
                        // cross-family path (e.g. REFINES + INHERITS_FROM) is not
                        // mistaken for a cycle. SUPERSEDES (redirect) and
                        // EPITOMIZES (Pillar→Vision axis) stay exempt.
                        const FILIATION: [&str; 6] = [
                            "SOLVES",
                            "BELONGS_TO",
                            "REFINES",
                            "TARGETS",
                            "EXPLAINS",
                            "VERIFIES",
                        ];
                        const NON_FILIATION_GUARDED: [&str; 6] = [
                            "INHERITS_FROM",
                            "USES",
                            "USED_BY",
                            "EXTENDS",
                            "COMPLIES_WITH",
                            "ORIGINATES",
                        ];
                        let cycle_set: Option<Vec<&str>> = if FILIATION.contains(&relation_type) {
                            Some(FILIATION.to_vec())
                        } else if NON_FILIATION_GUARDED.contains(&relation_type) {
                            Some(vec![relation_type])
                        } else {
                            None
                        };
                        if let Some(set) = cycle_set {
                            let in_list = set
                                .iter()
                                .map(|r| format!("'{}'", r))
                                .collect::<Vec<_>>()
                                .join(",");
                            let cycle_query = format!(
                                "WITH RECURSIVE ancestors(reachable) AS (SELECT target_id FROM soll.Edge WHERE source_id = ? AND relation_type IN ({inl}) UNION SELECT e.target_id FROM soll.Edge e JOIN ancestors a ON e.source_id = a.reachable WHERE e.relation_type IN ({inl})) SELECT COUNT(*) FROM ancestors WHERE reachable = ?",
                                inl = in_list
                            );
                            let cycle_hit = self
                                .graph_store
                                .query_count_param(&cycle_query, &json!([tgt, src]))
                                .unwrap_or(0);
                            if cycle_hit > 0 {
                                return Some(json!({
                                    "content": [{
                                        "type": "text",
                                        "text": format!(
                                            "Link `{}` -{}-> `{}` would create a cycle (target already reaches source via the same relation family).",
                                            src, relation_type, tgt
                                        )
                                    }],
                                    "isError": true,
                                    "data": {
                                        "status": "input_invalid",
                                        "operator_guidance": {
                                            "problem_class": "cycle_detected",
                                            "follow_up_tools": ["soll_acyclic_audit", "soll_query_context"],
                                            "confidence": "high",
                                        },
                                        "parameter_repair": {
                                            "tool": "soll_manager",
                                            "category": "cycle_detected",
                                            "invalid_field": "data.relation_type",
                                            "source_id": src,
                                            "target_id": tgt,
                                            "relation_type": relation_type,
                                            "hint": "guarded structural relations (filiation + INHERITS_FROM/USES/USED_BY/EXTENDS/COMPLIES_WITH/ORIGINATES) must stay acyclic ; this link would close a cycle. Use SUPERSEDES if you intended a redirect, or restructure the chain.",
                                            "follow_up_tools": ["soll_acyclic_audit"],
                                        },
                                        "canonical_source": "DEC-AXO-098",
                                    },
                                }));
                            }
                        }

                        // MIL-AXO-020 slice 4 — SUPERSEDES auto-flip. Same
                        // type endpoints, target not already retired, edge +
                        // status updates land in one CTE so neither survives.
                        if relation_type == "SUPERSEDES" {
                            let mut src_type = String::new();
                            let mut tgt_type = String::new();
                            let mut tgt_status = String::new();
                            let raw = self
                                .graph_store
                                .query_json_param(
                                    "SELECT id, type, status FROM soll.Node WHERE id IN (?, ?)",
                                    &json!([src, tgt]),
                                )
                                .unwrap_or_else(|_| "[]".to_string());
                            let rows: Vec<Vec<serde_json::Value>> =
                                serde_json::from_str(&raw).unwrap_or_default();
                            for row in &rows {
                                if row.len() < 3 {
                                    continue;
                                }
                                let id = row[0].as_str().unwrap_or("");
                                let ty = row[1].as_str().unwrap_or("").to_string();
                                let st = row[2].as_str().unwrap_or("").to_string();
                                if id == src {
                                    src_type = ty;
                                } else if id == tgt {
                                    tgt_type = ty;
                                    tgt_status = st;
                                }
                            }
                            if src_type != tgt_type || src_type.is_empty() {
                                return Some(json!({
                                    "content": [{
                                        "type": "text",
                                        "text": format!(
                                            "SUPERSEDES requires same-type endpoints (got source={} target={}).",
                                            src_type, tgt_type
                                        )
                                    }],
                                    "isError": true,
                                    "data": {
                                        "status": "input_invalid",
                                        "operator_guidance": {
                                            "problem_class": "supersedes_type_mismatch",
                                            "follow_up_tools": ["soll_query_context"],
                                            "confidence": "high",
                                        },
                                        "parameter_repair": {
                                            "tool": "soll_manager",
                                            "category": "supersedes_type_mismatch",
                                            "source_id": src,
                                            "target_id": tgt,
                                            "source_type": src_type,
                                            "target_type": tgt_type,
                                            "hint": "SUPERSEDES is a same-type retirement marker (DEC→DEC, CPT→CPT, GUI→GUI). Cross-type retirement is not modelled.",
                                            "follow_up_tools": ["soll_query_context"],
                                        },
                                        "canonical_source": "MIL-AXO-020",
                                    },
                                }));
                            }
                            if tgt_status == "superseded" {
                                return Some(json!({
                                    "content": [{
                                        "type": "text",
                                        "text": format!(
                                            "SUPERSEDES target `{}` is already retired (status=superseded).",
                                            tgt
                                        )
                                    }],
                                    "isError": true,
                                    "data": {
                                        "status": "input_invalid",
                                        "operator_guidance": {
                                            "problem_class": "supersedes_target_already_retired",
                                            "follow_up_tools": ["soll_query_context"],
                                            "confidence": "high",
                                        },
                                        "parameter_repair": {
                                            "tool": "soll_manager",
                                            "category": "supersedes_target_already_retired",
                                            "target_id": tgt,
                                            "target_status": "superseded",
                                            "hint": "find the active replacement via soll_query_context or sql SELECT id FROM soll.Edge WHERE source_id = '<replacement>' AND target_id = '<tgt>' AND relation_type = 'SUPERSEDES'",
                                            "follow_up_tools": ["soll_query_context"],
                                        },
                                        "canonical_source": "MIL-AXO-020",
                                    },
                                }));
                            }
                            // Single-statement edge + status flips.
                            let target_project = project_code_from_canonical_entity_id(src)
                                .or_else(|| project_code_from_canonical_entity_id(tgt))
                                .unwrap_or_default();
                            let cte = "WITH inserted AS (INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES (?, ?, 'SUPERSEDES', ?) ON CONFLICT (source_id, target_id, relation_type) DO NOTHING RETURNING source_id), src_flip AS (UPDATE soll.Node SET status = 'current' WHERE id = ? RETURNING id) UPDATE soll.Node SET status = 'superseded' WHERE id = ?";
                            let exec = self
                                .graph_store
                                .execute_param(cte, &json!([src, tgt, target_project, src, tgt]));
                            return match exec {
                                Ok(()) => Some(json!({
                                    "content": [{
                                        "type": "text",
                                        "text": format!(
                                            "SUPERSEDES applied: `{}` retires `{}` (status flipped).",
                                            src, tgt
                                        )
                                    }],
                                    "data": {
                                        "status": "ok",
                                        "edge": {
                                            "source_id": src,
                                            "target_id": tgt,
                                            "relation_type": "SUPERSEDES"
                                        },
                                        "source_status_after": "current",
                                        "target_status_after": "superseded"
                                    }
                                })),
                                Err(e) => Some(json!({
                                    "content": [{
                                        "type": "text",
                                        "text": format!("SUPERSEDES failed: {}", e)
                                    }],
                                    "isError": true,
                                    "data": {
                                        "status": "internal_error",
                                        "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                                    }
                                })),
                            };
                        }

                        match self.insert_validated_relation(relation_type, src, tgt, policy) {
                            Ok(inserted) => {
                                let mut payload = json!({
                                    "content": [{ "type": "text", "text": if inserted {
                                        format!("Link created: `{}` -> `{}` (via {})", src, tgt, rel_table)
                                    } else {
                                        format!("Link already present: `{}` -> `{}` (via {})", src, tgt, rel_table)
                                    }}],
                                    "data": {
                                        "project_code": project_code.as_deref().unwrap_or(""),
                                    }
                                });
                                if inserted {
                                    if let (Some(before), Some(code), Ok(after)) = (
                                        before_snapshot.as_ref(),
                                        project_code.as_deref(),
                                        project_code
                                            .as_deref()
                                            .ok_or_else(|| anyhow!("missing project"))
                                            .and_then(|value| {
                                                self.soll_completeness_snapshot(Some(value))
                                            }),
                                    ) {
                                        let _ = code;
                                        payload["data"]["mutation_feedback"] =
                                            self.mutation_feedback_payload(
                                                before,
                                                &after,
                                                vec![json!({
                                                    "id": format!("{}:{}:{}", src, relation_type, tgt),
                                                    "change_kind": "edge_created",
                                                    "source_id": src,
                                                    "target_id": tgt,
                                                    "relation_type": relation_type
                                                })],
                                                json!({
                                                    "nodes_created": 0,
                                                    "nodes_updated": 0,
                                                    "edges_created": 1
                                                }),
                                            );
                                    }
                                }
                                Some(payload)
                            }
                            Err(e) => {
                                let mut data =
                                    self.relation_guidance_for_link(src, tgt, explicit_rel);
                                if let Some(obj) = data.as_object_mut() {
                                    obj.insert("status".to_string(), json!("input_invalid"));
                                    obj.insert("parameter_repair".to_string(), json!({
                                        "invalid_field": "relation_type",
                                        "supplied_source_id": src,
                                        "supplied_target_id": tgt,
                                        "supplied_relation_type": explicit_rel,
                                        "follow_up_tools": ["soll_relation_schema", "sql"],
                                        "hint": "the link insert failed; verify both endpoints exist via `sql SELECT id FROM soll.Node WHERE id IN ('<src>','<tgt>')` and the relation_type is allowed via `soll_relation_schema`"
                                    }));
                                    obj.insert(
                                        "diagnostic_excerpt".to_string(),
                                        json!(e.to_string().chars().take(240).collect::<String>()),
                                    );
                                }
                                Some(json!({
                                    "content": [{ "type": "text", "text": Self::sanitized_link_error_text(&e) }],
                                    "isError": true,
                                    "data": data
                                }))
                            }
                        }
                    }
                    Err(e) => Some(json!({
                        "content": [{ "type": "text", "text": Self::sanitized_link_error_text(&e) }],
                        "isError": true,
                        "data": self.relation_guidance_for_link(src, tgt, explicit_rel)
                    })),
                }
            }
            // REQ-AXO-91592 — symmetric counterpart of action=link. Removes
            // a single SOLL edge (one row in soll.Edge) and records the
            // operation in soll.Revision + soll.RevisionChange for audit.
            // Required: data.source_id, data.target_id, data.relation_type.
            // Optional: data.force (bool, default false) — required for the
            // canonical EPITOMIZES Pillar→Vision structural edge.
            "unlink" => {
                let src = match data.get("source_id").and_then(|v| v.as_str()) {
                    Some(v) if !v.trim().is_empty() => v.trim().to_string(),
                    _ => {
                        return Some(json!({
                            "content": [{
                                "type": "text",
                                "text": "soll_manager(unlink): required `data.source_id` is missing."
                            }],
                            "isError": true,
                            "data": {
                                "status": "input_invalid",
                                "parameter_repair": {
                                    "tool": "soll_manager",
                                    "category": "required_field_missing",
                                    "invalid_field": "data.source_id",
                                    "hint": "supply data.source_id (canonical SOLL id, e.g. GUI-AXO-1005)",
                                }
                            }
                        }));
                    }
                };
                let tgt = match data.get("target_id").and_then(|v| v.as_str()) {
                    Some(v) if !v.trim().is_empty() => v.trim().to_string(),
                    _ => {
                        return Some(json!({
                            "content": [{
                                "type": "text",
                                "text": "soll_manager(unlink): required `data.target_id` is missing."
                            }],
                            "isError": true,
                            "data": {
                                "status": "input_invalid",
                                "parameter_repair": {
                                    "tool": "soll_manager",
                                    "category": "required_field_missing",
                                    "invalid_field": "data.target_id",
                                    "hint": "supply data.target_id (canonical SOLL id)",
                                }
                            }
                        }));
                    }
                };
                let relation_type = match data.get("relation_type").and_then(|v| v.as_str()) {
                    Some(v) if !v.trim().is_empty() => v.trim().to_string(),
                    _ => {
                        return Some(json!({
                            "content": [{
                                "type": "text",
                                "text": "soll_manager(unlink): required `data.relation_type` is missing — unlink does not infer; identify the exact edge to remove."
                            }],
                            "isError": true,
                            "data": {
                                "status": "input_invalid",
                                "parameter_repair": {
                                    "tool": "soll_manager",
                                    "category": "required_field_missing",
                                    "invalid_field": "data.relation_type",
                                    "hint": "supply data.relation_type (e.g. INHERITS_FROM, BELONGS_TO). Use `sql SELECT relation_type FROM soll.Edge WHERE source_id='<src>' AND target_id='<tgt>'` to discover the existing label.",
                                }
                            }
                        }));
                    }
                };
                let force = data.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

                // Protection — EPITOMIZES is the canonical Pillar→Vision
                // structural edge that EPITOMIZES the Vision; removing it
                // silently would orphan a Pillar from its anchor. Require
                // explicit force=true.
                const PROTECTED_RELATIONS: [&str; 1] = ["EPITOMIZES"];
                if PROTECTED_RELATIONS.contains(&relation_type.as_str()) && !force {
                    return Some(json!({
                        "content": [{
                            "type": "text",
                            "text": format!(
                                "soll_manager(unlink): `{}` is a protected relation type; supply data.force=true to confirm removal.",
                                relation_type
                            )
                        }],
                        "isError": true,
                        "data": {
                            "status": "input_invalid",
                            "parameter_repair": {
                                "tool": "soll_manager",
                                "category": "protected_edge",
                                "invalid_field": "data.force",
                                "source_id": src,
                                "target_id": tgt,
                                "relation_type": relation_type,
                                "hint": "EPITOMIZES anchors Pillars to the Vision (PIL→VIS). Set data.force=true if you really intend to break that anchor.",
                            }
                        }
                    }));
                }

                let match_count = self
                    .graph_store
                    .query_count_param(
                        "SELECT count(*) FROM soll.Edge WHERE source_id=? AND target_id=? AND relation_type=?",
                        &json!([src, tgt, relation_type]),
                    )
                    .unwrap_or(0);

                if match_count == 0 {
                    return Some(json!({
                        "content": [{
                            "type": "text",
                            "text": format!(
                                "soll_manager(unlink): no edge matches `{}` -[{}]-> `{}`.",
                                src, relation_type, tgt
                            )
                        }],
                        "isError": true,
                        "data": {
                            "status": "edge_not_found",
                            "parameter_repair": {
                                "tool": "soll_manager",
                                "category": "edge_not_found",
                                "source_id": src,
                                "target_id": tgt,
                                "relation_type": relation_type,
                                "hint": "verify the edge exists via `sql SELECT * FROM soll.Edge WHERE source_id='<src>' AND target_id='<tgt>'`",
                            }
                        }
                    }));
                }

                // Audit + delete. soll.Edge has a (source_id, target_id,
                // relation_type) PK so match_count is always 0 or 1 in
                // practice ; we still parameterise on count for forward
                // compat with any seed buggy enough to bypass the PK.
                let project_code = project_code_from_canonical_entity_id(&src)
                    .or_else(|| project_code_from_canonical_entity_id(&tgt))
                    .unwrap_or_else(|| "AXO".to_string());
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                let revision_id = format!("unlink-{}-{}", now_ms, src);
                let entity_id = format!("{}:{}:{}", src, relation_type, tgt);
                let before_json = json!({
                    "source_id": src,
                    "target_id": tgt,
                    "relation_type": relation_type,
                });
                let summary = format!("unlink: {} -[{}]-> {}", src, relation_type, tgt);

                // Two separate INSERTs + one DELETE — kept as three calls
                // because the existing graph_store API does not expose a
                // multi-statement transaction handle. The PG plugin runs
                // each call in implicit-commit mode ; the failure window is
                // tiny and the audit row would surface a half-applied
                // unlink (revision present, edge still there) which the
                // operator can re-execute idempotently. A future REQ may
                // wrap this in an explicit BEGIN/COMMIT.
                if let Err(e) = self.graph_store.execute_param(
                    "INSERT INTO soll.Revision (revision_id, project_code, author, source, summary, status, created_at, committed_at) \
                     VALUES (?, ?, 'soll_manager', 'mcp.unlink', ?, 'committed', ?, ?)",
                    &json!([
                        revision_id,
                        project_code,
                        summary,
                        now_ms,
                        now_ms,
                    ]),
                ) {
                    return Some(json!({
                        "content": [{
                            "type": "text",
                            "text": format!("soll_manager(unlink): revision insert failed: {}", e)
                        }],
                        "isError": true,
                        "data": {
                            "status": "internal_error",
                            "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>(),
                        }
                    }));
                }
                if let Err(e) = self.graph_store.execute_param(
                    "INSERT INTO soll.RevisionChange (revision_id, entity_type, entity_id, project_code, action, before_json, after_json, created_at) \
                     VALUES (?, 'edge', ?, ?, 'unlink', ?::jsonb, NULL, ?)",
                    &json!([
                        revision_id,
                        entity_id,
                        project_code,
                        before_json.to_string(),
                        now_ms,
                    ]),
                ) {
                    return Some(json!({
                        "content": [{
                            "type": "text",
                            "text": format!("soll_manager(unlink): revision_change insert failed: {}", e)
                        }],
                        "isError": true,
                        "data": {
                            "status": "internal_error",
                            "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>(),
                        }
                    }));
                }
                match self.graph_store.execute_param(
                    "DELETE FROM soll.Edge WHERE source_id=? AND target_id=? AND relation_type=?",
                    &json!([src, tgt, relation_type]),
                ) {
                    Ok(()) => Some(json!({
                        "content": [{
                            "type": "text",
                            "text": format!(
                                "Edge removed: `{}` -[{}]-> `{}` (revision {})",
                                src, relation_type, tgt, revision_id
                            )
                        }],
                        "data": {
                            "status": "ok",
                            "project_code": project_code,
                            "edges_removed": match_count,
                            "revision_id": revision_id,
                            "source_id": src,
                            "target_id": tgt,
                            "relation_type": relation_type,
                        }
                    })),
                    Err(e) => Some(json!({
                        "content": [{
                            "type": "text",
                            "text": format!("soll_manager(unlink): delete failed: {}", e)
                        }],
                        "isError": true,
                        "data": {
                            "status": "internal_error",
                            "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>(),
                            "audit_warning": format!(
                                "revision row {} was inserted before the DELETE failed ; re-run the unlink to converge",
                                revision_id
                            ),
                        }
                    })),
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_metadata_routed_fields, id_exists_envelope, resolve_create_status};
    use serde_json::json;

    // REQ-AXO-901955 — metadata-routed fields are honoured from a single
    // source for create AND update; top-level `tags` (previously dropped
    // unless nested under `metadata`) now lands in meta.

    #[test]
    fn metadata_routed_includes_tags_priority_and_skips_unknown() {
        let data = json!({
            "tags": ["a", "b"],
            "priority": "P1",
            "rationale": "because",
            "title": "ignored-canonical-column",
            "not_a_routed_field": "x"
        });
        let mut meta = json!({});
        apply_metadata_routed_fields(&data, &mut meta);
        assert_eq!(meta["tags"], json!(["a", "b"]));
        assert_eq!(meta["priority"], json!("P1"));
        assert_eq!(meta["rationale"], json!("because"));
        // canonical columns + unknown keys must NOT be folded into metadata
        assert!(meta.get("title").is_none());
        assert!(meta.get("not_a_routed_field").is_none());
    }

    #[test]
    fn metadata_routed_preserves_existing_meta_keys() {
        let data = json!({ "tags": ["new"] });
        let mut meta = json!({ "phase": "post-code", "tags": ["old"] });
        apply_metadata_routed_fields(&data, &mut meta);
        // pre-existing unrelated key survives; supplied field overrides
        assert_eq!(meta["phase"], json!("post-code"));
        assert_eq!(meta["tags"], json!(["new"]));
    }

    // REQ-AXO-323 Fault 4 — default status resolution.

    #[test]
    fn resolve_create_status_defaults_to_planned_when_omitted() {
        assert_eq!(resolve_create_status(None, "requirement", None), "planned");
    }

    #[test]
    fn resolve_create_status_defaults_to_planned_when_empty_string() {
        assert_eq!(resolve_create_status(Some(""), "concept", None), "planned");
    }

    #[test]
    fn resolve_create_status_returns_supplied_canonical_value() {
        assert_eq!(
            resolve_create_status(Some("current"), "requirement", None),
            "current"
        );
        assert_eq!(
            resolve_create_status(Some("delivered"), "decision", None),
            "delivered"
        );
    }

    #[test]
    fn resolve_create_status_preserves_non_canonical_for_downstream_validator() {
        // `in_progress` is rejected later by the canonical-vocabulary
        // validator (DEC-PRO-100). The defaulter must NOT silently rewrite
        // it to `planned` ; the LLM contract relies on the validator
        // surfacing the structured `parameter_repair` envelope.
        assert_eq!(
            resolve_create_status(Some("in_progress"), "requirement", None),
            "in_progress"
        );
    }

    #[test]
    fn resolve_create_status_validation_prefers_result_field() {
        assert_eq!(
            resolve_create_status(None, "validation", Some("delivered")),
            "delivered"
        );
    }

    #[test]
    fn resolve_create_status_validation_falls_back_to_supplied_when_no_result() {
        assert_eq!(
            resolve_create_status(Some("current"), "validation", None),
            "current"
        );
    }

    #[test]
    fn resolve_create_status_validation_defaults_when_all_empty() {
        assert_eq!(resolve_create_status(None, "validation", None), "planned");
        assert_eq!(
            resolve_create_status(Some(""), "validation", Some("")),
            "planned"
        );
    }

    #[test]
    fn resolve_create_status_validation_empty_result_falls_through_to_supplied() {
        // unwrap_or only kicks in when the Option is None ; an explicit
        // Some("") returns "" and the defaulter then kicks the empty
        // candidate to "planned". This documents that contract.
        assert_eq!(
            resolve_create_status(Some("current"), "validation", Some("")),
            "planned"
        );
    }

    #[test]
    fn id_exists_envelope_returns_canonical_error_shape() {
        let env = id_exists_envelope("REQ-AXO-001", "Requirement");
        assert_eq!(env["isError"].as_bool(), Some(true));
        assert_eq!(env["data"]["status"].as_str(), Some("id_exists"));
        assert_eq!(env["data"]["id"].as_str(), Some("REQ-AXO-001"));
        assert_eq!(env["data"]["entity_type"].as_str(), Some("Requirement"));
        assert_eq!(
            env["data"]["parameter_repair"]["category"].as_str(),
            Some("id_exists")
        );
        assert_eq!(
            env["data"]["parameter_repair"]["invalid_field"].as_str(),
            Some("data.id")
        );
        let hint = env["data"]["parameter_repair"]["hint"]
            .as_str()
            .unwrap_or_default();
        assert!(
            hint.contains("action=update"),
            "hint must steer caller toward action=update: {hint}"
        );
    }

    #[test]
    fn id_exists_envelope_steers_text_message_toward_update() {
        let env = id_exists_envelope("DEC-AXO-099", "Decision");
        let text = env["content"][0]["text"].as_str().unwrap_or_default();
        assert!(text.contains("DEC-AXO-099"));
        assert!(text.contains("action=update"));
        assert!(text.contains("Cannot create"));
    }
}
