use super::*;

// REQ-AXO-141 — Universal entry point for "documente" / "document this" /
// "save observation" workflows. The classifier maps free-form prose to the
// four canonical SOLL entity types using simple keyword heuristics. The
// classifier is intentionally conservative: when no keyword fires, it
// falls back to `concept` because that is the lowest-risk type (concepts
// have no acceptance criteria or status gating).

const REQUIREMENT_KEYWORDS: &[&str] = &[
    "problem",
    "gap",
    "friction",
    "broken",
    "missing",
    "must surface",
    "should surface",
    "needs to",
    "fails",
    "doesn't",
    "cannot",
    "regression",
    "bug",
    "fix needed",
    "improve",
];

const DECISION_KEYWORDS: &[&str] = &[
    "we'll",
    "we will",
    "decided",
    "picks ",
    "picked ",
    "chosen",
    "choose between",
    "going with",
    "tradeoff",
    "we pick",
    "accepted",
];

const GUIDELINE_KEYWORDS: &[&str] = &[
    "rule:",
    "always ",
    "never ",
    "convention",
    "policy",
    "style guide",
    "must always",
    "do not ",
    "guideline:",
    "must:",
];

const CONCEPT_KEYWORDS: &[&str] = &[
    "how it works",
    "mental model",
    "in essence",
    "the idea is",
    "the concept",
    "framework",
    "the loop",
];

/// REQ-AXO-901615 — Default relation type for `entity → Pillar` fallback parent.
/// Mirrors `relation_policy_for_pair(_, "PIL")` defaults: REQ/CPT/GUI/SKI all
/// use BELONGS_TO; Decision/Milestone/Validation default to attaching against
/// a Requirement, but absent any anchor we still pick the project pillar with
/// `BELONGS_TO` because the canonical policy table accepts that as a fallback
/// for the methodology entities used by document_intent (REQ/DEC/CPT/GUI). The
/// classifier only ever produces these four types, so this is well-defined.
fn default_relation_for_entity_to_pillar(entity_type: &str) -> &'static str {
    match entity_type {
        // CPT→PIL = BELONGS_TO (REQ-AXO-115)
        // REQ→PIL = BELONGS_TO
        // GUI→PIL = BELONGS_TO (REQ-AXO-274)
        // DEC→PIL has no direct policy; we fall through to BELONGS_TO which
        // soll_manager will accept iff the (DEC, PIL) pair is registered.
        // When the policy rejects it the LLM gets a precise parameter_repair
        // pointing to soll_relation_schema, which is the documented recovery
        // path (see REQ-AXO-901615 acceptance criterion 3).
        "requirement" | "concept" | "guideline" | "decision" => "BELONGS_TO",
        _ => "BELONGS_TO",
    }
}

fn classify_intent(intent: &str, body: &str) -> (&'static str, &'static str) {
    let haystack = format!("{} {}", intent, body).to_ascii_lowercase();
    if REQUIREMENT_KEYWORDS.iter().any(|kw| haystack.contains(kw)) {
        ("requirement", "matched_requirement_keyword")
    } else if GUIDELINE_KEYWORDS.iter().any(|kw| haystack.contains(kw)) {
        ("guideline", "matched_guideline_keyword")
    } else if DECISION_KEYWORDS.iter().any(|kw| haystack.contains(kw)) {
        ("decision", "matched_decision_keyword")
    } else if CONCEPT_KEYWORDS.iter().any(|kw| haystack.contains(kw)) {
        ("concept", "matched_concept_keyword")
    } else {
        ("concept", "no_keyword_match_default_concept")
    }
}

