// REQ-AXO-91485 / DEC-AXO-097 — IstSnapshotCache.
//
// One ArcSwap per process holds the per-project snapshots. Readers grab the
// current Arc<HashMap<project_code, Arc<IstGraph>>> lock-free ; writers
// publish a new map atomically when a load lands. REQ-AXO-901952 made the
// RAM snapshot the SINGLE source for structural graph queries — the former
// `AXON_IST_RAM_ENABLED` client opt-out toggle is removed (RAM unconditional).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;

use crate::ist_snapshot::snapshot::IstGraph;

/// REQ-AXO-902005 — per-project rebuild coordination, kept in a side map so the
/// hot snapshot map value stays `Arc<IstGraph>` (zero churn on the read path /
/// view methods). `in_flight`/`dirty` drive single-flight coalescing: while a
/// rebuild runs, a fresh `ist_mutated` sets `dirty` instead of spawning a second
/// loader; the running rebuild re-runs once on finish.
#[derive(Default, Clone, Copy)]
struct ProjectState {
    in_flight: bool,
    dirty: bool,
}

/// Atomic per-project snapshot cache. Cloning the cache handle is cheap (one
/// `Arc` clone) ; the snapshots themselves never move once published.
pub struct IstSnapshotCache {
    inner: Arc<ArcSwap<HashMap<String, Arc<IstGraph>>>>,
    /// REQ-AXO-902005 — rebuild single-flight + freshness, keyed by project.
    state: Arc<Mutex<HashMap<String, ProjectState>>>,
}

impl Default for IstSnapshotCache {
    fn default() -> Self {
        Self::new()
    }
}

