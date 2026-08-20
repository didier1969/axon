//! REQ-AXO-902387 — VRAM pressure on the B2 embed lane, measured as a RATIO.
//!
//! RÉUTILISE : `pipeline::stage_health::StageHealth` pour le patron (compteur de
//! stage process-wide derrière un `OnceLock`, exposé par `embedding_status`) ;
//! `pipeline::embedder_gpu::is_gpu_allocation_failure` pour la classification de
//! l'erreur. La MESURE elle-même n'existait pas : vérifié via `axon query
//! "compteur de pression VRAM ou ratio de bascule CPU sur fenêtre glissante"` —
//! `StageHealth` compte des échecs CUMULÉS depuis le boot (`total_failures`,
//! `consecutive_failures`, `error_rate`), ce qui ne peut pas exprimer « quelle
//! FRACTION des lots RÉCENTS est dégradée », et ne porte aucun plafond appris.
//!
//! # Pourquoi un ratio, et pas un compte
//!
//! Le 2026-08-20, le débit d'embed s'est effondré d'un facteur ~1000 sans qu'une
//! seule erreur soit levée : REQ-AXO-902373 venait de faire retomber un échec
//! d'allocation GPU sur le lane CPU au lieu de perdre le lot, donc chaque lot
//! prenait silencieusement le chemin lent. `b3_health` restait HEALTHY (il
//! surveille l'ÉCRITURE), et `embedding_status` ne rapportait aucun échec — parce
//! qu'il n'y en avait plus.
//!
//! Un COMPTE cumulé de bascules ne peut pas dire ça. Il n'a pas de sens sans le
//! nombre de lots qui ont tourné, et un total depuis le boot noie un incident
//! récent dans l'histoire ancienne. La jauge qui répond à « le lane GPU
//! fonctionne-t-il MAINTENANT » est la fraction des lots RÉCENTS qui ont basculé.
//!
//! # La fenêtre
//!
//! Les [`WINDOW`] derniers lots, dans les bits d'un seul `AtomicU64` — bit posé =
//! ce lot a basculé sur CPU. Sans verrou, sans allocation, sans horloge, et toute
//! l'histoire tient dans un load atomique. 64 lots ≈ 1-2 minutes de drain sain :
//! un changement de régime apparaît en quelques minutes et un incident ancien
//! s'efface tout seul.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;

/// Nombre de lots récents sur lesquels le ratio est calculé.
pub const WINDOW: u32 = 64;

/// Ratio de bascule à partir duquel le lane est DÉGRADÉ : le GPU refuse une part
/// significative des lots et le débit est déjà bien sous le nominal.
pub const DEGRADED_RATIO: f64 = 0.25;

/// Ratio de bascule à partir duquel le lane est CRITIQUE : le lane GPU est hors
/// service et le drain tourne sur CPU. C'est le régime du 2026-08-20, où le ratio
/// observé valait 1,0.
pub const CRITICAL_RATIO: f64 = 0.75;

/// En dessous de cette taille, retailler n'a plus de sens : à un texte par
/// inférence la demande d'arène est déjà minimale, donc un échec là n'est plus un
/// problème de dimensionnement.
pub const MIN_GPU_BATCH: usize = 1;

/// Succès consécutifs AU plafond courant avant de le remonter.
///
/// Remonter au PREMIER succès oscille : la demande d'arène croît avec
/// `jetons × taille`, et le réservoir trie par `token_count` croissant — donc la
/// composition des lots dérive vers des morceaux plus longs et une taille qui
/// passait cesse de passer. Chaque oscillation coûte un lot CPU entier, c'est-à-dire
/// exactement le coût que ce mécanisme existe pour éviter.
pub const SUCCESSES_BEFORE_RAISE: u64 = 32;

