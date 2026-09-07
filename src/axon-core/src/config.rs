use once_cell::sync::Lazy;
use serde::Deserialize;
use std::fs;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub indexing: IndexingConfig,
}

#[derive(Debug, Deserialize)]
pub struct IndexingConfig {
    #[serde(default = "default_supported_extensions")]
    pub supported_extensions: Vec<String>,
    #[serde(default = "default_ignored_directory_segments")]
    pub ignored_directory_segments: Vec<String>,
    #[serde(default = "default_soft_excluded_directory_segments_allowlist")]
    pub soft_excluded_directory_segments_allowlist: Vec<String>,
    #[serde(default = "default_use_git_global_ignore")]
    pub use_git_global_ignore: bool,
    #[serde(default = "default_legacy_axonignore_additive")]
    pub legacy_axonignore_additive: bool,
    #[serde(default = "default_ignore_reconcile_enabled")]
    pub ignore_reconcile_enabled: bool,
    #[serde(default = "default_ignore_reconcile_dry_run")]
    pub ignore_reconcile_dry_run: bool,
}

pub static CONFIG: Lazy<Config> = Lazy::new(|| {
    load_config().unwrap_or_else(|_| Config {
        indexing: IndexingConfig {
            supported_extensions: default_supported_extensions(),
            ignored_directory_segments: default_ignored_directory_segments(),
            soft_excluded_directory_segments_allowlist:
                default_soft_excluded_directory_segments_allowlist(),
            use_git_global_ignore: default_use_git_global_ignore(),
            legacy_axonignore_additive: default_legacy_axonignore_additive(),
            ignore_reconcile_enabled: default_ignore_reconcile_enabled(),
            ignore_reconcile_dry_run: default_ignore_reconcile_dry_run(),
        },
    })
});

/// REQ-AXO-902632 — la racine du parc, resolue a UN endroit. Le defaut en dur
/// vient de `runtime_boot`, ou il etait ecrit ; il n'est pas invente ici.
pub fn projects_root_dir() -> String {
    std::env::var("AXON_PROJECTS_ROOT").unwrap_or_else(|_| "/home/dstadel/projects".to_string())
}

/// REQ-AXO-902632 — LA racine de surveillance : celle depuis laquelle la marche
/// de reconciliation enumere et depuis laquelle la purge juge l'eligibilite.
///
/// Elle doit etre lisible AILLEURS que dans l'indexeur : `rescan_project` tourne
/// cote brain et enrolait des fichiers que cette racine-ci exclut — 8 190 lignes
/// `status='discovered'` a `content_hash` vide pour le tenant DFD, que la marche
/// n'a jamais parsees et que la purge a effacees. L'outil promettait un
/// enrolement que le systeme defaisait.
pub fn watch_root_dir() -> String {
    std::env::var("AXON_WATCH_DIR").unwrap_or_else(|_| projects_root_dir())
}

