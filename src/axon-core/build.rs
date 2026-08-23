use std::path::Path;
use std::process::Command;

/// REQ-AXO-902464 — graver l'identité de build DANS le binaire.
///
/// Le 2026-08-23, un promote a publié le binaire de la veille sous l'étiquette du
/// jour : `build_id=v0.8.0-1590-g13642f76` annoncé, code de `v0.8.0-1586-g43880d41`
/// servi. Les quatre contrôles de `preflight.sh` étaient verts — ils comparaient
/// tous des dérivés du même artefact périmé (sha↔sha, étiquette↔étiquette), et
/// aucun ne pouvait lire le CONTENU parce que le binaire ne portait aucune trace
/// de sa source.
///
/// `PIL-AXO-005` exige que « l'artefact corresponde au SHA promu ». Sans marque
/// dans le binaire, cette exigence n'était pas vérifiable : elle ne pouvait être
/// que déclarée. Depuis ici elle est mesurable — `axon_artifact_carries_build_id`
/// (scripts/lib/axon-version.sh) la lit, et `status` expose l'écart entre ce qui
/// est GRAVÉ et ce qui est DÉCLARÉ par l'environnement.
///
/// Source de l'identité, dans l'ordre : `AXON_BUILD_ID` (posé par le promote, qui
/// sait quel SHA il promeut) puis `git describe` du dépôt qui contient ce manifeste.
/// `unknown` si aucun des deux — jamais une valeur inventée qui se lirait comme
/// une preuve.
///
/// (build.rs était vide depuis l'extraction du piège de build C++ en plugin dynamique.)
fn main() {
    let build_id = std::env::var("AXON_BUILD_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(git_describe)
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=AXON_COMPILED_BUILD_ID={build_id}");

    // Le promote pose `AXON_BUILD_ID` : sans ceci, cargo réutiliserait un objet
    // compilé sous l'identité précédente — la marque mentirait, et le contrôle
    // bâti dessus mentirait avec elle.
    println!("cargo:rerun-if-env-changed=AXON_BUILD_ID");

    // Hors promote, l'identité vient de `git describe` : la rafraîchir quand HEAD
    // bouge, sinon un build de dev porterait le SHA d'un commit précédent.
    if let Some(head) = git_head_file() {
        println!("cargo:rerun-if-changed={}", head.display());
    }
}

fn git_describe() -> Option<String> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let output = Command::new("git")
        .args([
            "-C",
            &manifest_dir,
            "describe",
            "--tags",
            "--always",
            "--dirty",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let described = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!described.is_empty()).then_some(described)
}

/// Chemin du `HEAD` du dépôt — `git rev-parse --absolute-git-dir` le résout aussi
/// bien pour un dépôt ordinaire que pour un worktree détaché, où `.git` est un
/// FICHIER et non un répertoire. Le promote compile précisément dans un worktree
/// détaché (REQ-AXO-902391).
fn git_head_file() -> Option<std::path::PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let output = Command::new("git")
        .args(["-C", &manifest_dir, "rev-parse", "--absolute-git-dir"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let git_dir = String::from_utf8(output.stdout).ok()?.trim().to_string();
    let head = Path::new(&git_dir).join("HEAD");
    head.exists().then_some(head)
}
