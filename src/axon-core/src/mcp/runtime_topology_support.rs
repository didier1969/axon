use crate::bridge::RuntimeTruthFeed;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

/// REQ-AXO-901859 — canonical freshness window for the indexer lifecycle
/// heartbeat (`axon.EmbedderLifecycleHeartbeat`). Shared by the
/// runtime status composer and the topology snapshot so both judge indexer
/// liveness against the SAME threshold (PIL-AXO-001 single source of truth,
/// no duplicated value). Tick is ~5 s; 30 s tolerates a few missed ticks.
pub(crate) const EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS: i64 = 30_000;

/// REQ-AXO-901859 — the SINGLE canonical indexer liveness verdict, derived
/// solely from the PG heartbeat (`axon.EmbedderLifecycleHeartbeat`).
/// There is intentionally NO file/shadow-role fallback: under separate
/// brain/indexer processes the file feed false-negatives, and that second
/// source is exactly what let `status` and `embedding_status` disagree
/// (PIL-AXO-001). If the indexer has not published a fresh heartbeat it is
/// not provably alive — say so loudly rather than infer from launch mode.
pub(crate) struct IndexerLiveness {
    pub(crate) feed: RuntimeTruthFeed,
    pub(crate) ready: bool,
    /// Fail-loud provenance: `pg_heartbeat` (fresh row), `pg_heartbeat_stale`
    /// (row present but past the window), `no_heartbeat` (row absent).
    pub(crate) source: &'static str,
    /// REQ-AXO-902021 — operator/LLM-readable lifecycle verdict so `status`
    /// distinguishes a crashed/abandoned indexer (a heartbeat row that WENT
    /// stale = it was provably alive, then stopped publishing) from one that
    /// never published (absent row), instead of a flat "idle" that hid the
    /// crash-loop. `healthy` | `crashed_or_abandoned` | `never_launched` |
    /// `stopped_or_idle` | `exited_clean` (REQ-AXO-902581).
    pub(crate) lifecycle: &'static str,
    /// REQ-AXO-902581 — `observed` ou `inferred`. Le verdict dit d'où il vient :
    /// une inférence présentée comme une observation a coûté deux sessions
    /// entières de chasse à un crash qui n'existait pas.
    pub(crate) certainty: &'static str,
}

/// REQ-AXO-902021 — the heartbeat-provenance → lifecycle verdict mapping. A
/// stale row is the crash/abandon signal: only a once-running indexer writes a
/// row that can later go stale. An absent row means the indexer never published
/// a heartbeat (never launched, or died before the first tick).
pub(crate) const INDEXER_LIFECYCLE_HEALTHY: &str = "healthy";
pub(crate) const INDEXER_LIFECYCLE_CRASHED_OR_ABANDONED: &str = "crashed_or_abandoned";
pub(crate) const INDEXER_LIFECYCLE_NEVER_LAUNCHED: &str = "never_launched";

/// REQ-AXO-902581 — les deux verdicts qui manquaient, et la raison qu'ils ont.
///
/// « battement PG périmé ⇒ le processus a planté » est une INFÉRENCE, valide pour
/// un démon qui bat en continu, **structurellement fausse pour un processus qui
/// travaille par PASSES** — ce qu'est l'indexeur : il traite un lot, sort
/// proprement (`exit_code: 0`), et attend. VPC l'a mesuré
/// (`msg-6e20192347913f610856b2d8`) : la session 132 a « corrigé » TROIS causes de
/// mort d'un processus qui ne mourait pas, la session 133 a bâti un dossier
/// d'interblocage sur cette prémisse. Un verdict de panne inventé oriente des
/// sessions entières vers des causes qui n'existent pas.
///
/// `stopped_or_idle` est ce qu'on sait quand on ne sait rien d'autre : le
/// battement s'est tu. `exited_clean` demande une OBSERVATION du superviseur.
pub(crate) const INDEXER_LIFECYCLE_STOPPED_OR_IDLE: &str = "stopped_or_idle";
pub(crate) const INDEXER_LIFECYCLE_EXITED_CLEAN: &str = "exited_clean";

/// REQ-AXO-902581 — degré de certitude du verdict, dit dans la réponse. Une
/// inférence présentée comme une observation est le mode d'échec exact que ce REQ
/// corrige ; le nommer coûte un champ.
pub(crate) const LIFECYCLE_CERTAINTY_OBSERVED: &str = "observed";
pub(crate) const LIFECYCLE_CERTAINTY_INFERRED: &str = "inferred";

/// REQ-AXO-902581 — ce que le SUPERVISEUR dit du rôle, quand on a pu le lui
/// demander. `process-compose` distingue `Completed` (sortie 0, normale) de
/// `Failed` / `Restarting` : la source qui sait est joignable sur `:8080`, et
/// l'inférence l'ignorait.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IndexerSupervisorObservation {
    pub(crate) status: String,
    pub(crate) exit_code: i64,
    pub(crate) is_running: bool,
}

