use super::*;

impl McpServer {
    pub(crate) fn axon_soll_verify_requirements(&self, args: &Value) -> Option<Value> {
        self.axon_soll_verify_requirements_with_cached_coverage(args, None)
    }

    /// Memoized variant — same contract, but reuses a precomputed
    /// `RequirementCoverageSummary` when the caller (typically
    /// `axon_soll_work_plan`) has already paid the cost.
    pub(crate) fn axon_soll_verify_requirements_with_cached_coverage(
        &self,
        args: &Value,
        cached_coverage: Option<&RequirementCoverageSummary>,
    ) -> Option<Value> {
        // REQ-AXO-902467 — ne plus deviner le projet courant.
        let resolved_input = args
            .get("project_code")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| self.auto_resolve_project_code_str());
        let Some(project_code_input) = resolved_input.as_deref() else {
            return Some(crate::mcp::guidance::unresolved_project_error(
                "soll_verify_requirements",
                &self.known_project_codes_hint(),
            ));
        };
        // REQ-AXO-043 — wrong_project_scope contract via shared helper.
        // Previously `.ok()?` swallowed resolve_project_code errors and the
        // framework rendered a generic "Invalid arguments".
        let project_code =
            match self.resolve_project_code(project_code_input) {
                Ok(code) => code,
                Err(_) => {
                    return Some(self.wrong_project_scope_response(
                        project_code_input,
                        "soll_verify_requirements",
                    ));
                }
            };
        let owned_summary;
        let summary: &RequirementCoverageSummary = match cached_coverage {
            Some(c) => c,
            None => {
                owned_summary = self.requirement_coverage_summary(&project_code).ok()?;
                &owned_summary
            }
        };
        // Build compact text with top gaps for LLM actionability.
        let top_gaps: Vec<String> = summary
            .entries
            .iter()
            .filter(|e| e.state != "done")
            .take(5)
            .map(|e| {
                let dims: Vec<&str> = e.missing_dimensions.iter().map(String::as_str).collect();
                format!("  {} [{}]: missing {}", e.id, e.state, dims.join(", "))
            })
            .collect();
        let next_to_close = summary
            .entries
            .iter()
            .filter(|e| e.state == "partial")
            .min_by_key(|e| e.missing_dimensions.len())
            .map(|e| {
                format!(
                    "\nNext to close: {} (needs: {})",
                    e.id,
                    e.missing_dimensions.join(", ")
                )
            })
            .unwrap_or_default();
        // REQ-AXO-902337 piste 1 — name the broken file-evidence offenders in
        // the text surface (bounded) so an LLM sees WHICH references to purge
        // without dropping to raw SQL on soll.Traceability. Full list is
        // always in structuredContent.details[].broken_file_evidence_offenders.
        const BROKEN_OFFENDER_TEXT_CAP: usize = 15;
        let broken_lines: Vec<String> = summary
            .entries
            .iter()
            .flat_map(|e| {
                e.broken_file_evidence.iter().map(move |b| {
                    format!("  {} → {} (trc {})", e.id, b.artifact_ref, b.traceability_id)
                })
            })
            .collect();
        let broken_section = if broken_lines.is_empty() {
            String::new()
        } else {
            let total = broken_lines.len();
            let shown: Vec<String> = broken_lines
                .into_iter()
                .take(BROKEN_OFFENDER_TEXT_CAP)
                .collect();
            let footer = match total.saturating_sub(shown.len()) {
                0 => String::new(),
                more => format!(
                    "\n  (+{more} more — full list in structuredContent.details[].broken_file_evidence_offenders)"
                ),
            };
            format!(
                "\n\nBroken file evidence ({total} reference(s) to repair or remove):\n{}{}",
                shown.join("\n"),
                footer
            )
        };
        // REQ-AXO-902436 — refs the sweep could not judge are NOT reported as
        // broken, and their existence has to be said out loud: without this
        // line the verdict reads as full coverage while part of the corpus was
        // never measured, and `soll_remove_evidence(broken_only=true)` is the
        // action it invites.
        let unresolvable_section = match summary.unresolvable_file_evidence_count {
            0 => String::new(),
            n => format!(
                "\n\n⚠️ {n} file reference(s) NOT judged: the project root did not resolve, \
                 so a relative path could not be checked against anything. They are \
                 deliberately absent from the broken list above — unmeasured is not broken. \
                 Register the project path (`axon_init_project project_path=…`) and re-run \
                 before acting on this verdict."
            ),
        };
        let text = format!(
            "Requirement verification: done={}, partial={}, missing={}\n\nTop gaps:\n{}{}{}{}",
            summary.done,
            summary.partial,
            summary.missing,
            if top_gaps.is_empty() {
                "  (none)".to_string()
            } else {
                top_gaps.join("\n")
            },
            next_to_close,
            broken_section,
            unresolvable_section
        );