impl IstSnapshotCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ArcSwap::new(Arc::new(HashMap::new()))),
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            state: Arc::clone(&self.state),
        }
    }

    /// REQ-AXO-901952 — the IST RAM snapshot is the SINGLE source for
    /// structural graph queries (operator directive session 77, repeated 5×):
    /// no PG fallback, one query method. The former client opt-out toggle
    /// `AXON_IST_RAM_ENABLED` is removed — RAM is unconditional. Retained as a
    /// status reporter (always `true`) for the `ram_enabled` field surfaced by
    /// the ist_snapshot tools. Supersedes DEC-AXO-097 (IST RAM disable path).
    pub fn is_enabled() -> bool {
        true
    }

    pub fn get(&self, project_code: &str) -> Option<Arc<IstGraph>> {
        self.inner.load().get(project_code).cloned()
    }

    /// REQ-AXO-902625 — `rcu`, jamais `load` puis `store`.
    ///
    /// Le motif précédent — lire, cloner la carte ENTIÈRE, muter, écraser — perd
    /// les écritures concurrentes, et il les perd même sur des `project_code`
    /// DISJOINTS : ce n'est pas une collision de clé, c'est le `store` final qui
    /// remplace toute la carte, y compris les entrées qu'un voisin vient d'y
    /// mettre. Un *lost update* classique.
    ///
    /// Ce que ça cassait, mesuré : `cargo test --lib -- tools_context` rendait
    /// 28/1 avec un test PERDANT qui changeait d'un run à l'autre, chacun vert en
    /// isolation. Le partenaire de course n'était pas entre les tests RAM : c'est
    /// `ensure_ram_snapshot_warm` (tools_ist_snapshot.rs), déclenché par les 13
    /// tests qui construisent un `McpServer`. Et c'est AUSSI une course de
    /// production — `warm_all_ist_snapshots_at_boot` publie N projets pendant que
    /// des appels MCP publient en parallèle, donc un projet peut disparaître du
    /// cache en service.
    ///
    /// `rcu` boucle jusqu'à ce que le compare-and-swap réussisse : la closure est
    /// `FnMut` et peut être REJOUÉE, d'où les clones à chaque tentative.
    pub fn publish(&self, project_code: String, snapshot: Arc<IstGraph>) {
        self.inner.rcu(|current| {
            let mut next: HashMap<String, Arc<IstGraph>> = (**current).clone();
            next.insert(project_code.clone(), Arc::clone(&snapshot));
            next
        });
    }

    /// Voir `publish` — même défaut, même remède.
    ///
    /// L'ancien court-circuit « absent ⇒ ne rien faire » a disparu : il lisait la
    /// carte HORS du compare-and-swap, donc il pouvait décider sur un état périmé.
    /// Le prix est un clone quand il n'y a rien à retirer ; `evict` n'est pas un
    /// chemin chaud, et un raccourci qui rouvre la course ne vaut pas ce clone.
    pub fn evict(&self, project_code: &str) {
        self.inner.rcu(|current| {
            let mut next: HashMap<String, Arc<IstGraph>> = (**current).clone();
            next.remove(project_code);
            next
        });
    }

    pub fn project_codes(&self) -> Vec<String> {
        self.inner.load().keys().cloned().collect()
    }

    /// REQ-AXO-902005 — single-flight gate. Returns `true` when the caller wins
    /// the right to rebuild `project` (no rebuild was in flight). Returns
    /// `false` when a rebuild is already running — in that case the request is
    /// recorded as `dirty` so the in-flight rebuild re-runs once on completion,
    /// guaranteeing the snapshot reflects the latest mutation without spawning a
    /// second concurrent loader (no thundering herd).
    pub fn begin_rebuild(&self, project: &str) -> bool {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = guard.entry(project.to_string()).or_default();
        if st.in_flight {
            st.dirty = true;
            false
        } else {
            st.in_flight = true;
            st.dirty = false;
            true
        }
    }

    /// REQ-AXO-902005 — close out a rebuild. Returns `true` when a mutation
    /// landed during the rebuild (`dirty`): the caller must re-run the load to
    /// pick it up; `in_flight` is kept set so no other caller interleaves.
    /// Returns `false` when clean: `in_flight` is cleared.
    pub fn finish_rebuild(&self, project: &str) -> bool {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = guard.entry(project.to_string()).or_default();
        if st.dirty {
            st.dirty = false;
            true
        } else {
            st.in_flight = false;
            false
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ist_snapshot::snapshot::IstGraph;

    fn empty_snapshot() -> Arc<IstGraph> {
        Arc::new(IstGraph::build(vec![], vec![]))
    }

    #[test]
    fn cache_starts_empty() {
        let cache = IstSnapshotCache::new();
        assert!(cache.get("AXO").is_none());
        assert!(cache.project_codes().is_empty());
    }

    #[test]
    fn publish_then_get_returns_snapshot() {
        let cache = IstSnapshotCache::new();
        cache.publish("AXO".to_string(), empty_snapshot());
        assert!(cache.get("AXO").is_some());
        assert_eq!(cache.project_codes(), vec!["AXO".to_string()]);
    }

    #[test]
    fn publish_replaces_existing_project() {
        let cache = IstSnapshotCache::new();
        let first = empty_snapshot();
        let second = empty_snapshot();
        cache.publish("AXO".to_string(), Arc::clone(&first));
        cache.publish("AXO".to_string(), Arc::clone(&second));
        let got = cache.get("AXO").unwrap();
        assert!(Arc::ptr_eq(&got, &second));
        assert!(!Arc::ptr_eq(&got, &first));
    }

    #[test]
    fn evict_removes_project_without_affecting_others() {
        let cache = IstSnapshotCache::new();
        cache.publish("AXO".to_string(), empty_snapshot());
        cache.publish("OPT".to_string(), empty_snapshot());
        cache.evict("AXO");
        assert!(cache.get("AXO").is_none());
        assert!(cache.get("OPT").is_some());
    }

    #[test]
    fn handle_shares_same_arcswap() {
        let cache = IstSnapshotCache::new();
        let handle = cache.handle();
        cache.publish("AXO".to_string(), empty_snapshot());
        assert!(handle.get("AXO").is_some());
    }

    // REQ-AXO-902005 — single-flight coordinator.

    #[test]
    fn first_begin_rebuild_wins_second_marks_dirty() {
        let cache = IstSnapshotCache::new();
        assert!(cache.begin_rebuild("AXO"), "first caller wins the rebuild slot");
        assert!(
            !cache.begin_rebuild("AXO"),
            "second caller loses (rebuild already in flight)"
        );
        // The lost caller recorded dirty → finish must request a re-run.
        assert!(cache.finish_rebuild("AXO"), "dirty after concurrent request → re-run");
        // No further mutation → finish clears in_flight, next begin wins again.
        assert!(!cache.finish_rebuild("AXO"), "clean finish clears in_flight");
        assert!(cache.begin_rebuild("AXO"), "slot freed after clean finish");
    }

    #[test]
    fn rebuild_state_is_per_project() {
        let cache = IstSnapshotCache::new();
        assert!(cache.begin_rebuild("AXO"));
        // A different project is independent — it can start its own rebuild.
        assert!(cache.begin_rebuild("OPT"));
        assert!(!cache.finish_rebuild("AXO"));
        assert!(!cache.finish_rebuild("OPT"));
    }

    // -----------------------------------------------------------------------------
    // REQ-AXO-902625 — la course, et le MUTANT qui prouve qu'elle était réelle.
    // -----------------------------------------------------------------------------

    /// Deux écrivains sur des projets DISJOINTS doivent tous les deux survivre.
    ///
    /// C'est le cœur du défaut : on n'écrasait pas une clé partagée, on écrasait la
    /// CARTE. Le test lance assez d'écrivains et de tours pour que l'entrelacement
    /// se produise — un seul aller-retour ne reproduirait rien de fiable.
    #[test]
    fn deux_projets_DISJOINTS_publies_en_parallele_survivent_tous_les_deux() {
        use std::sync::Barrier;

        const ECRIVAINS: usize = 8;
        const TOURS: usize = 40;

        for _ in 0..TOURS {
            let cache = Arc::new(IstSnapshotCache::new());
            // La barrière fait partir tout le monde en même temps : sans elle, les
            // threads se sérialisent d'eux-mêmes et le test ne prouve rien.
            let depart = Arc::new(Barrier::new(ECRIVAINS));
            let mut mains = Vec::new();

            for n in 0..ECRIVAINS {
                let cache = Arc::clone(&cache);
                let depart = Arc::clone(&depart);
                mains.push(std::thread::spawn(move || {
                    depart.wait();
                    cache.publish(format!("P{n}"), empty_snapshot());
                }));
            }
            for main in mains {
                main.join().expect("un écrivain a paniqué");
            }

            let survivants = cache.project_codes().len();
            assert_eq!(
                survivants, ECRIVAINS,
                "{survivants} projet(s) sur {ECRIVAINS} ont survécu : une publication a été \
                 écrasée par une autre portant pourtant une clé DIFFÉRENTE"
            );
        }
    }

    /// `evict` ne doit pas emporter les voisins non plus.
    #[test]
    fn une_eviction_concurrente_n_emporte_pas_les_projets_voisins() {
        use std::sync::Barrier;

        for _ in 0..40 {
            let cache = Arc::new(IstSnapshotCache::new());
            cache.publish("GARDE".to_string(), empty_snapshot());
            cache.publish("JETE".to_string(), empty_snapshot());

            let depart = Arc::new(Barrier::new(2));
            let (c1, d1) = (Arc::clone(&cache), Arc::clone(&depart));
            let jeteur = std::thread::spawn(move || {
                d1.wait();
                c1.evict("JETE");
            });
            let (c2, d2) = (Arc::clone(&cache), Arc::clone(&depart));
            let poseur = std::thread::spawn(move || {
                d2.wait();
                c2.publish("NEUF".to_string(), empty_snapshot());
            });
            jeteur.join().expect("jeteur");
            poseur.join().expect("poseur");

            assert!(cache.get("GARDE").is_some(), "un projet intact a disparu");
            assert!(cache.get("NEUF").is_some(), "la publication concurrente a été perdue");
            assert!(cache.get("JETE").is_none(), "l'éviction n'a pas eu lieu");
        }
    }

    /// LE MUTANT — l'ANCIEN code, rejoué à l'identique sur la MÊME fixture.
    ///
    /// Un test d'absence doit fabriquer lui-même ce qu'il interdit (pratique 2169).
    /// Si `load`-cloner-muter-`store` ne perdait PAS de clé ici, les deux tests
    /// ci-dessus passeraient aussi bien sans le correctif et ne prouveraient rien.
    #[test]
    fn MUTANT_l_ancien_load_puis_store_perd_bien_une_ecriture() {
        use std::sync::Barrier;

        // L'ancien corps de `publish`, mot pour mot, sur la même structure.
        fn publish_ancien(
            inner: &ArcSwap<HashMap<String, Arc<IstGraph>>>,
            project_code: String,
            snapshot: Arc<IstGraph>,
        ) {
            let current = inner.load();
            let mut next: HashMap<String, Arc<IstGraph>> = (**current).clone();
            // Fenêtre explicite entre la lecture et l'écriture. Elle ne CRÉE pas le
            // défaut — elle le rend déterministe au lieu de dépendre de la chance de
            // l'ordonnanceur, ce qui est la seule façon d'en faire un test.
            std::thread::yield_now();
            next.insert(project_code, snapshot);
            inner.store(Arc::new(next));
        }

        const ECRIVAINS: usize = 8;
        let mut perte_observee = false;

        for _ in 0..200 {
            let inner: Arc<ArcSwap<HashMap<String, Arc<IstGraph>>>> =
                Arc::new(ArcSwap::new(Arc::new(HashMap::new())));
            let depart = Arc::new(Barrier::new(ECRIVAINS));
            let mut mains = Vec::new();

            for n in 0..ECRIVAINS {
                let inner = Arc::clone(&inner);
                let depart = Arc::clone(&depart);
                mains.push(std::thread::spawn(move || {
                    depart.wait();
                    publish_ancien(&inner, format!("P{n}"), empty_snapshot());
                }));
            }
            for main in mains {
                main.join().expect("écrivain");
            }

            if inner.load().len() < ECRIVAINS {
                perte_observee = true;
                break;
            }
        }

        assert!(
            perte_observee,
            "l'ancien `load` puis `store` n'a perdu AUCUNE écriture en 200 tours × \
             {ECRIVAINS} écrivains : la fixture ne reproduit pas la course, et les tests \
             de non-régression ci-dessus ne prouvent donc rien"
        );
    }
}
