//! REQ-AXO-902111 / DEC-AXO-901662 — `promote_status` MCP tool (T1 read-only).
//!
//! Thin surface over [`crate::release_reconciler`]: collects release facts and
//! returns `{phase, observed, gates, failed_gates, next_action, recovery}` so an
//! agent reads the release truth in one call instead of grepping the promote
//! scripts. Read-only — it never mutates runtime or release state.

use serde_json::{json, Value};

use super::runtime_topology_support::{
    IndexerSupervisorObservation,
    resolve_indexer_liveness, EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
};
use super::McpServer;
use crate::release_reconciler::IstOwnershipFacts;
use crate::release_reconciler::{
    attempt_next_action, evaluate_attempt_gate, evaluate_gates, evaluate_liveness_gates,
    evaluate_supervisor_gates, liveness_next_action, liveness_phase, next_action, phase,
    LivenessFacts, ReleaseFacts, SupervisorFacts,
};
use crate::supervisor_probe;

impl McpServer {
    /// REQ-AXO-902585 (défaut 3) — interroger le superviseur, en tolérant strictement
    /// son absence : injoignable ⇒ champs vides et cause NOMMÉE, jamais un faux vert.
    ///
    /// L'échappatoire `AXON_PROMOTE_STATUS_SUPERVISOR_PROBE=0` désactive la sonde ;
    /// elle se déclare alors comme désactivée, donc impossible à lire comme saine.
    fn collect_supervisor_facts(&self, heartbeat_age_ms: Option<i64>) -> SupervisorFacts {
        let mut facts = SupervisorFacts {
            heartbeat_age_ms,
            ..Default::default()
        };
        if std::env::var("AXON_PROMOTE_STATUS_SUPERVISOR_PROBE").as_deref() == Ok("0") {
            facts.error = Some("probe disabled (AXON_PROMOTE_STATUS_SUPERVISOR_PROBE=0)".into());
            return facts;
        }
        let port = supervisor_probe::supervisor_port_from(
            std::env::var("AXON_SUPERVISOR_PORT").ok(),
            &crate::env_alias::read_with_alias_or("AXON_INSTANCE", "AXON_INSTANCE_KIND", "live"),
        );
        let addr = supervisor_probe::supervisor_addr(port);
        let body = match supervisor_probe::fetch_processes(
            addr,
            std::time::Duration::from_millis(supervisor_probe::SUPERVISOR_CONNECT_TIMEOUT_MS),
            std::time::Duration::from_millis(supervisor_probe::SUPERVISOR_READ_TIMEOUT_MS),
        ) {
            Ok(body) => body,
            Err(error) => {
                facts.error = Some(error);
                return facts;
            }
        };
        let processes = match supervisor_probe::parse_processes(&body) {
            Ok(processes) => processes,
            Err(error) => {
                facts.error = Some(error);
                return facts;
            }
        };
        facts.reachable = true;
        if let Some(p) = processes.iter().find(|p| p.name == "axon-indexer") {
            facts.role_found = true;
            facts.status = p.status.clone();
            facts.restarts = p.restarts;
            facts.pid = p.pid;
            facts.age_ms = p.age_ms();
            // REQ-AXO-902581 — les deux champs qui permettent de distinguer une fin
            // de passe d'un crash. Ils étaient déjà parsés par `parse_processes` et
            // jetés ici même.
            facts.exit_code = p.exit_code;
            facts.is_running = p.is_running;
        }
        facts
    }

