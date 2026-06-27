//! REQ-AXO-902094 (S7) — surface MCP de consommation du squelette de contrats.
//!
//! `contract_status` rend les ContractNodes navigables comme l'IST (query / inspect
//! / impact), en LECTURE SEULE :
//! - sans `contract_id` → LISTE paginée des contrats du projet (forme désirée
//!   minimale + cycle de vie + état du sceau) ;
//! - avec `contract_id` → INSPECTION : forme désirée + sceau persisté + drift
//!   IST↔contrat recalculé LIVE ([`reconcile_contract`]) + arêtes de gouvernance
//!   (sortantes) et d'impact (entrantes).
//!
//! La couche persistante/réconciliation vit dans [`crate::contract::store`] (SRP) ;
//! ce module ne fait que la projeter en réponse MCP terse-par-défaut (GUI-AXO-1026
//! invariant 4 : verbose est opt-in via `mode`).

use serde_json::{json, Value};

use super::McpServer;
use crate::contract::store::{
    clear_seal, contract_edges, contract_status_str, count_contracts, list_contracts,
    live_incoming_call_count, load_contract, load_seal, reconcile_contract, retire_contract,
    ContractEdgeRow, DriftVerdict,
};
use crate::contract::ContractNode;

fn contract_err(msg: &str, status: &str) -> Value {
    json!({
        "content": [{"type":"text","text": format!("### 📐 contract_status — {msg}")}],
        "isError": true,
        "data": {"status": status, "error": msg}
    })
}

/// Projette un [`DriftVerdict`] (S6) en JSON stable pour le LLM : un `verdict`
/// nommé + `aligned` (true/false/null) + l'évidence-sol propre à la variante.
fn drift_json(v: &DriftVerdict) -> Value {
    match v {
        DriftVerdict::Unbound => json!({"verdict": "unbound", "aligned": null,
            "note": "realized_by absent — contrat planifié, binding S4 pas encore établi (pas un drift)"}),
        DriftVerdict::SymbolMissing { symbol_id } => json!({"verdict": "symbol_missing",
            "aligned": false, "symbol_id": symbol_id,
            "note": "l'ancre d'identité pointe un symbole absent de l'IST — candidat rename / re-anchor"}),
        DriftVerdict::KindMismatch { expected, observed_kind } => json!({"verdict": "kind_mismatch",
            "aligned": false, "expected": expected.tag(), "observed_kind": observed_kind}),
        DriftVerdict::ShapeDrift { baseline, observed } => json!({"verdict": "shape_drift",
            "aligned": false, "baseline": baseline, "observed": observed}),
        DriftVerdict::NoBaseline { observed } => json!({"verdict": "no_baseline", "aligned": null,
            "observed": observed, "note": "première réconciliation — aucune baseline figée"}),
        DriftVerdict::Aligned { observed } => json!({"verdict": "aligned", "aligned": true,
            "observed": observed}),
    }
}

fn edge_json(e: &ContractEdgeRow) -> Value {
    json!({"source": e.source_id, "relation": e.relation_type, "target": e.target_id})
}

impl McpServer {
    /// REQ-AXO-902094 — `contract_status` : liste OU inspection des ContractNodes.
    pub(crate) fn axon_contract_status(&self, args: &Value) -> Option<Value> {
        let verbose = args
            .get("mode")
            .and_then(Value::as_str)
            .is_some_and(|m| m.eq_ignore_ascii_case("verbose") || m.eq_ignore_ascii_case("full"));

        match args
            .get("contract_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(id) => self.contract_status_inspect(id, verbose),
            None => self.contract_status_list(args, verbose),
        }
    }

    fn contract_status_list(&self, args: &Value, verbose: bool) -> Option<Value> {
        let project = args
            .get("project")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.auto_resolve_project_code_str())
            .unwrap_or_else(|| "AXO".to_string());
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(25).clamp(1, 100) as i64;
        let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as i64;

