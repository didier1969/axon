//! Process-global pipeline-stage health signal for self-diagnosis
//! (REQ-AXO-902047, PIL-AXO-9006).
//!
//! Bridges a stage's error state to two consumers without threading handles
//! through the pipeline wiring:
//!   1. the vector sorted-drain's **systemic-failure backoff** — when the B3
//!      persist stage is failing every batch (e.g. a corrupt index, a schema
//!      mismatch), re-embedding work that cannot be written is wasted CPU; the
//!      drain backs off instead of spinning at hundreds of % CPU (the
//!      REQ-AXO-902046 incident),
//!   2. future cross-process publication (slice 2) so `embedding_status` /
//!      `pipeline_health` can surface the real error to an LLM in one call.
//!
//! In-RAM, lock-free for the hot counters; the last-error text is behind a
//! `Mutex` updated only on the (rare, by design) error path, deduplicated by
//! message signature so a 7000×-repeated error is one record with a count, not
//! a log flood.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Deduplicated record of the most recent distinct error a stage produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StageErrorRecord {
    /// Full error text (anyhow alternate Display — the whole `caused by` chain
    /// on one line, so the root PG/SQLSTATE detail is preserved, not masked).
    pub message: String,
    /// How many consecutive times THIS exact message repeated.
    pub count: u64,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
}

/// Health signal for one persist stage (currently B3 — the embedding writer).
#[derive(Debug, Default)]
pub struct StageHealth {
    consecutive_failures: AtomicU64,
    total_failures: AtomicU64,
    total_successes: AtomicU64,
    last_error: Mutex<Option<StageErrorRecord>>,
    /// REQ-AXO-902630 — échecs consécutifs PAR TENANT, à côté du compteur
    /// global, jamais à sa place.
    ///
    /// `consecutive_failures` est un compteur unique que `record_success` remet
    /// à zéro : le succès de N'IMPORTE quel tenant efface l'échec de tous les
    /// autres. Un tenant dont 100 % des lots sont refusés, entrelacé avec des
    /// tenants sains, ne franchit donc JAMAIS le seuil systémique — la surface
    /// reste verte pendant que le tenant est mort. Mesuré le 2026-09-06 sur
    /// `REQ-AXO-902626` : MRG refusé sur `indexedfile_project_code_fkey`
    /// pendant que AXO / KKI / FSF s'indexaient normalement.
    ///
    /// Cette carte répond à « QUEL tenant », que le compteur global ne peut pas
    /// porter. Le compteur global et `is_systemically_failing` sont laissés
    /// INTACTS : ils pilotent le freinage du drain amont, qui n'est pas en
    /// cause ici (REQ-AXO-902402 est encore ouvert dessus).
    per_tenant_consecutive: Mutex<BTreeMap<String, u64>>,
}

impl StageHealth {
    /// Record a failure. Returns the new consecutive-failure count so the
    /// caller can throttle its log (e.g. warn on 1 + every Nth). Dedupes the
    /// stored `last_error` by message signature.
    pub fn record_failure(&self, message: impl Into<String>, now_ms: i64) -> u64 {
        let n = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        self.total_failures.fetch_add(1, Ordering::Relaxed);
        let msg = message.into();
        if let Ok(mut guard) = self.last_error.lock() {
            match guard.as_mut() {
                Some(rec) if rec.message == msg => {
                    rec.count = rec.count.saturating_add(1);
                    rec.last_seen_ms = now_ms;
                }
                _ => {
                    *guard = Some(StageErrorRecord {
                        message: msg,
                        count: 1,
                        first_seen_ms: now_ms,
                        last_seen_ms: now_ms,
                    });
                }
            }
        }
        n
    }