    pub(crate) fn axon_promote_status(&self, _args: &Value) -> Option<Value> {
        let live_build_id = std::env::var("AXON_BUILD_ID").unwrap_or_default();
        let release_dir = std::env::current_dir()
            .unwrap_or_default()
            .join(".axon")
            .join("live-release");
        let facts = ReleaseFacts::collect(&release_dir, live_build_id);

        // REQ-AXO-902111 liveness slice — populate runtime liveness from the SAME
        // in-process sources `status` trusts (never the declared mode of this
        // brain_only process). Indexer = PG heartbeat → resolve_indexer_liveness;
        // brain = a real `SELECT 1` DB probe (catches up-but-DB-disconnected).
        let now_ms = crate::clock::now_unix_ms();
        let hb = self
            .graph_store
            .latest_lifecycle_heartbeat("indexer")
            .ok()
            .flatten();
        // REQ-AXO-902616 — sonder QUI tient le verrou d'écriture IST AVANT tout
        // verdict de vivacité, et renseigner les deux jeux de faits avec la MÊME
        // mesure : un battement frais écrit par un indexeur que le superviseur ne
        // suit plus n'est pas une vivacité, c'est une vivacité orpheline.
        let ist_ownership = probe_ist_ownership();
        let mut sup = self.collect_supervisor_facts(hb.as_ref().map(|r| now_ms - r.heartbeat_ms));
        sup.ist_ownership = ist_ownership.clone();
        // REQ-AXO-902581 — la sonde du superviseur passe AVANT le verdict de
        // vivacité, et le nourrit. Sur ce chemin la source qui sait est déjà
        // interrogée : l'ignorer pour inférer « battement périmé donc mort » était
        // gratuit ET faux. `None` quand la sonde n'a pas abouti ou n'a pas trouvé le
        // rôle — le verdict redevient alors `stopped_or_idle`/`inferred`, ce qui est
        // exactement ce qu'on sait.
        let observation = if sup.reachable && sup.role_found {
            Some(IndexerSupervisorObservation {
                status: sup.status.clone(),
                exit_code: sup.exit_code,
                is_running: sup.is_running,
            })
        } else {
            None
        };
        let live = resolve_indexer_liveness(
            now_ms,
            hb.as_ref().map(|r| r.heartbeat_ms),
            EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
            observation.as_ref(),
        );
        let supervised_pid = if sup.role_found && sup.pid > 0 {
            Some(sup.pid)
        } else {
            None
        };
        let lf = LivenessFacts {
            brain_serving: self.execute_raw_sql("SELECT 1").is_ok(),
            indexer_expected: facts.indexer_expected(),
            indexer_ready: live.ready,
            indexer_lifecycle: live.lifecycle.to_string(),
            indexer_source: live.source.to_string(),
            ist_ownership,
            supervised_pid,
        };

        let mut gates = evaluate_gates(&facts);
        gates.extend(evaluate_liveness_gates(&lf));
        // REQ-AXO-902585 (défaut 2) — la dernière tentative enregistrée a son mot à dire.
        gates.push(evaluate_attempt_gate(&facts));
        // REQ-AXO-902585 (défaut 3) — et le superviseur dit ce que le battement PG
        // ne peut pas dire : le compteur de redémarrages et l'âge du processus.
        //
        // ⚠ Ce gate rejoint `gates` ICI, et surtout PAS `evaluate_liveness_gates` :
        // `CutoverFacts::new_healthy()` exige que TOUS les gates de liveness passent,
        // et un `Unknown` y enverrait chaque cutover en auto-rollback.
        gates.extend(evaluate_supervisor_gates(&sup));
        // REQ-AXO-902585 — `failed_gates` ne porte que les gates VRAIMENT rouges.
        // Sûreté, pas style : `promote_live_safe.sh` teste `recon_failed ==
        // "indexer_alive"` par ÉGALITÉ EXACTE, et tout autre contenu le fait basculer
        // en redémarrage complet — « THIS INTERRUPTS THE BRAIN ». Un `Unknown` qui
        // fuiterait ici couperait le service à chaque promote sur un manifeste
        // ancien, c'est-à-dire presque toujours.
        let failed: Vec<&str> = gates.iter().filter(|g| g.is_red()).map(|g| g.name).collect();
        let unknown: Vec<&str> = gates
            .iter()
            .filter(|g| g.status == crate::release_reconciler::GateStatus::Unknown)
            .map(|g| g.name)
            .collect();
        // Liveness failures take precedence over the release-state phase/action.
        let ph = liveness_phase(&lf).unwrap_or_else(|| phase(&facts));
        // REQ-AXO-902585 — en DERNIER : un service à terre prime sur un déploiement
        // raté, mais un déploiement raté prime sur le silence.
        let action = liveness_next_action(&lf)
            .or_else(|| next_action(&facts))
            .or_else(|| attempt_next_action(&facts));
        let mut trace = facts.attempt.clone().unwrap_or_else(|| {
            json!({
                "release_attempt_id": facts.release_attempt_id.clone(),
                "phase": ph,
                "status": "unknown",
                "sha": Value::Null,
                "deadline_unix_ms": Value::Null,
                "last_event": Value::Null,
            })
        });
        if let Some(object) = trace.as_object_mut() {
            // REQ-AXO-902585 — dire si la tentative tracée est celle qui a produit le
            // manifeste servi. Sans ça, il fallait comparer deux ids à l'œil.
            object.insert(
                "manifest_release_attempt_id".to_string(),
                facts
                    .release_attempt_id
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
            object.insert(
                "diverges_from_manifest".to_string(),
                Value::Bool(
                    facts.attempt_id.is_some() && facts.attempt_id != facts.release_attempt_id,
                ),
            );
            object.insert(
                "artifact_sha256".to_string(),
                facts
                    .artifact_sha256
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
        }

        let gates_json: Vec<Value> = gates
            .iter()
            // REQ-AXO-902585 — `pass` reste, en lecture conservatrice (Unknown != succès),
            // et `status` porte le tri-état pour qui sait le lire.
            .map(|g| json!({ "name": g.name, "pass": g.passes(), "status": g.status_str(), "detail": g.detail }))
            .collect();

        let text = format!(
            "### 🚦 promote_status\n\nphase=**{}** — running={} manifest={}{}\nfailed_gates: {}\nnext_action: {}",
            ph,
            facts.live_build_id,
            facts.manifest_build_id.as_deref().unwrap_or("<none>"),
            if facts.pending_present {
                " · pending staging present"
            } else {
                ""
            },
            if failed.is_empty() {
                if unknown.is_empty() {
                    "none".to_string()
                } else {
                    format!("none (unknown: {})", unknown.join(", "))
                }
            } else {
                failed.join(", ")
            },
            action
                .as_deref()
                .unwrap_or("none — live matches the promoted manifest"),
        );

        Some(json!({
            "content": [{ "type": "text", "text": text }],
            "data": {
                "status": "ok",
                "phase": ph,
                "observed": {
                    "live_build_id": facts.live_build_id,
                    "manifest_build_id": facts.manifest_build_id,
                    "manifest_state": facts.manifest_state,
                    "core_qualification_status": facts.core_qualification_status,
                    "core_qualification_evidence": facts.core_qualification_evidence,
                    "qualification_source": facts.qualification_source,
                    "pending_present": facts.pending_present,
                    "pending_build_id": facts.pending_build_id,
                    "runtime_contract": facts.runtime_contract,
                    "release_attempt_id": facts.release_attempt_id,
                    "pending_release_attempt_id": facts.pending_release_attempt_id,
                    "artifact_sha256": facts.artifact_sha256,
                    "liveness": {
                        "brain_serving": lf.brain_serving,
                        "indexer_expected": lf.indexer_expected,
                        "indexer_ready": lf.indexer_ready,
                        "indexer_lifecycle": lf.indexer_lifecycle,
                        "indexer_source": lf.indexer_source,
                        // REQ-AXO-902585 — quadri-état en CHAÎNE, jamais un booléen
                        // qui se lirait « false » quand la vérité est « je ne sais pas ».
                        "indexer_restart_loop": sup.restart_loop_label(),
                        "supervisor": {
                            "source": "process_compose_http",
                            "reachable": sup.reachable,
                            "error": sup.error,
                            "role_found": sup.role_found,
                            "status": sup.status,
                            "pid": sup.pid,
                            "restarts": sup.restarts,
                            "process_age_ms": sup.age_ms,
                        },
                    },
                },
                "gates": gates_json,
                "trace": trace,
                "failed_gates": failed,
                // REQ-AXO-902585 — les gates qui n'ont PAS PU mesurer, tenus à part
                // des rouges : ce sont deux verdicts différents, et un seul des deux
                // doit déclencher une reprise.
                "unknown_gates": unknown,
                "next_action": action,
                // REQ-AXO-902256 — `promote_live.sh` is deleted; the resume path is a
                // re-run of promote_live_safe.sh, which detects the stranded pending.json
                // and replays the cutover on that build's candidate manifest (and the
                // REQ-AXO-902258 byte check runs on the way through, so a resume cannot
                // re-commit a wrong binary). Handing an operator a command that names a
                // deleted script is the kind of stale instruction this session spent hours
                // paying for.
                "recovery": {
                    "resume": "bash scripts/release/promote_live_safe.sh --project AXO   # auto-resumes a stranded pending via the cutover",
                    "re_promote": "bash scripts/release/promote_live_safe.sh --project AXO",
                    "rollback": "bash scripts/release/rollback_live.sh",
                    "direct_executor": "bin/axonctl cutover --project-root . --instance-kind live --manifest <candidate.json> --max-polls 120 --poll-interval-ms 2000"
                }
            }
        }))
    }
}

/// REQ-AXO-902616 — qui tient le verrou d'écriture IST, mesuré et non supposé.
///
/// L'autorité est le flock lui-même (`guard_liveness_ist`), jamais `/proc/<pid>` :
/// ce dernier répond vrai pour un zombie, et c'est exactement l'erreur que
/// `REQ-AXO-902157` a corrigée côté bash. Le `pid=` de la métadonnée sert
/// uniquement à NOMMER le propriétaire, jamais à juger de sa vivacité.
///
/// Une sonde qui échoue rend `Default` — « non mesuré » — et surtout pas
/// « personne ne tient » : c'est la confusion que `REQ-AXO-902616` supprime.
///
/// ⚠ Quand le verrou est libre, la sonde le prend et le relâche aussitôt
/// (mécanique de `guard_liveness`, REQ-AXO-902157). Un indexeur qui démarrerait
/// dans cette fenêtre de quelques microsecondes serait refusé et relancé par le
/// superviseur ; le coût est borné et le bénéfice — savoir QUI tient — ne
/// s'obtient pas autrement.
fn probe_ist_ownership() -> IstOwnershipFacts {
    probe_ist_ownership_at(&crate::runtime_writer_guard::resolve_db_root())
}

/// REQ-AXO-902616 — la sonde, paramétrée par sa racine pour être testable.
/// Le câblage EST le défaut qu'on répare : une sonde qu'aucun test ne peut
/// exécuter est une sonde dont on ne sait pas si elle est branchée
/// (`GUI-PRO-115`).
fn probe_ist_ownership_at(db_root: &str) -> IstOwnershipFacts {
    use crate::runtime_writer_guard::{guard_liveness_ist, parse_recorded_owner};
    let (held, recorded) = match guard_liveness_ist(db_root) {
        Ok(crate::runtime_writer_guard::GuardLiveness::Free { recorded_owner }) => {
            (false, recorded_owner)
        }
        Ok(crate::runtime_writer_guard::GuardLiveness::HeldByLiveProcess { recorded_owner }) => {
            (true, recorded_owner)
        }
        Err(_) => return IstOwnershipFacts::default(),
    };
    let (owner_identity, owner_pid) = recorded
        .as_deref()
        .map(parse_recorded_owner)
        .unwrap_or((None, None));
    IstOwnershipFacts {
        probed: true,
        held_by_live_process: held,
        owner_pid,
        owner_identity,
    }
}

/// REQ-AXO-902616 — la sonde est branchée, et elle ne suppose rien.
#[cfg(test)]
mod ist_ownership_probe_tests {
    use super::probe_ist_ownership_at;
    use crate::runtime_writer_guard::WriterGuard;
    use tempfile::tempdir;

    #[test]
    fn sans_verrou_la_sonde_mesure_et_dit_libre() {
        let dir = tempdir().expect("tempdir");
        let facts = probe_ist_ownership_at(dir.path().to_str().unwrap());
        assert!(facts.probed, "l'absence de verrou est une MESURE, pas un échec");
        assert!(!facts.held_by_live_process);
        assert_eq!(facts.label(Some(1)), "free");
    }

    #[test]
    fn un_verrou_tenu_par_un_processus_vivant_est_vu_et_nomme() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().to_str().unwrap().to_string();
        let _guard = WriterGuard::acquire_ist(&root).expect("acquisition");
        let facts = probe_ist_ownership_at(&root);
        assert!(facts.probed);
        assert!(
            facts.held_by_live_process,
            "un flock tenu se voit : c'est toute la question de REQ-AXO-902616"
        );
        assert_eq!(
            facts.owner_pid,
            Some(std::process::id() as i64),
            "et le propriétaire est NOMMÉ, pas supposé"
        );
        // Le processus supervisé est quelqu'un d'autre : divergence.
        assert_eq!(facts.label(Some(1)), "diverged");
        // Et lui-même : pas de divergence.
        assert_eq!(facts.label(Some(std::process::id() as i64)), "supervised");
    }

    #[test]
    fn une_racine_en_memoire_ne_fabrique_aucun_proprietaire() {
        let facts = probe_ist_ownership_at(":memory:");
        assert!(!facts.held_by_live_process);
        assert!(facts.owner_pid.is_none());
    }
}