        let total = count_contracts(&self.graph_store, &project).unwrap_or(0);
        let rows = match list_contracts(&self.graph_store, &project, limit, offset) {
            Ok(r) => r,
            Err(e) => return Some(contract_err(&format!("list failed: {e}"), "degraded")),
        };
        let contracts: Vec<Value> = rows
            .iter()
            .map(|c| {
                let mut o = json!({
                    "id": c.id,
                    "kind": c.kind.tag(),
                    "status": c.status,
                    "sealed": c.sealed,
                    "adequate": c.adequate,
                });
                if verbose {
                    o["signature"] = json!(c.signature);
                    o["realized_by"] = json!(c.realized_by);
                    o["seal_revision"] = json!(c.seal_revision);
                }
                o
            })
            .collect();
        let returned = contracts.len() as i64;
        let next_offset = if offset + returned < total { Some(offset + returned) } else { None };
        Some(json!({
            "content": [{"type":"text","text": format!(
                "### 📐 contract_status `{project}` — {returned}/{total} contrat(s){}",
                next_offset.map(|n| format!(" · next offset={n}")).unwrap_or_default()
            )}],
            "data": {
                "status": "ok",
                "project": project,
                "count": returned,
                "contracts": contracts,
                "pagination": {"limit": limit, "offset": offset, "total": total,
                               "returned": returned, "next_offset": next_offset},
                "next": {"tool": "contract_status",
                         "hint": "pass contract_id=<id> to inspect (seal + live IST drift), mode=verbose for bodies"}
            }
        }))
    }

    fn contract_status_inspect(&self, id: &str, verbose: bool) -> Option<Value> {
        let node = match load_contract(&self.graph_store, id) {
            Ok(Some(n)) => n,
            Ok(None) => return Some(contract_err(&format!("contrat introuvable: {id}"), "input_not_found")),
            Err(e) => return Some(contract_err(&format!("load failed: {e}"), "degraded")),
        };
        let seal = load_seal(&self.graph_store, id).ok().flatten();
        let drift = reconcile_contract(&self.graph_store, id).ok();
        let (outgoing, incoming) = contract_edges(&self.graph_store, id).unwrap_or_default();

        let lifecycle = if seal.is_some() {
            "sealed"
        } else if node.realized_by.is_some() {
            "bound"
        } else {
            "planned"
        };
        let seal_json = seal.as_ref().map(|s| {
            json!({"seal_hash": s.seal.0, "adequate": s.adequate, "revision": s.revision})
        });

        let mut data = json!({
            "status": "ok",
            "id": id,
            "kind": node.kind.tag(),
            "signature": node.signature,
            "lifecycle": lifecycle,
            "realized_by": node.realized_by,
            "sealed": seal.is_some(),
            "seal": seal_json,
            "drift": drift.as_ref().map(drift_json),
        });
        if verbose {
            data["why"] = json!(node.why);
            data["proves_ref"] = json!(node.proves_ref);
            data["post_conditions"] =
                json!(node.post_conditions.iter().map(|p| &p.0).collect::<Vec<_>>());
            // arêtes sortantes = gouvernance/identité que CE contrat porte (impact aval) ;
            // entrantes = qui pointe vers lui (impact amont, symétrique IST).
            data["governance_edges"] = json!(outgoing.iter().map(edge_json).collect::<Vec<_>>());
            data["impact_edges"] = json!(incoming.iter().map(edge_json).collect::<Vec<_>>());
        }

        let drift_label = drift
            .as_ref()
            .map(|d| match d {
                DriftVerdict::Aligned { .. } => "aligned",
                DriftVerdict::Unbound => "unbound",
                DriftVerdict::SymbolMissing { .. } => "symbol_missing",
                DriftVerdict::KindMismatch { .. } => "kind_mismatch",
                DriftVerdict::ShapeDrift { .. } => "shape_drift",
                DriftVerdict::NoBaseline { .. } => "no_baseline",
            })
            .unwrap_or("unknown");

        Some(json!({
            "content": [{"type":"text","text": format!(
                "### 📐 {id} — {} · {} · drift={}",
                node.kind.tag(), lifecycle, drift_label
            )}],
            "data": data
        }))
    }

    /// REQ-AXO-902095 (S8) — `contract_evolve` : transition d'évolution gouvernée.
    /// Slice 1 = `obsolete` ; `refactor`/`reorient` déclarés (slices ultérieures).
    pub(crate) fn axon_contract_evolve(&self, args: &Value) -> Option<Value> {
        let Some(id) = args
            .get("contract_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Some(contract_err("contract_id is required", "input_invalid"));
        };
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let node = match load_contract(&self.graph_store, id) {
            Ok(Some(n)) => n,
            Ok(None) => return Some(contract_err(&format!("contrat introuvable: {id}"), "input_not_found")),
            Err(e) => return Some(contract_err(&format!("load failed: {e}"), "degraded")),
        };
        match action.as_str() {
            "obsolete" => self.contract_evolve_obsolete(id, &node),
            "refactor" => self.contract_evolve_refactor(id),
            "reorient" => self.contract_evolve_reorient(id),
            _ => Some(contract_err("action must be one of obsolete|refactor|reorient", "input_invalid")),
        }
    }

    /// REQ-AXO-902095 (S8 refactor, DEC-AXO-901658 cas A) — VÉRIFIE qu'un changement
    /// comportement-préservé a gardé la frontière intacte : le sceau de frontière
    /// SURVIT si la forme IST-observée n'a pas dérivé de la baseline (reconcile =
    /// Aligned/NoBaseline/Unbound). Transition de VÉRIFICATION (aucune mutation) :
    /// si la frontière a dérivé, ce n'est pas un refactor → orienter vers reorient.
    fn contract_evolve_refactor(&self, id: &str) -> Option<Value> {
        let sealed = load_seal(&self.graph_store, id).ok().flatten().is_some();
        let drift = reconcile_contract(&self.graph_store, id).ok();
        let boundary_preserved = matches!(
            drift,
            Some(DriftVerdict::Aligned { .. })
                | Some(DriftVerdict::NoBaseline { .. })
                | Some(DriftVerdict::Unbound)
        );
        if boundary_preserved {
            Some(json!({
                "content": [{"type":"text","text": format!(
                    "### 📐 {id} — refactor OK : frontière préservée, sceau {} survit",
                    if sealed { "structurel" } else { "(non scellé)" }
                )}],
                "data": {"status":"ok","verdict":"boundary_preserved","id":id,"sealed":sealed,
                         "drift": drift.as_ref().map(drift_json)}
            }))
        } else {
            Some(json!({
                "content": [{"type":"text","text": format!(
                    "### 📐 {id} — PAS un refactor : la frontière a dérivé. Utilise `reorient` (intent-first) ou re_anchor si l'identité a bougé."
                )}],
                "data": {"status":"boundary_changed","verdict":"not_a_refactor","id":id,"sealed":sealed,
                         "drift": drift.as_ref().map(drift_json),
                         "next": {"tool":"contract_evolve","hint":"action=reorient si le changement d'intention est voulu"}}
            }))
        }
    }

    /// REQ-AXO-902095 (S8 reorient, DEC-AXO-901658 cas B) — intent-first : l'intention
    /// a délibérément changé → INVALIDE le sceau (clear_seal) + expose le blast-radius
    /// + exige une re-preuve. Le blast aval (contrat→contrat) est une slice future ;
    /// ici le rayon immédiat = ce contrat.
    fn contract_evolve_reorient(&self, id: &str) -> Option<Value> {
        let was_sealed = load_seal(&self.graph_store, id).ok().flatten().is_some();
        if let Err(e) = clear_seal(&self.graph_store, id) {
            return Some(contract_err(&format!("reorient failed: {e}"), "degraded"));
        }
        Some(json!({
            "content": [{"type":"text","text": format!(
                "### 📐 {id} — REORIENTED : sceau {} invalidé, re-preuve requise",
                if was_sealed { "structurel" } else { "(déjà absent)" }
            )}],
            "data": {"status":"ok","verdict":"reoriented","id":id,"seal_invalidated":was_sealed,
                     "blast_radius":[id],"needs_reproof":true,
                     "note":"re-sceller après re-preuve ; blast aval (contrat→contrat) = slice future",
                     "next": {"tool":"contract_status","hint":"inspecter le drift puis re-sceller une fois re-prouvé"}}
        }))
    }

    /// REQ-AXO-902095 (S8, DEC-AXO-901658 cas C) — obsolescence : tombstone
    /// `retired` (intent préservé), HARD-bloquée tant que des arêtes CALL entrantes
    /// vivantes pointent vers `realized_by` (retirer du code encore appelé =
    /// orphelins). No-op idempotent si déjà retired.
    fn contract_evolve_obsolete(&self, id: &str, node: &ContractNode) -> Option<Value> {
        if contract_status_str(&self.graph_store, id).ok().flatten().as_deref() == Some("retired") {
            return Some(json!({
                "content": [{"type":"text","text": format!("### 📐 {id} — déjà retired (no-op)")}],
                "data": {"status":"ok","verdict":"already_retired","id":id}
            }));
        }
        if let Some(sym) = &node.realized_by {
            let (live, sample) = live_incoming_call_count(&self.graph_store, sym).unwrap_or((0, Vec::new()));
            if live > 0 {
                return Some(json!({
                    "content": [{"type":"text","text": format!(
                        "### 📐 {id} — obsolescence BLOQUÉE : {live} appelant(s) vivant(s) de `{sym}`. Retire/redirige les appels entrants d'abord (impact/detect_remnants)."
                    )}],
                    "data": {"status":"blocked","verdict":"blocked_by_live_callers","id":id,
                             "symbol":sym,"live_callers":live,"sample_callers":sample,
                             "next":{"tool":"impact","hint":"impact symbol=<appelant> pour tracer puis retirer les arêtes CALL entrantes"}}
                }));
            }
        }
        if let Err(e) = retire_contract(&self.graph_store, id) {
            return Some(contract_err(&format!("retire failed: {e}"), "degraded"));
        }
        Some(json!({
            "content": [{"type":"text","text": format!(
                "### 📐 {id} — RETIRED (obsolescence ; tombstone, intention préservée)"
            )}],
            "data": {"status":"ok","verdict":"retired","id":id}
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::ContractKind;

    #[test]
    fn drift_json_maps_every_verdict_with_aligned_flag() {
        assert_eq!(drift_json(&DriftVerdict::Aligned { observed: "h".into() })["aligned"], json!(true));
        assert_eq!(
            drift_json(&DriftVerdict::ShapeDrift { baseline: "a".into(), observed: "b".into() })["aligned"],
            json!(false)
        );
        // Unbound / NoBaseline ne sont PAS des drifts → aligned=null (ni vrai ni faux).
        assert_eq!(drift_json(&DriftVerdict::Unbound)["aligned"], json!(null));
        assert_eq!(
            drift_json(&DriftVerdict::NoBaseline { observed: "h".into() })["aligned"],
            json!(null)
        );
        // KindMismatch projette le tag canonique du kind attendu.
        let km = drift_json(&DriftVerdict::KindMismatch {
            expected: ContractKind::Function,
            observed_kind: "struct".into(),
        });
        assert_eq!(km["verdict"], json!("kind_mismatch"));
        assert_eq!(km["expected"], json!("function"));
        assert_eq!(km["observed_kind"], json!("struct"));
    }

    #[test]
    fn edge_json_shape_is_source_relation_target() {
        let e = ContractEdgeRow {
            source_id: "CON-AXO-1".into(),
            relation_type: "SOLVES".into(),
            target_id: "REQ-AXO-262".into(),
        };
        assert_eq!(
            edge_json(&e),
            json!({"source": "CON-AXO-1", "relation": "SOLVES", "target": "REQ-AXO-262"})
        );
    }
}
