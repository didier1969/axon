use serde_json::{json, Value};

use super::format::{evidence_by_mode, format_standard_contract};
use super::tools_framework::WHY_CACHE_TTL_MS;
use super::tools_framework_status_guidance::project_status_operator_guidance;
use super::tools_framework_support::{
    cache_read, cache_write, load_structural_snapshots, persist_structural_snapshot,
    structural_history_path,
};
use super::McpServer;
use crate::mcp::format::Compte;

fn compact_project_status_brief_data(data: &Value) -> Value {
    let project_code = data.get("project_code").cloned().unwrap_or(Value::Null);
    let conception = data.get("conception").unwrap_or(&Value::Null);
    let soll = data.get("soll_context").unwrap_or(&Value::Null);
    let list_count = |key: &str| {
        soll.get(key)
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0)
    };

    let vision_raw = data.get("vision").cloned().unwrap_or(Value::Null);
    let compact_vision = if let Some(obj) = vision_raw.as_object() {
        let id = obj.get("id").and_then(Value::as_str).unwrap_or("unavailable");
        let title = obj.get("title").and_then(Value::as_str).unwrap_or("unavailable");
        let status = obj.get("status").and_then(Value::as_str).unwrap_or("unknown");
        let desc = obj.get("description").and_then(Value::as_str).unwrap_or("");
        let summary = if desc.is_empty() || desc == "unavailable" {
            desc.to_string()
        } else {
            let first = desc.split(['.', '\n']).next().unwrap_or(desc).trim();
            if first.len() > 140 {
                format!("{}…", &first[..137])
            } else {
                first.to_string()
            }
        };
        json!({
            "id": id,
            "title": title,
            "status": status,
            "source": obj.get("source").and_then(Value::as_str).unwrap_or("SOLL"),
            "summary": summary,
            "body_chars": desc.chars().count(),
            "expand_with": {
                "tool": "soll_get",
                "arguments": { "id": id }
            }
        })
    } else {
        vision_raw
    };

    json!({
        "project_code": project_code,
        "snapshot_id": data.get("snapshot_id").cloned().unwrap_or(Value::Null),
        "generated_at": data.get("generated_at").cloned().unwrap_or(Value::Null),
        "delta_vs_previous": data.get("delta_vs_previous").cloned().unwrap_or(Value::Null),
        "vision": compact_vision,
        "conception_summary": {
            "module_count": conception.get("module_count").cloned().unwrap_or(Value::Null),
            "interface_count": conception.get("interface_count").cloned().unwrap_or(Value::Null),
            "contract_count": conception.get("contract_count").cloned().unwrap_or(Value::Null),
            "flow_count": conception.get("flow_count").cloned().unwrap_or(Value::Null),
        },
        "runtime": data.get("runtime").cloned().unwrap_or(Value::Null),
        "truth_cockpit": data.get("truth_cockpit").cloned().unwrap_or(Value::Null),
        "anomalies_summary": data.pointer("/anomalies/summary").cloned().unwrap_or_else(|| json!({})),
        "snapshot_storage": data.get("snapshot_storage").cloned().unwrap_or(Value::Null),
        "operator_guidance": data.get("operator_guidance").cloned().unwrap_or(Value::Null),
        "next_action": data.get("next_action").cloned().unwrap_or(Value::Null),
        "soll_context_counts": {
            "visions": list_count("visions"),
            "requirements": list_count("requirements"),
            "decisions": list_count("decisions"),
            "revisions": list_count("revisions"),
        },
        "stage_timings_ms": data.get("stage_timings_ms").cloned().unwrap_or(Value::Null),
        "expand_anomalies": {
            "tool": "anomalies",
            "arguments": { "project": project_code.clone(), "mode": "verbose" }
        },
        "expand_conception": {
            "tool": "conception_view",
            "arguments": { "project": project_code.clone() }
        },
        "expand_soll_context": {
            "tool": "soll_query_context",
            "arguments": { "project_code": project_code.clone() }
        },
        "omitted_in_brief": [
            "conception.modules", "conception.interfaces", "conception.contracts",
            "conception.flows", "anomalies.findings", "anomalies.recommendations",
            "soll_context.visions", "soll_context.requirements",
            "soll_context.decisions", "soll_context.revisions",
            "vision.description"
        ],
        "detail_continuation": {
            "tool": "project_status",
            "arguments": {
                "project_code": project_code,
                "mode": "verbose"
            }
        }
    })
}

