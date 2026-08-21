use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeTuningState {
    pub vector_workers: usize,
    pub graph_workers: usize,
    pub chunk_batch_size: usize,
    pub file_vectorization_batch_size: usize,
    pub vector_ready_queue_depth: usize,
    pub vector_persist_queue_bound: usize,
    pub vector_max_inflight_persists: usize,
    pub embed_micro_batch_max_items: usize,
    pub embed_micro_batch_max_total_tokens: usize,
    pub semantic_sleep_scale_pct: usize,
    pub semantic_idle_sleep_scale_pct: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeTuningSnapshot {
    pub version: u64,
    pub state: RuntimeTuningState,
}

static RUNTIME_TUNING_SNAPSHOT: OnceLock<Mutex<Option<RuntimeTuningSnapshot>>> = OnceLock::new();

fn runtime_tuning_snapshot_slot() -> &'static Mutex<Option<RuntimeTuningSnapshot>> {
    RUNTIME_TUNING_SNAPSHOT.get_or_init(|| Mutex::new(None))
}

pub fn normalize_runtime_tuning_state(mut state: RuntimeTuningState) -> RuntimeTuningState {
    state.vector_workers = state.vector_workers.max(1);
    state.graph_workers = state.graph_workers.clamp(0, 64);
    state.chunk_batch_size = state.chunk_batch_size.max(16);
    state.file_vectorization_batch_size = state.file_vectorization_batch_size.max(4);
    state.vector_ready_queue_depth = state.vector_ready_queue_depth.max(1);
    state.vector_persist_queue_bound = state.vector_persist_queue_bound.max(1);
    state.vector_max_inflight_persists = state
        .vector_max_inflight_persists
        .max(1)
        .min(state.vector_persist_queue_bound);
    state.embed_micro_batch_max_items = state.embed_micro_batch_max_items.max(8);
    state.embed_micro_batch_max_total_tokens = state.embed_micro_batch_max_total_tokens.max(512);
    state.semantic_sleep_scale_pct = state.semantic_sleep_scale_pct.clamp(25, 400);
    state.semantic_idle_sleep_scale_pct = state.semantic_idle_sleep_scale_pct.clamp(25, 400);
    state
}

/// REQ-AXO-902415 — which of the two things actually happened.
///
/// The distinction is not cosmetic: REQ-AXO-902414 burned three refuted
/// hypotheses and four full test suites because nothing in the signature, the
/// name, or the return value said whether the bootstrap had been honoured or
/// silently dropped. The symptom pointed the opposite way from the cause — the
/// environment variables were correctly set, and a probe watching them would
/// have come back empty without exonerating anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuningOrigin {
    /// The slot was empty: the bootstrap was normalized and RETAINED.
    Bootstrapped,
    /// The slot was already filled: the bootstrap was never asked for.
    Inherited,
}

/// REQ-AXO-902415 — resolve the process-wide tuning snapshot, taking a way to
/// BUILD a bootstrap rather than a bootstrap.
///
/// The previous shape (`fn(bootstrap: RuntimeTuningState)`) had two faults that
/// share one root — it demanded a value it might not use, and never said which:
///
/// * **The caller paid, every time.** `bootstrap_runtime_tuning_state_from_env`
///   reads a dozen environment variables plus the whole lane config. That ran on
///   EVERY call, including the embed batch path (`embedder.rs`, right after
///   `encode_batch`, twice per batch), and `get_or_insert` threw it away on all
///   but the first. Not a correctness bug — waste, and the reason the argument
///   existed at all. `FnOnce` means it is computed only when it is needed.
/// * **Nobody could tell.** Hence [`TuningOrigin`] in the return.
///
/// The name says what it does: it RESOLVES a snapshot, bootstrapping only if
/// there is nothing to inherit.
pub fn resolve_runtime_tuning_snapshot(
    bootstrap: impl FnOnce() -> RuntimeTuningState,
) -> (RuntimeTuningSnapshot, TuningOrigin) {
    resolve_in_slot(runtime_tuning_snapshot_slot(), bootstrap)
}

