//! REQ-AXO-902538 / REQ-AXO-902542 / REQ-AXO-902545 — le garde « deja en train de
//! servir » de `scripts/start.sh`, garde en CLASSE.
//!
//! Le defaut, signale par TROIS locataires independamment (OPV 2026-08-27,
//! APS 2026-08-28, OPV 2026-08-28) : `./scripts/axon-live start --indexer-graph`
//! voyait le port du brain occupe, imprimait « Healthy Axon already serving on
//! :44129. Stop first. » et sortait **rc=0** sans demarrer l'indexeur — pendant que
//! `status` prescrivait cette commande exacte comme remede. Le contrat de
//! recuperation etait donc inexecutable dans l'etat brain-only qu'il pretendait
//! corriger, et il se declarait reussi.
//!
//! POURQUOI EN RUST ET PAS EN SHELL. `tests/shell/*.sh` se lancent a la main, ils
//! ne passent pas dans la porte `GUI-AXO-1034` (meme raison que
//! `release_scripts_tests.rs`). Le compagnon `scripts/lib/axon-supervisor.test.sh`
//! couvre la DECISION de `axon_start_missing_roles` avec des I/O bouchonnees ;
//! celui-ci pin le CABLAGE, c'est-a-dire le seul endroit ou le defaut vivait.
//!
//! CE QUE CETTE GARDE NE PROUVE PAS, dit ici plutot que sous-entendu : elle lit du
//! texte. Elle ne demarre aucun indexeur et ne verifie aucun code de retour reel.
//! Elle interdit exactement une chose — qu'un `exit 0` de la branche « brain sain »
//! soit atteint sans avoir consulte les roles demandes — parce que c'est cette
//! forme-la, et pas une autre, qui a ete livree trois fois.

/// Racine du depot : `CARGO_MANIFEST_DIR` pointe sur `src/axon-core`.
fn racine_du_depot() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

const APPEL_ATTENDU: &str = "axon_start_missing_roles";
const OUVERTURE_DE_BRANCHE: &str = "if axon_brain_healthy";
const PRESENCE_REELLE: &str = "_axon_role_process_alive";
const DEMARRAGE_DE_ROLE: &str = "axon_restart_role_verified";

/// Extrait le corps de la branche `if axon_brain_healthy … then … fi`, en comptant
/// les `if`/`fi` de niveau ligne. Rend `None` quand la branche est absente ou non
/// refermee — les deux cas sont des echecs pour l'appelant, jamais un silence.
fn corps_de_la_branche_brain_sain(script: &str) -> Option<Vec<&str>> {
    let mut lignes = script.lines().enumerate();
    let (debut, _) = lignes.find(|(_, l)| l.trim_start().starts_with(OUVERTURE_DE_BRANCHE))?;
    let mut profondeur = 1usize;
    let mut corps = Vec::new();
    for ligne in script.lines().skip(debut + 1) {
        let nu = ligne.trim();
        if nu == "fi" {
            profondeur -= 1;
            if profondeur == 0 {
                return Some(corps);
            }
        } else if nu.starts_with("if ") {
            profondeur += 1;
        }
        corps.push(ligne);
    }
    None
}

/// Le verdict, PUR, pour pouvoir etre joue sur le script reel ET sur un mutant.
/// `Ok(())` = la branche consulte les roles demandes avant tout succes.
fn verdict_du_garde(script: &str) -> Result<(), String> {
    let Some(corps) = corps_de_la_branche_brain_sain(script) else {
        return Err(format!(
            "branche `{OUVERTURE_DE_BRANCHE}` absente ou non refermee — le garde ne peut plus etre situe"
        ));
    };
    let rang_appel = corps
        .iter()
        .position(|l| l.contains(APPEL_ATTENDU) && !l.trim_start().starts_with('#'));
    let Some(rang_appel) = rang_appel else {
        return Err(format!(
            "la branche « brain sain » ne consulte JAMAIS les roles demandes (`{APPEL_ATTENDU}` absent) : \
             un `start --indexer-graph` y redevient un no-op qui se declare reussi"
        ));
    };
    for (rang, ligne) in corps.iter().enumerate() {
        let nu = ligne.trim();
        if nu.starts_with('#') {
            continue;
        }
        if nu == "exit 0" && rang < rang_appel {
            return Err(format!(
                "`exit 0` ligne {} de la branche, AVANT l'appel a `{APPEL_ATTENDU}` (ligne {}) : \
                 le start sort en succes sans avoir rien demarre",
                rang + 1,
                rang_appel + 1
            ));
        }
    }
    Ok(())
}

