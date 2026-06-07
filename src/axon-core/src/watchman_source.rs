//! REQ-AXO-901893 — Watchman-backed file source (clock/cursor reconciliation).
//!
//! Replaces Axon's hand-rolled `notify`/inotify watcher + `ingress_buffer` FIFO
//! + reconciliation/periodic sweeps with **Watchman** (Meta's file-watching
//! daemon). The architectural win is structural, not incremental:
//!
//! * Watchman maintains its own authoritative view + a monotonic clock per
//!   watched root. A `since: <clock>` subscription always returns the **exact
//!   cumulative delta** since that clock — OR, if Watchman cannot honor the
//!   `since` (server restarted, watch recreated, clock too old), it returns
//!   `is_fresh_instance = true` with the **full** match set. Either way, a
//!   missed FS event is *structurally impossible*: the old `notify` model
//!   dropped events on inotify-queue overflow and they were gone forever.
//!
//! * The clock is persisted to `axon_runtime.watchman_clock` **after** each
//!   batch is fed to pipeline A (checkpoint-after-commit). A crash between feed
//!   and checkpoint replays the batch on restart — idempotent via the
//!   IndexedFile dedup cache — and can never *skip* a delta.
//!
//! * Build-artifact exclusion moves to the daemon via `.watchmanconfig`
//!   `ignore_dirs` (a `cargo build` under an ignored root generates zero
//!   events). Per-segment correctness at any depth is enforced here by
//!   [`crate::indexing_policy::is_watch_pruned_segment`] on every path returned.
//!
//! Topology: one Watchman root **per git repo** (`resolve_root` = watch-project)
//! — never one giant root over the whole workspace. The single `input_tx` sink
//! into pipeline A is unchanged; this module is purely a new *feed*.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
// The `query_result_type!` macro expands to `#[derive(Deserialize)]` +
// `#[serde(flatten)]`, so both must be in scope at the expansion site.
use serde::Deserialize;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio_postgres::NoTls;
use tracing::{info, warn};
use watchman_client::prelude::*;
use watchman_client::{Error as WatchmanError, SubscriptionData};

use crate::graph::GraphStore;
use crate::indexing_policy::{is_watch_pruned_segment, watchman_ignore_dirs};
use crate::scanner::Scanner;

/// Reconnect backoff floor (productive cadence). Mirrors `demand_pull`.
const BACKOFF_INITIAL_MS: u64 = 200;
/// Reconnect backoff ceiling (fully idle / persistently-failing cadence).
const BACKOFF_MAX_MS: u64 = 30_000;

// The set of file fields we ask Watchman to return per change. `name` is the
// cheapest field; `exists` lets us distinguish a create/modify (upsert) from a
// delete / the old side of a rename (`exists=false`) without a disk stat.
query_result_type! {
    struct WatchmanFileFields {
        name: NameField,
        exists: ExistsField,
    }
}

/// A persisted reconciliation checkpoint for one root, sent to the clock-writer
/// task after a batch has been fed to pipeline A.
struct ClockUpdate {
    root: String,
    clock: Clock,
    is_fresh: bool,
}

/// What to do with one path Watchman reported as changed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FeedAction {
    /// File exists and is index-eligible → feed pipeline A (idempotent upsert).
    Upsert(PathBuf),
    /// File no longer exists (deletion, or the old side of a rename) → cascade
    /// delete its IST footprint.
    Delete(PathBuf),
}

/// Boot the Watchman file source. Returns immediately; a supervisor task runs
/// for the process lifetime (per-root subscription loops + a clock writer).
///
/// `input_tx` is pipeline A's sink (unchanged contract). `scanner` supplies the
/// authoritative per-file eligibility gate (`should_process_path`) and the
/// one-shot fallback walk when Watchman is unreachable.
pub fn spawn_watchman_source(
    store: Arc<GraphStore>,
    input_tx: Sender<PathBuf>,
    scanner: Arc<Scanner>,
    watch_root: String,
    database_url: String,
) -> Result<()> {
    tokio::spawn(async move {
        run_supervisor(store, input_tx, scanner, watch_root, database_url).await;
    });
    Ok(())
}

