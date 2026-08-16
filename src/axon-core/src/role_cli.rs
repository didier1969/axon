//! REQ-AXO-902338 — analyse d'arguments des binaires de RÔLE (`axon-brain`,
//! `axon-indexer`, `axon-core`).
//!
//! RÉUTILISE : `embedder::gpu_preflight::GPU_LIB_PROBE_FLAG` pour laisser passer
//! le drapeau interne de sonde GPU ; `runtime_boot::resource_release_identity`
//! pour lire l'identité écrite par le promote. Aucun analyseur d'arguments
//! partagé n'existait — vérifié via axon query "command line argument parsing
//! CLI flags version help for binaries" (aucun symbole Rust couvrant : pas de
//! `clap` au manifeste, et les 8 binaires de bench/diag parsent chacun leur
//! `std::env::args()` DANS leur propre `bin/*.rs`, donc hors d'atteinte d'une
//! bibliothèque). Les trois binaires de rôle, eux, n'en avaient aucun.
//!
//! Le défaut fermé ici
//! -------------------
//! Ces binaires n'avaient AUCUNE analyse d'arguments : `main()` faisait trois
//! lignes et appelait directement `run_brain()` / `run_indexer()`. Tout argument
//! était donc ignoré en silence, et le binaire faisait son comportement par
//! défaut — DÉMARRER un rôle d'ingestion complet.
//!
//! Le 2026-08-15 21:46, `./bin/axon-indexer --version` — une commande perçue
//! comme une lecture — a démarré un second indexeur sur l'hôte live. Le garde
//! d'écrivain l'a bien refusé (`flock` LOCK_NB ; il nomme le détenteur, et il
//! s'exécute AVANT le bootstrap du schéma), mais le processus avait déjà
//! journalisé son démarrage et émis des signaux de rôle. Le déclencheur, lui,
//! était bien ici : **un drapeau inconnu traité comme un argument nul**.
//!
//! Ce que ce module impose
//! -----------------------
//! 1. `--version` / `-V` : rend l'identité et sort 0. Sans rien démarrer.
//! 2. `--help` / `-h` : rend l'usage et sort 0. Sans rien démarrer.
//! 3. Tout autre argument : REFUS explicite, sortie 2, l'argument est nommé.
//!    Un binaire qui ignore ce qu'il ne comprend pas transforme une faute de
//!    frappe en opération.
//!
//! La décision est une fonction PURE ([`decide`]) : elle se teste sans processus,
//! sans environnement et sans effet de bord. Le seul enrobage impur est
//! [`handle`], qui lit `std::env::args()` et écrit sur la sortie standard.

use crate::embedder::gpu_preflight::GPU_LIB_PROBE_FLAG;

/// Code de sortie d'un usage invalide. 2 = convention Unix pour « mauvais
/// arguments », distinct de 1 (échec d'exécution) : un superviseur peut donc
/// distinguer « je ne sais pas démarrer ça » de « j'ai démarré et j'ai échoué ».
pub const EXIT_USAGE: i32 = 2;

/// Ce que le binaire doit faire des arguments reçus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleCliDecision {
    /// Aucun argument, ou un drapeau interne reconnu : démarrer le rôle.
    Run,
    /// Rendre l'identité du binaire et sortir 0.
    PrintVersion,
    /// Rendre l'usage et sortir 0.
    PrintHelp,
    /// Argument non reconnu : refuser en le nommant.
    Reject { arg: String },
}

/// Décide, sans aucun effet de bord, ce qu'il faut faire des arguments.
///
/// Le premier argument reconnu tranche, et le premier argument NON reconnu
/// refuse immédiatement — `--bogus --version` est un refus, pas une version.
/// Un drapeau mal orthographié ne doit jamais être « rattrapé » par un drapeau
/// valide qui le suit : c'est exactement ainsi qu'un argument inattendu se
/// retrouve exécuté.
pub fn decide(args: &[String]) -> RoleCliDecision {
    for arg in args {
        match arg.as_str() {
            // Drapeau INTERNE (REQ-AXO-902027) : le parent ré-exécute le binaire
            // en sonde `dlopen` jetable. Ce qui suit est son chemin de lib, pas
            // un argument à valider ici.
            GPU_LIB_PROBE_FLAG => return RoleCliDecision::Run,
            "--help" | "-h" => return RoleCliDecision::PrintHelp,
            "--version" | "-V" => return RoleCliDecision::PrintVersion,
            other => {
                return RoleCliDecision::Reject {
                    arg: other.to_string(),
                }
            }
        }
    }
    RoleCliDecision::Run
}