/// Vue instantanée de la pression VRAM du lane B2. Publique parce que
/// `embedding_status` et `status` la rendent : une dégradation de cette ampleur
/// doit être lisible depuis la surface MCP, pas seulement dans les journaux — c'est
/// toute la leçon de l'incident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbedPressureSnapshot {
    /// Lots comptés dans la fenêtre (< `WINDOW` juste après le démarrage).
    pub observed: u32,
    /// Parmi eux, combien sont retombés sur le lane CPU.
    pub cpu_fallbacks: u32,
    /// Plus grand lot que le GPU accepte actuellement ; `None` = aucun plafond
    /// appris (rien n'a encore échoué).
    pub gpu_batch_cap: Option<usize>,
    /// Combien de fois un lot a dû être découpé parce que le GPU refusait sa taille.
    pub resizes: u64,
    /// Combien de fois la session ORT a dû être RECRÉÉE pour rendre son arène.
    /// L'arène BFC croît de façon monotone et ne libère jamais : passé un certain
    /// point, aucune taille de lot ne passe plus, et seule une session neuve
    /// récupère la VRAM. Un compte qui monte tout seul dit que le budget est trop
    /// serré pour la charge — c'est le signal à donner à l'opérateur.
    pub session_recycles: u64,
    /// Totaux depuis le boot, pour que l'appelant calcule un taux s'il le veut.
    pub gpu_batches_total: u64,
    pub cpu_batches_total: u64,
}

impl EmbedPressureSnapshot {
    /// Fraction des lots récents servis sur CPU. `None` tant que rien n'a été
    /// observé — une fenêtre vide n'est PAS une pression nulle, et la rendre comme
    /// 0,0 serait exactement le bug de verdict vacuous que ce dépôt paie en boucle
    /// (REQ-AXO-902384).
    pub fn cpu_fallback_ratio(&self) -> Option<f64> {
        if self.observed == 0 {
            return None;
        }
        Some(f64::from(self.cpu_fallbacks) / f64::from(self.observed))
    }

    /// `healthy` / `degraded` / `critical` / `not_armed`. Le dénominateur
    /// ([`Self::observed`]) est toujours disponible à côté.
    pub fn verdict(&self) -> &'static str {
        match self.cpu_fallback_ratio() {
            None => "not_armed",
            Some(r) if r >= CRITICAL_RATIO => "critical",
            Some(r) if r >= DEGRADED_RATIO => "degraded",
            Some(_) => "healthy",
        }
    }
}

/// Fenêtre glissante + plafond de lot appris, pour le lane B2 d'un processus.
#[derive(Debug, Default)]
pub struct EmbedPressure {
    /// Bit i = le i-ème lot le plus récent a basculé sur CPU.
    window: AtomicU64,
    /// Lots jamais enregistrés, saturé à `WINDOW` pour servir de dénominateur.
    observed: AtomicU64,
    /// Plus grand lot accepté par le GPU ; 0 = aucun plafond appris.
    gpu_batch_cap: AtomicUsize,
    /// Succès GPU consécutifs depuis la dernière baisse ou hausse du plafond.
    successes_at_cap: AtomicU64,
    resizes: AtomicU64,
    session_recycles: AtomicU64,
    gpu_batches_total: AtomicU64,
    cpu_batches_total: AtomicU64,
}

