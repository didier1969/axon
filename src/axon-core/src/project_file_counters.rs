//! REQ-AXO-901749 — incremental per-project filesystem counters.
//!
//! The scanner already knows, during its boot walk, how many files exist on
//! disk and how many are eligible per project. Persisting those counts in a
//! process-global `DashMap<project_code, ProjectFileCounters>` lets the watcher
//! adjust them incrementally on CREATE/DELETE and lets `embedding_status` /
//! the dashboard read them in O(1) — replacing the global 60 s-TTL filesystem
//! rescan (`tools_system::compute_fs_counters`) that walks the whole
//! `AXON_WATCH_DIR` on every expiry.
//!
//! Only the two filesystem-derived counters (`disk_files`, `eligible_files`)
//! live here; the PG-derived counts (indexed / chunks / embeddings) stay
//! PG-sourced via the dashboard composite.
//!
//! Counters are best-effort hints, not a transactional ledger: watcher events
//! can race or arrive for paths the boot walk never counted, so decrements
//! clamp at zero rather than underflow. A periodic reconcile against a fresh
//! walk (kept as a slow safety net) corrects any accumulated drift.

use dashmap::DashMap;
use std::sync::OnceLock;

/// Filesystem-derived file counts for one project.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProjectFileCounters {
    /// Every regular file under the project root (minus pruned build dirs).
    pub disk_files: i64,
    /// Subset the scanner would actually index (`should_process_path`).
    pub eligible_files: i64,
}

static REGISTRY: OnceLock<DashMap<String, ProjectFileCounters>> = OnceLock::new();

fn registry() -> &'static DashMap<String, ProjectFileCounters> {
    REGISTRY.get_or_init(DashMap::new)
}

/// Replace a project's counters wholesale — used by the scanner boot walk and
/// the periodic reconcile, both of which compute authoritative totals.
pub fn set_counts(project_code: &str, disk_files: i64, eligible_files: i64) {
    registry().insert(
        project_code.to_string(),
        ProjectFileCounters {
            disk_files: disk_files.max(0),
            eligible_files: eligible_files.max(0),
        },
    );
}

/// Watcher CREATE: a new file appeared. `eligible` is the scanner's verdict for
/// the path so the eligible counter only moves for indexable files.
pub fn record_created(project_code: &str, eligible: bool) {
    let mut entry = registry().entry(project_code.to_string()).or_default();
    entry.disk_files += 1;
    if eligible {
        entry.eligible_files += 1;
    }
}

/// Watcher DELETE: a file disappeared. Clamps at zero (see module docs).
pub fn record_removed(project_code: &str, eligible: bool) {
    let mut entry = registry().entry(project_code.to_string()).or_default();
    entry.disk_files = (entry.disk_files - 1).max(0);
    if eligible {
        entry.eligible_files = (entry.eligible_files - 1).max(0);
    }
}

/// O(1) read of one project's counters; `None` if never populated.
pub fn snapshot(project_code: &str) -> Option<ProjectFileCounters> {
    registry().get(project_code).map(|entry| *entry.value())
}

/// Sum across every known project — the global figure the legacy
/// `cached_fs_counters()` returned, now derived without a filesystem walk.
pub fn totals() -> ProjectFileCounters {
    registry()
        .iter()
        .fold(ProjectFileCounters::default(), |mut acc, entry| {
            acc.disk_files += entry.disk_files;
            acc.eligible_files += entry.eligible_files;
            acc
        })
}

/// Every project's counters as `(project_code, counters)` pairs — used by the
/// dashboard per-project panel.
pub fn all() -> Vec<(String, ProjectFileCounters)> {
    registry()
        .iter()
        .map(|entry| (entry.key().clone(), *entry.value()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test uses a unique project_code so the process-global registry does
    // not bleed state across tests running in the same process.
    #[test]
    fn set_counts_then_snapshot_roundtrips() {
        set_counts("T-SET", 100, 40);
        assert_eq!(
            snapshot("T-SET"),
            Some(ProjectFileCounters {
                disk_files: 100,
                eligible_files: 40
            })
        );
    }

    #[test]
    fn unknown_project_snapshot_is_none() {
        assert_eq!(snapshot("T-UNKNOWN-XYZ"), None);
    }

    #[test]
    fn created_and_removed_adjust_incrementally() {
        set_counts("T-INC", 10, 5);
        record_created("T-INC", true);
        assert_eq!(snapshot("T-INC").unwrap().disk_files, 11);
        assert_eq!(snapshot("T-INC").unwrap().eligible_files, 6);
        // Ineligible create bumps disk only.
        record_created("T-INC", false);
        assert_eq!(snapshot("T-INC").unwrap().disk_files, 12);
        assert_eq!(snapshot("T-INC").unwrap().eligible_files, 6);
        record_removed("T-INC", true);
        assert_eq!(snapshot("T-INC").unwrap().disk_files, 11);
        assert_eq!(snapshot("T-INC").unwrap().eligible_files, 5);
    }

    #[test]
    fn decrement_clamps_at_zero() {
        set_counts("T-CLAMP", 0, 0);
        record_removed("T-CLAMP", true);
        let snap = snapshot("T-CLAMP").unwrap();
        assert_eq!(snap.disk_files, 0);
        assert_eq!(snap.eligible_files, 0);
    }

    #[test]
    fn created_on_unseen_project_starts_from_zero() {
        record_created("T-FRESH", true);
        assert_eq!(
            snapshot("T-FRESH"),
            Some(ProjectFileCounters {
                disk_files: 1,
                eligible_files: 1
            })
        );
    }

    #[test]
    fn totals_sum_across_projects() {
        set_counts("T-TOT-A", 10, 4);
        set_counts("T-TOT-B", 20, 7);
        let totals = totals();
        // Other tests' projects also contribute; assert the floor we added.
        assert!(totals.disk_files >= 30);
        assert!(totals.eligible_files >= 11);
    }
}