/// Identité du binaire, rendue sans démarrer quoi que ce soit.
///
/// Fonction PURE : les valeurs viennent de l'appelant, pas de l'environnement,
/// pour qu'un test puisse en épingler la forme exacte.
///
/// La distinction de provenance est le fond du message, pas de la décoration :
/// `package_version` est gravée dans CE binaire à la compilation, donc toujours
/// vraie de lui. Les trois autres viennent de l'environnement de l'appel (ou du
/// fichier d'identité écrit par le promote), pas du processus en service. Un
/// opérateur qui lit « build » sans savoir d'où il sort peut conclure faux sur
/// la version installée — dont la vérité reste `.axon/live-release/current.json`
/// croisé au `sha256sum` du binaire.
pub fn version_report(
    role: &str,
    package_version: &str,
    release_version: Option<&str>,
    build_id: Option<&str>,
    install_generation: Option<&str>,
) -> String {
    let mut out = format!("{role} {package_version}\n");
    out.push_str(&format!(
        "  package_version    {package_version}  (compilé dans ce binaire)\n"
    ));
    out.push_str(&field_line(
        "release_version",
        release_version,
        "AXON_RELEASE_VERSION",
    ));
    out.push_str(&field_line("build_id", build_id, "AXON_BUILD_ID"));
    out.push_str(&field_line(
        "install_generation",
        install_generation,
        "AXON_INSTALL_GENERATION",
    ));
    out.push_str(
        "\nCes trois derniers champs décrivent l'ENVIRONNEMENT de cet appel, pas le\n\
         processus en service. La version réellement installée se lit dans\n\
         .axon/live-release/current.json (runtime_version.build_id), croisée avec\n\
         le sha256 du binaire.",
    );
    out
}

fn field_line(name: &str, value: Option<&str>, env_var: &str) -> String {
    match value {
        Some(v) => format!("  {name:<18} {v}  (env {env_var})\n"),
        None => format!("  {name:<18} —  ({env_var} absent)\n"),
    }
}

/// Texte d'usage. Il dit explicitement ce que fait l'invocation SANS argument —
/// c'est la seule information qui manquait le jour de l'incident.
pub fn help_text(role: &str, what_it_starts: &str) -> String {
    format!(
        "{role} — {what_it_starts}\n\
         \n\
         USAGE\n\
         \x20 {role}                démarre le rôle et ne rend la main qu'à l'arrêt\n\
         \x20 {role} --version      rend l'identité de ce binaire, ne démarre rien\n\
         \x20 {role} --help         ce texte, ne démarre rien\n\
         \n\
         Tout autre argument est REFUSÉ (sortie {EXIT_USAGE}). Ce binaire est démarré par\n\
         le superviseur (process-compose) ; le lancer à la main sur un hôte où le\n\
         runtime tourne déjà se heurte au garde d'écrivain, qui refuse en nommant\n\
         le détenteur du verrou."
    )
}

/// Message de refus. Il nomme l'argument fautif ET rappelle l'enjeu : sans
/// refus, cet argument aurait DÉMARRÉ le rôle.
pub fn rejection_text(role: &str, arg: &str) -> String {
    format!(
        "{role}: argument inconnu « {arg} ».\n\
         Refus délibéré : sans lui, cet argument serait ignoré et le rôle DÉMARRERAIT.\n\
         Voir `{role} --help`."
    )
}

