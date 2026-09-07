//! REQ-AXO-902640 — les soumissions Nexus des scripts de livraison, gardees en
//! CLASSE.
//!
//! Le defaut, deux fois : `--class` portait le dimensionnement CPU ; il est MORT
//! et IGNORE depuis REQ-VPC-290, et rien ne l'a remplace. Une soumission qui ne
//! declare pas `--cpus` recoit le defaut de transition du courtier — 4 coeurs —
//! pendant que cargo lance ses threads. Aucune erreur, aucune ligne de log :
//! seulement une compilation qui rampe, puis un timeout. REQ-AXO-902629 a
//! corrige `scripts/setup.sh` et laisse `scripts/release/promote_live_safe.sh`
//! intact ; l'incident est revenu par la porte d'a cote.
//!
//! POURQUOI EN RUST ET PAS EN SHELL. `tests/shell/*.sh` se lancent a la main
//! (« Run: bash tests/shell/… »), ils ne passent pas dans la porte
//! `GUI-AXO-1034`. Une garde qui ne tourne pas est une garde muette — le motif
//! meme que ce lot ferme. Celle-ci tourne dans `cargo test --lib`.

/// Racine du depot : `CARGO_MANIFEST_DIR` pointe sur `src/axon-core`.
fn racine_du_depot() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Tous les `.sh` sous `scripts/`, en `(chemin relatif, contenu)`.
fn scripts_shell() -> Vec<(String, String)> {
    let racine = racine_du_depot().join("scripts");
    let mut out = Vec::new();
    let mut pile = vec![racine.clone()];
    while let Some(dir) = pile.pop() {
        let Ok(entrees) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entree in entrees.flatten() {
            let chemin = entree.path();
            if chemin.is_dir() {
                pile.push(chemin);
                continue;
            }
            if chemin.extension().and_then(|e| e.to_str()) != Some("sh") {
                continue;
            }
            let Ok(texte) = std::fs::read_to_string(&chemin) else {
                continue;
            };
            let rel = chemin
                .strip_prefix(&racine)
                .unwrap_or(&chemin)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, texte));
        }
    }
    out.sort();
    out
}

/// Une soumission reperee dans un script : sa premiere ligne, et la commande
/// entiere continuations `\` comprises.
#[derive(Debug, PartialEq, Eq)]
struct Soumission {
    ligne: usize,
    commande: String,
}

/// Reconnait `nexus-job run|submit` et sa forme indirecte `"$nexus_job_bin" run`,
/// puis recolle les continuations. C'est la recolle qui compte : les drapeaux
/// vivent sur les lignes SUIVANTES, une lecture ligne a ligne ne verrait jamais
/// `--cpus`.
fn soumissions_nexus(texte: &str) -> Vec<Soumission> {
    let lignes: Vec<&str> = texte.lines().collect();
    let mut out = Vec::new();
    for (i, ligne) in lignes.iter().enumerate() {
        let nu = ligne.trim_start();
        if nu.starts_with('#') {
            continue;
        }
        // Le verbe doit suivre IMMEDIATEMENT le binaire. Un `contains(" run")`
        // a d'abord accuse `local nexus_job_bin runner` : « runner » contient
        // « run ». Une garde qui accuse une declaration de variable apprend a
        // se mefier d'elle.
        let jetons: Vec<&str> = nu.split_whitespace().collect();
        let soumet = jetons.windows(2).any(|paire| {
            let porte_binaire = paire[0].contains("nexus-job")
                || paire[0].contains("nexus_job_bin")
                || paire[0].contains("NEXUS_JOB_BIN");
            porte_binaire && (paire[1] == "run" || paire[1] == "submit")
        });
        if !soumet {
            continue;
        }
        let mut commande = String::new();
        let mut j = i;
        loop {
            let courante = lignes[j];
            commande.push_str(courante.trim_end().trim_end_matches('\\'));
            commande.push(' ');
            if !courante.trim_end().ends_with('\\') || j + 1 >= lignes.len() {
                break;
            }
            j += 1;
        }
        out.push(Soumission {
            ligne: i + 1,
            commande,
        });
    }
    out
}

/// Ce qui manque, ou ce qui est mort, sur une soumission donnee.
fn griefs(commande: &str) -> Vec<&'static str> {
    let mut griefs = Vec::new();
    if !commande.contains("--memory") {
        griefs.push("`--memory` absent");
    }
    if !commande.contains("--cpus") {
        griefs.push("`--cpus` absent — le courtier en prete 4");
    }
    if commande.contains("--class") {
        griefs.push("`--class` present alors qu'il est MORT depuis REQ-VPC-290");
    }
    griefs
}