fn default_supported_extensions() -> Vec<String> {
    vec![
        "py".to_string(),
        "ex".to_string(),
        "exs".to_string(),
        "rs".to_string(),
        "go".to_string(),
        "java".to_string(),
        "c".to_string(),
        "cpp".to_string(),
        "h".to_string(),
        "js".to_string(),
        "jsx".to_string(),
        "ts".to_string(),
        "tsx".to_string(),
        "sql".to_string(),
        "md".to_string(),
        "markdown".to_string(),
        "txt".to_string(),
        "json".to_string(),
        "yml".to_string(),
        "yaml".to_string(),
        "toml".to_string(),
        "conf".to_string(),
        "html".to_string(),
        "css".to_string(),
        // REQ-AXO-902631 — ces extensions ont TOUJOURS eu un parser
        // (`parser::get_parser_for_file`) et n'ont jamais franchi ce filtre : le
        // scanner les ecartait avant que le parser existe pour elles. Mesure du
        // 2026-09-06 : 1 054 fichiers du parc, dont 716 `.hpp` — et 100 % des
        // en-tetes du tenant MRG, qui s'en plaignait par courrier. Les parsers
        // C#, Ruby, Kotlin, PHP et Scheme etaient INATTEIGNABLES en production.
        // L'invariant est tenu par `le_scanner_admet_tout_ce_que_le_parser_sait_lire`.
        "hpp".to_string(),
        "cc".to_string(),
        "cxx".to_string(),
        "hxx".to_string(),
        "cs".to_string(),
        "rb".to_string(),
        "ruby".to_string(),
        "kt".to_string(),
        "kts".to_string(),
        "php".to_string(),
        "scm".to_string(),
        "ss".to_string(),
        "sld".to_string(),
        "sls".to_string(),
        "htm".to_string(),
        "scss".to_string(),
        "tql".to_string(),
        "typeql".to_string(),
        "dl".to_string(),
        "datalog".to_string(),
        "ini".to_string(),
        // llmlang: a `.lll` file is parsed by the shell-out bridge (parser/lll.rs
        // → `lll export-ist`), which yields semantic symbols (content-hash,
        // purity, contracts). Without this the scanner excludes it pre-parse
        // (ignored_by_extension) — REQ-LLL-021.
        "lll".to_string(),
    ]
}

/// REQ-AXO-902638 — ARBITRÉ PAR L'OPÉRATEUR le 2026-09-07 : les trois segments
/// que `blocked_subtree_hint_segments` portait sans jamais les appliquer sont
/// ADMIS dans la seule liste d'exclusion vivante.
///
/// L'arithmétique de la décision, mesurée sur `ist.indexedfile` avant qu'elle
/// soit prise : `_bmad` 608 fichiers indexés, `_bmad-output` 10, `pg_wal` 0 — les
/// segments WAL n'ayant pas d'extension, le filtre d'extension les écartait déjà.
/// Les 618 lignes partent d'un tenant tiers (OptiPlanner) à la prochaine purge.
///
/// Réversible dans les deux sens : retirer un segment d'ici le fait revenir au
/// `rescan_project` suivant. Rien n'est détruit.
fn default_ignored_directory_segments() -> Vec<String> {
    vec![
        ".fastembed_cache".to_string(),
        "pg_wal".to_string(),
        "_bmad".to_string(),
        "_bmad-output".to_string(),
    ]
}

// REQ-AXO-902634 — `default_blocked_subtree_hint_segments`,
// `default_subtree_hint_cooldown_ms` et `default_subtree_hint_retry_budget` ont
// ete RETIREES le 2026-09-07. Elles parametraient un tampon de hints de
// sous-arbre que `REQ-AXO-901893` a arrache (`pg_notify` -> listener ->
// ingress_buffer, « both ripped ») et dont le consommateur promis,
// `record_subtree_hint`, n'a jamais existe comme fonction.
//
// Les TROIS segments que la liste portait en plus de
// `ignored_directory_segments` — `pg_wal`, `_bmad`, `_bmad-output` — ont ete
// ARBITRES le 2026-09-07 (REQ-AXO-902638) : l'operateur les ADMET, et ils vivent
// desormais dans `default_ignored_directory_segments` ci-dessus, seule liste
// d'exclusion dure. La garde
// `la_politique_d_exclusion_de_repertoires_n_a_qu_une_liste_et_aucun_champ_orphelin`
// tient l'invariant : une seule liste, aucun champ orphelin.

fn default_soft_excluded_directory_segments_allowlist() -> Vec<String> {
    Vec::new()
}

fn default_use_git_global_ignore() -> bool {
    false
}

fn default_legacy_axonignore_additive() -> bool {
    true
}

fn default_ignore_reconcile_enabled() -> bool {
    true
}

fn default_ignore_reconcile_dry_run() -> bool {
    true
}

fn load_config() -> anyhow::Result<Config> {
    // Try to find .axon/capabilities.toml in current or parent dirs
    let mut path = std::env::current_dir()?;
    loop {
        let config_path = path.join(".axon").join("capabilities.toml");
        if config_path.exists() {
            let content = fs::read_to_string(config_path)?;
            let config: Config = toml::from_str(&content)?;
            return Ok(config);
        }
        if !path.pop() {
            break;
        }
    }
    anyhow::bail!("Config not found")
}