        // REQ-AXO-91527 (MIL-AXO-019 Tier B) — tri-modal envelope.
        // Coverage summary reads `RequirementCoverageSummary` (SQL-derived
        // via `soll.Node` JOIN `soll.Edge` JOIN `soll.Traceability`). Live
        // PG surface ; a follow-up slice can route through the SOLL
        // petgraph snapshot (REQ-AXO-322) for sub-ms p99 once the
        // snapshot exposes coverage scoring.
        let total_available = summary.entries.len() as u64;
        // REQ-AXO-902583 — friction rapportée par DOC : « 49 Ko pour quatre chiffres.
        // La réponse a été écrêtée sur disque par notre client. Le verdict utile tenait
        // en {done, missing, partial, total}. »
        //
        // Perdre l'information utile par EXCÈS d'information est le pire échec possible
        // pour une surface : le client a bien reçu la réponse, et n'a pas pu la lire.
        //
        // REQ-AXO-902598 — compact is now the safe default. APS measured a 333k-token
        // response for 612 requirements after the opt-in brief mode already existed:
        // discoverability is not a safety boundary. Full details require the explicit
        // `mode="verbose"`; the compact path returns before constructing them.
        let compact = args.get("mode").and_then(Value::as_str) != Some("verbose");
        if compact {
            let top_gaps_text = if top_gaps.is_empty() {
                "  (none)".to_string()
            } else {
                top_gaps.join("\n")
            };
            return Some(json!({
                "content": [{"type":"text","text": format!(
                    "Requirement verification ({project_code}) — mode=brief\n\
                     done={} · partial={} · missing={} · total={}\n\nTop gaps:\n{}{}\n\n\
                     _↳ réponse compacte par défaut ; passez `mode=\"verbose\"` \
                     pour demander explicitement les listes détaillées._",
                    summary.done, summary.partial, summary.missing, summary.entries.len(),
                    top_gaps_text, next_to_close
                )}],
                "data": {
                    "project_code": project_code,
                    "mode": "brief",
                    "done": summary.done,
                    "partial": summary.partial,
                    "missing": summary.missing,
                    "summary": {
                        "done": summary.done,
                        "partial": summary.partial,
                        "missing": summary.missing,
                        "total": summary.entries.len()
                    },
                    // Dit ce qui MANQUE et pourquoi. Une réponse abrégée qui tait son
                    // abrègement se lit comme une réponse complète — et un appelant
                    // conclurait « aucune exigence partielle » sur une liste absente.
                    "top_gaps": top_gaps,
                    "next_to_close": next_to_close,
                    "omitted_in_brief": ["details", "requirements", "completion_model", "completeness_axes"],
                    "unresolvable_file_evidence_count": summary.unresolvable_file_evidence_count,
                    "total_available": total_available,
                    "status": "ok"
                }
            }));
        }

