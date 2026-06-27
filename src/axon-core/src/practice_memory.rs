//! REQ-AXO-902131 — governed cross-tenant best-practice memory: the pure
//! governance math (Physarum trust + FSRS decay + prune + stagnation monitor).
//!
//! This module is DB-free and side-effect-free so the curves are unit-tested
//! without a runtime (same discipline as `mailbox.rs`). The DB ops live in the
//! `tools_practice` MCP handlers; here are only the formulas that decide how a
//! practice's trust and retrievability evolve as it is used, reinforced, and aged.
//!
//! Ported from the proven Nexus lesson-loop (DEC-NEX-008): `LessonWeights` (N4
//! Physarum reinforcement) → [`reinforce_trust`]/[`decay_trust`]; FSRS atrophy (P5
//! replaceability) → [`retrievability`]/[`update_stability`]; prune → [`should_prune`];
//! `MetaMonitor` (P6) → [`assess_stagnation`].

/// FSRS forgetting-curve constants (v4/v5 power-law, as cited by MemArchitect in
/// REQ-NEX-024). `R(t,S) = (1 + FACTOR·t/S)^DECAY`.
pub const FSRS_FACTOR: f32 = 19.0 / 81.0;
pub const FSRS_DECAY: f32 = -0.5;

/// Physarum conductivity coupling: a tube (practice) reinforces with the flux
/// (useful recalls) through it and atrophies otherwise (`dD/dt = f(|Q|) − μ·D`).
pub const TRUST_REINFORCE_GAIN: f32 = 0.30;
pub const TRUST_DECAY_MU: f32 = 0.05;
pub const TRUST_FLOOR: f32 = 0.0;
pub const TRUST_CEIL: f32 = 1.0;

/// Prune thresholds (prune = mark `status='pruned'`, never DELETE).
pub const PRUNE_TRUST_FLOOR: f32 = 0.08;
pub const PRUNE_RETRIEVABILITY_FLOOR: f32 = 0.05;
/// Grace period: a fresh, never-used contribution is protected from pruning.
pub const PRUNE_GRACE_DAYS: f32 = 14.0;

/// FSRS retrievability ∈ (0,1] = the "live relevance" of a practice: how recallable
/// it still is `days_since_use` days after its last use, given its `stability`.
/// Monotonically DECREASING in time and INCREASING in stability.
pub fn retrievability(days_since_use: f32, stability: f32) -> f32 {
    let t = days_since_use.max(0.0);
    (1.0 + FSRS_FACTOR * t / stability.max(1e-3)).powf(FSRS_DECAY)
}

/// Update FSRS stability on reinforcement. A harder-to-recall practice that still
/// proved useful gets the biggest stability boost (spacing effect); a forgetting
/// signal (`usefulness < 0.5`) shrinks stability so the practice ages out faster.
/// MVP form (not a 19-weight fit — we lack the dataset; the product value is
/// "used + useful ⇒ lasts longer"), bounded to [0.1, 3650] days.
pub fn update_stability(stability: f32, retrievability: f32, usefulness: f32) -> f32 {
    let s = stability.max(0.1);
    let r = retrievability.clamp(0.0, 1.0);
    if usefulness >= 0.5 {
        (s * (1.0 + (1.0 - r) * (1.0 + usefulness))).clamp(0.1, 3650.0)
    } else {
        (s * (0.4 + 0.4 * usefulness)).clamp(0.1, 3650.0)
    }
}

/// Physarum reinforcement on use+feedback (flux through the tube). `usefulness ∈
/// [0,1]`: >0.5 strengthens trust, <0.5 thins it; logistic-bounded so trust stays
/// in [0,1] and gains shrink as trust approaches the ceiling.
pub fn reinforce_trust(trust: f32, usefulness: f32) -> f32 {
    let t = trust.clamp(TRUST_FLOOR, TRUST_CEIL);
    let u = usefulness.clamp(0.0, 1.0);
    let headroom = if u >= 0.5 { (1.0 - t).max(0.05) } else { t.max(0.05) };
    let delta = TRUST_REINFORCE_GAIN * (2.0 * u - 1.0) * headroom;
    (t + delta).clamp(TRUST_FLOOR, TRUST_CEIL)
}

