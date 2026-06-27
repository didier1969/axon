//! S5 (REQ-AXO-902092) — expansion demand-driven aux coutures + auto-amorçage
//! legacy (DEC-AXO-901659).
//!
//! Deux mécanismes purs :
//! - [`propose_from_observed`] : sur un codebase existant, un symbole OBSERVÉ dans
//!   l'IST (signature + tests + lien SOLL) devient un ContractNode **proposé**
//!   (`realized_by` déjà rempli = implémenté), à ratifier. Rompt la circularité
//!   « pour cesser d'abduire, tout abduire » (panel VAL-AXO-148, risque #6).
//! - [`should_expand`] / [`next_pull`] : l'expansion est tirée à la demande (pull)
//!   et bornée aux coutures (module / interface), jamais aux fonctions-feuilles —
//!   le compilateur + les tests restent l'oracle des feuilles (anti-BDUF).

use super::{ContractKind, ContractNode, PostCondition};

/// État d'expansion d'un contrat dans la frontière BFS (portée par le graphe).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpansionStatus {
    /// Couture pas encore expansée : candidate au pull.
    LeafPending,
    /// Déjà expansée en enfants-contrats.
    Expanded,
    /// Surface de contrat fermée : l'implémenteur remplit librement en dessous.
    Frozen,
}

/// Dérive un ContractNode PROPOSÉ depuis un symbole observé dans l'IST. Le contrat
/// n'est pas abduit ex nihilo : `realized_by` est déjà renseigné (le code existe),
/// les post-conditions sont dérivées des tests observés. Le contrat reste à
/// ratifier par un humain/agent (signalé ici par `realized_by = Some`).
pub fn propose_from_observed(
    kind: ContractKind,
    signature: &str,
    why: &str,
    symbol_id: &str,
    observed_tests: &[&str],
) -> ContractNode {
    ContractNode {
        kind,
        signature: signature.to_string(),
        why: why.to_string(),
        post_conditions: derive_postconditions(observed_tests),
        proves_ref: format!("observed:{symbol_id}"),
        realized_by: Some(symbol_id.to_string()),
    }
}

/// Heuristique de dérivation de post-conditions depuis les noms de tests observés.
/// Dédupliquée, ordre stable. (À l'industrialisation, raffinée par analyse du corps
/// des tests ; ici la signature observée suffit à amorcer une proposition.)
pub fn derive_postconditions(test_names: &[&str]) -> Vec<PostCondition> {
    let mut out: Vec<PostCondition> = Vec::new();
    for name in test_names {
        let lower = name.to_ascii_lowercase();
        let tag = if lower.contains("normaliz") {
            "normalized"
        } else if lower.contains("non_numeric") || lower.contains("skip") || lower.contains("invalid") {
            "filters_invalid"
        } else if lower.contains("default") {
            "default_fallback"
        } else if lower.contains("disable") || lower.contains("empty") {
            "disable_semantics"
        } else if lower.contains("dedup") {
            "dedup"
        } else if lower.contains("sort") {
            "sorted"
        } else {
            "behavioral"
        };
        let cond = PostCondition(tag.to_string());
        if !out.contains(&cond) {
            out.push(cond);
        }
    }
    out
}

/// Borne de couture : on expanse une couture (module / interface) encore en
/// attente, jamais une fonction-feuille ni une couture déjà figée.
pub fn should_expand(kind: ContractKind, status: ExpansionStatus) -> bool {
    matches!(status, ExpansionStatus::LeafPending)
        && matches!(kind, ContractKind::Module | ContractKind::Interface)
}

/// Pull : retourne l'index du prochain contrat à expanser dans la frontière, ou
/// `None` si rien n'est expansible (tout figé / feuilles). Modèle « tiré à la
/// demande » plutôt que BFS-push exhaustif.
pub fn next_pull(frontier: &[(ContractKind, ExpansionStatus)]) -> Option<usize> {
    frontier
        .iter()
        .position(|(kind, status)| should_expand(*kind, *status))
}
