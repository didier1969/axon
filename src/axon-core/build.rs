use std::path::Path;

/// Génère uniquement le manifeste DDL canonique.
///
/// REQ-AXO-902543 découple désormais l'identité de release de `build.rs` : la
/// compilation produit une section ELF réservée, puis `scripts/setup.sh` la
/// estampille dans les seuls artefacts livrés. Un changement de SHA ne force donc
/// plus la recompilation de toute la crate.
fn main() {
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
    let ddl_dir = Path::new(&manifest_dir)
        .join("..")
        .join("..")
        .join("db")
        .join("ddl");

    // Recompiler quand le RÉPERTOIRE change : c'est ceci, et non la garde de test,
    // qui empêche la divergence de revenir. Un fichier ajouté rouvre la liste.
    println!("cargo:rerun-if-changed={}", ddl_dir.display());

    let mut fichiers: Vec<std::path::PathBuf> = std::fs::read_dir(&ddl_dir)
        .unwrap_or_else(|e| panic!("db/ddl unreadable ({}): {e}", ddl_dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
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
        let nom = f
            .file_name()
            .and_then(|n| n.to_str())
            .expect("nom de fichier UTF-8");
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