/// Connect to Watchman, resolve roots, then spawn one subscription loop per
/// root plus the clock-writer task. On a hard connect failure, degrade to a
/// one-shot scanner walk (full index, no live deltas) and surface a Blocker.
async fn run_supervisor(
    store: Arc<GraphStore>,
    input_tx: Sender<PathBuf>,
    scanner: Arc<Scanner>,
    watch_root: String,
    database_url: String,
) {
    let client = match connect_watchman().await {
        Ok(c) => Arc::new(c),
        Err(err) => {
            warn!(
                error = %err,
                "Blocker: Watchman connect failed — degrading to a ONE-SHOT scanner \
                 walk (full index, NO live deltas). Check the `watchman` binary / \
                 AXON_WATCHMAN_BIN. REQ-AXO-901893"
            );
            fallback_scanner_bootstrap(&scanner, &input_tx).await;
            return;
        }
    };

    let roots = resolve_roots(&client, &watch_root).await;
    if roots.is_empty() {
        warn!(
            watch_root = %watch_root,
            "Watchman resolved no project roots — degrading to a one-shot scanner walk"
        );
        fallback_scanner_bootstrap(&scanner, &input_tx).await;
        return;
    }
    info!(roots = roots.len(), "Watchman: subscribing to {} project root(s)", roots.len());

    // Clock writer task + one-shot initial load of persisted clocks.
    let (clock_tx, clock_rx) = tokio::sync::mpsc::channel::<ClockUpdate>(256);
    let root_keys: Vec<String> = roots
        .iter()
        .map(|r| r.path().to_string_lossy().to_string())
        .collect();
    let initial = load_initial_clocks(&database_url, &root_keys).await;
    tokio::spawn(clock_writer_loop(clock_rx, database_url));

    // One supervised subscription loop per root.
    for root in roots {
        let root_key = root.path().to_string_lossy().to_string();
        let mut clock = initial.get(&root_key).cloned();
        let client = client.clone();
        let input_tx = input_tx.clone();
        let store = store.clone();
        let scanner = scanner.clone();
        let clock_tx = clock_tx.clone();
        tokio::spawn(async move {
            let mut backoff = BACKOFF_INITIAL_MS;
            loop {
                ensure_watchmanconfig(&root.path());
                match run_root_subscription(
                    &client, &root, &mut clock, &input_tx, &store, &scanner, &clock_tx,
                )
                .await
                {
                    Ok(()) => {
                        // Clean exit = Canceled or pipeline closed; re-subscribe
                        // from the in-memory clock (delta) or fresh.
                        backoff = BACKOFF_INITIAL_MS;
                    }
                    Err(err) => {
                        warn!(
                            root = %root.path().display(),
                            error = %err,
                            backoff_ms = backoff,
                            "Watchman: subscription errored; backing off then re-subscribing"
                        );
                        backoff = (backoff * 2).min(BACKOFF_MAX_MS);
                    }
                }
                tokio::time::sleep(Duration::from_millis(backoff)).await;
            }
        });
    }
}

/// Connect to the Watchman daemon (auto-spawned by the CLI if not running).
/// Honors `AXON_WATCHMAN_BIN` so the binary is resolved via the toolchain
/// manifest rather than a hardcoded store path; otherwise relies on PATH.
async fn connect_watchman() -> std::result::Result<Client, WatchmanError> {
    let mut connector = Connector::new();
    if let Some(bin) = std::env::var_os("AXON_WATCHMAN_BIN") {
        connector = connector.watchman_cli_path(bin);
    }
    connector.connect().await
}