    /// Record a successful batch — resets the consecutive-failure counter so a
    /// transient blip does not latch the backoff.
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.total_successes.fetch_add(1, Ordering::Relaxed);
    }

    /// REQ-AXO-902630 — même chose que [`Self::record_failure`], en retenant
    /// AUSSI quel tenant a échoué. Le compteur global est mis à jour à
    /// l'identique : ce chemin n'enlève rien, il ajoute la granularité.
    pub fn record_failure_for(
        &self,
        project_code: &str,
        message: impl Into<String>,
        now_ms: i64,
    ) -> u64 {
        if let Ok(mut guard) = self.per_tenant_consecutive.lock() {
            let entry = guard.entry(project_code.to_string()).or_insert(0);
            *entry = entry.saturating_add(1);
        }
        self.record_failure(message, now_ms)
    }

    /// REQ-AXO-902630 — succès d'UN tenant : efface le compteur de CE tenant,
    /// et lui seul. Le compteur global est remis à zéro comme avant, parce que
    /// le freinage du drain raisonne sur le flux entier.
    pub fn record_success_for(&self, project_code: &str) {
        if let Ok(mut guard) = self.per_tenant_consecutive.lock() {
            guard.remove(project_code);
        }
        self.record_success();
    }

    /// REQ-AXO-902630 — les tenants dont les échecs consécutifs atteignent
    /// `threshold`, du plus atteint au moins atteint. Vide = personne.
    pub fn systemically_failing_tenants(&self, threshold: u64) -> Vec<(String, u64)> {
        let mut rows: Vec<(String, u64)> = self
            .per_tenant_consecutive
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .filter(|(_, count)| **count >= threshold)
                    .map(|(code, count)| (code.clone(), *count))
                    .collect()
            })
            .unwrap_or_default();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        rows
    }

    pub fn consecutive_failures(&self) -> u64 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }

    pub fn total_failures(&self) -> u64 {
        self.total_failures.load(Ordering::Relaxed)
    }

    pub fn total_successes(&self) -> u64 {
        self.total_successes.load(Ordering::Relaxed)
    }

    /// True once the stage has failed `threshold` times in a row with no
    /// intervening success — i.e. the failure is systemic (not a single poison
    /// row) and the upstream drain should back off.
    pub fn is_systemically_failing(&self, threshold: u64) -> bool {
        self.consecutive_failures() >= threshold
    }

    pub fn last_error(&self) -> Option<StageErrorRecord> {
        self.last_error.lock().ok().and_then(|g| g.clone())
    }

    /// REQ-AXO-902630 — instantané de la carte par tenant, non filtré.
    pub fn per_tenant_consecutive(&self) -> Vec<(String, u64)> {
        self.per_tenant_consecutive
            .lock()
            .map(|guard| guard.iter().map(|(k, v)| (k.clone(), *v)).collect())
            .unwrap_or_default()
    }

    /// REQ-AXO-902047 slice 1b — flat, owned snapshot of the health counters
    /// for cross-process publication (the indexer captures this on every
    /// heartbeat tick and UPSERTs it so the brain's `embedding_status` reads
    /// the real B3 error state in one MCP call, no log access).
    pub fn snapshot(&self) -> StageHealthSnapshot {
        StageHealthSnapshot {
            consecutive_failures: self.consecutive_failures(),
            total_failures: self.total_failures(),
            total_successes: self.total_successes(),
            last_error: self.last_error(),
            // REQ-AXO-902630 — la carte ENTIÈRE, non filtrée : le seuil n'est
            // pas le même pour A3 (3) et B3 (8), il appartient au lecteur.
            per_tenant_consecutive: self.per_tenant_consecutive(),
        }
    }
}

/// REQ-AXO-902047 slice 1b — owned, serializable snapshot of a [`StageHealth`].
/// Decoupled from the atomics so it can cross the process boundary (heartbeat
/// UPSERT) and be compared in tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StageHealthSnapshot {
    pub consecutive_failures: u64,
    pub total_failures: u64,
    pub total_successes: u64,
    pub last_error: Option<StageErrorRecord>,
    /// REQ-AXO-902630 — échecs consécutifs par tenant, non filtrés par seuil.
    pub per_tenant_consecutive: Vec<(String, u64)>,
}

impl StageHealthSnapshot {
    /// True once consecutive failures reach the systemic threshold — the same
    /// verdict the drain uses to back off, surfaced to readers as DEGRADED.
    pub fn is_systemically_failing(&self, threshold: u64) -> bool {
        self.consecutive_failures >= threshold
    }

    /// REQ-AXO-902630 — les tenants au-dessus du seuil, du plus atteint au
    /// moins atteint. C'est la question que `is_systemically_failing` ne peut
    /// pas poser : un tenant mort masqué par des tenants sains.
    pub fn systemically_failing_tenants(&self, threshold: u64) -> Vec<(String, u64)> {
        let mut rows: Vec<(String, u64)> = self
            .per_tenant_consecutive
            .iter()
            .filter(|(_, count)| *count >= threshold)
            .cloned()
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        rows
    }

    /// Error rate over the lifetime of the process: failures / (failures +
    /// successes). Returns 0.0 when no batch has been attempted yet.
    pub fn error_rate(&self) -> f64 {
        let total = self.total_failures + self.total_successes;
        if total == 0 {
            0.0
        } else {
            self.total_failures as f64 / total as f64
        }
    }
}

