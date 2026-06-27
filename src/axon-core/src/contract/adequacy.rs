//! S2 (REQ-AXO-902089) — moteur d'adéquation : le gate anti « théâtre du sceau ».
//!
//! `adequacy_score = mutation-kill-rate × couverture des post-conditions`, gaté
//! par des seuils. Le panel VAL-AXO-148 (risque #1) : le même LLM écrit le muscle
//! ET le `proves` ; un `proves` vert-mais-faible scellerait du vide. La parade :
//! ne sceller QUE si le bundle tue assez de mutants ET couvre les post-conditions.
//!
//! Ce module fournit le SCORING + le GATE (pur, testable). La génération réelle
//! des mutants (cargo-mutants côté Rust de production) est branchée plus tard ;
//! ici les résultats de mutation sont fournis en entrée (`mutation_killed`).

/// Seuils d'adéquation. Défaut = production (kill-rate ≥ 0.80, couverture = 1.00).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdequacyThresholds {
    pub min_kill_rate: f64,
    pub min_coverage: f64,
}

impl Default for AdequacyThresholds {
    fn default() -> Self {
        Self { min_kill_rate: 0.80, min_coverage: 1.00 }
    }
}

/// Verdict d'adéquation d'un bundle `proves` vis-à-vis d'un contrat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdequacyReport {
    pub kill_rate: f64,
    pub coverage: f64,
    pub killed: usize,
    pub total: usize,
    pub passed: bool,
}

/// Évalue l'adéquation depuis les issues de mutation (`true` = mutant *tué* par le
/// bundle) et la couverture des post-conditions (fraction des classes que le
/// bundle peut discriminer). Un ensemble de mutants vide ⇒ kill-rate 0 ⇒ échec
/// (un bundle qu'on ne peut pas mettre à l'épreuve n'est pas adéquat).
pub fn assess(
    mutation_killed: &[bool],
    coverage: f64,
    thresholds: &AdequacyThresholds,
) -> AdequacyReport {
    let total = mutation_killed.len();
    let killed = mutation_killed.iter().filter(|k| **k).count();
    let kill_rate = if total == 0 { 0.0 } else { killed as f64 / total as f64 };
    let passed = total > 0
        && kill_rate >= thresholds.min_kill_rate
        && coverage >= thresholds.min_coverage;
    AdequacyReport { kill_rate, coverage, killed, total, passed }
}