        let snapshot = self
            .soll_completeness_snapshot_with_cached_coverage(Some(&project_code), Some(summary))
            .ok()?;
        let details = summary
            .entries
            .iter()
            .map(|entry| {
                let missing_dimensions_detailed = entry
                    .missing_dimensions
                    .iter()
                    .map(|dimension| requirement_dimension_descriptor(dimension))
                    .collect::<Vec<_>>();
                let next_actions_detailed = entry
                    .missing_dimensions
                    .iter()
                    .map(|dimension| {
                        let descriptor = requirement_dimension_descriptor(dimension);
                        json!({
                            "dimension": requirement_dimension_canonical_name(dimension),
                            "legacy_dimension": dimension,
                            "action": descriptor.get("next_action").cloned().unwrap_or(Value::Null),
                            "mutation_class": match dimension.as_ref() {
                                "status" | "criteria" => "update_requirement",
                                "evidence" => "attach_evidence",
                                "validation" => "link_validation",
                                "broken_file_evidence" => "repair_evidence",
                                _ => "inspect_requirement"
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                json!({
                    "id": entry.id,
                    "state": entry.state,
                    "completion_state": entry.state,
                    "coverage_reason": requirement_state_reason(&entry.state, &entry.missing_dimensions),
                    "status": entry.status,
                    "evidence_count": entry.evidence_count,
                    "validation_count": entry.validation_count,
                    "has_criteria": entry.has_criteria,
                    "criteres_declares": entry.criteres_resume,
                    "broken_file_evidence_count": entry.broken_file_evidence_count,
                    "broken_file_evidence_offenders": entry
                        .broken_file_evidence
                        .iter()
                        .map(|b| json!({
                            "traceability_id": b.traceability_id,
                            "path": b.artifact_ref
                        }))
                        .collect::<Vec<_>>(),
                    "missing_dimensions": entry.missing_dimensions,
                    "missing_dimensions_detailed": missing_dimensions_detailed,
                    "suggested_next_actions": entry.suggested_next_actions,
                    "next_actions_detailed": next_actions_detailed
                })
            })
            .collect::<Vec<_>>();
        let completion_model = json!({
            "required_dimensions": [
                requirement_dimension_descriptor("status"),
                requirement_dimension_descriptor("criteria"),
                requirement_dimension_descriptor("evidence"),
                requirement_dimension_descriptor("validation")
            ],
            "warning_dimensions": [
                requirement_dimension_descriptor("broken_file_evidence")
            ],
            "done_rule": "EITHER status is `completed` or `delivered` (terminal — done by definition, REQ-AXO-136) OR (status is `current`|`accepted` AND acceptance criteria exist AND supporting evidence exists AND no broken file evidence)",
            "partial_rule": "some required dimensions exist but not all required dimensions are satisfied",
            "missing_rule": "required dimensions are mostly absent or requirement status is not yet operationally accepted"
        });
        Some(json!({
            "content": [{"type":"text","text": text}],
            "data": {
                "project_code": project_code,
                "done": summary.done,
                "partial": summary.partial,
                "missing": summary.missing,
                "summary": {
                    "done": summary.done,
                    "partial": summary.partial,
                    "missing": summary.missing,
                    "total": summary.entries.len()
                },
                "details": details,
                "requirements": details,
                // REQ-AXO-902436 — first-class "not measured" count, distinct
                // from zero-broken. A consumer that ignores it is at least
                // able to see it.
                "unresolvable_file_evidence_count": summary.unresolvable_file_evidence_count,
                "completion_model": completion_model,
                "completeness_axes": {
                    "concept_completeness": snapshot.concept_complete(),
                    "implementation_completeness": snapshot.implementation_complete(),
                    "evidence_ready": snapshot.evidence_ready()
                },
                "guidance_source": "server-side canonical soll completeness evaluator",
                "surfaces_used": ["soll_pg"],
                "total_available": total_available,
                "next_call_hint": "soll_attach_evidence entity_id=<req-id> for the next missing dimension"
            }
        }))
    }

    pub(crate) fn build_plan_operations(&self, project_code: &str, args: &Value) -> Vec<Value> {
        let mut operations = Vec::new();

        if let Some(plan) = args.get("plan") {
            // REQ-AXO-092 — guideline / stakeholder / validation were absent
            // from the plan ingest loop, so a `plan.guidelines` array was
            // silently dropped on the floor with no diagnostic. Storage layer
            // (storage.rs::entity_type_cap) already maps all three; the ingest
            // loop just needed to enumerate them.
            for entity in [
                "pillar",
                "requirement",
                "decision",
                "milestone",
                "vision",
                "concept",
                "stakeholder",
                "validation",
                "guideline",
            ] {
                if let Some(items) = plan.get(format!("{}s", entity)).and_then(|v| v.as_array()) {
                    for item in items {
                        if let Some(obj) = item.as_object() {
                            let title = obj.get("title").and_then(|v| v.as_str()).unwrap_or("");
                            let logical_key = obj
                                .get("logical_key")
                                .and_then(|v| v.as_str())
                                .unwrap_or(title);
                            if logical_key.is_empty() {
                                continue;
                            }
                            let existing_id =
                                self.resolve_soll_id(entity, project_code, title, logical_key);
                            let kind = if existing_id.is_some() {
                                "update"
                            } else {
                                "create"
                            };
                            operations.push(json!({
                                "kind": kind,
                                "entity": entity,
                                "project_code": project_code,
                                "logical_key": logical_key,
                                "entity_id": existing_id,
                                "payload": Value::Object(obj.clone())
                            }));
                        }
                    }
                }
            }
        }

        if let Some(relations) = args.get("relations").and_then(|v| v.as_array()) {
            for rel in relations {
                if let Some(obj) = rel.as_object() {
                    operations.push(json!({
                        "kind": "link",
                        "entity": "relation",
                        "project_code": project_code,
                        "payload": Value::Object(obj.clone())
                    }));
                }
            }
        }

        operations
    }
}