/// Le verdict de la fonction de completion, PUR lui aussi : lu sur la lib reelle par
/// la garde, et sur les mutants par `MUTANT_la_garde_de_presence_sait_ROUGIR_*`.
/// `Ok(())` = elle consulte la presence REELLE du processus avant tout demarrage.
fn verdict_de_la_completion(source: &str) -> Result<(), String> {
    let Some(debut) = source.find(&format!("{APPEL_ATTENDU}()")) else {
        return Err(format!(
            "`{APPEL_ATTENDU}` n'est pas DEFINIE : start.sh l'appelle, une definition absente \
             rendrait le garde muet a l'execution"
        ));
    };
    let corps = &source[debut..];
    let fin = corps.find("\n}\n").map(|i| i + 2).unwrap_or(corps.len());
    let corps = &corps[..fin];
    let Some(rang_demarrage) = corps.find(DEMARRAGE_DE_ROLE) else {
        return Err(format!(
            "`{APPEL_ATTENDU}` ne sait pas demarrer un role absent (`{DEMARRAGE_DE_ROLE}` absent) : \
             elle ne repare plus rien"
        ));
    };
    let Some(rang_presence) = corps.find(PRESENCE_REELLE) else {
        return Err(format!(
            "`{APPEL_ATTENDU}` ne consulte JAMAIS `{PRESENCE_REELLE}` avant de demarrer un role : \
             un /readyz muet n'est pas une absence, et doubler un role vivant fait un takeover du \
             verrou d'ecrivain IST qui TUE le processus en train de travailler (mesure du 2026-09-07)"
        ));
    };
    if rang_presence >= rang_demarrage {
        return Err(format!(
            "`{PRESENCE_REELLE}` est consulte APRES `{DEMARRAGE_DE_ROLE}` : le duplicata est deja \
             parti quand la question est posee — la consultation doit PRECEDER le demarrage"
        ));
    }
    Ok(())
}

fn start_script() -> String {
    let chemin = racine_du_depot().join("scripts").join("start.sh");
    std::fs::read_to_string(&chemin)
        .unwrap_or_else(|e| panic!("scripts/start.sh doit etre lisible ({}) : {e}", chemin.display()))
}

#[test]
fn un_brain_sain_ne_fait_plus_sortir_le_start_en_succes_sans_verifier_les_roles() {
    if let Err(motif) = verdict_du_garde(&start_script()) {
        panic!("REQ-AXO-902538/902542/902545 — {motif}");
    }
}

#[test]
fn la_fonction_de_completion_des_roles_existe_dans_la_lib_superviseur() {
    let chemin = racine_du_depot()
        .join("scripts")
        .join("lib")
        .join("axon-supervisor.sh");
    let source = std::fs::read_to_string(&chemin).expect("axon-supervisor.sh doit etre lisible");
    assert!(
        source.contains(&format!("{APPEL_ATTENDU}()")),
        "`{APPEL_ATTENDU}` doit etre DEFINIE dans scripts/lib/axon-supervisor.sh — \
         start.sh l'appelle, une definition absente rendrait le garde muet a l'execution"
    );
}

/// REQ-AXO-902538 & al., SECOND defaut — celui-la a ete paye sur le runtime live.
///
/// Le premier correctif ne connaissait que deux etats : « sert son /readyz » ou « ne
/// sert pas ». Sur un indexeur live qui TOURNAIT et vectorisait a 3,9 GiB mais dont le
/// /readyz etait muet, il a envoye un start. Le duplicata a pris le verrou d'ecrivain
/// IST et a SIGTERM le vivant (« IST writer takeover » dans le log de l'indexeur), puis
/// le superviseur a boucle : 12 redemarrages.
///
/// Un endpoint muet n'est PAS une preuve d'absence. La fonction doit donc consulter la
/// PRESENCE REELLE du processus avant tout demarrage. Cette garde pin cette
/// consultation : sans elle, le duplicata destructeur revient en silence.
#[test]
fn la_completion_des_roles_consulte_la_presence_reelle_avant_de_demarrer() {
    let chemin = racine_du_depot()
        .join("scripts")
        .join("lib")
        .join("axon-supervisor.sh");
    let source = std::fs::read_to_string(&chemin).expect("axon-supervisor.sh doit etre lisible");
    if let Err(motif) = verdict_de_la_completion(&source) {
        panic!("REQ-AXO-902538/902542/902545 — {motif}");
    }
}