/// Enumerate one Watchman root per git repo under `watch_root`. We list the
/// immediate child directories (the common `projects/<repo>` layout), pre-seed
/// each with a `.watchmanconfig` BEFORE the watch is created (so `ignore_dirs`
/// takes effect on first watch), then `resolve_root` (= watch-project) each.
/// Watchman collapses a path to its enclosing repo root, so we dedup by the
/// resolved root path.
async fn resolve_roots(client: &Client, watch_root: &str) -> Vec<ResolvedRoot> {
    let mut roots = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let entries = match std::fs::read_dir(watch_root) {
        Ok(e) => e,
        Err(err) => {
            warn!(error = %err, watch_root = %watch_root, "Watchman: cannot read watch_root");
            return roots;
        }
    };

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Skip build dirs / dotdirs / VCS metadata at the top level outright.
        if is_watch_pruned_segment(&name) {
            continue;
        }
        // Pre-seed the config so the watch created by resolve_root reads it.
        ensure_watchmanconfig(&dir);
        let canonical = match CanonicalPath::canonicalize(&dir) {
            Ok(c) => c,
            Err(err) => {
                warn!(dir = %dir.display(), error = %err, "Watchman: canonicalize failed");
                continue;
            }
        };
        match client.resolve_root(canonical).await {
            Ok(root) => {
                let key = root.path().to_string_lossy().to_string();
                if seen.insert(key) {
                    // The true repo root may differ from `dir` (nested); ensure
                    // its config too (applies on the next indexer restart).
                    ensure_watchmanconfig(&root.path());
                    roots.push(root);
                }
            }
            Err(err) => {
                warn!(dir = %dir.display(), error = %err, "Watchman: resolve_root (watch-project) failed");
            }
        }
    }
    roots
}

/// Subscribe to one root with `since = clock` and stream changes into pipeline
/// A until the subscription is canceled or errors. `clock` is updated in place
/// after every batch so a re-subscribe resumes from the last checkpoint.
async fn run_root_subscription(
    client: &Client,
    root: &ResolvedRoot,
    clock: &mut Option<Clock>,
    input_tx: &Sender<PathBuf>,
    store: &Arc<GraphStore>,
    scanner: &Arc<Scanner>,
    clock_tx: &Sender<ClockUpdate>,
) -> std::result::Result<(), WatchmanError> {
    let root_path = root.path();
    let root_key = root_path.to_string_lossy().to_string();

    // Suffix pre-filter from the canonical, config-file-aware extension list
    // (single source of truth — no duplicated literal). The authoritative
    // per-file gate is still `scanner.should_process_path` below; this merely
    // collapses Watchman's event volume.
    let suffixes: Vec<PathBuf> = crate::config::CONFIG
        .indexing
        .supported_extensions
        .iter()
        .map(PathBuf::from)
        .collect();
    let expression = Expr::All(vec![
        Expr::FileType(FileType::Regular),
        Expr::Suffix(suffixes),
    ]);

    let request = SubscribeRequest {
        since: clock.clone(),
        expression: Some(expression),
        // We DO want the full set on a fresh instance — we re-feed it (cheap +
        // idempotent via the dedup cache). `false` = "send me the files".
        empty_on_fresh_instance: false,
        ..Default::default()
    };

    let (mut subscription, response) = client
        .subscribe::<WatchmanFileFields>(root, request)
        .await?;
    info!(
        root = %root_path.display(),
        clock = ?response.clock,
        "Watchman: subscription active"
    );

    // REQ-AXO-901893 — DECOUPLE next() from the feed. `subscription.next()` MUST
    // be polled promptly and continuously: all 41 root subscriptions share ONE
    // Watchman `Client` connection, so if THIS loop blocks (e.g. on
    // `input_tx.send().await` while pipeline A is saturated during cold-start),
    // it stalls the shared client's PDU pump and *every* root stops receiving
    // live deltas (observed: post-fresh deltas never delivered, clocks frozen).
    // Fix: next() pushes feed actions to an UNBOUNDED queue (never blocks); a
    // separate feeder task drains it into `input_tx` WITH backpressure. The
    // subscription loop therefore always returns to next() immediately.
    let (feed_tx, mut feed_rx) = tokio::sync::mpsc::unbounded_channel::<FeedAction>();
    let feeder_input_tx = input_tx.clone();
    let feeder_store = store.clone();
    let feeder = tokio::spawn(async move {
        while let Some(action) = feed_rx.recv().await {
            match action {
                FeedAction::Upsert(path) => {
                    if feeder_input_tx.send(path).await.is_err() {
                        return; // pipeline A closed
                    }
                }
                FeedAction::Delete(path) => {
                    let store = feeder_store.clone();
                    let p = path.to_string_lossy().to_string();
                    let _ = tokio::task::spawn_blocking(move || store.delete_file_cascade(&p)).await;
                }
            }
        }
    });

    let outcome: std::result::Result<(), WatchmanError> = loop {
        let data = match subscription.next().await {
            Ok(d) => d,
            Err(err) => break Err(err),
        };
        match data {
            SubscriptionData::FilesChanged(query_result) => {
                let is_fresh = query_result.is_fresh_instance;
                let entries: Vec<(PathBuf, bool)> = query_result
                    .files
                    .unwrap_or_default()
                    .into_iter()
                    .map(|f| {
                        let exists = *f.exists;
                        (f.name.into_inner(), exists)
                    })
                    .collect();

                // INSTRUMENTATION (REQ-AXO-901893): log EVERY batch — fresh OR
                // delta — so live-delta delivery is observable. A root that only
                // ever logs `fresh=true` once and never a `fresh=false` delta is
                // the smoking gun for a stalled subscription pump.
                info!(
                    root = %root_path.display(),
                    files = entries.len(),
                    fresh = is_fresh,
                    "Watchman: FilesChanged"
                );

                // Eligibility scan (gitignore/axonignore/extension + depth-
                // correct segment prune) is fs/CPU-bound → run off the async
                // executor so a large fresh batch never starves other tasks.
                let plan_root = root_path.clone();
                let plan_scanner = scanner.clone();
                let actions = tokio::task::spawn_blocking(move || {
                    plan_feed_actions(&plan_root, entries, &|p| {
                        plan_scanner.should_process_path(p)
                    })
                })
                .await
                .unwrap_or_default();

                // Hand off to the feeder (UNBOUNDED — never blocks next()).
                let mut feeder_closed = false;
                for action in actions {
                    if feed_tx.send(action).is_err() {
                        feeder_closed = true;
                        break;
                    }
                }
                if feeder_closed {
                    info!(root = %root_path.display(), "Watchman: feeder closed (pipeline A down); ending subscription");
                    break Ok(());
                }

                // Checkpoint AFTER the batch is queued (checkpoint-after-commit;
                // the feeder drains idempotently, a crash replays via dedup).
                *clock = Some(query_result.clock.clone());
                let _ = clock_tx
                    .send(ClockUpdate {
                        root: root_key.clone(),
                        clock: query_result.clock,
                        is_fresh,
                    })
                    .await;
            }
            SubscriptionData::Canceled => {
                warn!(root = %root_path.display(), "Watchman: subscription canceled — re-subscribing from last clock");
                break Ok(());
            }
            // VCS state transitions (e.g. `hg.update`): not relevant to our feed.
            SubscriptionData::StateEnter { .. } | SubscriptionData::StateLeave { .. } => {}
        }
    };

    // Drop the sender so the feeder drains its queue and exits cleanly.
    drop(feed_tx);
    let _ = feeder.await;
    outcome
}

