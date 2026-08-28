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
//! Le linker réserve désormais une section de taille fixe, estampillée dans les
//! seuls exécutables livrés après compilation. Ce module l'expose et nomme
//! l'écart. C'est `PIL-AXO-9005` à l'échelle d'un binaire : la
//! dérive entre l'état désiré (le manifeste) et l'état observé (ce qui tourne)
//! devient une anomalie de première classe, pas un mystère opérationnel.

use std::sync::OnceLock;

const BUILD_ID_SECTION_LEN: usize = 128;
const BUILD_ID_SECTION_NAME: &str = ".axon_build_id";

const fn empty_build_id_section() -> [u8; BUILD_ID_SECTION_LEN] {
    let mut section = [0u8; BUILD_ID_SECTION_LEN];
    let unknown = b"unknown";
    let mut index = 0;
    while index < unknown.len() {
        section[index] = unknown[index];
        index += 1;
    }
    section
}

/// Fixed-size release identity slot. `setup.sh` updates this ELF section after
/// linking. Changing the promoted SHA therefore relinks/stamps only delivered
/// executables and no longer invalidates every codegen unit in `axon-core`.
#[used]
#[link_section = ".axon_build_id"]
static COMPILED_BUILD_ID_SECTION: [u8; BUILD_ID_SECTION_LEN] = empty_build_id_section();

static COMPILED_BUILD_ID: OnceLock<String> = OnceLock::new();

pub fn compiled_build_id() -> &'static str {
    COMPILED_BUILD_ID
        .get_or_init(|| {
            std::env::current_exe()
                .ok()
                .and_then(|path| std::fs::read(path).ok())
                .and_then(|binary| {
                    elf_section(&binary, BUILD_ID_SECTION_NAME).map(ToOwned::to_owned)
                })
                .and_then(|section| {
                    let end = section
                        .iter()
                        .position(|byte| *byte == 0)
                        .unwrap_or(section.len());
                    std::str::from_utf8(&section[..end])
                        .ok()
                        .map(str::trim)
                        .map(str::to_owned)
                })
                .filter(|identity| !identity.is_empty())
                .unwrap_or_else(|| "unknown".to_string())
        })
        .as_str()
}

fn elf_section<'a>(binary: &'a [u8], wanted: &str) -> Option<&'a [u8]> {
    if binary.get(..6)? != b"\x7fELF\x02\x01" {
        return None;
    }
    let shoff = read_u64(binary, 0x28)? as usize;
    let shentsize = read_u16(binary, 0x3a)? as usize;
    let shnum = read_u16(binary, 0x3c)? as usize;
    let shstrndx = read_u16(binary, 0x3e)? as usize;
    if shentsize < 64 || shstrndx >= shnum {
        return None;
    }
    let string_header = shoff.checked_add(shstrndx.checked_mul(shentsize)?)?;
    let string_offset = read_u64(binary, string_header.checked_add(24)?)? as usize;
    let string_size = read_u64(binary, string_header.checked_add(32)?)? as usize;
    let strings = binary.get(string_offset..string_offset.checked_add(string_size)?)?;

    for index in 0..shnum {
        let header = shoff.checked_add(index.checked_mul(shentsize)?)?;
        let name_offset = read_u32(binary, header)? as usize;
        let name = c_string(strings.get(name_offset..)?)?;
        if name == wanted.as_bytes() {
            let offset = read_u64(binary, header.checked_add(24)?)? as usize;
            let size = read_u64(binary, header.checked_add(32)?)? as usize;
            return binary.get(offset..offset.checked_add(size)?);
        }
    }
    None
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?,
    ))
}

fn c_string(bytes: &[u8]) -> Option<&[u8]> {
    Some(&bytes[..bytes.iter().position(|byte| *byte == 0)?])
}

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
    identity_match_for(compiled_build_id(), declared)
}

fn identity_match_for(compiled: &str, declared: &str) -> IdentityMatch {
    if compiled.is_empty() || compiled == "unknown" {
        return IdentityMatch::Unknown;
    }
    if declared.is_empty() {
        return IdentityMatch::Unknown;
    }
    if declared == compiled {
        IdentityMatch::Match
    } else {
        IdentityMatch::Drift
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_contains_a_reserved_build_identity_section() {
        // La marque doit EXISTER : c'est elle que `axon_artifact_carries_build_id`
        // cherche dans le binaire publié. Une marque vide rendrait la sonde de
        // contenu du promote vraie sur n'importe quel fichier.
        assert!(
            !compiled_build_id().is_empty(),
            "AXON_COMPILED_BUILD_ID est vide — la sonde de contenu du promote n'aurait rien à lire"
        );
    }

    #[test]
    fn declared_identity_equal_to_engraved_is_a_match() {
        if compiled_build_id() == "unknown" {
            assert_eq!(identity_match("unknown"), IdentityMatch::Unknown);
        } else {
            assert_eq!(identity_match(compiled_build_id()), IdentityMatch::Match);
        }
    }

    #[test]
    fn the_incident_of_2026_08_23_is_reported_as_drift() {
        // Étiquette d'une release, binaire d'une autre : exactement ce qui a été
        // servi à 75 tenants sans qu'aucune garde ne le dise.
        assert_eq!(
            identity_match_for(
                "v0.8.0-1590-g13642f76",
                "v0.8.0-1586-g43880d41-definitely-not-this-build"
            ),
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