impl McpServer {
    pub(crate) fn axon_document_intent(&self, args: &Value) -> Option<Value> {
        let intent = args.get("intent")?.as_str()?;
        let body = args.get("body")?.as_str()?;
        if intent.trim().is_empty() {
            return Some(json!({
                "content": [{"type":"text","text":"document_intent: `intent` is empty"}],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                    "parameter_repair": {
                        "invalid_field": "intent",
                        "hint": "supply a one-line summary in `intent` (used as the SOLL title)",
                        "follow_up_tools": ["help"]
                    }
                }
            }));
        }
        if body.trim().is_empty() {
            return Some(json!({
                "content": [{"type":"text","text":"document_intent: `body` is empty"}],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                    "parameter_repair": {
                        "invalid_field": "body",
                        "hint": "supply the full description / rationale in `body`",
                        "follow_up_tools": ["help"]
                    }
                }
            }));
        }

        let suggest_type = args.get("suggest_type").and_then(|v| v.as_str());
        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let explicit_project = args.get("project_code").and_then(|v| v.as_str());
        let auto_project = if explicit_project.is_none() {
            self.auto_resolve_project_code_str()
        } else {
            None
        };
        let project_code = explicit_project
            .map(str::to_string)
            .or(auto_project)
            .unwrap_or_else(|| "AXO".to_string());

        let (entity_type, classifier_reason) = match suggest_type {
            Some(t) if matches!(t, "requirement" | "decision" | "concept" | "guideline") => {
                (t, "explicit_suggest_type")
            }
            Some(other) => {
                return Some(json!({
                    "content": [{"type":"text","text": format!("document_intent: invalid `suggest_type` `{}`", other)}],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "parameter_repair": {
                            "invalid_field": "suggest_type",
                            "supplied_value": other,
                            "accepted_values": ["requirement", "decision", "concept", "guideline"],
                            "hint": "either omit `suggest_type` (server classifies) or pass one of the accepted values",
                            "follow_up_tools": ["help"]
                        }
                    }
                }));
            }
            None => classify_intent(intent, body),
        };

        // REQ-AXO-901615 — accept optional `attach_to` + `relation_type`.
        // If the operator passes them, forward verbatim. If absent, auto-infer
        // a fallback parent (the lowest-id `current` Pillar in the project) so
        // the documented "universal entry point" contract (CPT-AXO-019)
        // actually delivers when no anchor is in working memory. The LLM can
        // override later via `soll_manager(action=link, ...)` once they know
        // the canonical anchor.
        let explicit_attach_to = args
            .get("attach_to")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string);
        let explicit_relation_type = args
            .get("relation_type")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_uppercase());

        let (attach_to, relation_type, attach_source) = match explicit_attach_to {
            Some(target) => {
                let rel = explicit_relation_type.unwrap_or_else(|| {
                    default_relation_for_entity_to_pillar(entity_type).to_string()
                });
                (target, rel, "explicit_argument")
            }
            None => match self.default_project_pillar(&project_code) {
                Some(pillar_id) => (
                    pillar_id,
                    explicit_relation_type.unwrap_or_else(|| {
                        default_relation_for_entity_to_pillar(entity_type).to_string()
                    }),
                    "auto_inferred_project_pillar",
                ),
                None => {
                    // Acceptance criterion #3 — return a clear error message
                    // listing the suggested anchors so the LLM can retry.
                    let suggestions = self.suggest_attach_to_candidates(&project_code);
                    return Some(json!({
                        "content": [{"type":"text","text": format!(
                            "document_intent could not infer a parent for project `{}` ; supply `attach_to=<canonical_id>` and optionally `relation_type`. Suggested candidates: {:?}",
                            project_code, suggestions
                        )}],
                        "isError": true,
                        "data": {
                            "status": "input_invalid",
                            "classification": {
                                "entity_type": entity_type,
                                "classifier_reason": classifier_reason
                            },
                            "parameter_repair": {
                                "invalid_field": "attach_to",
                                "hint": "no current Pillar found for inference ; pass attach_to=<canonical PIL/CPT id>",
                                "suggested_anchors": suggestions,
                                "follow_up_tools": ["soll_query_context", "soll_relation_schema"]
                            }
                        }
                    }));
                }
            },
        };

        // REQ-AXO-141 — delegate to soll_manager.create so canonical id
        // assignment, project_code validation, and Registry counters all
        // go through the canonical mutation path. The wrapper only
        // pre-classifies + post-processes the response shape.
        let create_args = json!({
            "action": "create",
            "entity": entity_type,
            "data": {
                "project_code": project_code,
                "title": intent,
                "description": body,
                "status": "planned",
                "attach_to": attach_to,
                "relation_type": relation_type,
                "metadata": {
                    "tags": tags,
                    "originator": "document_intent_mcp",
                    "classifier_reason": classifier_reason,
                    "attach_source": attach_source
                }
            }
        });

        let response = self.axon_soll_manager(&create_args)?;
        let inner_data = response.get("data").cloned().unwrap_or(Value::Null);
        let canonical_id = inner_data
            .get("created_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let success = !canonical_id.is_empty()
            && response.get("isError").and_then(|v| v.as_bool()) != Some(true);

        if !success {
            // Surface upstream failure with a parameter_repair pointer so
            // the LLM can recover via direct soll_manager call.
            let upstream_text = response
                .get("content")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("upstream soll_manager call failed");
            return Some(json!({
                "content": [{"type":"text","text": format!("document_intent: classification ok ({}), but SOLL create failed: {}", entity_type, upstream_text)}],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                    "classification": {
                        "entity_type": entity_type,
                        "classifier_reason": classifier_reason,
                    },
                    "upstream": inner_data,
                    "parameter_repair": {
                        "invalid_field": "data",
                        "follow_up_tools": ["soll_manager", "help"],
                        "hint": format!(
                            "retry directly via soll_manager(action=create, entity={}, data=...) after addressing the upstream error",
                            entity_type
                        )
                    }
                }
            }));
        }

        Some(json!({
            "content": [{"type":"text","text": format!(
                "document_intent: recorded {} `{}` as `{}` attached to `{}` via {} ({}, tags={:?}, attach_source={})",
                entity_type, intent, canonical_id, attach_to, relation_type, classifier_reason, tags, attach_source
            )}],
            "data": {
                "status": "ok",
                "canonical_id": canonical_id,
                "entity_type": entity_type,
                "classifier_reason": classifier_reason,
                "project_code": project_code,
                "tags": tags,
                "attach_to": attach_to,
                "relation_type": relation_type,
                "attach_source": attach_source,
                "follow_up_tools": ["soll_manager", "soll_attach_evidence"],
                "next_action": {
                    "tool": "soll_manager",
                    "kind": "link",
                    "when": "if_a_more_specific_anchor_is_known"
                },
                "hint": format!(
                    "node was attached to `{}` via {}. If a more specific parent (concept/requirement) is known, add a second edge via `soll_manager(action=link, source_id={}, target_id=<id>, relation_type=...)`. Use `soll_attach_evidence` once artifacts land.",
                    attach_to, relation_type, canonical_id
                ),
                "upstream": inner_data
            }
        }))
    }

    /// REQ-AXO-901615 — return the lowest-id `current` Pillar in the project,
    /// used as the inferred parent when document_intent is called without
    /// explicit `attach_to`. Returns None if no current pillar exists.
    fn default_project_pillar(&self, project_code: &str) -> Option<String> {
        let escaped = escape_sql(project_code);
        let query = format!(
            "SELECT id FROM soll.Node \
             WHERE project_code = '{escaped}' \
               AND type = 'Pillar' \
               AND status = 'current' \
             ORDER BY id ASC \
             LIMIT 1"
        );
        self.query_single_column(&query)
            .ok()
            .and_then(|rows| rows.into_iter().next())
            .filter(|s| !s.trim().is_empty())
    }

    /// REQ-AXO-901615 — list a handful of plausible anchors so the LLM can
    /// retry document_intent with a canonical attach_to when auto-inference
    /// failed (no current Pillar in the project).
    fn suggest_attach_to_candidates(&self, project_code: &str) -> Vec<String> {
        let escaped = escape_sql(project_code);
        let query = format!(
            "SELECT id FROM soll.Node \
             WHERE project_code = '{escaped}' \
               AND type IN ('Pillar', 'Concept', 'Requirement') \
               AND status IN ('current', 'planned') \
             ORDER BY type DESC, id ASC \
             LIMIT 8"
        );
        self.query_single_column(&query).unwrap_or_default()
    }
}