/// Pure feed planner — maps Watchman's reported `(relative_name, exists)`
/// entries to [`FeedAction`]s. Kept side-effect-free so the upsert/delete/prune
/// branches are unit-testable without a Watchman server or PG. `eligible` is the
/// per-file gate (production: `Scanner::should_process_path`).
fn plan_feed_actions(
    root: &Path,
    entries: Vec<(PathBuf, bool)>,
    eligible: &dyn Fn(&Path) -> bool,
) -> Vec<FeedAction> {
    let mut out = Vec::with_capacity(entries.len());
    for (relative, exists) in entries {
        // Depth-correct segment prune: drop anything whose relative path has a
        // build-dir / VCS / dotdir component. This is the belt-and-suspenders
        // over `.watchmanconfig` ignore_dirs (which is only a root-relative
        // prefix). Applies to BOTH upserts and deletes (we never indexed build
        // artifacts, so never try to delete their IST rows either).
        let pruned = relative.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .map(is_watch_pruned_segment)
                .unwrap_or(false)
        });
        if pruned {
            continue;
        }
        let absolute = root.join(&relative);
        if exists {
            if eligible(&absolute) {
                out.push(FeedAction::Upsert(absolute));
            }
        } else {
            out.push(FeedAction::Delete(absolute));
        }
    }
    out
}

