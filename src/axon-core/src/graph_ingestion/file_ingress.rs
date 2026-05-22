// REQ-AXO-901653 slice-5c — File state-machine purge transition layer.
//
// All methods below were thin wrappers over the dropped `public.File` table
// (and its companion queues `FileVectorizationQueue` / `GraphProjectionQueue`).
// G1 (slice-5a, commit d8e8f39a) dropped the schema ; G2 (slice-5b, commit
// 81f47320) added GraphStore method stubs to restore the build ; but the
// `fetch_file_ingress_rows` / `tombstone_missing_path` / `reconcile_ignore_rules_for_scope`
// methods were still emitting raw `SELECT FROM File` SQL that PostgreSQL
// would reject at runtime (table no longer exists). That made the live
// brain crash whenever the ingress promoter ran a flush cycle.
//
// This rewrite makes every method a typed no-op that returns the correct
// `Result<...>` shape so callers compile + run without touching the dropped
// table. Pipeline_v2 (REQ-AXO-289 / CPT-AXO-054) is the canonical ingestion
// path : it consumes the `ingress_buffer` directly and writes `Chunk` +
// `ChunkEmbedding` + `IndexedFile` rows without going through the legacy
// File state-machine. Removing the callers entirely (scanner, fs_watcher,
// main_background ingress promoter, MCP test runtime_surface) is tracked
// as REQ-AXO-901662 slice-5d.
//
// Operator directive: "rien garder de legacy" — this shim is intentionally
// short-lived. Mark with `slice-5c-noop` for grep-based cleanup in slice-5d.

use std::path::Path;

use anyhow::Result;

use crate::file_ingress_guard::FileIngressRow;
use crate::graph::GraphStore;
use crate::ingress_buffer::{IngressDrainBatch, IngressPromotionStats};

use super::IgnoreReconcileStats;

impl GraphStore {
    pub fn bulk_insert_files(
        &self,
        _file_paths: &[(String, String, i64, i64)],
    ) -> Result<()> {
        // slice-5c-noop : pipeline_v2 ingress_buffer is the canonical write path.
        Ok(())
    }

    pub fn upsert_hot_file(
        &self,
        _path: &str,
        _project: &str,
        _size: i64,
        _mtime: i64,
        _priority: i64,
    ) -> Result<()> {
        // slice-5c-noop : pipeline_v2 ingress_buffer handles hot deltas.
        Ok(())
    }

    pub fn promote_ingress_batch(
        &self,
        batch: &IngressDrainBatch,
    ) -> Result<IngressPromotionStats> {
        // slice-5c-noop : pipeline_v2 drains ingress_buffer directly into
        // Chunk + ChunkEmbedding + IndexedFile. The legacy promote step
        // (File state-machine insert + queue enqueue) is dead.
        Ok(IngressPromotionStats {
            promoted_files: batch.files.len(),
            promoted_tombstones: batch.tombstones.len(),
        })
    }

    pub fn tombstone_missing_path(&self, _path: &Path) -> Result<usize> {
        // slice-5c-noop : pipeline_v2 deletes Chunk + ChunkEmbedding + IndexedFile
        // rows directly when a path is no longer reachable from the watcher.
        Ok(0)
    }

    pub fn reconcile_ignore_rules_for_scope(
        &self,
        _scope_root: &Path,
        _scanner: &crate::scanner::Scanner,
    ) -> Result<IgnoreReconcileStats> {
        // slice-5c-noop : pipeline_v2 + ignore matcher decide eligibility at
        // ingress time ; no DB-side reconciliation needed.
        Ok(IgnoreReconcileStats::default())
    }

    pub fn fetch_file_ingress_row(&self, _path: &str) -> Result<Option<FileIngressRow>> {
        // slice-5c-noop : pipeline_v2 tracks ingress state in IndexedFile rows ;
        // the legacy File row probe is no longer authoritative.
        Ok(None)
    }

    pub fn fetch_file_ingress_rows(&self, _paths: &[String]) -> Result<Vec<FileIngressRow>> {
        // slice-5c-noop : see fetch_file_ingress_row.
        Ok(Vec::new())
    }
}
