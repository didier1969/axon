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

    emit_canonical_ddl_manifest();
}

/// REQ-AXO-902328 — dériver du RÉPERTOIRE la liste des fichiers DDL compilés.
///
/// Le dépôt applique le DDL canonique par trois chemins. Deux parcouraient
/// `db/ddl/` ; le troisième — la liste `include_str!` de `postgres/ddl.rs`, celle
/// que le brain rejoue à CHAQUE boot — était écrite à la main, et il lui manquait
/// **9 fichiers sur 25** : tout le mailbox cross-projet, le schéma `axon` des
/// pratiques, les contrats, les secrets projet. Un brain démarré sans le chemin
/// shell ne créait aucune de leurs tables.
///
/// Le trou n'était pas un oubli distrait : il était écrit et justifié — « *the
/// other 14..22 files are unrelated* » — un jugement vrai pour le commit qui l'a
/// posé et faux pour le bootstrap. Une liste manuelle invite ce raisonnement local
/// sur une décision globale ; un répertoire l'interdit.
///
/// La règle de sélection est celle du SHELL (`[0-9][0-9]_*.sql`), qui est celle
/// ayant réellement appliqué les 25 fichiers en production — pas une quatrième
/// inventée ici.
fn emit_canonical_ddl_manifest() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let ddl_dir = Path::new(&manifest_dir).join("..").join("..").join("db").join("ddl");

    // Recompiler quand le RÉPERTOIRE change : c'est ceci, et non la garde de test,
    // qui empêche la divergence de revenir. Un fichier ajouté rouvre la liste.
    println!("cargo:rerun-if-changed={}", ddl_dir.display());

    let mut fichiers: Vec<std::path::PathBuf> = std::fs::read_dir(&ddl_dir)
        .unwrap_or_else(|e| panic!("db/ddl unreadable ({}): {e}", ddl_dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| {
                    let o = n.as_bytes();
                    n.ends_with(".sql")
                        && o.len() > 3
                        && o[0].is_ascii_digit()
                        && o[1].is_ascii_digit()
                        && o[2] == b'_'
                })
        })
        .collect();
    // Ordre lexical = ordre de dépendance (00 → 24), le même que le glob shell.
    fichiers.sort();

    assert!(
        !fichiers.is_empty(),
        "db/ddl/ contains no [0-9][0-9]_*.sql file — refusing to compile a brain \
         that would bootstrap an empty schema (REQ-AXO-902328)"
    );

    let mut rendu = String::from("const CANONICAL_DDL_FILES: &[(&str, &str)] = &[\n");
    for f in &fichiers {
        let nom = f.file_name().and_then(|n| n.to_str()).expect("nom de fichier UTF-8");
        // `include_str!` grave le contenu dans le binaire ; recompiler si UN fichier
        // change, pas seulement si le répertoire gagne une entrée.
        println!("cargo:rerun-if-changed={}", f.display());
        rendu.push_str(&format!(
            "    ({:?}, include_str!({:?})),\n",
            nom,
            f.display().to_string()
        ));
    }
    rendu.push_str("];\n");

    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("canonical_ddl_files.rs");
    std::fs::write(&out, rendu).expect("write canonical_ddl_files.rs");
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