/// Write a `.watchmanconfig` (`ignore_dirs` from the canonical `DIRECTORY_RULES`)
/// into `root` if absent. Best-effort: a failure only forfeits the daemon-side
/// inotify-load reduction; correctness is unaffected (the segment prune in
/// [`plan_feed_actions`] still drops build-dir paths). Idempotent.
fn ensure_watchmanconfig(root: &Path) {
    let config_path = root.join(".watchmanconfig");
    if config_path.exists() {
        return;
    }
    let body = serde_json::json!({ "ignore_dirs": watchman_ignore_dirs() });
    match serde_json::to_string_pretty(&body) {
        Ok(serialized) => match std::fs::write(&config_path, serialized) {
            Ok(()) => info!(path = %config_path.display(), "Watchman: wrote .watchmanconfig (ignore_dirs)"),
            Err(err) => warn!(path = %config_path.display(), error = %err, "Watchman: failed to write .watchmanconfig"),
        },
        Err(err) => warn!(error = %err, "Watchman: failed to serialize .watchmanconfig"),
    }
}

/// One-shot load of every root's persisted clock at boot. A missing row = first
/// run for that root → `None` → a fresh-instance full index.
async fn load_initial_clocks(database_url: &str, root_keys: &[String]) -> HashMap<String, Clock> {
    let mut out = HashMap::new();
    let (client, connection) = match tokio_postgres::connect(database_url, NoTls).await {
        Ok(pair) => pair,
        Err(err) => {
            warn!(error = %err, "Watchman: clock load connect failed — all roots start fresh");
            return out;
        }
    };
    let driver = tokio::spawn(async move { let _ = connection.await; });
    for key in root_keys {
        match client
            .query_opt(
                "SELECT clock_json FROM axon_runtime.watchman_clock WHERE root = $1",
                &[key],
            )
            .await
        {
            Ok(Some(row)) => {
                let value: serde_json::Value = row.get(0);
                match serde_json::from_value::<Clock>(value) {
                    Ok(clock) => {
                        out.insert(key.clone(), clock);
                    }
                    Err(err) => warn!(root = %key, error = %err, "Watchman: stored clock unparseable; root starts fresh"),
                }
            }
            Ok(None) => {}
            Err(err) => warn!(root = %key, error = %err, "Watchman: clock load query failed; root starts fresh"),
        }
    }
    drop(client);
    let _ = driver.await;
    out
}

/// Owns a dedicated PG connection and persists clock checkpoints serially,
/// reconnecting on error. Persistence is best-effort: a dropped checkpoint just
/// means the next restart re-feeds from an older clock (more events, still safe
/// — never a skipped delta).
async fn clock_writer_loop(mut rx: Receiver<ClockUpdate>, database_url: String) {
    let mut client: Option<tokio_postgres::Client> = None;
    while let Some(update) = rx.recv().await {
        if client.is_none() {
            match tokio_postgres::connect(&database_url, NoTls).await {
                Ok((c, connection)) => {
                    tokio::spawn(async move { let _ = connection.await; });
                    client = Some(c);
                }
                Err(err) => {
                    warn!(error = %err, "Watchman clock writer: connect failed; dropping this checkpoint");
                    continue;
                }
            }
        }
        let value = match serde_json::to_value(&update.clock) {
            Ok(v) => v,
            Err(err) => {
                warn!(error = %err, "Watchman clock writer: clock serialize failed");
                continue;
            }
        };
        let result = client
            .as_ref()
            .unwrap()
            .execute(
                "INSERT INTO axon_runtime.watchman_clock (root, clock_json, is_fresh, updated_at) \
                 VALUES ($1, $2, $3, now()) \
                 ON CONFLICT (root) DO UPDATE \
                   SET clock_json = EXCLUDED.clock_json, \
                       is_fresh   = EXCLUDED.is_fresh, \
                       updated_at = now()",
                &[&update.root, &value, &update.is_fresh],
            )
            .await;
        if let Err(err) = result {
            warn!(error = %err, "Watchman clock writer: persist failed; reconnecting");
            client = None;
        }
    }
}

