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

// ---------------------------------------------------------------------------
// REQ-AXO-902639 — DIRE quelle configuration d'indexation est en vigueur, et
// depuis quand.
//
// Le 2026-09-07, apres un promote verifie (`build_identity: match`,
// `truth_status: canonical`), CINQ `rescan_project full=true` ont rendu le meme
// `enrolled: 1243` — le compte d'AVANT le correctif — sans un mot. Le binaire
// servi etait bien le neuf. Ce qui ne l'etait pas, c'etait la configuration :
// `.axon/capabilities.toml` declarait `ignored_directory_segments`, cette cle
// REMPLACE le `#[serde(default = "...")]` compile (elle ne fusionne pas), et le
// `Lazy` avait ete force au demarrage du processus, donc l'edition du fichier
// n'existait pas encore pour lui. Deux autorites muettes empilees. Il a fallu
// un redemarrage pour voir `enrolled: 625` — delta 618, exactement l'attendu.
//
// CE QUE CE BLOC FAIT, ET CE QU'IL NE FAIT PAS. Il ne recharge rien a chaud et
// ne fusionne rien : ce sont deux autres decisions. Il dit seulement, a qui
// demande, d'ou vient la configuration en vigueur, a quel instant elle a ete
// lue, quelles cles surchargent le defaut compile, et si le fichier a bouge
// DEPUIS. C'est ce dernier signal qui aurait economise les cinq rescans.
// ---------------------------------------------------------------------------

/// D'ou vient la configuration en vigueur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    /// Le fichier lu, chemin absolu tel que resolu par la remontee des parents.
    File(String),
    /// Aucun `.axon/capabilities.toml` trouve : les defauts compiles.
    CompiledDefaults,
}

impl ConfigSource {
    pub fn label(&self) -> String {
        match self {
            ConfigSource::File(chemin) => chemin.clone(),
            ConfigSource::CompiledDefaults => "<defauts compiles>".to_string(),
        }
    }
}

/// Une cle DECLAREE dans le fichier, donc qui remplace le defaut compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigKeyOverride {
    pub key: String,
    /// Presents dans le fichier, absents du defaut compile.
    pub added: Vec<String>,
    /// Presents dans le defaut compile, absents du fichier — LE piege : une
    /// cle declaree remplace, elle ne complete pas.
    pub removed: Vec<String>,
    /// Pour une cle qui n'est pas une liste : sa valeur, rendue en texte.
    pub scalar: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigProvenance {
    pub source: ConfigSource,
    pub loaded_at_unix_ms: u64,
    pub overriding_keys: Vec<ConfigKeyOverride>,
}

/// Le fichier a change APRES la lecture : ce qui tourne n'est plus ce qui est
/// ecrit sur le disque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigStaleness {
    pub path: String,
    pub modified_at_unix_ms: u64,
    pub loaded_at_unix_ms: u64,
}

impl ConfigStaleness {
    pub fn age_ms(&self) -> u64 {
        self.modified_at_unix_ms
            .saturating_sub(self.loaded_at_unix_ms)
    }
}

fn maintenant_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// La regle de peremption, prise en ENTREE plutot qu'au disque : c'est ce qui
/// rend ses DEUX verdicts atteignables sans toucher un fichier.
pub fn staleness_from(
    path: &str,
    modified_at_unix_ms: u64,
    loaded_at_unix_ms: u64,
) -> Option<ConfigStaleness> {
    if modified_at_unix_ms <= loaded_at_unix_ms {
        return None;
    }
    Some(ConfigStaleness {
        path: path.to_string(),
        modified_at_unix_ms,
        loaded_at_unix_ms,
    })
}

