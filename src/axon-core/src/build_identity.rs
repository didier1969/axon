//! REQ-AXO-902464 — l'identité de build **gravée** dans le binaire, et l'écart
//! avec celle que l'environnement **déclare**.
//!
//! RÉUTILISE : néant — vérifié via `query "build identity compiled build id drift"`
//! puis `retrieve_context`. Le seul site existant (`runtime_boot.rs:779`,
//! `parse_build_info_identity`) LIT `AXON_BUILD_ID` depuis l'environnement ; aucun
//! symbole ne compare le code compilé à l'étiquette déclarée — c'est précisément
//! l'absence que REQ-AXO-902464 mesure. Ce module fournit la valeur gravée ;
//! `tools_framework_runtime_status.rs` la confronte à la valeur déclarée qu'il
//! lisait déjà.
//!
//! Un binaire Axon annonçait jusqu'ici son identité en lisant `AXON_BUILD_ID` dans
//! son environnement, c'est-à-dire un fichier `bin/*.build-info` que le déploiement
//! pose *à côté* de lui. Rien ne liait cette étiquette au code réellement compilé.
//! Le 2026-08-23 les deux ont divergé d'une journée entière sans qu'aucune garde ne
//! puisse le voir : `build_id=v0.8.0-1590-g13642f76` annoncé, code de
//! `v0.8.0-1586-g43880d41` exécuté, quatre contrôles de release verts.
//!
//! `build.rs` grave désormais l'identité au moment de la compilation. Ce module
//! l'expose et nomme l'écart. C'est `PIL-AXO-9005` à l'échelle d'un binaire : la
//! dérive entre l'état désiré (le manifeste) et l'état observé (ce qui tourne)
//! devient une anomalie de première classe, pas un mystère opérationnel.

/// Identité du build **gravée à la compilation** (`build.rs`).
///
/// `unknown` quand ni `AXON_BUILD_ID` ni `git describe` n'étaient disponibles au
/// moment de compiler — un aveu, jamais une valeur de remplacement qui se lirait
/// comme une preuve.
pub const COMPILED_BUILD_ID: &str = env!("AXON_COMPILED_BUILD_ID");

/// Verdict de correspondance entre l'identité gravée et l'identité déclarée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityMatch {
    /// Le binaire porte exactement l'identité que l'environnement annonce.
    Match,
    /// Le binaire n'a pas pu être marqué à la compilation, ou rien n'est déclaré :
    /// on ne SAIT pas. Distinct de `Drift` — « non mesuré » n'est pas « mesuré et
    /// faux » (doléance KKI #204).
    Unknown,
    /// Le binaire exécuté n'est pas celui que l'étiquette annonce.
    Drift,
}

impl IdentityMatch {
    pub fn as_str(self) -> &'static str {
        match self {
            IdentityMatch::Match => "match",
            IdentityMatch::Unknown => "unknown",
            IdentityMatch::Drift => "drift",
        }
    }
}

/// Compare l'identité gravée à celle que l'environnement déclare.
///
/// `declared` est ce que `AXON_BUILD_ID` (donc `bin/*.build-info`, donc le manifeste
/// de release) affirme. Un `Drift` signifie littéralement : *le binaire qui répond
/// n'est pas celui que la release annonce*.
pub fn identity_match(declared: &str) -> IdentityMatch {
    if COMPILED_BUILD_ID.is_empty() || COMPILED_BUILD_ID == "unknown" {
        return IdentityMatch::Unknown;
    }
    if declared.is_empty() {
        return IdentityMatch::Unknown;
    }
    if declared == COMPILED_BUILD_ID {
        IdentityMatch::Match
    } else {
        IdentityMatch::Drift
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_build_id_is_engraved_at_compile_time() {
        // La marque doit EXISTER : c'est elle que `axon_artifact_carries_build_id`
        // cherche dans le binaire publié. Une marque vide rendrait la sonde de
        // contenu du promote vraie sur n'importe quel fichier.
        assert!(
            !COMPILED_BUILD_ID.is_empty(),
            "AXON_COMPILED_BUILD_ID est vide — la sonde de contenu du promote n'aurait rien à lire"
        );
    }

    #[test]
    fn declared_identity_equal_to_engraved_is_a_match() {
        assert_eq!(identity_match(COMPILED_BUILD_ID), IdentityMatch::Match);
    }

    #[test]
    fn the_incident_of_2026_08_23_is_reported_as_drift() {
        // Étiquette d'une release, binaire d'une autre : exactement ce qui a été
        // servi à 75 tenants sans qu'aucune garde ne le dise.
        assert_eq!(
            identity_match("v0.8.0-1586-g43880d41-definitely-not-this-build"),
            IdentityMatch::Drift
        );
    }

    #[test]
    fn an_unmeasurable_identity_is_never_reported_as_drift() {
        // « non calculé » est un état de premier rang, distinct de « faux ».
        // Une déclaration vide ne prouve aucune dérive.
        assert_eq!(identity_match(""), IdentityMatch::Unknown);
    }
}
