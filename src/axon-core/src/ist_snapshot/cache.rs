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
pub const DEFAULT_CACHE_CAPACITY: usize = 16;
pub const DEFAULT_TTL_SECS: u64 = 1800; // 30 minutes

/// REQ-AXO-902005 / REQ-AXO-902647 — per-project rebuild coordination and LRU/TTL access tracking.
#[derive(Clone, Copy)]
struct ProjectState {
    in_flight: bool,
    dirty: bool,
    last_accessed: std::time::Instant,
    inserted_at: std::time::Instant,
}

impl Default for ProjectState {
    fn default() -> Self {
        let now = std::time::Instant::now();
        Self {
            in_flight: false,
            dirty: false,
            last_accessed: now,
            inserted_at: now,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct IstCacheStats {
    pub capacity: usize,
    pub ttl_secs: u64,
    pub cached_count: usize,
    pub cached_projects: Vec<String>,
}

/// Atomic per-project snapshot cache with LRU capacity & TTL eviction (REQ-AXO-902647).
pub struct IstSnapshotCache {
    inner: Arc<ArcSwap<HashMap<String, Arc<IstGraph>>>>,
    /// REQ-AXO-902005 — rebuild single-flight + freshness + LRU/TTL timestamps, keyed by project.
    state: Arc<Mutex<HashMap<String, ProjectState>>>,
    capacity: usize,
    ttl: std::time::Duration,
}

impl Default for IstSnapshotCache {
    fn default() -> Self {
        Self::new()
    }
}

impl IstSnapshotCache {
    pub fn new() -> Self {
        let capacity = std::env::var("AXON_IST_SNAPSHOT_CACHE_CAPACITY")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_CACHE_CAPACITY);
        let ttl_secs = std::env::var("AXON_IST_SNAPSHOT_TTL_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_TTL_SECS);
        Self::with_policy(capacity, std::time::Duration::from_secs(ttl_secs))
    }

    pub fn with_policy(capacity: usize, ttl: std::time::Duration) -> Self {
        Self {
            inner: Arc::new(ArcSwap::new(Arc::new(HashMap::new()))),
            state: Arc::new(Mutex::new(HashMap::new())),
            capacity,
            ttl,
        }
    }

    pub fn handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            state: Arc::clone(&self.state),
            capacity: self.capacity,
            ttl: self.ttl,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn ttl(&self) -> std::time::Duration {
        self.ttl
    }

    pub fn cache_stats(&self) -> IstCacheStats {
        let projects = self.project_codes();
        IstCacheStats {
            capacity: self.capacity,
            ttl_secs: self.ttl.as_secs(),
            cached_count: projects.len(),
            cached_projects: projects,
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
        let snap = self.inner.load().get(project_code).cloned()?;

        // REQ-AXO-902647 — Check TTL expiration if active
        if self.ttl > std::time::Duration::ZERO {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = state.get(project_code) {
                if entry.last_accessed.elapsed() > self.ttl {
                    drop(state);
                    self.evict(project_code);
                    return None;
                }
            }
        }

        // Update last_accessed timestamp for LRU
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let entry = state.entry(project_code.to_string()).or_default();
            entry.last_accessed = std::time::Instant::now();
        }

        Some(snap)
    }

    /// REQ-AXO-902625 / REQ-AXO-902647 — `rcu`, jamais `load` puis `store`.
    /// Éviction LRU automatique lorsque la capacité est dépassée, suivie de `malloc_trim`.
    pub fn publish(&self, project_code: String, snapshot: Arc<IstGraph>) {
        let now = std::time::Instant::now();
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let entry = state.entry(project_code.clone()).or_default();
            entry.last_accessed = now;
            entry.inserted_at = now;
        }

        let mut evicted_keys: Vec<String> = Vec::new();
        self.inner.rcu(|current| {
            let mut next: HashMap<String, Arc<IstGraph>> = (**current).clone();
            next.insert(project_code.clone(), Arc::clone(&snapshot));

            evicted_keys.clear();
            if self.capacity > 0 && next.len() > self.capacity {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let mut candidates: Vec<(String, std::time::Instant)> = next
                    .keys()
                    .filter(|k| *k != &project_code)
                    .map(|k| {
                        let last = state.get(k).map(|s| s.last_accessed).unwrap_or(now);
                        (k.clone(), last)
                    })
                    .collect();
                candidates.sort_by_key(|(_, last)| *last);
                let to_remove = next.len().saturating_sub(self.capacity);
                for (k, _) in candidates.into_iter().take(to_remove) {
                    next.remove(&k);
                    evicted_keys.push(k);
                }
            }
            next
        });

        if !evicted_keys.is_empty() {
            {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                for k in &evicted_keys {
                    state.remove(k);
                }
            }
            crate::runtime_observability::malloc_trim_system_allocator();
        }
    }

    /// REQ-AXO-902647 — explicit eviction with glibc malloc_trim.
    pub fn evict(&self, project_code: &str) -> bool {
        let mut removed = false;
        self.inner.rcu(|current| {
            let mut next: HashMap<String, Arc<IstGraph>> = (**current).clone();
            if next.remove(project_code).is_some() {
                removed = true;
            }
            next
        });
        if removed {
            {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                state.remove(project_code);
            }
            crate::runtime_observability::malloc_trim_system_allocator();
        }
        removed
    }

    /// REQ-AXO-902647 — clear all cached snapshots and trim system allocator.
    pub fn evict_all(&self) -> usize {
        let mut count = 0;
        self.inner.rcu(|current| {
            count = current.len();
            HashMap::new()
        });
        if count > 0 {
            {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                state.clear();
            }
            crate::runtime_observability::malloc_trim_system_allocator();
        }
        count
    }

    /// REQ-AXO-902647 — prune snapshots that exceeded inactivity TTL.
    pub fn prune_expired(&self) -> usize {
        if self.ttl == std::time::Duration::ZERO {
            return 0;
        }
        let expired_keys: Vec<String> = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .iter()
                .filter(|(_, s)| s.last_accessed.elapsed() > self.ttl)
                .map(|(k, _)| k.clone())
                .collect()
        };
        if expired_keys.is_empty() {
            return 0;
        }
        let mut pruned = 0;
        self.inner.rcu(|current| {
            let mut next: HashMap<String, Arc<IstGraph>> = (**current).clone();
            for k in &expired_keys {
                if next.remove(k).is_some() {
                    pruned += 1;
                }
            }
            next
        });
        if pruned > 0 {
            {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                for k in &expired_keys {
                    state.remove(k);
                }
            }
            crate::runtime_observability::malloc_trim_system_allocator();
        }
        pruned
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
        assert!(
            cache.begin_rebuild("AXO"),
            "first caller wins the rebuild slot"
        );
        assert!(
            !cache.begin_rebuild("AXO"),
            "second caller loses (rebuild already in flight)"
        );
        // The lost caller recorded dirty → finish must request a re-run.
        assert!(
            cache.finish_rebuild("AXO"),
            "dirty after concurrent request → re-run"
        );
        // No further mutation → finish clears in_flight, next begin wins again.
        assert!(
            !cache.finish_rebuild("AXO"),
            "clean finish clears in_flight"
        );
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
            assert!(
                cache.get("NEUF").is_some(),
                "la publication concurrente a été perdue"
            );
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

    // -----------------------------------------------------------------------------
    // REQ-AXO-902647 — LRU & TTL cache eviction + explicit evict & cache stats
    // -----------------------------------------------------------------------------

    #[test]
    fn req_902647_lru_eviction_when_capacity_exceeded() {
        use std::time::Duration;

        // Cache limité à 2 projets avec TTL infini (1h)
        let cache = IstSnapshotCache::with_policy(2, Duration::from_secs(3600));
        cache.publish("P1".to_string(), empty_snapshot());
        cache.publish("P2".to_string(), empty_snapshot());

        assert!(cache.get("P1").is_some(), "P1 présent");
        assert!(cache.get("P2").is_some(), "P2 présent");
        assert_eq!(cache.project_codes().len(), 2);

        // Insertion d'un 3ème projet P3: P1 est le plus ancien accédé car P2 a été accédé par get("P2")
        cache.publish("P3".to_string(), empty_snapshot());

        assert_eq!(
            cache.project_codes().len(),
            2,
            "La taille du cache doit être bornée à la capacité (2)"
        );
        assert!(
            cache.get("P1").is_none(),
            "P1 doit avoir été évincé car LRU"
        );
        assert!(cache.get("P2").is_some(), "P2 doit être présent");
        assert!(cache.get("P3").is_some(), "P3 doit être présent");

        // Toucher P2 pour que P3 devienne le LRU
        assert!(cache.get("P2").is_some());

        // Insertion de P4: P3 doit être évincé car P2 a été accédé plus récemment
        cache.publish("P4".to_string(), empty_snapshot());
        assert_eq!(cache.project_codes().len(), 2);
        assert!(
            cache.get("P3").is_none(),
            "P3 doit avoir été évincé car LRU"
        );
        assert!(cache.get("P2").is_some(), "P2 doit être préservé");
        assert!(cache.get("P4").is_some(), "P4 doit être présent");
    }

    #[test]
    fn req_902647_ttl_expiration_eviction_and_prune() {
        use std::time::Duration;

        // Cache avec TTL très court (50 ms)
        let cache = IstSnapshotCache::with_policy(10, Duration::from_millis(50));
        cache.publish("SHORT".to_string(), empty_snapshot());
        assert!(cache.get("SHORT").is_some(), "SHORT présent immédiatement");

        std::thread::sleep(Duration::from_millis(60));

        // get() sur une entrée expirée doit évincer et renvoyer None
        assert!(
            cache.get("SHORT").is_none(),
            "SHORT doit avoir expiré par TTL"
        );
        assert_eq!(
            cache.project_codes().len(),
            0,
            "Cache nettoyé après get expiré"
        );

        // Test de prune_expired
        cache.publish("PRUNE1".to_string(), empty_snapshot());
        cache.publish("PRUNE2".to_string(), empty_snapshot());
        assert_eq!(cache.project_codes().len(), 2);

        std::thread::sleep(Duration::from_millis(60));
        let pruned = cache.prune_expired();
        assert_eq!(pruned, 2, "prune_expired doit évincer 2 entrées expirées");
        assert_eq!(
            cache.project_codes().len(),
            0,
            "Cache vide après prune_expired"
        );
    }

    #[test]
    fn req_902647_explicit_evict_and_evict_all() {
        use std::time::Duration;

        let cache = IstSnapshotCache::with_policy(10, Duration::from_secs(3600));
        cache.publish("A".to_string(), empty_snapshot());
        cache.publish("B".to_string(), empty_snapshot());
        cache.publish("C".to_string(), empty_snapshot());

        assert!(cache.evict("B"), "B doit être retiré et retourner true");
        assert!(!cache.evict("B"), "Second evict sur B doit retourner false");
        assert!(cache.get("B").is_none());
        assert!(cache.get("A").is_some());
        assert!(cache.get("C").is_some());

        let count = cache.evict_all();
        assert_eq!(
            count, 2,
            "evict_all doit avoir retiré les 2 projets restants (A et C)"
        );
        assert_eq!(cache.project_codes().len(), 0);
        assert!(cache.get("A").is_none());
        assert!(cache.get("C").is_none());
    }

    #[test]
    fn req_902647_cache_stats_reporting() {
        use std::time::Duration;

        let cache = IstSnapshotCache::with_policy(5, Duration::from_secs(1200));
        cache.publish("STAT1".to_string(), empty_snapshot());
        cache.publish("STAT2".to_string(), empty_snapshot());

        let stats = cache.cache_stats();
        assert_eq!(stats.capacity, 5);
        assert_eq!(stats.ttl_secs, 1200);
        assert_eq!(stats.cached_count, 2);
        assert!(stats.cached_projects.contains(&"STAT1".to_string()));
        assert!(stats.cached_projects.contains(&"STAT2".to_string()));
    }
}