/// Les cles DECLAREES dans `[indexing]`, comparees au defaut compile.
///
/// Serde ne peut pas repondre a cette question : `#[serde(default = "...")]`
/// rend une valeur identique que la cle ait ete ecrite ou omise. Il faut relire
/// le TOML en `toml::Value` pour savoir laquelle etait PRESENTE.
pub fn overriding_keys(indexing: &toml::value::Table) -> Vec<ConfigKeyOverride> {
    let listes: [(&str, Vec<String>); 3] = [
        ("supported_extensions", default_supported_extensions()),
        (
            "ignored_directory_segments",
            default_ignored_directory_segments(),
        ),
        (
            "soft_excluded_directory_segments_allowlist",
            default_soft_excluded_directory_segments_allowlist(),
        ),
    ];
    let mut out = Vec::new();
    for (cle, defaut) in listes {
        let Some(valeur) = indexing.get(cle) else {
            continue;
        };
        let Some(tableau) = valeur.as_array() else {
            continue;
        };
        let declares: Vec<String> = tableau
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        out.push(ConfigKeyOverride {
            key: cle.to_string(),
            added: declares
                .iter()
                .filter(|d| !defaut.contains(d))
                .cloned()
                .collect(),
            removed: defaut
                .iter()
                .filter(|d| !declares.contains(d))
                .cloned()
                .collect(),
            scalar: None,
        });
    }
    for cle in [
        "use_git_global_ignore",
        "legacy_axonignore_additive",
        "ignore_reconcile_enabled",
        "ignore_reconcile_dry_run",
    ] {
        let Some(valeur) = indexing.get(cle) else {
            continue;
        };
        out.push(ConfigKeyOverride {
            key: cle.to_string(),
            added: Vec::new(),
            removed: Vec::new(),
            scalar: Some(valeur.to_string()),
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

fn config_par_defaut() -> Config {
    Config {
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
    }
}

static CONFIG_PROVENANCE: std::sync::OnceLock<ConfigProvenance> = std::sync::OnceLock::new();

pub static CONFIG: Lazy<Config> = Lazy::new(|| {
    let (config, provenance) = load_config().unwrap_or_else(|_| {
        (
            config_par_defaut(),
            ConfigProvenance {
                source: ConfigSource::CompiledDefaults,
                loaded_at_unix_ms: maintenant_unix_ms(),
                overriding_keys: Vec::new(),
            },
        )
    });
    let _ = CONFIG_PROVENANCE.set(provenance);
    config
});

/// La provenance de la configuration EN VIGUEUR.
///
/// Force le `Lazy` : sans cela, un appelant qui demande la provenance avant tout
/// autre lecteur recevrait « pas encore chargee » et croirait a un defaut.
pub fn config_provenance() -> &'static ConfigProvenance {
    Lazy::force(&CONFIG);
    CONFIG_PROVENANCE.get().expect(
        "le Lazy vient d'etre force : la provenance est posee dans le meme bloc que la config",
    )
}

/// Le fichier lu a-t-il change depuis ? `None` quand il n'a pas bouge, quand la
/// source est le defaut compile, ou quand le fichier a disparu.
pub fn config_staleness() -> Option<ConfigStaleness> {
    let provenance = config_provenance();
    let ConfigSource::File(chemin) = &provenance.source else {
        return None;
    };
    let modifie = std::fs::metadata(chemin)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    staleness_from(chemin, modifie, provenance.loaded_at_unix_ms)
}

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
        "xml".to_string(),
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
        "cu".to_string(),
        "cuh".to_string(),
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
        "proto".to_string(),
        "graphql".to_string(),
        "gql".to_string(),
        "nix".to_string(),
        "dockerfile".to_string(),
        "tf".to_string(),
        "tfvars".to_string(),
        "service".to_string(),
        "timer".to_string(),
        "socket".to_string(),
        "target".to_string(),
        "rego".to_string(),
        "env".to_string(),
        "fbs".to_string(),
        "capnp".to_string(),
        "avsc".to_string(),
        "wit".to_string(),
        "erl".to_string(),
        "hrl".to_string(),
        "zig".to_string(),
        "jl".to_string(),
        "hs".to_string(),
        "lhs".to_string(),
        "ml".to_string(),
        "mli".to_string(),
        "re".to_string(),
        "scala".to_string(),
        "sc".to_string(),
        "clj".to_string(),
        "cljs".to_string(),
        "cljc".to_string(),
        "edn".to_string(),
        "drl".to_string(),
        "lua".to_string(),
        "groovy".to_string(),
        "gvy".to_string(),
        "gy".to_string(),
        "gsh".to_string(),
        "gradle".to_string(),
        "sh".to_string(),
        "bash".to_string(),
        "ebuild".to_string(),
        "mk".to_string(),
        "just".to_string(),
        "fish".to_string(),
        "zsh".to_string(),
        "zsh-theme".to_string(),
        "cmake".to_string(),
        "cql".to_string(),
        "cypher".to_string(),
        "prisma".to_string(),
        "esdl".to_string(),
        "edgeql".to_string(),
        "sparql".to_string(),
        "rq".to_string(),
        "ttl".to_string(),
        "cue".to_string(),
        "jsonnet".to_string(),
        "libsonnet".to_string(),
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

fn load_config() -> anyhow::Result<(Config, ConfigProvenance)> {
    // Try to find .axon/capabilities.toml in current or parent dirs
    let mut path = std::env::current_dir()?;
    loop {
        let config_path = path.join(".axon").join("capabilities.toml");
        if config_path.exists() {
            let content = fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&content)?;
            // REQ-AXO-902639 — second parcours, en `toml::Value` : serde a deja
            // remplace les cles absentes par leur defaut, il ne peut plus dire
            // lesquelles etaient ECRITES. C'est cette question-la qui manquait.
            let brut: toml::Value = toml::from_str(&content)?;
            let declarees = brut
                .get("indexing")
                .and_then(|v| v.as_table())
                .map(overriding_keys)
                .unwrap_or_default();
            let provenance = ConfigProvenance {
                source: ConfigSource::File(config_path.to_string_lossy().to_string()),
                loaded_at_unix_ms: maintenant_unix_ms(),
                overriding_keys: declarees,
            };
            return Ok((config, provenance));
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

    // -----------------------------------------------------------------------
    // REQ-AXO-902639 — la provenance et la peremption, sur des entrees
    // substituables : les DEUX verdicts doivent etre atteignables sans toucher
    // un fichier ni dependre de l'etat du poste.
    // -----------------------------------------------------------------------

    fn table(toml_source: &str) -> toml::value::Table {
        toml::from_str::<toml::Value>(toml_source)
            .expect("TOML de test")
            .get("indexing")
            .and_then(|v| v.as_table())
            .cloned()
            .unwrap_or_default()
    }

    #[test]
    fn une_cle_ABSENTE_du_fichier_ne_surcharge_rien() {
        let declarees = overriding_keys(&table("[indexing]\n"));
        assert!(
            declarees.is_empty(),
            "un fichier qui ne declare rien ne surcharge rien : {declarees:?}"
        );
    }

    #[test]
    fn une_cle_PRESENTE_est_signalee_meme_quand_elle_recopie_le_defaut() {
        // Le piege exact du 2026-09-07 : la cle recopiait le defaut PLUS trois
        // segments, et rien ne disait qu'elle remplacait le defaut compile.
        // Une cle presente doit se voir, meme identique — c'est elle qui decide.
        let declarees = overriding_keys(&table(
            "[indexing]\nignored_directory_segments = [\".fastembed_cache\"]\n",
        ));
        assert_eq!(declarees.len(), 1, "{declarees:?}");
        assert_eq!(declarees[0].key, "ignored_directory_segments");
        assert!(declarees[0].added.is_empty(), "{declarees:?}");
        // ... et ce qu'elle FAIT DISPARAITRE est nomme : c'est le sens du mot
        // « remplace ». Le defaut compile en porte quatre depuis REQ-AXO-902638.
        assert_eq!(
            declarees[0].removed,
            vec!["pg_wal", "_bmad", "_bmad-output"],
            "{declarees:?}"
        );
    }

    #[test]
    fn une_cle_qui_AJOUTE_et_une_cle_qui_RETIRE_se_distinguent() {
        let declarees = overriding_keys(&table(
            "[indexing]\nignored_directory_segments = [\".fastembed_cache\", \"pg_wal\", \
             \"_bmad\", \"_bmad-output\", \"node_modules\"]\n",
        ));
        assert_eq!(declarees.len(), 1, "{declarees:?}");
        assert_eq!(declarees[0].added, vec!["node_modules"], "{declarees:?}");
        assert!(declarees[0].removed.is_empty(), "{declarees:?}");
    }

    #[test]
    fn une_cle_scalaire_declaree_est_rendue_avec_sa_valeur() {
        let declarees = overriding_keys(&table("[indexing]\nuse_git_global_ignore = false\n"));
        assert_eq!(declarees.len(), 1, "{declarees:?}");
        assert_eq!(declarees[0].key, "use_git_global_ignore");
        assert_eq!(
            declarees[0].scalar.as_deref(),
            Some("false"),
            "{declarees:?}"
        );
    }

    #[test]
    fn la_peremption_dit_OUI_quand_le_fichier_a_bouge_APRES_la_lecture() {
        let verdict = staleness_from("/x/.axon/capabilities.toml", 2_000, 1_000)
            .expect("un fichier modifie apres la lecture DOIT etre signale");
        assert_eq!(verdict.path, "/x/.axon/capabilities.toml");
        assert_eq!(verdict.age_ms(), 1_000);
    }

    #[test]
    fn la_peremption_dit_NON_quand_le_fichier_n_a_pas_bouge() {
        // Les deux moities comptent. Une garde qui crierait toujours serait
        // ignoree en une semaine, et le signal du 2026-09-07 serait reperdu.
        assert!(
            staleness_from("/x", 1_000, 1_000).is_none(),
            "mtime == loaded"
        );
        assert!(staleness_from("/x", 999, 1_000).is_none(), "mtime < loaded");
    }

    #[test]
    fn la_provenance_en_vigueur_est_toujours_repondue() {
        // Elle force le `Lazy` : la question ne peut pas rendre « pas encore
        // chargee », qui se lirait comme « les defauts s'appliquent ».
        let provenance = config_provenance();
        assert!(
            provenance.loaded_at_unix_ms > 0,
            "l'instant du chargement doit etre conserve : {provenance:?}"
        );
        match &provenance.source {
            ConfigSource::File(chemin) => assert!(
                chemin.ends_with("capabilities.toml"),
                "la source nommee doit etre le fichier LU : {chemin}"
            ),
            ConfigSource::CompiledDefaults => {
                assert!(provenance.overriding_keys.is_empty())
            }
        }
    }
}