/// Consecutive-failure count at which B3 is judged systemically broken and the
/// drain backs off. 8 batches (~tens of seconds at production cadence) is long
/// enough to rule out a single transient flush, short enough to stop the CPU
/// hemorrhage quickly.
pub const B3_SYSTEMIC_FAILURE_THRESHOLD: u64 = 8;
pub const A3_SYSTEMIC_FAILURE_THRESHOLD: u64 = 3;

/// REQ-AXO-902630 — rendre les tenants en échec pour la colonne
/// `axon.indexer_runtime_truth.a3_failing_tenants`. `None` quand la liste est
/// vide : une colonne NULL dit « personne », une chaîne vide dirait « une
/// valeur qu'on n'a pas su écrire ».
pub fn render_failing_tenants(rows: &[(String, u64)]) -> Option<String> {
    if rows.is_empty() {
        return None;
    }
    serde_json::to_string(rows).ok()
}

/// REQ-AXO-902630 — lecture de la colonne. JSON, pas de découpe de chaîne
/// maison : un tenant ne peut pas contenir de virgule, mais s'en remettre à
/// cette hypothèse est exactement la classe de bug de la pratique 2177.
pub fn parse_failing_tenants(raw: Option<&str>) -> Vec<(String, u64)> {
    raw.filter(|text| !text.trim().is_empty())
        .and_then(|text| serde_json::from_str::<Vec<(String, u64)>>(text).ok())
        .unwrap_or_default()
}

static A3_HEALTH: OnceLock<StageHealth> = OnceLock::new();
static B3_HEALTH: OnceLock<StageHealth> = OnceLock::new();

pub fn a3_health() -> &'static StageHealth {
    A3_HEALTH.get_or_init(StageHealth::default)
}