/// The resolution itself, against a slot passed in.
///
/// REQ-AXO-902415 — the process-wide slot is a `OnceLock` that no test can
/// empty, and emptying it would race every other test in the binary anyway. A
/// guard whose INPUT cannot be substituted cannot be falsified: it would only
/// ever observe whichever branch the rest of the suite happened to leave behind.
/// Taking the slot as a parameter makes both branches reachable on demand —
/// including the one that matters most, "the bootstrap was never called".
fn resolve_in_slot(
    slot: &Mutex<Option<RuntimeTuningSnapshot>>,
    bootstrap: impl FnOnce() -> RuntimeTuningState,
) -> (RuntimeTuningSnapshot, TuningOrigin) {
    let mut guard = slot.lock().unwrap_or_else(|poison| poison.into_inner());
    match *guard {
        Some(existing) => (existing, TuningOrigin::Inherited),
        None => {
            let snapshot = RuntimeTuningSnapshot {
                version: 1,
                state: normalize_runtime_tuning_state(bootstrap()),
            };
            *guard = Some(snapshot);
            (snapshot, TuningOrigin::Bootstrapped)
        }
    }
}

pub fn resolve_runtime_tuning_state(
    bootstrap: impl FnOnce() -> RuntimeTuningState,
) -> RuntimeTuningState {
    resolve_runtime_tuning_snapshot(bootstrap).0.state
}

/// REQ-AXO-902415 — same treatment as [`resolve_runtime_tuning_snapshot`], and
/// for the same reason: this `get_or_insert` also discards `bootstrap` whenever
/// the slot is already filled, which after startup is always. Leaving one of the
/// two doors with the old shape would have left the trap in place for whoever
/// reads this file next.
#[allow(clippy::too_many_arguments)]
pub fn update_runtime_tuning_state(
    bootstrap: impl FnOnce() -> RuntimeTuningState,
    vector_workers: Option<usize>,
    graph_workers: Option<usize>,
    chunk_batch_size: Option<usize>,
    file_vectorization_batch_size: Option<usize>,
    vector_ready_queue_depth: Option<usize>,
    vector_persist_queue_bound: Option<usize>,
    vector_max_inflight_persists: Option<usize>,
    embed_micro_batch_max_items: Option<usize>,
    embed_micro_batch_max_total_tokens: Option<usize>,
    semantic_sleep_scale_pct: Option<usize>,
    semantic_idle_sleep_scale_pct: Option<usize>,
) -> RuntimeTuningSnapshot {
    let slot = runtime_tuning_snapshot_slot();
    let mut guard = slot.lock().unwrap_or_else(|poison| poison.into_inner());
    let current = guard.get_or_insert_with(|| RuntimeTuningSnapshot {
        version: 1,
        state: bootstrap(),
    });
    let mut next = current.state;
    if let Some(value) = vector_workers {
        next.vector_workers = value.max(1);
    }
    if let Some(value) = graph_workers {
        next.graph_workers = value;
    }
    if let Some(value) = chunk_batch_size {
        next.chunk_batch_size = value.max(1);
    }
    if let Some(value) = file_vectorization_batch_size {
        next.file_vectorization_batch_size = value.max(1);
    }
    if let Some(value) = vector_ready_queue_depth {
        next.vector_ready_queue_depth = value.max(1);
    }
    if let Some(value) = vector_persist_queue_bound {
        next.vector_persist_queue_bound = value.max(1);
    }
    if let Some(value) = vector_max_inflight_persists {
        next.vector_max_inflight_persists = value.max(1);
    }
    if let Some(value) = embed_micro_batch_max_items {
        next.embed_micro_batch_max_items = value.max(1);
    }
    if let Some(value) = embed_micro_batch_max_total_tokens {
        next.embed_micro_batch_max_total_tokens = value.max(1);
    }
    if let Some(value) = semantic_sleep_scale_pct {
        next.semantic_sleep_scale_pct = value.max(1);
    }
    if let Some(value) = semantic_idle_sleep_scale_pct {
        next.semantic_idle_sleep_scale_pct = value.max(1);
    }
    next = normalize_runtime_tuning_state(next);
    if next != current.state {
        current.version = current.version.saturating_add(1);
        current.state = next;
    }
    *current
}

#[cfg(test)]
pub fn reset_runtime_tuning_snapshot(bootstrap: RuntimeTuningState) -> RuntimeTuningSnapshot {
    let snapshot = RuntimeTuningSnapshot {
        version: 1,
        state: normalize_runtime_tuning_state(bootstrap),
    };
    let slot = runtime_tuning_snapshot_slot();
    *slot.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(snapshot);
    snapshot
}