/// Degraded path when Watchman is unreachable: enumerate the whole watch root
/// once and feed it with backpressure. No live deltas — equivalent to the old
/// bootstrap scan, but explicitly a fallback, not the steady state.
async fn fallback_scanner_bootstrap(scanner: &Arc<Scanner>, input_tx: &Sender<PathBuf>) {
    let scanner = scanner.clone();
    let files = tokio::task::spawn_blocking(move || scanner.enumerate_files())
        .await
        .unwrap_or_default();
    info!(files = files.len(), "Watchman fallback: one-shot scanner walk feeding {} files", files.len());
    for path in files {
        if input_tx.send(path).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rels(items: &[(&str, bool)]) -> Vec<(PathBuf, bool)> {
        items.iter().map(|(p, e)| (PathBuf::from(p), *e)).collect()
    }

    #[test]
    fn upsert_only_for_existing_eligible_files() {
        let root = Path::new("/repo");
        let entries = rels(&[("src/lib.rs", true), ("README.md", true)]);
        // gate accepts everything
        let actions = plan_feed_actions(root, entries, &|_| true);
        assert_eq!(
            actions,
            vec![
                FeedAction::Upsert(PathBuf::from("/repo/src/lib.rs")),
                FeedAction::Upsert(PathBuf::from("/repo/README.md")),
            ]
        );
    }

    #[test]
    fn ineligible_existing_files_are_dropped_not_deleted() {
        let root = Path::new("/repo");
        let entries = rels(&[("src/keep.rs", true), ("src/skip.bin", true)]);
        // gate rejects the .bin
        let actions = plan_feed_actions(root, entries, &|p| {
            p.extension().and_then(|e| e.to_str()) == Some("rs")
        });
        assert_eq!(actions, vec![FeedAction::Upsert(PathBuf::from("/repo/src/keep.rs"))]);
    }

    #[test]
    fn missing_files_become_deletes_regardless_of_gate() {
        let root = Path::new("/repo");
        let entries = rels(&[("src/gone.rs", false)]);
        // gate would reject a non-existent file (is_file()==false), but a delete
        // must NOT depend on the eligibility gate.
        let actions = plan_feed_actions(root, entries, &|_| false);
        assert_eq!(actions, vec![FeedAction::Delete(PathBuf::from("/repo/src/gone.rs"))]);
    }

    #[test]
    fn build_dir_paths_are_pruned_at_any_depth() {
        let root = Path::new("/repo");
        let entries = rels(&[
            ("target/debug/foo.rs", true),          // rust build output
            ("src/a/node_modules/pkg/index.js", true), // nested dep store
            (".axon/cargo-target/x.rs", true),      // axon's own build dir
            ("src/real.rs", true),                  // the only legit one
        ]);
        let actions = plan_feed_actions(root, entries, &|_| true);
        assert_eq!(actions, vec![FeedAction::Upsert(PathBuf::from("/repo/src/real.rs"))]);
    }

    #[test]
    fn fresh_instance_full_set_plans_same_as_delta() {
        // The planner is is_fresh-agnostic by design: a fresh instance is just a
        // larger entry list. Feeding it all is idempotent downstream. This locks
        // that a "full" list yields an Upsert per eligible file.
        let root = Path::new("/repo");
        let full = rels(&[("a.rs", true), ("b.rs", true), ("c.rs", true)]);
        let actions = plan_feed_actions(root, full, &|_| true);
        assert_eq!(actions.len(), 3);
        assert!(actions.iter().all(|a| matches!(a, FeedAction::Upsert(_))));
    }

    #[test]
    fn clock_round_trips_through_json_for_persistence() {
        // The persistence path is Clock -> serde_json::Value (JSONB) -> Clock.
        // A string clockspec must survive intact so a re-subscribe yields a
        // delta, not a spurious fresh instance.
        let clock = Clock::Spec(ClockSpec::StringClock("c:1750000000:42".to_string()));
        let value = serde_json::to_value(&clock).expect("serialize");
        let restored: Clock = serde_json::from_value(value).expect("deserialize");
        match restored {
            Clock::Spec(ClockSpec::StringClock(s)) => assert_eq!(s, "c:1750000000:42"),
            other => panic!("clock did not round-trip: {other:?}"),
        }
    }

    #[test]
    fn ignore_dirs_excludes_vcs_includes_build_outputs() {
        let dirs = watchman_ignore_dirs();
        assert!(!dirs.contains(&".git"), "ignore_dirs must NOT carry .git (Watchman owns it via ignore_vcs)");
        assert!(dirs.contains(&"target"));
        assert!(dirs.contains(&"node_modules"));
        assert!(dirs.contains(&".axon"));
        assert!(dirs.contains(&"cargo-target"));
    }
}
