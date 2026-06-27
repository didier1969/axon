//! S3 (REQ-AXO-902090) — sceau structurel pur + attestation empirique hors-hash.
//!
//! Deux canaux SÉPARÉS (DEC-AXO-901657 redéfini ; panel VAL-AXO-148 risque #2) :
//! - [`structural_seal`] : `H(shape_hash, proves_ref, adequacy_verdict, [enfants])`
//!   — déterministe, propage à la racine, re-validation log(n). C'est le vrai Merkle.
//!   `None` quand l'adéquation échoue : **pas de métrique adéquate ⇒ pas de sceau.**
//! - [`EmpiricalAttestation`] : verdict horodaté + TTL des `proves` empiriques
//!   (vrai I/O, flaky) — JAMAIS injecté dans le hash structurel (sémantique
//!   `no-cache` Bazel), surfacé comme dimension de fraîcheur séparée.
//!
//! Sealing PARTIEL : des sous-arbres non scellés sont tolérés sans pénalité
//! (DEC-AXO-901659) — un parent agrège les sceaux de ses enfants *présents*.

use super::sha256_hex;

/// Sceau structurel d'un ContractNode (hex tronqué, content-addressed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralSeal(pub String);

/// Calcule le sceau structurel. Retourne `None` si l'adéquation échoue — c'est le
/// gate anti « théâtre du sceau » : un `proves` inadéquat ne produit aucun sceau,
/// donc rien à propager vers le parent.
///
/// `child_seals` = sceaux des enfants DÉJÀ scellés (bottom-up). Pour qu'un parent
/// scelle, tous ses enfants *requis* doivent être présents ici (la composition est
/// décidée par l'appelant : sealing partiel = enfants optionnels omis).
pub fn structural_seal(
    shape_hash: &str,
    proves_ref: &str,
    adequacy_passed: bool,
    child_seals: &[StructuralSeal],
) -> Option<StructuralSeal> {
    if !adequacy_passed {
        return None;
    }
    let mut children: Vec<&str> = child_seals.iter().map(|s| s.0.as_str()).collect();
    children.sort_unstable();
    let canonical = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        shape_hash,
        proves_ref,
        "ADEQUATE",
        children.join(",")
    );
    Some(StructuralSeal(sha256_hex(&canonical)[..16].to_string()))
}

/// Scelle un nœud en exigeant que TOUS ses enfants requis soient déjà scellés
/// (composition bottom-up). Retourne `None` si l'adéquation échoue OU si un enfant
/// requis manque (`None`). Le sealing partiel s'exprime en n'inscrivant pas un
/// enfant comme *requis* (DEC-AXO-901659), pas en relâchant cette règle.
pub fn seal_node(
    shape_hash: &str,
    proves_ref: &str,
    adequacy_passed: bool,
    required_children: &[Option<StructuralSeal>],
) -> Option<StructuralSeal> {
    if !adequacy_passed {
        return None;
    }
    let mut present = Vec::with_capacity(required_children.len());
    for child in required_children {
        present.push(child.clone()?);
    }
    structural_seal(shape_hash, proves_ref, adequacy_passed, &present)
}

/// Résultat d'un run de `proves` empirique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Green,
    Red,
}

/// Attestation empirique : horodatée, périssable, et **explicitement hors** du
/// sceau structurel (elle ne doit jamais entrer dans [`structural_seal`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmpiricalAttestation {
    pub result: Verdict,
    pub at_ms: u64,
    pub ttl_s: u64,
}

impl EmpiricalAttestation {
    pub fn new(result: Verdict, at_ms: u64, ttl_s: u64) -> Self {
        Self { result, at_ms, ttl_s }
    }

    /// Fraîche si `now_ms` est dans la fenêtre TTL depuis l'attestation.
    pub fn is_fresh(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.at_ms) <= self.ttl_s.saturating_mul(1000)
    }
}