/// REQ-AXO-902415 — l'API exigeait un `bootstrap`, le calculait, puis le jetait
/// en silence quand l'emplacement était déjà rempli.
///
/// Ces tests portent sur `resolve_in_slot` avec un emplacement à eux : la
/// résolution est exactement la même fonction que celle du processus, mais les
/// DEUX branches deviennent atteignables à volonté. Contre l'emplacement global
/// on ne pourrait observer que celle que le reste de la suite a laissée.
#[cfg(test)]
mod resolution_says_which_branch_it_took {
    use super::*;
    use std::cell::Cell;

    fn a_state(vector_workers: usize) -> RuntimeTuningState {
        RuntimeTuningState {
            vector_workers,
            graph_workers: 2,
            chunk_batch_size: 32,
            file_vectorization_batch_size: 8,
            vector_ready_queue_depth: 24,
            vector_persist_queue_bound: 64,
            vector_max_inflight_persists: 4,
            embed_micro_batch_max_items: 16,
            embed_micro_batch_max_total_tokens: 4096,
            semantic_sleep_scale_pct: 100,
            semantic_idle_sleep_scale_pct: 100,
        }
    }

    #[test]
    fn an_empty_slot_bootstraps_and_says_so() {
        let slot = Mutex::new(None);
        let called = Cell::new(false);

        let (snapshot, origin) = resolve_in_slot(&slot, || {
            called.set(true);
            a_state(7)
        });

        assert_eq!(origin, TuningOrigin::Bootstrapped);
        assert!(
            called.get(),
            "contrôle positif : sur un emplacement vide le bootstrap DOIT être \
             demandé, sinon l'assertion suivante ne mesure rien"
        );
        assert_eq!(snapshot.state.vector_workers, 7, "la valeur fournie est retenue");
        assert_eq!(snapshot.version, 1);
    }

    #[test]
    fn a_filled_slot_never_asks_for_the_bootstrap() {
        // LE test. L'ancienne signature prenait une VALEUR : l'appelant payait
        // le calcul — une douzaine de lectures d'environnement plus la config de
        // voie, deux fois par lot d'embed — et `get_or_insert` le jetait. Aucune
        // signature prenant une valeur ne peut exprimer cette assertion.
        let slot = Mutex::new(Some(RuntimeTuningSnapshot {
            version: 3,
            state: a_state(11),
        }));
        let called = Cell::new(false);

        let (snapshot, origin) = resolve_in_slot(&slot, || {
            called.set(true);
            a_state(7)
        });

        assert_eq!(origin, TuningOrigin::Inherited);
        assert!(
            !called.get(),
            "l'emplacement était rempli : le bootstrap ne doit même pas être \
             CALCULÉ. C'est le gaspillage que REQ-AXO-902415 retire, et le seul \
             endroit où il est observable."
        );
        assert_eq!(
            snapshot.state.vector_workers, 11,
            "c'est l'instantané en place qui est rendu, pas le bootstrap"
        );
        assert_eq!(snapshot.version, 3, "la version en place est préservée");
    }

    /// REQ-AXO-902414 — le défaut que cette forme a coûté : trois hypothèses
    /// réfutées et quatre suites complètes, parce que rien ne disait laquelle
    /// des deux branches avait été prise. Le symptôme pointait à l'opposé de la
    /// cause — les variables d'environnement étaient correctement posées, et une
    /// sonde qui les surveillait serait revenue vide sans rien disculper.
    #[test]
    fn the_two_branches_are_distinguishable_from_the_return_value_alone() {
        let empty = Mutex::new(None);
        let filled = Mutex::new(Some(RuntimeTuningSnapshot {
            version: 1,
            state: a_state(11),
        }));

        let (_, first) = resolve_in_slot(&empty, || a_state(7));
        let (_, second) = resolve_in_slot(&empty, || a_state(7));
        let (_, third) = resolve_in_slot(&filled, || a_state(7));

        assert_ne!(
            first, second,
            "le MEME appel, deux fois, ne fait pas la même chose — et c'est ce \
             que l'ancienne valeur de retour taisait"
        );
        assert_eq!(second, third, "les deux héritages sont indiscernables entre eux");
    }
}