impl McpServer {
    fn project_status_degradation_display(indexed_files: i64, degraded_notes: &[String]) -> String {
        let mut notes = degraded_notes.to_vec();
        if indexed_files == 0 {
            notes.push(
                "aucun fichier indexe pour ce projet — les metriques derivees du code ne \
                 sont PAS mesurees ; voir `diagnose_indexing`"
                    .to_string(),
            );
        }
        if notes.is_empty() {
            "none in this snapshot — this is not a claim that global runtime is healthy; \
             `status` is the runtime authority"
                .to_string()
        } else {
            notes.join(", ")
        }
    }

    #[cfg(test)]
    pub(crate) fn project_status_degradation_display_for_tests(
        indexed_files: i64,
        degraded_notes: &[String],
    ) -> String {
        Self::project_status_degradation_display(indexed_files, degraded_notes)
    }

    /// REQ-AXO-901926 — resolve the CANONICAL current Vision for a project.
    /// The previous `.first()` over `soll_query_context.visions` surfaced a
    /// rejected/test Vision (shared-PG test fixtures inject `VIS-*-90x`
    /// 'Test Vision' nodes — see REQ-AXO-91560 test isolation) and could miss
    /// the real one entirely under the top-N limit. We select the `current`
    /// Vision with the most incoming EPITOMIZES edges (the real Vision is
    /// epitomized by every Pillar; test fixtures have none), tie-broken by id.
    fn canonical_current_vision(&self, project_code: &str) -> Option<Value> {
        let pc = project_code.replace('\'', "''");
        let sql = format!(
            "SELECT n.id, n.title, n.status, left(n.description, 280) AS body \
             FROM soll.Node n \
             WHERE n.type = 'Vision' AND n.status = 'current' AND n.project_code = '{pc}' \
             ORDER BY (SELECT count(*) FROM soll.Edge e WHERE e.target_id = n.id AND e.relation_type = 'EPITOMIZES') DESC, n.id ASC \
             LIMIT 1"
        );
        let raw = self.graph_store.query_json(&sql).ok()?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).ok()?;
        let row = rows.into_iter().next()?;
        Some(json!({
            "id": row.first().and_then(Value::as_str).unwrap_or("unknown"),
            "title": row.get(1).and_then(Value::as_str).unwrap_or("unknown"),
            "status": row.get(2).and_then(Value::as_str).unwrap_or("current"),
            "description": row.get(3).and_then(Value::as_str).unwrap_or("unavailable"),
            "source": "SOLL"
        }))
    }

    pub(super) fn axon_project_status_impl(&self, args: &Value) -> Option<Value> {
        let mode = args.get("mode").and_then(|value| value.as_str());
        // REQ-AXO-902467 — ne plus deviner : ce site passait de « argument
        // absent » a `"AXO"` sans tenter la resolution.
        let resolved = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .map(String::from)
            .or_else(|| self.auto_resolve_project_code_str());
        let Some(project_code) = resolved.as_deref() else {
            return Some(crate::mcp::guidance::unresolved_project_error(
                "project_status",
                &self.known_project_codes_hint(),
            ));
        };

        // REQ-AXO-901982 — per-stage timing so the next measurement pinpoints
        // the occasional multi-second spike (telemetry shows project_status avg
        // ~11.5s driven by rare ~60s runs, while each sub-call is individually
        // fast). Observability before optimization (PIL-AXO-9006, TOC discipline).
        let t_status = std::time::Instant::now();
        let status = self.axon_status(&json!({
            "mode": mode.unwrap_or("brief"),
            "project_code": project_code
        }))?;
        let ms_status = t_status.elapsed().as_millis() as u64;
        let status_data = status.get("data").cloned().unwrap_or_else(|| json!({}));

        // REQ-AXO-901926 — the anomalies tool is RAM-first (PIL-AXO-9002) +
        // TTL-cached, so the old "decoupled to prevent timeout" stub (which
        // forced the structural counts to 0/0/0 even though `anomalies` returns
        // them instantly) is obsolete. Pull the real summary; fall back to an
        // empty summary only if the call fails.
        let t_anomalies = std::time::Instant::now();
        let anomalies_data = self
            .axon_anomalies(&json!({ "project": project_code, "mode": "brief" }))
            .and_then(|resp| resp.get("data").cloned())
            .unwrap_or_else(|| json!({ "summary": {}, "findings": [], "recommendations": [] }));
        let ms_anomalies = t_anomalies.elapsed().as_millis() as u64;
        let t_soll = std::time::Instant::now();
        let soll_context = self.axon_soll_query_context(&json!({
            "project_code": project_code,
            "limit": 5
        }))?;
        let ms_soll_context = t_soll.elapsed().as_millis() as u64;
        let soll_data = soll_context
            .get("data")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let t_conception = std::time::Instant::now();
        let conception = self.cached_conception_view(project_code);
        let ms_conception = t_conception.elapsed().as_millis() as u64;
        // REQ-AXO-901926 — canonical current Vision first (robust against
        // rejected/test-fixture visions); fall back to the soll_query_context
        // list only if the direct lookup yields nothing.
        let t_vision = std::time::Instant::now();
        let vision = self
            .canonical_current_vision(project_code)
            .or_else(|| {
                soll_data
                    .get("visions")
                    .and_then(|value| value.as_array())
                    .and_then(|items| items.first())
                    .and_then(|value| value.as_str())
                    .map(Self::parse_soll_vision_entry)
            })
            .unwrap_or_else(|| {
                json!({
                    "id": "unavailable",
                    "title": "unavailable",
                    "status": "unknown",
                    "description": "unavailable",
                    "source": "SOLL"
                })
            });
        let ms_vision = t_vision.elapsed().as_millis() as u64;

        // REQ-AXO-901948 — canonical Validation count as the authoritative
        // fallback for coverage. The IST-derived `validation_coverage_score`
        // can be absent when the project's indexed projection is stale
        // (`indexed_projections_not_fresh`), but the canonical SOLL still
        // holds the Validation nodes. project_status must never assert
        // "unknown"/absence when a canonical read contradicts it — same
        // contract the Vision line already honours (REQ-AXO-901926).
        let t_validation = std::time::Instant::now();
        let canonical_validation_count = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node WHERE type = 'Validation' AND status = 'current' AND project_code = '{}'",
                project_code.replace('\'', "''")
            ))
            .unwrap_or(0);
        let ms_validation = t_validation.elapsed().as_millis() as u64;

        let fichiers_indexes = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM ist.indexedfile WHERE project_code = '{}'",
                project_code.replace('\'', "''")
            ))
            .unwrap_or(0);

        let anomaly_summary = anomalies_data
            .get("summary")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let t_snapshot = std::time::Instant::now();
        let previous_snapshot = load_structural_snapshots(project_code).into_iter().last();
        let previous_summary = previous_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.get("anomaly_summary"));
        let snapshot_id = format!("project-status-{}-{}", project_code, crate::clock::now_unix_ms());
        let generated_at = crate::clock::now_unix_ms();
        let delta_vs_previous =
            Self::build_project_status_delta(previous_summary, &anomaly_summary);

        let runtime_degraded_notes = status_data
            .pointer("/availability/degraded_notes")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect::<Vec<_>>();

        let modified_since = status_data
            .pointer("/truth_cockpit/staleness/modified_files_since")
            .and_then(Value::as_u64)
            .unwrap_or(0);

        let ist_writer_unhealthy = runtime_degraded_notes
            .iter()
            .any(|note| note.starts_with("ist_writer_"));

        // REQ-AXO-902546: réconcilier project_status avec la fraîcheur réelle du projet.
        // Si le projet a des fichiers indexés (fichiers_indexes > 0), 0 fichier modifié (modified_since == 0)
        // et qu'aucun échec d'écrivain n'est en cours, le snapshot IST est synchronisé avec le code source
        // ("snapshot in sync with source"). La note globale runtime `indexed_projections_not_fresh`
        // (qui signale simplement que le démon indexeur n'est pas en écoute continue de fond) ne bloque
        // PAS les lectures ni les conclusions de ce projet.
        let snapshot_in_sync = fichiers_indexes > 0 && modified_since == 0 && !ist_writer_unhealthy;

        let mut project_blockers = Vec::<String>::new();
        let mut degraded_notes = Vec::<String>::new();

        let is_indexer_idle_note = |note: &str| {
            note == "indexed_projections_not_fresh"
                || note == "indexer_feed_degraded"
                || note == "indexer_heartbeat_absent"
                || note == "runtime_authority_not_converged"
        };

        if fichiers_indexes == 0 {
            let empty_note = "aucun fichier indexe pour ce projet — les metriques derivees du code ne sont PAS mesurees ; voir `diagnose_indexing`".to_string();
            project_blockers.push(empty_note.clone());
            degraded_notes.push(empty_note);
        } else if !snapshot_in_sync {
            for note in &runtime_degraded_notes {
                project_blockers.push(note.clone());
                degraded_notes.push(note.clone());
            }
        } else {
            for note in &runtime_degraded_notes {
                if !is_indexer_idle_note(note) {
                    project_blockers.push(note.clone());
                    degraded_notes.push(note.clone());
                }
            }
        }

        let snapshot_record = json!({
            "snapshot_id": snapshot_id,
            "generated_at": generated_at,
            "project_code": project_code,
            "anomaly_summary": anomaly_summary,
            "conception_summary": {
                "module_count": conception.get("module_count").cloned().unwrap_or_else(|| json!(0)),
                "interface_count": conception.get("interface_count").cloned().unwrap_or_else(|| json!(0)),
                "contract_count": conception.get("contract_count").cloned().unwrap_or_else(|| json!(0)),
                "flow_count": conception.get("flow_count").cloned().unwrap_or_else(|| json!(0))
            },
            "provenance": "aggregated",
            "confidence": "medium"
        });
        let snapshot_storage = match persist_structural_snapshot(project_code, &snapshot_record) {
            Ok(()) => json!({
                "scope": "derived_non_canonical",
                "path": structural_history_path(project_code).to_string_lossy().to_string(),
                "persisted": true
            }),
            Err(error) => {
                let note = format!("snapshot_persistence_failed:{error}");
                project_blockers.push(note.clone());
                degraded_notes.push(note.clone());
                json!({
                    "scope": "derived_non_canonical",
                    "path": structural_history_path(project_code).to_string_lossy().to_string(),
                    "persisted": false,
                    "error": error,
                    "degraded_notes": degraded_notes.clone()
                })
            }
        };
        let ms_snapshot = t_snapshot.elapsed().as_millis() as u64;
        let ms_total = t_status.elapsed().as_millis() as u64;
        let stage_timings_ms = json!({
            "status": ms_status,
            "anomalies": ms_anomalies,
            "soll_context": ms_soll_context,
            "conception": ms_conception,
            "vision": ms_vision,
            "validation_count": ms_validation,
            "snapshot_io": ms_snapshot,
            "total": ms_total
        });
        let operator_guidance =
            project_status_operator_guidance(&degraded_notes, &snapshot_storage, &vision, project_code);
        let next_best_action = operator_guidance
            .get("next_action")
            .cloned()
            .unwrap_or(Value::Null);

        let status_recovery_hint = status_data
            .pointer("/truth_cockpit/recovery_hint")
            .cloned()
            .unwrap_or(Value::Null);

        let recovery_hint = if fichiers_indexes == 0 {
            json!({
                "action": "diagnose_indexing",
                "command": format!("diagnose_indexing project={project_code}"),
                "reason": "aucun fichier indexe pour ce projet",
                "verification": "fichiers_indexes > 0"
            })
        } else if !project_blockers.is_empty() {
            if !status_recovery_hint.is_null() {
                status_recovery_hint
            } else {
                let (_, hint) = crate::mcp::tools_framework_runtime_status::derive_recovery_action(&project_blockers);
                hint
            }
        } else {
            Value::Null
        };

        let mut proof_gaps = Vec::<Value>::new();
        if anomaly_summary.get("validation_coverage_score").is_none()
            && canonical_validation_count == 0
        {
            proof_gaps.push(json!("validation_coverage_unknown"));
        }
        if vision
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("unavailable")
            == "unavailable"
        {
            proof_gaps.push(json!("canonical_vision_unavailable"));
        }
        if snapshot_storage
            .get("persisted")
            .and_then(|value| value.as_bool())
            == Some(false)
        {
            proof_gaps.push(json!("snapshot_storage_not_persisted"));
        }
        let status_cache_meta = status_data.get("cache_meta").cloned().unwrap_or_else(|| {
            let now = crate::clock::now_unix_ms();
            json!({
                "is_cached": false,
                "epoch_ms": now,
                "cache_age_ms": 0,
                "ttl_ms": super::tools_framework::STATUS_CACHE_TTL_MS,
            })
        });
        let authority_epoch_ms = status_cache_meta
            .get("epoch_ms")
            .and_then(Value::as_i64)
            .unwrap_or_else(crate::clock::now_unix_ms);

        let truth_cockpit = json!({
            "current_blocker": project_blockers
                .first()
                .cloned()
                .map(Value::String)
                .unwrap_or(Value::Null),
            "next_best_action": next_best_action,
            "recovery_hint": recovery_hint,
            "confidence": "high",
            "freshness": {
                "state": if project_blockers.is_empty() { "fresh" } else { "degraded" },
                "degraded_notes": degraded_notes,
                "runtime_truth_status": status_data.get("truth_status").cloned().unwrap_or(Value::Null),
                "epoch_ms": authority_epoch_ms,
                "cache_meta": status_cache_meta,
            },
            "proof_gaps": proof_gaps,
            "llm_instruction": "Use `next_best_action` first; if freshness is degraded, label project-wide conclusions partial and follow the named MCP tool."
        });
        let public_tools = status_data
            .get("public_tools")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect::<Vec<_>>();
        let public_tool_count = status_data
            .get("public_tool_count")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(public_tools.len());
        let brief_mode = mode.unwrap_or("brief") == "brief";
        let public_tools_evidence = if public_tool_count == 0 {
            "unknown".to_string()
        } else if brief_mode {
            format!(
                "{} tools (use `status mode=verbose` for list)",
                public_tool_count
            )
        } else {
            public_tools.join(", ")
        };
        let runtime_data = if brief_mode {
            json!({
                "runtime_mode": status_data.get("runtime_mode").cloned().unwrap_or(Value::Null),
                "runtime_profile": status_data.get("runtime_profile").cloned().unwrap_or(Value::Null),
                "truth_status": status_data.get("truth_status").cloned().unwrap_or(Value::Null),
                "drain_state": status_data.get("drain_state").cloned().unwrap_or(Value::Null),
                "availability": status_data.get("availability").cloned().unwrap_or_else(|| json!({})),
                "runtime_version": status_data.get("runtime_version").cloned().unwrap_or_else(|| json!({})),
                "runtime_state": status_data.pointer("/runtime_authority/runtime_state").cloned().unwrap_or_else(|| json!({})),
                "file_vectorization_queue": status_data.get("file_vectorization_queue").cloned().unwrap_or_else(|| json!({})),
                "public_tool_count": public_tool_count,
                "mode": "brief_compact"
            })
        } else {
            status_data.clone()
        };

        // REQ-AXO-901948 — render the IST coverage score when present, else
        // fall back to the canonical Validation count instead of "unknown".
        let validation_coverage_display = anomaly_summary
            .get("validation_coverage_score")
            .map(|value| value.to_string())
            .unwrap_or_else(|| {
                if canonical_validation_count > 0 {
                    format!("{canonical_validation_count} (canonical count)")
                } else {
                    "unknown".to_string()
                }
            });



        // Trois etats, jamais deux. Deux lectures distinctes, parce que les grandeurs
        // n'ont pas la meme source — le rapporteur le releve lui-meme et exclut
        // `orphan_intent` de son reproche : c'est un compte SOLL, il reste valide sur
        // un projet non indexe.
        let compte_code = |cle: &str| -> String {
            if fichiers_indexes == 0 {
                return Compte::non_calcule("aucun fichier indexe pour ce projet").rendre();
            }
            match anomaly_summary.get(cle) {
                Some(v) if v.is_i64() || v.is_u64() => v.to_string(),
                _ => Compte::non_calcule("anomalies n'a pas mesure cette grandeur").rendre(),
            }
        };
        let compte_soll = |cle: &str| -> String {
            match anomaly_summary.get(cle) {
                Some(v) if v.is_i64() || v.is_u64() => v.to_string(),
                _ => Compte::non_calcule("anomalies n'a pas mesure cette grandeur").rendre(),
            }
        };

        let evidence = format!(
            "**Vision:** `{}` - {}\n\
**Vision status:** `{}`\n\
**Runtime mode/profile:** `{}` / `{}`\n\
**Drain state:** `{}`\n\
**Public tools:** {}\n\
**Wrappers / Orphan code / Orphan intent:** {} / {} / {}\n\
**Validation coverage:** {}\n\
**Snapshot degradation notes (runtime + project):** {}\n\
**Global runtime authority:** `status`\n",
            vision
                .get("id")
                .and_then(|value| value.as_str())
                .unwrap_or("unavailable"),
            vision
                .get("title")
                .and_then(|value| value.as_str())
                .unwrap_or("unavailable"),
            vision
                .get("status")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            status_data
                .get("runtime_mode")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            status_data
                .get("runtime_profile")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            status_data
                .get("drain_state")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            public_tools_evidence,
            // REQ-AXO-902409 (doleance DVM #255) — `.unwrap_or(0)` rendait
            // « 0 / 0 / 0 » indistinguable d'un projet sain sur un projet dont
            // `health` disait, dans la MEME minute, « eligible 192, indexed 0 ».
            // `anomalies` publie desormais `null` pour une grandeur non calculee ;
            // ce lecteur-ci le RELAIE au lieu de le convertir en zero.
            compte_code("wrapper_count"),
            compte_code("orphan_code_count"),
            compte_soll("orphan_intent_count"),
            validation_coverage_display,
            // REQ-AXO-902607 / KKI #402 — cette ligne est un SNAPSHOT compose
            // de la sante runtime et de la couverture projet. Un `none` nu etait
            // lu comme une negation de `status`; nommer l'autorite et la portee.
            Self::project_status_degradation_display(fichiers_indexes, &degraded_notes)
        );
        let report = format!(
            "## 🧭 Project Status\n\n{}",
            format_standard_contract(
                "ok",
                "live project situation assembled from MCP read surfaces",
                &format!("project:{}", project_code),
                &evidence_by_mode(&evidence, mode),
                &[
                    "use `why` on a specific symbol to inspect rationale",
                    "use `path` for source/sink flow",
                    "use `anomalies` for the full structural findings payload"
                ],
                "high",
            )
        );

        let mut response = json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "project_code": project_code,
                "snapshot_id": snapshot_id,
                "generated_at": generated_at,
                "delta_vs_previous": delta_vs_previous,
                "vision": vision,
                "conception": conception,
                "runtime": runtime_data,
                "truth_cockpit": truth_cockpit,
                "anomalies": {
                    "summary": anomaly_summary,
                    "findings": anomalies_data.get("findings").cloned().unwrap_or_else(|| json!([])),
                    "recommendations": anomalies_data.get("recommendations").cloned().unwrap_or_else(|| json!([]))
                },
                "snapshot_storage": snapshot_storage,
                "operator_guidance": operator_guidance.clone(),
                "next_action": operator_guidance
                    .get("next_action")
                    .cloned()
                    .unwrap_or(Value::Null),
                "soll_context": {
                    "visions": soll_data.get("visions").cloned().unwrap_or_else(|| json!([])),
                    "requirements": soll_data.get("requirements").cloned().unwrap_or_else(|| json!([])),
                    "decisions": soll_data.get("decisions").cloned().unwrap_or_else(|| json!([])),
                    "revisions": soll_data.get("revisions").cloned().unwrap_or_else(|| json!([]))
                },
                "canonical_sources": Self::canonical_sources_snapshot(),
                "stage_timings_ms": stage_timings_ms
            }
        });
        // REQ-AXO-902609 — the old brief response embedded complete
        // conception, anomaly findings, and SOLL lists.  The text was concise,
        // but structuredContent was not.  Preserve decisions and counters,
        // state every omitted subtree, and make verbose retrieval executable.
        if brief_mode {
            let full_data = response.get("data").cloned().unwrap_or_else(|| json!({}));
            response["data"] = compact_project_status_brief_data(&full_data);
        }
        Some(response)
    }

    pub(super) fn axon_why_impl(&self, args: &Value) -> Option<Value> {
        let mode = args
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or("brief");
        let cache_key = format!(
            "{}::{}::{}::{}",
            args.get("symbol")
                .and_then(|value| value.as_str())
                .or_else(|| args.get("question").and_then(|value| value.as_str()))
                .unwrap_or("*"),
            args.get("project")
                .and_then(|value| value.as_str())
                .unwrap_or("*"),
            mode,
            args.get("include_graph")
                .and_then(|value| value.as_bool())
                .unwrap_or(mode != "brief")
        );
        let now_ms = crate::clock::now_unix_ms();
        if let Some(cached) = cache_read(Self::why_cache(), &cache_key, now_ms, WHY_CACHE_TTL_MS) {
            return Some(cached);
        }
        let include_graph = args
            .get("include_graph")
            .and_then(|value| value.as_bool())
            .unwrap_or(mode != "brief");
        // REQ-AXO-043 — `symbol=""` previously produced a malformed
        // "Why does  exist?" question (double space) that retrieve_context
        // happily processed, returning Status: ok with arbitrary supporting
        // evidence. Trim and reject empty symbol BEFORE falling through.
        let question = args
            .get("question")
            .and_then(|value| value.as_str())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                args.get("symbol")
                    .and_then(|value| value.as_str())
                    .map(|symbol| symbol.trim().to_string())
                    .filter(|symbol| !symbol.is_empty())
                    .map(|symbol| format!("Why does {} exist?", symbol))
            });
        let question = match question {
            Some(value) => value,
            None => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": "why requires a non-empty `symbol` or `question`. Pass either a target symbol id/name or a free-form question (example: symbol=\"axon_query\" or question=\"why does the queue admission policy reject?\")."
                    }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "missing_field": "symbol_or_question",
                        "next_action": "supply at least one of `symbol` or `question`",
                        "operator_guidance": {
                            "problem_class": "input_invalid",
                            "likely_cause": "empty_or_whitespace_symbol_and_question",
                            "next_best_actions": [
                                "supply a non-empty `symbol` argument (canonical id or name)",
                                "or supply a non-empty `question` describing the rationale you want",
                            ],
                            "follow_up_tools": ["query", "inspect", "retrieve_context"],
                            "confidence": "high",
                        },
                        "parameter_repair": {
                            "invalid_field": "symbol|question",
                            "accepted_aliases": ["symbol", "question"],
                            "follow_up_tools": ["query", "inspect", "retrieve_context"],
                            "hint": "supply at least one of `symbol` (canonical id or name) or `question` (free-form rationale prompt); example: symbol=\"axon_query\" or question=\"why does the queue admission policy reject?\""
                        }
                    }
                }));
            }
        };
        let mut response = self.axon_retrieve_context(&json!({
            "question": question,
            "project": args.get("project").and_then(|value| value.as_str()),
            "mode": mode,
            "top_k": args.get("top_k").cloned().unwrap_or_else(|| json!(if mode == "brief" { 3 } else { 6 })),
            "token_budget": args.get("token_budget").cloned().unwrap_or_else(|| json!(if mode == "brief" { 700 } else { 1400 })),
            "include_soll": true,
            "include_graph": include_graph
        }))?;
        if let Some(data) = response
            .get_mut("data")
            .and_then(|value| value.as_object_mut())
        {
            data.insert("framework_alias".to_string(), json!("why"));
        }
        // REQ-AXO-902429 / REQ-AXO-902579 — Surfacing living replacement(s) when querying a superseded SOLL node
        if let Some(sym) = args.get("symbol").and_then(|v| v.as_str()) {
            let sym_trimmed = sym.trim();
            let sql = "SELECT e.source_id, COALESCE(n.title, '') \
                       FROM soll.Edge e \
                       JOIN soll.Node n ON n.id = e.source_id \
                       WHERE e.target_id = ? AND e.relation_type = 'SUPERSEDES' \
                       ORDER BY e.source_id ASC";
            let reps: Vec<(String, String)> = self
                .graph_store
                .query_json_param(sql, &json!([sym_trimmed]))
                .ok()
                .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
                .map(|rows| {
                    rows.into_iter()
                        .filter_map(|r| {
                            let rep_id = r.get(0)?.as_str()?.to_string();
                            let rep_title = r.get(1).and_then(Value::as_str).unwrap_or("").to_string();
                            Some((rep_id, rep_title))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !reps.is_empty() {
                let first_rep = &reps[0];
                if let Some(data) = response.get_mut("data").and_then(|v| v.as_object_mut()) {
                    data.insert("superseded_by".to_string(), json!(first_rep.0));
                    data.insert("superseded_by_title".to_string(), json!(first_rep.1));
                    let all_ids: Vec<String> = reps.iter().map(|(id, _)| id.clone()).collect();
                    data.insert("superseded_by_all".to_string(), json!(all_ids));
                }
                if let Some(content_arr) = response.get_mut("content").and_then(|v| v.as_array_mut()) {
                    if let Some(first_item) = content_arr.first_mut().and_then(|v| v.as_object_mut()) {
                        if let Some(text_val) = first_item.get_mut("text").and_then(|v| v.as_str()) {
                            let notice = if reps.len() == 1 {
                                format!(
                                    "ℹ Notice: `{sym_trimmed}` has been superseded by `{}` ({}).\n\n{}",
                                    first_rep.0,
                                    if first_rep.1.is_empty() { "replacement node" } else { &first_rep.1 },
                                    text_val
                                )
                            } else {
                                let reps_str = reps
                                    .iter()
                                    .map(|(rep_id, rep_title)| {
                                        if rep_title.is_empty() {
                                            format!("`{rep_id}`")
                                        } else {
                                            format!("`{rep_id}` ({rep_title})")
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                format!(
                                    "ℹ Notice: `{sym_trimmed}` has been superseded by: {reps_str}.\n\n{}",
                                    text_val
                                )
                            };
                            first_item.insert("text".to_string(), json!(notice));
                        }
                    }
                }
            }
        }
        Self::summarize_why_response(args, &mut response);
        cache_write(Self::why_cache(), cache_key, now_ms, &response);
        Some(response)
    }
}