/// Enrobage impur appelé en tête de `main()` de chaque binaire de rôle.
///
/// Rend `None` quand il faut démarrer le rôle, `Some(code)` quand le processus
/// doit sortir immédiatement avec ce code.
pub fn handle(role: &str, what_it_starts: &str) -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match decide(&args) {
        RoleCliDecision::Run => None,
        RoleCliDecision::PrintVersion => {
            // Le promote écrit l'identité du manifeste dans ce fichier ; la
            // relire ici rend `--version` cohérent avec ce que le runtime
            // rapporterait (REQ-AXO-902064).
            crate::runtime_boot::resource_release_identity();
            let package_version = env!("CARGO_PKG_VERSION");
            println!(
                "{}",
                version_report(
                    role,
                    package_version,
                    std::env::var("AXON_RELEASE_VERSION").ok().as_deref(),
                    std::env::var("AXON_BUILD_ID").ok().as_deref(),
                    std::env::var("AXON_INSTALL_GENERATION").ok().as_deref(),
                )
            );
            Some(0)
        }
        RoleCliDecision::PrintHelp => {
            println!("{}", help_text(role, what_it_starts));
            Some(0)
        }
        RoleCliDecision::Reject { arg } => {
            eprintln!("{}", rejection_text(role, &arg));
            Some(EXIT_USAGE)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_argument_runs_the_role() {
        assert_eq!(decide(&[]), RoleCliDecision::Run);
    }

    #[test]
    fn version_and_help_never_run_the_role() {
        for flag in ["--version", "-V"] {
            assert_eq!(
                decide(&args(&[flag])),
                RoleCliDecision::PrintVersion,
                "`{flag}` doit rendre la version"
            );
        }
        for flag in ["--help", "-h"] {
            assert_eq!(
                decide(&args(&[flag])),
                RoleCliDecision::PrintHelp,
                "`{flag}` doit rendre l'usage"
            );
        }
    }

    /// LE test de l'incident : avant REQ-AXO-902338, chacun de ces arguments
    /// laissait démarrer un second rôle sur un hôte live.
    #[test]
    fn unknown_arguments_are_refused_never_silently_run() {
        for arg in [
            "--dry-run",
            "--check",
            "--versionn",
            "-v",
            "status",
            "--config=/etc/axon.toml",
        ] {
            assert_eq!(
                decide(&args(&[arg])),
                RoleCliDecision::Reject {
                    arg: arg.to_string()
                },
                "`{arg}` doit être refusé, pas exécuté comme un démarrage"
            );
        }
    }

    /// Un drapeau valide qui SUIT une faute de frappe ne doit pas la rattraper :
    /// sinon `--dry-runn --version` démarrerait au lieu de refuser.
    #[test]
    fn a_valid_flag_after_a_typo_does_not_rescue_it() {
        assert_eq!(
            decide(&args(&["--dry-runn", "--version"])),
            RoleCliDecision::Reject {
                arg: "--dry-runn".to_string()
            }
        );
    }

    /// Le drapeau interne de sonde GPU (REQ-AXO-902027) doit passer : le parent
    /// ré-exécute le binaire avec, et son chemin de lib le suit.
    #[test]
    fn internal_gpu_probe_flag_still_runs() {
        assert_eq!(
            decide(&args(&[GPU_LIB_PROBE_FLAG, "/nix/store/x/libonnxruntime.so"])),
            RoleCliDecision::Run
        );
    }

    #[test]
    fn version_report_names_the_provenance_of_every_field() {
        let out = version_report(
            "axon-indexer",
            "0.8.0",
            Some("v0.8.0-1493-g24ad3d31"),
            Some("v0.8.0-1493-g24ad3d31"),
            Some("live-20260815T203213Z"),
        );
        assert!(out.starts_with("axon-indexer 0.8.0\n"));
        assert!(out.contains("compilé dans ce binaire"));
        assert!(out.contains("(env AXON_BUILD_ID)"));
        assert!(
            out.contains("current.json"),
            "le rapport doit renvoyer vers la source autoritative de l'installé, \
             sinon il invite à conclure faux sur la version en service"
        );
    }

    #[test]
    fn version_report_marks_absent_fields_instead_of_inventing_them() {
        let out = version_report("axon-brain", "0.8.0", None, None, None);
        assert!(
            out.contains("(AXON_BUILD_ID absent)"),
            "un champ absent doit se dire absent, pas se replier en silence sur \
             la version du paquet : {out}"
        );
    }

    #[test]
    fn help_says_that_a_bare_invocation_starts_the_role() {
        let out = help_text("axon-indexer", "indexeur IST");
        assert!(out.contains("démarre le rôle"));
        assert!(out.contains("--version"));
    }

    #[test]
    fn rejection_names_the_offending_argument() {
        let out = rejection_text("axon-brain", "--dry-run");
        assert!(out.contains("--dry-run"));
        assert!(out.contains("DÉMARRERAIT"));
    }
}
