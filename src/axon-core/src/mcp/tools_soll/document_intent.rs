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

/// REQ-AXO-901615 — relation par defaut pour le parent de repli `entite -> Pillar`.
///
/// REQ-AXO-902470 (GUI-PRO-013, DRY) — cette fonction RECOPIAIT a la main les
/// defauts de `relation_policy_for_pair(_, "PIL")` ; son commentaire disait
/// « Mirrors ». Une copie manuelle d'une table de politique diverge : elle codait
/// `DEC -> PIL` en `BELONGS_TO` alors que cette paire est ABSENTE de la matrice,
/// donc elle emettait une relation que l'ecriture refuse.
///
/// Elle INTERROGE desormais la matrice. Le repli `BELONGS_TO` ne subsiste que
/// pour les paires que la matrice ignore (`DEC -> PIL`) : `soll_manager` rendra
/// alors un `parameter_repair` precis pointant vers `soll_relation_schema`, qui
/// est le chemin de recuperation documente (REQ-AXO-901615, critere 3). Le repli
/// est un aveu d'absence, pas une seconde source de verite.
fn default_relation_for_entity_to_pillar(entity_type: &str) -> &'static str {
    let prefix = match entity_type {
        "requirement" => "REQ",
        "concept" => "CPT",
        "guideline" => "GUI",
        "decision" => "DEC",
        _ => return "BELONGS_TO",
    };
    relation_policy_for_pair(prefix, "PIL")
        .and_then(|policy| policy.default)
        .unwrap_or("BELONGS_TO")
}