/// REQ-AXO-902190 lot 3 — coverage for the `serde(default = "...")` value
/// providers. Trivial in isolation, but each IS the load-bearing fallback
/// when `.axon/capabilities.toml` omits a field; a silent value change here
/// changes indexing behavior repo-wide without any config diff to review.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_supported_extensions_covers_core_languages_and_lll() {
        let exts = default_supported_extensions();
        for expected in ["rs", "py", "ex", "md", "lll"] {
            assert!(
                exts.iter().any(|e| e == expected),
                "expected {expected} in {exts:?}"
            );
        }
    }

    /// REQ-AXO-902631 — L'INVARIANT MANQUANT. Deux tables decrivaient la meme
    /// chose sans jamais se confronter : `parser::get_parser_for_file` (ce que
    /// le systeme sait lire) et cette liste (ce que le scanner laisse passer).
    /// La seconde est evaluee EN AMONT de la premiere, donc toute extension
    /// presente la-bas et absente ici rend son parser inatteignable — sans une
    /// seule erreur, sans une seule ligne de log. Mesure du defaut : 1 054
    /// fichiers du parc, 716 `.hpp`, et cinq langages entiers muets.
    ///
    /// La relation est DIRECTIONNELLE, pas une egalite : le scanner a le droit
    /// d'admettre PLUS (`json`, `toml` sont indexes comme texte, sans parser).
    #[test]
    fn le_scanner_admet_tout_ce_que_le_parser_sait_lire() {
        let admises = default_supported_extensions();
        let inatteignables: Vec<&str> = crate::parser::PARSEABLE_EXTENSIONS
            .iter()
            .copied()
            .filter(|ext| !admises.iter().any(|a| a == ext))
            .collect();
        assert!(
            inatteignables.is_empty(),
            "ces extensions ont un parser mais le scanner ne les laisse jamais passer \
             — leur parser est inatteignable en production : {inatteignables:?}"
        );
    }

    #[test]
    fn default_ignored_directory_segments_excludes_fastembed_cache() {
        // REQ-AXO-902638 — les trois segments arbitres le 2026-09-07 s'ajoutent au
        // cache d'embeddings. L'egalite EXACTE est voulue : elle fait rougir tout
        // ajout ou retrait silencieux, y compris une reintroduction de la liste
        // morte que `REQ-AXO-902634` a enterree.
        assert_eq!(
            default_ignored_directory_segments(),
            vec![".fastembed_cache", "pg_wal", "_bmad", "_bmad-output"]
        );
    }

    #[test]
    fn default_soft_excluded_directory_segments_allowlist_is_empty() {
        assert!(default_soft_excluded_directory_segments_allowlist().is_empty());
    }

    #[test]
    fn default_use_git_global_ignore_is_false() {
        assert!(!default_use_git_global_ignore());
    }

    #[test]
    fn default_legacy_axonignore_additive_is_true() {
        assert!(default_legacy_axonignore_additive());
    }

    #[test]
    fn default_ignore_reconcile_enabled_is_true() {
        assert!(default_ignore_reconcile_enabled());
    }

    #[test]
    fn default_ignore_reconcile_dry_run_is_true() {
        assert!(default_ignore_reconcile_dry_run());
    }

    /// REQ-AXO-902634 — la politique d'exclusion de repertoires n'a qu'UNE
    /// liste, et aucun champ de `IndexingConfig` ne survit sans lecteur.
    ///
    /// La destructuration est EXHAUSTIVE (pas de `..`) : ajouter un champ a
    /// `IndexingConfig` casse la COMPILATION de cette garde tant que l'auteur
    /// ne l'a pas nomme ici, donc tant qu'il n'a pas dit qui le lit. C'est le
    /// seul point du crate ou l'oubli est mecaniquement impossible.
    ///
    /// Le defaut qu'elle interdit a coute : `blocked_subtree_hint_segments`
    /// portait TROIS segments de plus que `ignored_directory_segments`
    /// (`_bmad`, `_bmad-output`, `pg_wal`) et avait bien un lecteur — mais un
    /// lecteur qu'aucun chemin de production n'atteignait. Deux listes pour
    /// une seule verite, exactement la forme de `supported_extensions`
    /// (REQ-AXO-902636). Mesure du 2026-09-07 : 618 fichiers `_bmad` indexes
    /// pendant que la cle censee les exclure etait tenue pour active.
    ///
    /// La garde ne se contente pas de compter les champs : elle PROUVE que
    /// `ignored_directory_segments` change le verdict de l'autorite vivante
    /// (`classify_path`, celle que le walker consulte via
    /// `scanner::is_noise_directory`). Une garde qui compare une constante a
    /// elle-meme est morte-nee.
    #[test]
    fn la_politique_d_exclusion_de_repertoires_n_a_qu_une_liste_et_aucun_champ_orphelin() {
        use crate::indexing_policy::{classify_path, PathDisposition};
        use std::path::Path;

        let config = IndexingConfig {
            supported_extensions: default_supported_extensions(),
            ignored_directory_segments: default_ignored_directory_segments(),
            soft_excluded_directory_segments_allowlist:
                default_soft_excluded_directory_segments_allowlist(),
            use_git_global_ignore: default_use_git_global_ignore(),
            legacy_axonignore_additive: default_legacy_axonignore_additive(),
            ignore_reconcile_enabled: default_ignore_reconcile_enabled(),
            ignore_reconcile_dry_run: default_ignore_reconcile_dry_run(),
        };

        // Chaque champ est nomme avec le lecteur de PRODUCTION qui le consulte.
        // Un champ sans lecteur nommable n'a rien a faire ici.
        let IndexingConfig {
            // `Scanner::should_process_path` — filtre d'extension.
            supported_extensions: _,
            // `indexing_policy::classify_path` — l'autorite que le walker
            // consulte a la descente via `scanner::is_noise_directory`.
            ignored_directory_segments: _,
            // `indexing_policy::classify_internal` — reouverture d'une
            // exclusion douce.
            soft_excluded_directory_segments_allowlist: _,
            // `Scanner::build_walker_from`.
            use_git_global_ignore: _,
            // `Scanner::is_ignored_by_legacy_axonignore`.
            legacy_axonignore_additive: _,
            // Reconciliation des regles d'ignore (`scanner::reconcile_*`).
            ignore_reconcile_enabled: _,
            ignore_reconcile_dry_run: _,
        } = &config;

        // UNE seule liste d'exclusion dure. Une deuxieme rouvrirait la porte
        // au defaut : deux tables qui ne se confrontent jamais.
        let root = Path::new("/w");
        let cible = Path::new("/w/proj/segment_temoin/fichier");
        assert!(
            matches!(
                classify_path(root, cible, &config, &[]),
                PathDisposition::Allow
            ),
            "le temoin doit passer AVANT qu'on l'exclue, sinon la mesure suivante ne prouve rien"
        );

        let mut config_exclu = IndexingConfig {
            supported_extensions: config.supported_extensions.clone(),
            ignored_directory_segments: config.ignored_directory_segments.clone(),
            soft_excluded_directory_segments_allowlist: config
                .soft_excluded_directory_segments_allowlist
                .clone(),
            use_git_global_ignore: config.use_git_global_ignore,
            legacy_axonignore_additive: config.legacy_axonignore_additive,
            ignore_reconcile_enabled: config.ignore_reconcile_enabled,
            ignore_reconcile_dry_run: config.ignore_reconcile_dry_run,
        };
        config_exclu
            .ignored_directory_segments
            .push("segment_temoin".to_string());
        assert!(
            !matches!(
                classify_path(root, cible, &config_exclu, &[]),
                PathDisposition::Allow
            ),
            "`ignored_directory_segments` doit changer le verdict de `classify_path` — \
             sinon la cle est decorative et l'operateur reglera dans le vide"
        );
    }
}