/// Natural atrophy of an unused tube on a tick, modulated by retrievability: a
/// practice that is still retrievable barely decays; a forgotten one thins fast.
/// Always ≤ the input trust.
pub fn decay_trust(trust: f32, retrievability: f32) -> f32 {
    let t = trust.clamp(TRUST_FLOOR, TRUST_CEIL);
    let r = retrievability.clamp(0.0, 1.0);
    (t * (1.0 - TRUST_DECAY_MU * (1.0 - r))).clamp(TRUST_FLOOR, TRUST_CEIL)
}

/// Prune decision: a practice whose trust AND retrievability have both collapsed is
/// replaceable. Fresh never-used contributions are protected for `PRUNE_GRACE_DAYS`.
pub fn should_prune(trust: f32, retrievability: f32, use_count: i32, age_days: f32) -> bool {
    if age_days < PRUNE_GRACE_DAYS && use_count == 0 {
        return false;
    }
    trust < PRUNE_TRUST_FLOOR || retrievability < PRUNE_RETRIEVABILITY_FLOOR
}

/// Meta-monitor verdict (P6): the store stops improving when write/reinforce churn
/// is low AND mean trust is flat over the window.
#[derive(Debug, Clone)]
pub struct StagnationVerdict {
    pub stagnating: bool,
    pub churn: f32,
    pub mean_trust: f32,
    pub reason: String,
}

/// Assess stagnation from 30-day activity counters + the mean-trust delta.
pub fn assess_stagnation(
    adds_30d: i32,
    prunes_30d: i32,
    reinforces_30d: i32,
    mean_trust: f32,
    mean_trust_prev: f32,
) -> StagnationVerdict {
    let churn = (adds_30d + prunes_30d + reinforces_30d) as f32;
    let trust_delta = (mean_trust - mean_trust_prev).abs();
    let stagnating = churn < 3.0 && trust_delta < 0.02;
    StagnationVerdict {
        stagnating,
        churn,
        mean_trust,
        reason: if stagnating {
            "low churn + flat trust → restructure / seed new practices".to_string()
        } else {
            "healthy turnover".to_string()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrievability_decreases_in_time_and_increases_in_stability() {
        let s = 10.0;
        assert!(retrievability(0.0, s) >= 0.999); // just-used ≈ 1.0
        assert!(retrievability(1.0, s) > retrievability(30.0, s)); // monotone ↓ in t
        assert!(retrievability(30.0, 50.0) > retrievability(30.0, 5.0)); // ↑ in S
        let r = retrievability(30.0, s);
        assert!((0.0..=1.0).contains(&r));
    }

    #[test]
    fn reinforce_trust_is_bounded_and_directional() {
        // success raises, failure lowers, both stay in [0,1].
        assert!(reinforce_trust(0.5, 1.0) > 0.5);
        assert!(reinforce_trust(0.5, 0.0) < 0.5);
        assert!(reinforce_trust(0.99, 1.0) <= 1.0);
        assert!(reinforce_trust(0.01, 0.0) >= 0.0);
        // a useful recall (0.55) nudges up; a useless one (0.2) nudges down.
        assert!(reinforce_trust(0.5, 0.55) > 0.5);
    }

    #[test]
    fn decay_trust_never_increases() {
        for &r in &[0.0_f32, 0.3, 0.7, 1.0] {
            assert!(decay_trust(0.8, r) <= 0.8);
        }
        // a fully-retrievable practice barely decays; a forgotten one decays more.
        assert!(decay_trust(0.8, 1.0) > decay_trust(0.8, 0.0));
    }

    #[test]
    fn update_stability_grows_on_success_shrinks_on_forget() {
        let s = 10.0;
        let r = retrievability(5.0, s);
        assert!(update_stability(s, r, 1.0) >= s); // success grows
        assert!(update_stability(s, r, 0.0) < s); // forget shrinks
    }

    #[test]
    fn should_prune_respects_grace_and_floors() {
        // fresh never-used = protected even with collapsed trust.
        assert!(!should_prune(0.0, 0.0, 0, 1.0));
        // aged + collapsed = prune.
        assert!(should_prune(0.05, 0.5, 3, 30.0));
        assert!(should_prune(0.5, 0.01, 3, 30.0));
        // healthy = keep.
        assert!(!should_prune(0.6, 0.6, 10, 100.0));
    }

    #[test]
    fn stagnation_flags_low_churn_flat_trust() {
        let v = assess_stagnation(0, 0, 1, 0.50, 0.50);
        assert!(v.stagnating);
        let healthy = assess_stagnation(10, 2, 8, 0.62, 0.50);
        assert!(!healthy.stagnating);
    }
}