impl EmbedPressure {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, fell_back: bool) {
        // Décale la fenêtre et grave le lot le plus récent en bit 0.
        let mut prev = self.window.load(Ordering::Relaxed);
        loop {
            let next = (prev << 1) | u64::from(fell_back);
            match self
                .window
                .compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(actual) => prev = actual,
            }
        }
        let seen = self.observed.load(Ordering::Relaxed);
        if seen < u64::from(WINDOW) {
            self.observed.store(seen + 1, Ordering::Relaxed);
        }
    }

    /// Un lot de `size` textes a été servi sur le GPU.
    pub fn record_gpu_batch(&self, size: usize) {
        self.gpu_batches_total.fetch_add(1, Ordering::Relaxed);
        self.push(false);
        // Seuls les succès AU plafond courant comptent pour le remonter : qu'un lot
        // plus petit passe ne dit rien sur le fait qu'un plus grand passerait.
        let cap = self.gpu_batch_cap.load(Ordering::Relaxed);
        if cap != 0 && size >= cap {
            self.successes_at_cap.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Un lot n'a pas pu être servi sur GPU même à [`MIN_GPU_BATCH`] et a été
    /// recalculé sur CPU.
    pub fn record_cpu_batch(&self) {
        self.cpu_batches_total.fetch_add(1, Ordering::Relaxed);
        self.push(true);
    }

    /// La session ORT a été recréée pour rendre son arène saturée.
    pub fn record_session_recycle(&self) {
        self.session_recycles.fetch_add(1, Ordering::Relaxed);
    }

    /// Le GPU a refusé `size` ; retenir le plafond plus bas.
    pub fn record_resize(&self, new_cap: usize) {
        self.resizes.fetch_add(1, Ordering::Relaxed);
        self.gpu_batch_cap
            .store(new_cap.max(MIN_GPU_BATCH), Ordering::Relaxed);
        self.successes_at_cap.store(0, Ordering::Relaxed);
    }

    /// Plus grand lot à tenter sur GPU maintenant, pour un appelant qui demande
    /// `requested`. Ne dépasse jamais ce qui est demandé.
    pub fn effective_batch_cap(&self, requested: usize) -> usize {
        let cap = self.gpu_batch_cap.load(Ordering::Relaxed);
        if cap == 0 {
            return requested;
        }
        // Une série soutenue de succès au plafond mérite une remontée prudente : la
        // pression qui l'avait fait baisser a pu disparaître (un voisin a rendu sa
        // VRAM).
        if self.successes_at_cap.load(Ordering::Relaxed) >= SUCCESSES_BEFORE_RAISE {
            let raised = cap.saturating_mul(2);
            self.gpu_batch_cap.store(raised, Ordering::Relaxed);
            self.successes_at_cap.store(0, Ordering::Relaxed);
            return raised.min(requested);
        }
        cap.min(requested)
    }

    pub fn snapshot(&self) -> EmbedPressureSnapshot {
        let observed = u32::try_from(self.observed.load(Ordering::Relaxed)).unwrap_or(WINDOW);
        let mask = if observed >= 64 {
            u64::MAX
        } else {
            (1u64 << observed) - 1
        };
        let cap = self.gpu_batch_cap.load(Ordering::Relaxed);
        EmbedPressureSnapshot {
            observed,
            cpu_fallbacks: (self.window.load(Ordering::Relaxed) & mask).count_ones(),
            gpu_batch_cap: (cap != 0).then_some(cap),
            resizes: self.resizes.load(Ordering::Relaxed),
            session_recycles: self.session_recycles.load(Ordering::Relaxed),
            gpu_batches_total: self.gpu_batches_total.load(Ordering::Relaxed),
            cpu_batches_total: self.cpu_batches_total.load(Ordering::Relaxed),
        }
    }
}

static B2_PRESSURE: OnceLock<EmbedPressure> = OnceLock::new();

/// Jauge de pression B2 du processus (miroir de `stage_health::b3_health`).
pub fn b2_pressure() -> &'static EmbedPressure {
    B2_PRESSURE.get_or_init(EmbedPressure::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_window_is_not_armed_never_healthy() {
        let p = EmbedPressure::new();
        let snap = p.snapshot();
        assert_eq!(snap.observed, 0);
        assert_eq!(snap.cpu_fallback_ratio(), None);
        // Tout l'enjeu : « rien mesuré » ne doit JAMAIS se rendre comme « tout va bien ».
        assert_eq!(snap.verdict(), "not_armed");
    }

    #[test]
    fn all_gpu_batches_read_healthy() {
        let p = EmbedPressure::new();
        for _ in 0..10 {
            p.record_gpu_batch(8);
        }
        let snap = p.snapshot();
        assert_eq!(snap.observed, 10);
        assert_eq!(snap.cpu_fallbacks, 0);
        assert_eq!(snap.verdict(), "healthy");
    }

    #[test]
    fn the_2026_08_20_regime_reads_critical() {
        // Chaque lot sur CPU : l'incident qu'aucun signal n'exprimait.
        let p = EmbedPressure::new();
        for _ in 0..20 {
            p.record_cpu_batch();
        }
        let snap = p.snapshot();
        assert_eq!(snap.cpu_fallback_ratio(), Some(1.0));
        assert_eq!(snap.verdict(), "critical");
    }

    #[test]
    fn window_ages_out_an_old_incident() {
        let p = EmbedPressure::new();
        for _ in 0..64 {
            p.record_cpu_batch();
        }
        assert_eq!(p.snapshot().verdict(), "critical");
        // Une fenêtre pleine de reprises doit ramener le verdict à healthy — un
        // compteur cumulé ne le ferait jamais.
        for _ in 0..64 {
            p.record_gpu_batch(8);
        }
        let snap = p.snapshot();
        assert_eq!(snap.cpu_fallbacks, 0);
        assert_eq!(snap.verdict(), "healthy");
        // Les totaux depuis le boot se souviennent des deux régimes.
        assert_eq!(snap.cpu_batches_total, 64);
        assert_eq!(snap.gpu_batches_total, 64);
    }

    #[test]
    fn a_partial_window_reports_its_own_denominator() {
        // 2 bascules sur 4 lots = 0,5, pas 2/64 : le dénominateur est ce qui a été
        // observé, jamais la taille nominale de la fenêtre.
        let p = EmbedPressure::new();
        p.record_cpu_batch();
        p.record_gpu_batch(8);
        p.record_cpu_batch();
        p.record_gpu_batch(8);
        let snap = p.snapshot();
        assert_eq!(snap.observed, 4);
        assert_eq!(snap.cpu_fallbacks, 2);
        assert_eq!(snap.cpu_fallback_ratio(), Some(0.5));
        // 0,5 : au-dessus du seuil dégradé, en dessous du critique.
        assert_eq!(snap.verdict(), "degraded");
    }

    #[test]
    fn cap_lowers_on_resize_and_bounds_the_request() {
        let p = EmbedPressure::new();
        assert_eq!(p.effective_batch_cap(64), 64, "aucun plafond appris");
        p.record_resize(16);
        assert_eq!(p.effective_batch_cap(64), 16);
        assert_eq!(p.effective_batch_cap(8), 8, "ne dépasse jamais la demande");
    }

    #[test]
    fn a_single_success_does_not_raise_the_cap() {
        // La garde anti-oscillation : réussir à 16, remonter à 32, échouer,
        // redescendre à 16, recommencer — chaque cycle coûtant un lot CPU entier.
        let p = EmbedPressure::new();
        p.record_resize(16);
        p.record_gpu_batch(16);
        assert_eq!(p.effective_batch_cap(64), 16);
    }

    #[test]
    fn a_sustained_run_of_successes_raises_the_cap() {
        let p = EmbedPressure::new();
        p.record_resize(16);
        for _ in 0..SUCCESSES_BEFORE_RAISE {
            p.record_gpu_batch(16);
        }
        assert_eq!(p.effective_batch_cap(64), 32, "remontée prudente après série");
        // Et le compteur repart de zéro : la hausse suivante exige sa propre série.
        p.record_gpu_batch(32);
        assert_eq!(p.effective_batch_cap(64), 32);
    }

    #[test]
    fn successes_below_the_cap_do_not_earn_a_raise() {
        // Qu'un petit lot passe ne dit rien sur le fait qu'un plus grand passerait.
        let p = EmbedPressure::new();
        p.record_resize(16);
        for _ in 0..SUCCESSES_BEFORE_RAISE * 2 {
            p.record_gpu_batch(4);
        }
        assert_eq!(p.effective_batch_cap(64), 16);
    }

    #[test]
    fn resize_never_drops_below_one() {
        let p = EmbedPressure::new();
        p.record_resize(0);
        assert_eq!(p.effective_batch_cap(64), MIN_GPU_BATCH);
    }
}