/// Pure so the verdict is unit-tested without a live `GraphStore`.
pub(crate) fn resolve_indexer_liveness(
    now_ms: i64,
    indexer_heartbeat_ms: Option<i64>,
    freshness_window_ms: i64,
    supervisor: Option<&IndexerSupervisorObservation>,
) -> IndexerLiveness {
    let window = freshness_window_ms.max(0) as u64;
    match indexer_heartbeat_ms {
        Some(heartbeat_ms) => {
            let now_u = now_ms.max(0) as u64;
            let heartbeat_u = heartbeat_ms.max(0) as u64;
            // saturating_sub folds clock skew (future-dated heartbeat) to
            // age 0 — a just-written row counts as fresh, not distrusted.
            let fresh = now_u.saturating_sub(heartbeat_u) <= window;
            let feed = RuntimeTruthFeed::from_observed_times(
                now_u,
                Some(heartbeat_u),
                Some(heartbeat_u),
                window,
                if fresh {
                    None::<String>
                } else {
                    Some("indexer_heartbeat_stale".to_string())
                },
            );
            IndexerLiveness {
                ready: fresh,
                source: if fresh {
                    "pg_heartbeat"
                } else {
                    "pg_heartbeat_stale"
                },
                lifecycle: if fresh {
                    INDEXER_LIFECYCLE_HEALTHY
                } else {
                    // REQ-AXO-902581 — le battement s'est tu. Sans OBSERVATION du
                    // superviseur, tout ce qu'on sait est qu'il s'est tu ; conclure
                    // au crash est une inférence, et elle est fausse sur un
                    // processus à passes. Avec l'observation on tranche vraiment :
                    // `Completed` + sortie 0 = fin de passe NORMALE ; un processus
                    // encore en marche mais muet, ou sorti non-zéro, est bien la
                    // panne que ce verdict décrivait.
                    match supervisor {
                        None => INDEXER_LIFECYCLE_STOPPED_OR_IDLE,
                        Some(o) if !o.is_running && o.exit_code == 0 => {
                            INDEXER_LIFECYCLE_EXITED_CLEAN
                        }
                        Some(_) => INDEXER_LIFECYCLE_CRASHED_OR_ABANDONED,
                    }
                },
                certainty: if fresh || supervisor.is_some() {
                    LIFECYCLE_CERTAINTY_OBSERVED
                } else {
                    LIFECYCLE_CERTAINTY_INFERRED
                },
                feed,
            }
        }
        None => IndexerLiveness {
            feed: RuntimeTruthFeed::from_observed_times(
                0,
                None,
                None,
                window,
                Some("indexer_heartbeat_absent".to_string()),
            ),
            ready: false,
            source: "no_heartbeat",
            lifecycle: INDEXER_LIFECYCLE_NEVER_LAUNCHED,
            // L'absence de LIGNE est un fait, pas une inférence : personne n'a
            // jamais publié de battement.
            certainty: LIFECYCLE_CERTAINTY_OBSERVED,
        },
    }
}

pub(crate) fn split_run_root(project_root: &str, instance_kind: &str, role_slug: &str) -> PathBuf {
    let mut path = PathBuf::from(project_root);
    if instance_kind == "dev" {
        path.push(".axon-dev");
    } else {
        path.push(".axon");
    }
    path.push(format!("run-{role_slug}"));
    path
}

pub(crate) fn split_runtime_state_from_file(path: &PathBuf) -> Option<HashMap<String, String>> {
    let file = OpenOptions::new().read(true).open(path).ok()?;
    let reader = BufReader::new(file);
    let mut values = HashMap::new();
    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        values.insert(
            key.trim().to_string(),
            value.trim().trim_matches('"').to_string(),
        );
    }
    Some(values)
}

pub(crate) fn runtime_truth_feed_snapshot(feed: &RuntimeTruthFeed) -> Value {
    let state = if feed.stale {
        "stale"
    } else if feed.degraded_reason.is_some() {
        "degraded"
    } else {
        "fresh"
    };

    json!({
        "state": state,
        "stale": feed.stale,
        "observed_age_ms": feed.observed_age_ms,
        "stale_after_ms": feed.stale_after_ms,
        "last_heartbeat_at_ms": feed.last_heartbeat_at_ms,
        "last_good_payload_at_ms": feed.last_good_payload_at_ms,
        "degraded_reason": feed.degraded_reason
    })
}

#[cfg(test)]
mod resolve_indexer_liveness_tests {
    use super::*;

    #[test]
    fn fresh_heartbeat_is_ready_and_canonical() {
        let now = 1_000_000;
        let live = resolve_indexer_liveness(
            now,
            Some(now - 3_000),
            EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
            None,
        );
        assert!(!live.feed.stale, "fresh heartbeat yields a non-stale feed");
        assert!(live.feed.degraded_reason.is_none());
        assert!(live.ready);
        assert_eq!(live.source, "pg_heartbeat");
        assert_eq!(live.lifecycle, INDEXER_LIFECYCLE_HEALTHY);
    }