fn soumissions_fautives(scripts: &[(String, String)]) -> Vec<String> {
    let mut fautives = Vec::new();
    for (chemin, texte) in scripts {
        for soumission in soumissions_nexus(texte) {
            let griefs = griefs(&soumission.commande);
            if griefs.is_empty() {
                continue;
            }
            fautives.push(format!(
                "scripts/{chemin}:{} — {}",
                soumission.ligne,
                griefs.join(" · ")
            ));
        }
    }
    fautives
}

#[test]
fn toute_soumission_nexus_declare_memoire_ET_cpus() {
    let scripts = scripts_shell();
    assert!(
        !scripts.is_empty(),
        "aucun script lu sous `scripts/` — la garde balaierait le vide et rendrait \
         vert sans rien avoir regarde"
    );
    let fautives = soumissions_fautives(&scripts);
    assert!(
        fautives.is_empty(),
        "ces soumissions Nexus laissent le courtier decider a leur place \
         (REQ-AXO-902640, meme defaut que REQ-AXO-902629).\n\
         `--class` est mort depuis REQ-VPC-290 et c'est lui qui portait le \
         dimensionnement CPU : sans `--cpus`, le job recoit 4 coeurs, cargo en \
         lance 14, et la seule trace est un timeout. Norme du depot \
         (GUI-AXO-1034) : `--memory 12G --cpus 12`.\n  {}",
        fautives.join("\n  ")
    );
}

/// La garde sait dire NON, et sur chaque grief separement. Sans cela elle
/// pourrait n'exiger que `--memory` — ce que le site fautif declarait deja.
#[test]
#[allow(non_snake_case)]
fn MUTANT_la_garde_de_soumission_sait_dire_NON_sur_chaque_grief() {
    let conforme = "  \"$nexus_job_bin\" run \\\n      --project AXON \\\n      \
                    --memory 12G \\\n      --cpus 12 \\\n      -- runner\n";
    let sans_cpus = "  \"$nexus_job_bin\" run \\\n      --project AXON \\\n      \
                     --memory 6G \\\n      -- runner\n";
    let sans_memoire = "  nexus-job submit \\\n      --cpus 12 \\\n      -- runner\n";
    let avec_class = "  nexus-job run \\\n      --class medium \\\n      --memory 12G \\\n      \
                      --cpus 12 \\\n      -- runner\n";

    let verdict = |texte: &str| soumissions_fautives(&[("x.sh".to_string(), texte.to_string())]);

    assert!(verdict(conforme).is_empty(), "{:?}", verdict(conforme));

    let v = verdict(sans_cpus);
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(v[0].contains("`--cpus` absent"), "{v:?}");
    assert!(!v[0].contains("`--memory` absent"), "{v:?}");

    let v = verdict(sans_memoire);
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(v[0].contains("`--memory` absent"), "{v:?}");
    assert!(!v[0].contains("`--cpus` absent"), "{v:?}");

    let v = verdict(avec_class);
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(v[0].contains("`--class` present"), "{v:?}");

    // Une ligne commentee n'est pas une soumission : la garde ne doit pas
    // accuser la documentation qui explique le defaut.
    assert!(verdict("  # nexus-job run --class medium\n").is_empty());
    // Et une mention de `nexus-job` qui ne soumet rien non plus.
    assert!(verdict("  NEXUS_JOB_BIN=\"$(command -v nexus-job)\"\n").is_empty());
    // Ni une declaration de variable dont le mot suivant COMMENCE par « run ».
    assert!(verdict("  local nexus_job_bin runner\n").is_empty());
    assert!(verdict("  nexus_job_bin=\"$(command -v nexus-job || true)\"\n").is_empty());
}

/// La recolle des continuations est la moitie qui fait tout le travail : sans
/// elle, `--cpus` vit sur une ligne que la garde ne lirait jamais.
#[test]
#[allow(non_snake_case)]
fn MUTANT_la_recolle_va_chercher_les_drapeaux_des_lignes_SUIVANTES() {
    let texte = "  nexus-job run \\\n      --memory 12G \\\n      --cpus 12 \\\n      -- x\n";
    let trouvees = soumissions_nexus(texte);
    assert_eq!(trouvees.len(), 1, "{trouvees:?}");
    assert!(trouvees[0].commande.contains("--cpus 12"), "{trouvees:?}");
    assert_eq!(trouvees[0].ligne, 1);

    // Et elle s'arrete a la fin de la commande : le drapeau d'une soumission
    // VOISINE ne doit pas exonerer celle-ci.
    let deux = "  nexus-job run --memory 6G -- a\n  nexus-job run --cpus 12 -- b\n";
    let fautives = soumissions_fautives(&[("x.sh".to_string(), deux.to_string())]);
    assert_eq!(fautives.len(), 2, "{fautives:?}");
    assert!(fautives[0].contains("`--cpus` absent"), "{fautives:?}");
    assert!(fautives[1].contains("`--memory` absent"), "{fautives:?}");
}