/// REQ-AXO-902081 — target-aware default relation. A Decision pointed at a
/// Requirement resolves it (DEC→REQ SOLVES, the canonical pair); everything else
/// falls back to the entity→Pillar default. Keeps an explicit `attach_to` from
/// emitting a forbidden DEC→PIL `BELONGS_TO`.
fn default_relation_for_target(entity_type: &str, target_id: &str) -> &'static str {
    if entity_type == "decision" && target_id.starts_with("REQ-") {
        return "SOLVES";
    }
    default_relation_for_entity_to_pillar(entity_type)
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
                // REQ-AXO-902081 — target-aware default so an explicit Decision→REQ
                // gets SOLVES (the canonical pair), not BELONGS_TO (DEC→PIL only).
                let rel = explicit_relation_type.unwrap_or_else(|| {
                    default_relation_for_target(entity_type, &target).to_string()
                });
                (target, rel, "explicit_argument")
            }
            // REQ-AXO-902081 — type-aware parent inference: a Decision attaches to
            // the Requirement it resolves (DEC→REQ SOLVES), NOT the project Pillar
            // (DEC→PIL is a forbidden relation → the old default raised
            // forbidden_relation_for_type). Structural types still default to the
            // Pillar via BELONGS_TO.
            None => match self.infer_anchor_for_entity(&project_code, entity_type) {
                Some((anchor_id, rel)) => (
                    anchor_id,
                    explicit_relation_type.unwrap_or_else(|| rel.to_string()),
                    "auto_inferred_anchor",
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

    /// REQ-AXO-902081 — the anchor Requirement a Decision resolves (DEC→REQ SOLVES).
    /// The most recent open Requirement in the project is the inferred default; the
    /// LLM can override with an explicit `attach_to`.
    fn default_decision_anchor(&self, project_code: &str) -> Option<String> {
        let escaped = escape_sql(project_code);
        let query = format!(
            "SELECT id FROM soll.Node \
             WHERE project_code = '{escaped}' \
               AND type = 'Requirement' \
               AND status IN ('planned', 'current') \
             ORDER BY id DESC \
             LIMIT 1"
        );
        self.query_single_column(&query)
            .ok()
            .and_then(|rows| rows.into_iter().next())
            .filter(|s| !s.trim().is_empty())
    }

    /// REQ-AXO-902081 — type-aware parent inference so the default never emits a
    /// forbidden pair. A Decision attaches to its anchor Requirement via SOLVES
    /// (DEC→PIL has no policy); structural types attach to the project Pillar via
    /// their canonical relation. Returns None when no anchor exists → the caller
    /// surfaces the suggestion-bearing error instead of a forbidden relation.
    fn infer_anchor_for_entity(
        &self,
        project_code: &str,
        entity_type: &str,
    ) -> Option<(String, &'static str)> {
        match entity_type {
            "decision" => self
                .default_decision_anchor(project_code)
                .map(|req| (req, "SOLVES")),
            _ => self
                .default_project_pillar(project_code)
                .map(|pillar| (pillar, default_relation_for_entity_to_pillar(entity_type))),
        }
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
    use super::{classify_intent, default_relation_for_target};

    #[test]
    fn decision_to_requirement_defaults_to_solves_not_belongs_to() {
        // REQ-AXO-902081 — DEC→REQ is SOLVES; DEC→PIL has no policy (would be
        // rejected), so a Decision pointed at a Requirement must not default to
        // BELONGS_TO.
        assert_eq!(default_relation_for_target("decision", "REQ-AXO-902081"), "SOLVES");
        // A Decision pointed at a Pillar keeps the (questionable) pillar default;
        // structural types are always BELONGS_TO regardless of target.
        assert_eq!(default_relation_for_target("decision", "PIL-AXO-001"), "BELONGS_TO");
        assert_eq!(default_relation_for_target("requirement", "REQ-AXO-1"), "BELONGS_TO");
        assert_eq!(default_relation_for_target("concept", "PIL-AXO-001"), "BELONGS_TO");
    }

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
    /// REQ-AXO-902470 — la garde lit la MATRICE, elle ne reecrit pas la table
    /// attendue en dur. L'ancienne version assertait `== "BELONGS_TO"` : elle
    /// aurait valide la fonction meme apres une divergence, puisqu'elle
    /// comparait la copie a une TROISIEME copie ecrite dans le test.
    #[test]
    fn default_relation_to_pillar_follows_the_relation_matrix() {
        use super::default_relation_for_entity_to_pillar;
        use crate::mcp::tools_soll::relation_policy::relation_policy_for_pair;

        for (entity_type, prefix) in [
            ("requirement", "REQ"),
            ("concept", "CPT"),
            ("guideline", "GUI"),
            ("decision", "DEC"),
        ] {
            let from_matrix = relation_policy_for_pair(prefix, "PIL").and_then(|p| p.default);
            let produced = default_relation_for_entity_to_pillar(entity_type);
            match from_matrix {
                Some(expected) => assert_eq!(
                    produced, expected,
                    "{entity_type} ({prefix} -> PIL) : la fonction diverge de la matrice"
                ),
                // La matrice ignore la paire (cas mesure : DEC -> PIL). Le repli
                // est alors legitime, et c'est `soll_manager` qui tranchera avec
                // un parameter_repair.
                None => assert_eq!(
                    produced, "BELONGS_TO",
                    "{entity_type} ({prefix} -> PIL) : paire absente de la matrice, \
                     le repli documente est BELONGS_TO"
                ),
            }
        }
    }

    /// CONTROLE POSITIF — au moins UNE des paires doit exister dans la matrice.
    /// Sans lui, une matrice vide ferait passer le test ci-dessus par la branche
    /// de repli sur les quatre types, sans rien eprouver.
    #[test]
    fn at_least_one_classifier_pair_is_actually_in_the_matrix() {
        use crate::mcp::tools_soll::relation_policy::relation_policy_for_pair;
        let known = ["REQ", "CPT", "GUI", "DEC"]
            .iter()
            .filter(|p| relation_policy_for_pair(p, "PIL").is_some())
            .count();
        assert!(
            known > 0,
            "aucune paire entite -> PIL dans la matrice : la garde ci-dessus ne \
             testerait plus que son propre repli"
        );
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