/// Process-global B3 (embedding persist) health signal.
pub fn b3_health() -> &'static StageHealth {
    B3_HEALTH.get_or_init(StageHealth::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_failure_increments_consecutive_and_dedupes_same_message() {
        let h = StageHealth::default();
        assert_eq!(h.record_failure("boom", 10), 1);
        assert_eq!(h.record_failure("boom", 20), 2);
        assert_eq!(h.record_failure("boom", 30), 3);
        assert_eq!(h.consecutive_failures(), 3);
        assert_eq!(h.total_failures(), 3);
        let rec = h.last_error().unwrap();
        assert_eq!(rec.message, "boom");
        assert_eq!(rec.count, 3, "same message must dedupe into one record");
        assert_eq!(rec.first_seen_ms, 10);
        assert_eq!(rec.last_seen_ms, 30);
    }

    #[test]
    fn distinct_message_replaces_last_error_record() {
        let h = StageHealth::default();
        h.record_failure("missing chunk number 0 for toast value (XX001)", 1);
        h.record_failure("different vector dimensions 1024 and 0", 2);
        let rec = h.last_error().unwrap();
        assert_eq!(rec.message, "different vector dimensions 1024 and 0");
        assert_eq!(rec.count, 1);
    }

    #[test]
    fn record_success_resets_consecutive_but_keeps_totals() {
        let h = StageHealth::default();
        h.record_failure("x", 1);
        h.record_failure("x", 2);
        assert!(h.is_systemically_failing(2));
        h.record_success();
        assert_eq!(h.consecutive_failures(), 0);
        assert!(!h.is_systemically_failing(2));
        assert_eq!(h.total_failures(), 2);
        assert_eq!(h.total_successes(), 1);
    }

    #[test]
    fn snapshot_mirrors_live_counters_and_last_error() {
        let h = StageHealth::default();
        h.record_success();
        h.record_failure("missing chunk number 0 for toast value (XX001)", 42);
        let snap = h.snapshot();
        assert_eq!(snap.consecutive_failures, 1);
        assert_eq!(snap.total_failures, 1);
        assert_eq!(snap.total_successes, 1);
        assert_eq!(
            snap.last_error.as_ref().map(|r| r.message.as_str()),
            Some("missing chunk number 0 for toast value (XX001)")
        );
    }

    #[test]
    fn snapshot_error_rate_and_systemic_verdict() {
        let empty = StageHealthSnapshot::default();
        assert_eq!(empty.error_rate(), 0.0, "no batches → no error rate");
        assert!(!empty.is_systemically_failing(B3_SYSTEMIC_FAILURE_THRESHOLD));

        let snap = StageHealthSnapshot {
            consecutive_failures: B3_SYSTEMIC_FAILURE_THRESHOLD,
            total_failures: 3,
            total_successes: 1,
            last_error: None,
            per_tenant_consecutive: Vec::new(),
        };
        assert!(snap.is_systemically_failing(B3_SYSTEMIC_FAILURE_THRESHOLD));
        assert_eq!(snap.error_rate(), 0.75);
    }

    #[test]
    fn systemic_failure_latches_only_at_threshold() {
        let h = StageHealth::default();
        for i in 1..B3_SYSTEMIC_FAILURE_THRESHOLD {
            h.record_failure("e", i as i64);
            assert!(
                !h.is_systemically_failing(B3_SYSTEMIC_FAILURE_THRESHOLD),
                "must not latch before threshold"
            );
        }
        h.record_failure("e", 99);
        assert!(h.is_systemically_failing(B3_SYSTEMIC_FAILURE_THRESHOLD));
    }

    // ------------------------------------------------------------------
    // REQ-AXO-902630 — un tenant mort masqué par des tenants sains.
    // Scénario verbatim du 2026-09-06 : MRG refusé sur
    // `indexedfile_project_code_fkey`, AXO / KKI / FSF sains.
    // ------------------------------------------------------------------

    #[test]
    fn un_tenant_mort_reste_visible_quand_les_autres_reussissent() {
        let h = StageHealth::default();
        // Entrelacement réel : MRG échoue, un tenant sain réussit, et ainsi de
        // suite. Le compteur GLOBAL est remis à zéro à chaque succès.
        for i in 0..10 {
            h.record_failure_for("MRG", "23503 foreign key violation", i);
            h.record_success_for("AXO");
        }
        assert_eq!(
            h.consecutive_failures(),
            0,
            "le compteur global est remis à zéro par le tenant sain — c'est \
             exactement pour cela qu'il ne peut pas voir MRG"
        );
        assert!(
            !h.is_systemically_failing(A3_SYSTEMIC_FAILURE_THRESHOLD),
            "la surface d'AVANT reste verte : le défaut est bien celui-là"
        );
        assert_eq!(
            h.systemically_failing_tenants(A3_SYSTEMIC_FAILURE_THRESHOLD),
            vec![("MRG".to_string(), 10)],
            "la carte par tenant doit, elle, voir MRG"
        );
    }

    #[test]
    fn le_succes_d_un_tenant_n_efface_que_son_propre_compteur() {
        let h = StageHealth::default();
        for i in 0..4 {
            h.record_failure_for("MRG", "fk", i);
            h.record_failure_for("NXA", "fk", i);
        }
        h.record_success_for("MRG");
        assert_eq!(
            h.systemically_failing_tenants(A3_SYSTEMIC_FAILURE_THRESHOLD),
            vec![("NXA".to_string(), 4)],
            "MRG guéri sort de la liste, NXA y reste"
        );
    }

    #[test]
    fn sous_le_seuil_aucun_tenant_n_est_denonce() {
        let h = StageHealth::default();
        h.record_failure_for("MRG", "fk", 1);
        h.record_failure_for("MRG", "fk", 2);
        assert!(
            h.systemically_failing_tenants(A3_SYSTEMIC_FAILURE_THRESHOLD)
                .is_empty(),
            "2 < 3 : un hoquet n'est pas une panne"
        );
    }

    #[test]
    fn le_freinage_du_drain_n_est_pas_touche() {
        // Non-régression : `record_failure_for` doit alimenter le compteur
        // global EXACTEMENT comme `record_failure`. Le backoff amont
        // (REQ-AXO-902402 encore ouvert) ne doit rien voir de ce changement.
        let temoin = StageHealth::default();
        let sujet = StageHealth::default();
        for i in 0..A3_SYSTEMIC_FAILURE_THRESHOLD {
            temoin.record_failure("e", i as i64);
            sujet.record_failure_for("MRG", "e", i as i64);
        }
        assert_eq!(sujet.consecutive_failures(), temoin.consecutive_failures());
        assert_eq!(sujet.total_failures(), temoin.total_failures());
        assert_eq!(
            sujet.is_systemically_failing(A3_SYSTEMIC_FAILURE_THRESHOLD),
            temoin.is_systemically_failing(A3_SYSTEMIC_FAILURE_THRESHOLD)
        );
    }

    #[test]
    fn le_transport_json_fait_l_aller_retour() {
        let rows = vec![("MRG".to_string(), 12u64), ("NXA".to_string(), 5u64)];
        let rendu = render_failing_tenants(&rows).expect("une liste non vide se rend");
        assert_eq!(parse_failing_tenants(Some(&rendu)), rows);
        // Vide = NULL en colonne, pas une chaîne vide.
        assert!(render_failing_tenants(&[]).is_none());
        assert!(parse_failing_tenants(None).is_empty());
        assert!(parse_failing_tenants(Some("")).is_empty());
        assert!(
            parse_failing_tenants(Some("pas du json")).is_empty(),
            "une colonne illisible ne doit pas paniquer, seulement se taire"
        );
    }
}