    #[test]
    fn stale_heartbeat_is_degraded_not_ready() {
        let now = 1_000_000;
        let live = resolve_indexer_liveness(
            now,
            Some(now - 60_000),
            EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
            None,
        );
        assert!(live.feed.stale);
        assert_eq!(
            live.feed.degraded_reason.as_deref(),
            Some("indexer_heartbeat_stale")
        );
        assert!(!live.ready);
        assert_eq!(live.source, "pg_heartbeat_stale");
        // REQ-AXO-902581 — corrige REQ-AXO-902021. Un battement qui s'est tu prouve
        // que le processus a cessé de PUBLIER, pas qu'il est mort : sur un indexeur
        // qui travaille par passes, la fin de passe est le cas NORMAL. Sans
        // observation du superviseur, le verdict dit ce qu'il sait et rien de plus.
        assert_eq!(live.lifecycle, INDEXER_LIFECYCLE_STOPPED_OR_IDLE);
        assert_eq!(live.certainty, LIFECYCLE_CERTAINTY_INFERRED);
    }

    /// REQ-AXO-902581, critère d'acceptation — le MÊME chemin doit rendre DEUX
    /// verdicts différents selon ce que le superviseur observe. Une garde qui ne
    /// peut pas rendre l'autre verdict ne prouve rien.
    #[test]
    fn fin_de_passe_et_crash_reel_ne_rendent_PAS_le_meme_verdict() {
        let now = 1_000_000;
        let perime = Some(now - 60_000);

        // Fin de passe : process-compose rend `Completed`, sortie 0, arrêté.
        let fin_de_passe = resolve_indexer_liveness(
            now,
            perime,
            EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
            Some(&IndexerSupervisorObservation {
                status: "Completed".to_string(),
                exit_code: 0,
                is_running: false,
            }),
        );
        assert_eq!(
            fin_de_passe.lifecycle, INDEXER_LIFECYCLE_EXITED_CLEAN,
            "un indexeur sorti proprement n'est JAMAIS `crashed_or_abandoned`"
        );
        assert_eq!(fin_de_passe.certainty, LIFECYCLE_CERTAINTY_OBSERVED);

        // Crash réel : sortie non nulle.
        let crash = resolve_indexer_liveness(
            now,
            perime,
            EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
            Some(&IndexerSupervisorObservation {
                status: "Failed".to_string(),
                exit_code: 1,
                is_running: false,
            }),
        );
        assert_eq!(
            crash.lifecycle, INDEXER_LIFECYCLE_CRASHED_OR_ABANDONED,
            "une sortie non nulle EST la panne que ce verdict décrit"
        );
        assert_eq!(crash.certainty, LIFECYCLE_CERTAINTY_OBSERVED);

        // Le troisième cas, souvent confondu avec les deux autres : le processus
        // tourne toujours mais ne publie plus. Ce n'est ni une fin de passe ni une
        // sortie — c'est un blocage, et la remédiation (redémarrer l'indexeur) est
        // bien celle que `crashed_or_abandoned` porte.
        let muet = resolve_indexer_liveness(
            now,
            perime,
            EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
            Some(&IndexerSupervisorObservation {
                status: "Running".to_string(),
                exit_code: 0,
                is_running: true,
            }),
        );
        assert_eq!(muet.lifecycle, INDEXER_LIFECYCLE_CRASHED_OR_ABANDONED);

        // MUTANT — les trois verdicts sortent du MÊME battement périmé. Si l'un
        // d'eux ne dépendait pas de l'observation, ce test passerait sans le
        // correctif.
        assert_ne!(fin_de_passe.lifecycle, crash.lifecycle);
        assert_ne!(fin_de_passe.lifecycle, muet.lifecycle);
    }

    #[test]
    fn absent_heartbeat_is_loud_not_silent() {
        let live =
            resolve_indexer_liveness(1_000_000, None, EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS, None);
        assert!(live.feed.stale);
        assert_eq!(
            live.feed.degraded_reason.as_deref(),
            Some("indexer_heartbeat_absent")
        );
        assert!(!live.ready);
        assert_eq!(live.source, "no_heartbeat");
        // REQ-AXO-902021 — no row ever = the indexer never published a
        // heartbeat (never launched, or died before the first tick).
        assert_eq!(live.lifecycle, INDEXER_LIFECYCLE_NEVER_LAUNCHED);
    }

    #[test]
    fn future_heartbeat_clock_skew_counts_fresh() {
        let now = 1_000_000;
        let live = resolve_indexer_liveness(
            now,
            Some(now + 10_000),
            EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
            None,
        );
        assert!(
            live.ready,
            "a just-written (skewed) heartbeat is still proof of life"
        );
        assert_eq!(live.source, "pg_heartbeat");
    }
}
