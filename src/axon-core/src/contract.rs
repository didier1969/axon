//! REQ-AXO-902087 — couche de contrats structurels canoniques (« squelette »).
//!
//! Charnière entre SOLL-prose (le *pourquoi*) et le code (le *comment*) : un
//! ContractNode porte la promesse structurelle (signature typée + post-conditions)
//! d'une couture architecturale, AVANT et indépendamment de son implémentation.
//!
//! Tranche 1 = le cœur PUR validé par le prototype VAL-AXO-149 :
//! - [`ContractNode`] typé + [`ContractNode::shape_hash`] déterministe + validation
//!   de bonne-formation (le gain B2, DEC-AXO-901655 : un jsonb prose ne valide pas).
//! - [`adequacy`] : le gate anti « théâtre du sceau » (DEC-AXO-901657, risque #1 du
//!   panel VAL-AXO-148) — un `proves` faible NE scelle PAS.
//! - [`seal`] : `structural_seal` pur (propage, log(n)) vs `empirical_attestation`
//!   hors-hash (l'invariant Merkle tient).
//!
//! Persistance (store B2, S1 REQ-AXO-902088), binding/certification (S4
//! REQ-AXO-902091) et réconciliation IST (S6 REQ-AXO-902093) sont des tranches
//! ultérieures qui s'appuient sur ce cœur.

pub mod adequacy;
pub mod binding;
pub mod certification;
pub mod seal;

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// Granularité structurelle d'un contrat. Borné aux coutures (DEC-AXO-901659) :
/// module / interface / fonction-frontière / type — jamais chaque fonction privée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractKind {
    Module,
    Interface,
    Function,
    Type,
}

impl ContractKind {
    fn tag(self) -> &'static str {
        match self {
            ContractKind::Module => "module",
            ContractKind::Interface => "interface",
            ContractKind::Function => "function",
            ContractKind::Type => "type",
        }
    }
}

/// Post-condition promise par le contrat : identité déclarée d'un prédicat dont
/// la version exécutable vit dans le bundle `proves`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PostCondition(pub String);

/// Échec de bonne-formation d'un contrat (le gain B2 : détecté à la création,
/// là où un corps jsonb prose laisserait passer silencieusement).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractError {
    /// Signature vide.
    EmptySignature,
    /// Signature sans type de retour (`->`) — non machine-validable.
    UntypedSignature,
    /// Aucune post-condition déclarée : rien à prouver, sceau impossible.
    NoPostConditions,
}

/// Un ContractNode « vu de lui-même » — noyau typé minimal (logique S1).
///
/// Le panel VAL-AXO-148 a montré qu'un LLM émet fiablement `{why, promise,
/// proves}` ; les champs de gouvernance (state/expansion/seal) restent hors de
/// ce noyau chaud et sont portés par les tranches ultérieures.
#[derive(Debug, Clone)]
pub struct ContractNode {
    pub kind: ContractKind,
    /// Promesse typée, p.ex. `parse_seq_buckets_from_env(raw: Option<&str>) -> Vec<usize>`.
    pub signature: String,
    /// `SOLVES <SOLL-id>` — le pourquoi gouvernant, fusionné à la récupération.
    pub why: String,
    /// Post-conditions promises (le « contrat au sens large »).
    pub post_conditions: Vec<PostCondition>,
    /// Identité du bundle `proves` (les obligations de test).
    pub proves_ref: String,
    /// Symbole IST qui le réalise, une fois le binding établi (S4). `None` = planned.
    pub realized_by: Option<String>,
}

impl ContractNode {
    /// Rejette un contrat malformé (le gain B2 : DEC-AXO-901655). Le `shape_hash`
    /// et le sceau exigent un contrat bien formé ; valider à la création garantit
    /// l'intégrité du hash à la racine.
    pub fn validate(&self) -> Result<(), ContractError> {
        let sig = self.signature.trim();
        if sig.is_empty() {
            return Err(ContractError::EmptySignature);
        }
        if !sig.contains("->") {
            return Err(ContractError::UntypedSignature);
        }
        if self.post_conditions.is_empty() {
            return Err(ContractError::NoPostConditions);
        }
        Ok(())
    }

    /// Hash déterministe de la *forme désirée* du contrat (kind + signature +
    /// post-conditions triées). Indépendant de toute donnée empirique ; c'est la
    /// base du `structural_seal`. Suppose le contrat bien formé ([`Self::validate`]).
    pub fn shape_hash(&self) -> String {
        let mut conds: Vec<&str> = self.post_conditions.iter().map(|p| p.0.as_str()).collect();
        conds.sort_unstable();
        let canonical = format!(
            "{}\u{1f}{}\u{1f}{}",
            self.kind.tag(),
            self.signature.trim(),
            conds.join(",")
        );
        sha256_hex(&canonical)
    }
}

/// SHA-256 hex (idiome partagé avec `pipeline::stage_a1::sha256_hex` ; gardé local
/// pour ne pas coupler la couche contrat aux internes du pipeline — SRP).
pub(crate) fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{:02x}", byte);
    }
    out
}

#[cfg(test)]
#[path = "contract_tests.rs"]
mod contract_tests;