/// ⛔ La garde ci-dessus lit un fichier reel : sans mutant elle n'a jamais rougi, et
/// une garde qui n'a jamais rougi ne prouve rien (pratique gouvernee 2314). Ce test
/// rejoue la fonction A DEUX ETATS — celle qui a reellement tue l'indexeur live le
/// 2026-09-07 — et exige que le verdict la REFUSE en nommant ce qui manque.
#[test]
#[allow(non_snake_case)]
fn MUTANT_la_garde_de_presence_sait_ROUGIR_sur_la_version_a_DEUX_etats() {
    let deux_etats = r#"
axon_start_missing_roles() {
    local instance_kind="${1:?}" budget_s="${2:?}"
    shift 2
    local proc failed=()
    for proc in "$@"; do
        if _axon_role_serving "$instance_kind" "$proc"; then
            continue
        fi
        if ! axon_restart_role_verified "$instance_kind" "$proc" "$budget_s"; then
            failed+=("$proc")
        fi
    done
    (( ${#failed[@]} == 0 ))
}
"#;
    let verdict = verdict_de_la_completion(deux_etats);
    assert!(
        verdict.is_err(),
        "la version a DEUX etats doit etre refusee — c'est elle qui a envoye un start sur un \
         indexeur vivant et provoque le takeover du verrou d'ecrivain IST"
    );
    assert!(
        verdict.unwrap_err().contains(PRESENCE_REELLE),
        "le motif doit nommer `{PRESENCE_REELLE}`, sinon il n'oriente pas la reparation"
    );

    let trop_tard = r#"
axon_start_missing_roles() {
    local proc
    for proc in "$@"; do
        axon_restart_role_verified "$instance_kind" "$proc" "$budget_s" || true
        _axon_role_process_alive "$project_root" "$instance_kind" "$proc" || true
    done
}
"#;
    assert!(
        verdict_de_la_completion(trop_tard).is_err(),
        "consulter la presence APRES avoir demarre ne protege rien : le duplicata est deja parti"
    );

    let conforme = r#"
axon_start_missing_roles() {
    local proc
    for proc in "$@"; do
        if _axon_role_serving "$instance_kind" "$proc"; then
            continue
        elif _axon_role_process_alive "$project_root" "$instance_kind" "$proc"; then
            presents_muets+=("$proc")
        else
            axon_restart_role_verified "$instance_kind" "$proc" "$budget_s"
        fi
    done
}
"#;
    assert!(
        verdict_de_la_completion(conforme).is_ok(),
        "la forme a TROIS etats doit passer — une garde qui refuse aussi le remede est inutilisable"
    );
}

/// ⛔ Une garde qui n'a jamais rougi ne prouve rien (pratique gouvernee 2314). Ce
/// test rejoue le code d'ORIGINE — pas une variante plausible — et exige que le
/// verdict le REFUSE. Les deux mutants sont les deux formes du defaut : sortir en
/// succes sans consulter, et consulter trop tard.
#[test]
#[allow(non_snake_case)]
fn MUTANT_le_garde_sait_ROUGIR_sur_le_code_d_origine() {
    let origine = r#"
if ! axon_port_is_free "$AXON_BRAIN_PORT"; then
    if axon_brain_healthy "$AXON_BRAIN_PORT"; then
        echo "Healthy Axon already serving on :$AXON_BRAIN_PORT. Stop first."
        exit 0
    fi
    echo "reclaiming"
fi
"#;
    let verdict = verdict_du_garde(origine);
    assert!(
        verdict.is_err(),
        "le code d'ORIGINE doit etre refuse — s'il passe, cette garde ne garde rien"
    );
    assert!(
        verdict.unwrap_err().contains(APPEL_ATTENDU),
        "le motif du refus doit nommer l'appel manquant, sinon il n'oriente pas la reparation"
    );

    let trop_tard = r#"
if ! axon_port_is_free "$AXON_BRAIN_PORT"; then
    if axon_brain_healthy "$AXON_BRAIN_PORT"; then
        exit 0
        axon_start_missing_roles "$AXON_INSTANCE_KIND" 180 axon-brain
    fi
fi
"#;
    assert!(
        verdict_du_garde(trop_tard).is_err(),
        "un `exit 0` place AVANT la consultation doit etre refuse : c'est le meme no-op, deplace"
    );

    let conforme = r#"
if ! axon_port_is_free "$AXON_BRAIN_PORT"; then
    if axon_brain_healthy "$AXON_BRAIN_PORT"; then
        if axon_start_missing_roles "$AXON_INSTANCE_KIND" 180 axon-brain axon-indexer; then
            exit 0
        fi
        exit 1
    fi
fi
"#;
    assert!(
        verdict_du_garde(conforme).is_ok(),
        "la forme corrigee doit passer — une garde qui refuse aussi le remede est inutilisable"
    );
}

/// La branche imbriquee ne doit pas faire perdre le `fi` de fermeture : sans le
/// comptage de profondeur, l'extraction s'arreterait au premier `fi` interne et la
/// garde deviendrait aveugle a tout ce qui suit.
#[test]
fn l_extraction_de_branche_compte_les_if_imbriques() {
    let script = r#"
if axon_brain_healthy "$P"; then
    if quelque_chose; then
        echo interne
    fi
    exit 0
fi
"#;
    let corps = corps_de_la_branche_brain_sain(script).expect("la branche doit etre trouvee");
    assert!(
        corps.iter().any(|l| l.trim() == "exit 0"),
        "l'`exit 0` situe APRES un `fi` interne doit rester dans le corps extrait"
    );
}
