//! S4 (REQ-AXO-902091) — le KEEPER (DEC-AXO-901656, ratifiée).
//!
//! Certification = **verdict DÉRIVÉ d'une preuve liée au code, jamais une
//! déclaration**. Il n'existe AUCUN chemin d'API pour certifier par simple « fait »
//! d'un LLM : [`certify`] exige les vraies entrées de preuve (vert + adéquat) et
//! hashe l'evidence CONTRE l'état du code (anti vert-périmé, GUI-PRO-006).

use super::sha256_hex;

/// Preuve de certification : l'evidence liée à l'état du code sur lequel elle a
/// tourné. Pas de variante « certifié sans preuve » — l'absence de certification
/// est simplement `Option::None` côté appelant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certification {
    pub evidence_hash: String,
    pub code_state_hash: String,
}

/// Certifie un contrat. Retourne `None` tant que la preuve n'est pas verte ET
/// adéquate — un LLM ne peut pas fabriquer une `Certification` par déclaration,
/// seul un run de preuve réel (vert + adequacy gate, cf. [`super::adequacy`]) la
/// produit. L'`evidence_hash` lie l'evidence à `code_state_hash`.
pub fn certify(
    proves_green: bool,
    adequacy_passed: bool,
    code_state_hash: &str,
    evidence: &str,
) -> Option<Certification> {
    if !(proves_green && adequacy_passed) {
        return None;
    }
    let evidence_hash = sha256_hex(&format!("{}\u{1f}{}", evidence, code_state_hash));
    Some(Certification { evidence_hash, code_state_hash: code_state_hash.to_string() })
}

impl Certification {
    /// Vrai seulement si le code n'a pas changé depuis la preuve : une
    /// certification verte pour un code désormais modifié ne compte plus
    /// (anti vert-périmé).
    pub fn is_valid_for(&self, current_code_hash: &str) -> bool {
        self.code_state_hash == current_code_hash
    }
}
