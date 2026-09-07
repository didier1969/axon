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

// ---------------------------------------------------------------------------
// REQ-AXO-902628 — la preuve de complétion d'un promote, exercée DANS la porte.
//
// Le juge vit dans `scripts/release/promote_completion_evidence.py`, appelé par
// `promote_live_safe.sh`. Il est en Python et non dans le shell précisément pour
// qu'une garde puisse l'exercer d'ici : les neuf `tests/shell/test_promote_*.sh`
// se lancent à la main, aucun runner ne les agrège, la CI n'en appelle aucun.
// Une garde qui ne tourne pas est une garde muette — la famille de défauts que
// ce REQ ferme.
// ---------------------------------------------------------------------------

fn juge_de_completion() -> std::path::PathBuf {
    racine_du_depot()
        .join("scripts")
        .join("release")
        .join("promote_completion_evidence.py")
}

/// Rend le verdict du juge sur un journal écrit à la volée.
fn verdict_du_juge(lignes_jsonl: &str) -> String {
    let dossier = std::env::temp_dir().join(format!(
        "axon-902628-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dossier).expect("dossier temporaire");
    let journal = dossier.join("attempt.jsonl");
    std::fs::write(&journal, lignes_jsonl).expect("journal temporaire");
    let sortie = std::process::Command::new("python3")
        .arg(juge_de_completion())
        .arg(&journal)
        .output()
        .expect("le juge doit être exécutable");
    let _ = std::fs::remove_dir_all(&dossier);
    String::from_utf8_lossy(&sortie.stdout).trim().to_string()
}

fn evenement(nom: &str, phase: &str) -> String {
    format!("{{\"event\":\"{nom}\",\"phase\":\"{phase}\",\"status\":\"x\"}}\n")
}

/// Un promote complet est reconnu comme tel — et ce n'est PAS acquis : rendre le
/// juge muet ferait passer le test du dessous tout seul.
#[test]
fn un_promote_complet_est_prouve_complet() {
    let mut journal = String::new();
    for etape in ["build", "manifest", "cutover_finalize"] {
        journal.push_str(&evenement("step_started", etape));
        journal.push_str(&evenement("step_completed", etape));
    }
    let verdict = verdict_du_juge(&journal);
    assert!(verdict.starts_with("complete:"), "{verdict}");
    assert!(verdict.contains("cutover_finalize passed"), "{verdict}");
}

/// LE cas mesuré : le promote du 2026-09-06 13:53, tué pendant `build`.
/// Une étape démarrée, aucune terminée, et le script sortait `rc=0`.
#[test]
fn une_etape_DEMARREE_mais_jamais_terminee_est_une_mort_en_plein_vol() {
    let verdict = verdict_du_juge(&evenement("step_started", "build"));
    assert!(verdict.starts_with("incomplete:"), "{verdict}");
    assert!(
        verdict.contains("build"),
        "l'étape restée ouverte doit être NOMMÉE — sinon l'opérateur la cherche : {verdict}"
    );
}

/// L'autre moitié du prédicat : toutes les étapes closes, mais le cutover jamais
/// atteint. Sans ce second test, un juge qui ne regarderait QUE les orphelines
/// passerait pour correct — et laisserait filer deux des sept cas réels.
#[test]
fn des_etapes_toutes_CLOSES_sans_cutover_ne_prouvent_rien() {
    let mut journal = String::new();
    for etape in ["build", "manifest"] {
        journal.push_str(&evenement("step_started", etape));
        journal.push_str(&evenement("step_completed", etape));
    }
    let verdict = verdict_du_juge(&journal);
    assert!(verdict.starts_with("incomplete:"), "{verdict}");
    assert!(verdict.contains("cutover_finalize"), "{verdict}");
    assert!(
        verdict.contains("manifest"),
        "la dernière étape atteinte doit être dite : {verdict}"
    );
}

/// Un journal vide, une ligne illisible, un fichier absent : jamais vide, jamais
/// muet. Un verdict vide se lirait comme un succès dans le shell appelant.
#[test]
fn le_juge_n_est_JAMAIS_muet() {
    assert!(verdict_du_juge("").starts_with("incomplete:"));
    assert!(verdict_du_juge("ceci n'est pas du json\n").starts_with("incomplete:"));
    let sortie = std::process::Command::new("python3")
        .arg(juge_de_completion())
        .arg("/ce/chemin/n/existe/pas.jsonl")
        .output()
        .expect("le juge doit répondre même sur un chemin mort");
    let verdict = String::from_utf8_lossy(&sortie.stdout);
    assert!(verdict.trim().starts_with("incomplete:"), "{verdict}");
}

/// L'écrivain doit CONSULTER le juge. Corriger le juge sans le brancher
/// laisserait `on_promote_exit` décider sur `$?` — le défaut d'origine, intact.
#[test]
fn on_promote_exit_ne_decide_plus_sur_le_seul_code_de_retour() {
    let script = std::fs::read_to_string(
        racine_du_depot()
            .join("scripts")
            .join("release")
            .join("promote_live_safe.sh"),
    )
    .expect("promote_live_safe.sh doit être lisible");
    assert!(
        script.contains("promote_completion_evidence.py"),
        "l'écrivain doit appeler le juge de complétion"
    );
    assert!(
        script.contains("axon_promote_lease_release incomplete"),
        "et savoir écrire un statut terminal `incomplete`"
    );
    assert!(
        !script.contains(
            "if [[ \"$rc\" -eq 0 ]]; then\n    axon_promote_lease_release completed \
             \"promotion process exited with rc=0\"\n"
        ),
        "la décision fondée sur le seul `$rc` ne doit plus exister"
    );
}

/// REQ-AXO-902628 — le juge, joué sur les journaux RÉELS de ce poste.
///
/// Mesure du 2026-09-07 : 84 journaux, dont SEPT portent `status: completed`
/// sans preuve de cutover (cinq sans une seule étape terminée). Ce test ne fixe
/// pas ces comptes — ils bougent à chaque promote. Il vérifie que sur des
/// données réelles le juge sait rendre les DEUX verdicts, ce qu'aucune fixture
/// ne peut prouver.
///
/// PORTÉE, dite franchement : `.axon/live-release/attempts/` n'est pas versionné.
/// Sur une machine qui n'en a pas, ce test n'a rien à regarder et le dit au lieu
/// de rendre un vert silencieux.
#[test]
fn le_juge_rend_les_DEUX_verdicts_sur_les_journaux_reels() {
    let dossier = racine_du_depot()
        .join(".axon")
        .join("live-release")
        .join("attempts");
    let Ok(entrees) = std::fs::read_dir(&dossier) else {
        eprintln!(
            "REQ-AXO-902628 — aucun journal de tentative sous {} : ce contrôle n'a \
             RIEN vérifié sur ce poste (le répertoire n'est pas versionné).",
            dossier.display()
        );
        return;
    };
    let mut complets = 0usize;
    let mut incomplets = 0usize;
    for entree in entrees.flatten() {
        let chemin = entree.path();
        if chemin.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(sortie) = std::process::Command::new("python3")
            .arg(juge_de_completion())
            .arg(&chemin)
            .output()
        else {
            continue;
        };
        let verdict = String::from_utf8_lossy(&sortie.stdout);
        let verdict = verdict.trim();
        assert!(
            verdict.starts_with("complete:") || verdict.starts_with("incomplete:"),
            "verdict ni l'un ni l'autre sur {} : {verdict}",
            chemin.display()
        );
        if verdict.starts_with("complete:") {
            complets += 1;
        } else {
            incomplets += 1;
        }
    }
    if complets + incomplets == 0 {
        eprintln!("REQ-AXO-902628 — répertoire présent mais vide : rien n'a été vérifié.");
        return;
    }
    assert!(
        complets > 0 && incomplets > 0,
        "sur {} journaux réels, le juge doit rendre les DEUX verdicts — {complets} \
         complets et {incomplets} incomplets. Un juge qui n'en rend qu'un seul est \
         soit aveugle, soit paranoïaque, et rien ici ne le distinguerait.",
        complets + incomplets
    );
}

/// REQ-AXO-902628 — LA garde de la dette assumée : DEUX juges, UN seul verdict.
///
/// Le prédicat de complétion existe deux fois — en Python (`promote_completion_evidence.py`,
/// appelé par l'écrivain shell) et en Rust (`release_reconciler::preuve_de_completion`,
/// lu par la porte `promote_status`, qui ne doit pas forker un `python3` par appel).
/// Deux implémentations du même jugement DÉRIVENT, c'est une loi ; alors la surface
/// rementirait, autrement.
///
/// Cette garde refuse la dérive au caractère près, sur les journaux RÉELS du poste —
/// ce qu'aucune fixture ne peut prouver, parce que les vrais journaux portent des
/// formes que personne n'invente (14 étapes, lignes tronquées, phases inattendues).
///
/// PORTÉE, dite franchement : `.axon/live-release/attempts/` n'est pas versionné.
/// Sur une machine qui n'en a pas, ce contrôle n'a rien à regarder et il le DIT au
/// lieu de rendre un vert silencieux.
#[test]
fn les_DEUX_juges_rendent_LE_MEME_verdict_sur_les_journaux_reels() {
    let dossier = racine_du_depot()
        .join(".axon")
        .join("live-release")
        .join("attempts");
    let Ok(entrees) = std::fs::read_dir(&dossier) else {
        eprintln!(
            "REQ-AXO-902628 — aucun journal sous {} : la garde croisée n'a RIEN \
             vérifié sur ce poste (le répertoire n'est pas versionné).",
            dossier.display()
        );
        return;
    };
    let mut compares = 0usize;
    let mut complets = 0usize;
    for entree in entrees.flatten() {
        let chemin = entree.path();
        if chemin.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(contenu) = std::fs::read_to_string(&chemin) else {
            continue;
        };
        let rust = crate::release_reconciler::preuve_de_completion(&contenu);
        let Ok(sortie) = std::process::Command::new("python3")
            .arg(juge_de_completion())
            .arg(&chemin)
            .output()
        else {
            continue;
        };
        let python = String::from_utf8_lossy(&sortie.stdout).trim().to_string();
        assert_eq!(
            rust,
            python,
            "les deux juges DIVERGENT sur {} — l'écrivain shell et la porte Rust \
             ne jugeraient plus le même promote de la même façon.",
            chemin.display()
        );
        if rust.starts_with("complete:") {
            complets += 1;
        }
        compares += 1;
    }
    if compares == 0 {
        eprintln!("REQ-AXO-902628 — répertoire présent mais vide : rien n'a été comparé.");
        return;
    }
    // Sans cette seconde exigence, deux juges également aveugles se
    // confirmeraient l'un l'autre et la garde serait décorative.
    assert!(
        complets > 0 && complets < compares,
        "sur {compares} journaux réels, les deux juges doivent rendre les DEUX \
         verdicts ({complets} complets). Un accord parfait sur un seul verdict ne \
         distingue pas deux juges justes de deux juges aveugles."
    );
}

/// La même comparaison sur des formes CHOISIES — celles que le corpus réel ne
/// contient pas forcément : journal vide, ligne illisible, phase absente.
#[test]
fn les_DEUX_juges_coincident_AUSSI_sur_les_formes_limites() {
    let cas: Vec<String> = vec![
        String::new(),
        "ceci n'est pas du json\n".to_string(),
        evenement("step_started", "build"),
        format!(
            "{}{}",
            evenement("step_started", "cutover_finalize"),
            evenement("step_completed", "cutover_finalize")
        ),
        // Un événement SANS phase : le seul endroit où les deux langages
        // rendaient un mot différent (`None` contre `<none>`) avant d'être alignés.
        "{\"event\":\"step_started\"}\n".to_string(),
    ];
    for journal in cas {
        let rust = crate::release_reconciler::preuve_de_completion(&journal);
        let python = verdict_du_juge(&journal);
        assert_eq!(rust, python, "divergence sur le journal {journal:?}");
    }
}

/// REQ-AXO-902628 — la garde croisée SAIT-ELLE rougir ?
///
/// Les deux juges ont été alignés dans le même lot que la garde qui les compare.
/// Une garde d'équivalence qui n'a jamais échoué ne prouve rien — c'est exactement
/// le grief porté contre les neuf `tests/shell/test_promote_*.sh`. Ce mutant
/// REMET la divergence (le Python rendait `None` là où le Rust rend `<none>`) et
/// exige que la comparaison la voie.
///
/// Il garde deux choses d'un coup : que la comparaison a des dents, et que
/// l'alignement `PHASE_ABSENTE` ne disparaisse pas du Python sans bruit.
#[test]
#[allow(non_snake_case)]
fn MUTANT_la_garde_croisee_sait_ROUGIR() {
    const ALIGNEMENT: &str = "evenement.get(\"phase\") or PHASE_ABSENTE";
    let source = std::fs::read_to_string(juge_de_completion()).expect("le juge doit être lisible");
    assert!(
        source.contains(ALIGNEMENT),
        "l'alignement des deux juges a disparu du Python — la garde croisée ne \
         compare plus rien de comparable"
    );
    let mutant_source = source.replace(ALIGNEMENT, "evenement.get(\"phase\")");

    let dossier = std::env::temp_dir().join(format!(
        "axon-902628-mutant-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dossier).expect("dossier temporaire");
    let mutant = dossier.join("juge_mutant.py");
    std::fs::write(&mutant, mutant_source).expect("juge mutant");
    // Un événement SANS phase : la seule forme où les deux langages divergeaient.
    let journal = dossier.join("sans_phase.jsonl");
    std::fs::write(&journal, "{\"event\":\"step_started\"}\n").expect("journal");

    let sortie = std::process::Command::new("python3")
        .arg(&mutant)
        .arg(&journal)
        .output()
        .expect("le juge mutant doit s'exécuter");
    let python_mutant = String::from_utf8_lossy(&sortie.stdout).trim().to_string();
    let rust = crate::release_reconciler::preuve_de_completion("{\"event\":\"step_started\"}\n");
    let _ = std::fs::remove_dir_all(&dossier);

    assert_ne!(
        rust, python_mutant,
        "la comparaison des deux juges est AVEUGLE : elle accepte `{python_mutant}` \
         pour `{rust}`. Une garde d'équivalence qui ne sait pas rougir est décorative."
    );
    assert!(
        python_mutant.contains("None") && rust.contains("<none>"),
        "la divergence attendue est celle du rendu d'une phase absente — rust={rust} \
         mutant={python_mutant}"
    );
}