#[cfg(test)]
mod document_intent_classifier_tests {
    use super::classify_intent;

    #[test]
    fn classifies_requirement_when_body_describes_problem_or_gap() {
        let (kind, _) = classify_intent(
            "Indexer fails on empty file",
            "the watcher cannot index empty files because the validator rejects 0-byte content",
        );
        assert_eq!(kind, "requirement");
    }

    #[test]
    fn classifies_decision_when_body_describes_choice() {
        let (kind, _) = classify_intent(
            "Pick option A",
            "After review we will go with option A; tradeoff documented in DEC-AXO-064",
        );
        assert_eq!(kind, "decision");
    }

    #[test]
    fn classifies_guideline_when_body_describes_rule() {
        let (kind, _) = classify_intent(
            "TDD before implementation",
            "Always write the test first; convention enforced by GUI-PRO-001",
        );
        assert_eq!(kind, "guideline");
    }

    #[test]
    fn classifies_concept_when_no_keyword_fires() {
        let (kind, reason) = classify_intent(
            "Vector pipeline shape",
            "Embeddings flow from chunker to GPU subprocess to ChunkEmbedding table.",
        );
        assert_eq!(kind, "concept");
        assert_eq!(reason, "no_keyword_match_default_concept");
    }

    /// REQ-AXO-901615 — fallback relation table must produce BELONGS_TO for
    /// all four classifier outputs so document_intent without attach_to lands
    /// the node on the default project Pillar.
    #[test]
    fn default_relation_to_pillar_is_belongs_to_for_all_classifier_outputs() {
        use super::default_relation_for_entity_to_pillar;
        for entity_type in ["requirement", "decision", "concept", "guideline"] {
            assert_eq!(
                default_relation_for_entity_to_pillar(entity_type),
                "BELONGS_TO",
                "entity_type={entity_type}"
            );
        }
    }

    #[test]
    fn requirement_wins_over_concept_keyword_when_both_present() {
        // "framework" alone is concept; combined with "fix needed" the
        // requirement signal must dominate (problem-class keyword).
        let (kind, _) = classify_intent(
            "Framework gap",
            "the framework is broken — fix needed before next release",
        );
        assert_eq!(kind, "requirement");
    }
}
