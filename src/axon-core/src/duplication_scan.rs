// REQ-AXO-902185 slice 1 — near-duplicate (semantic clone) detection, persisted
// as `SIMILAR_TO` graph edges rather than a cached scalar.
//
// Why edges, not a cache: a `SIMILAR_TO` pair persisted in `ist.edge` loads
// into the RAM CSR snapshot exactly like CALLS/CONTAINS at `ist_snapshot_warm`
// (PIL-AXO-9002), so `duplication_score`'s clone-pair count becomes a plain
// O(E) relation-type count over the warm graph — zero PG round-trip per
// `structural_health_index`/`structural_health_worklist` call, matching the
// RAM-native design of the other 5 SHI axes. A TTL'd scalar cache would have
// worked too, but doesn't survive a process restart and isn't individually
// inspectable per symbol the way a graph edge is.
//
// Why FULL RECONCILE (delete-then-reinsert), not incremental upsert: an
// out-of-band `SIMILAR_TO` edge is NOT touched by the per-file re-index purge
// that keeps CALLS current (REQ-AXO-902204). An incremental upsert would let
// stale pairs survive after either side of the pair changes or is deleted —
// the exact phantom-edge class already found and fixed once this month for
// CALLS (REQ-AXO-902203, 37% of the graph fragmented). Measured cost of a
// full reconcile: ~1m10s for ~9300 AXO symbols via the pgvector HNSW index
// (chunk_embedding_hnsw_idx) — cheap enough for an out-of-band batch, but
// still far too slow for the RAM-native SHI hot path, hence this lives here
// and is invoked out-of-band (dev-test / follow-up scheduling slice), never
// inline from `compute_shi_raw_metrics`.
//
// Why the HNSW index and not exact brute force: an exact all-pairs cosine
// join over the same ~9300 symbols measured at ~9m42s (materializing
// candidates first defeats the index — Postgres can only use
// `chunk_embedding_hnsw_idx` when the `ORDER BY … <=> …` targets the base
// table column directly, inside a per-row `LATERAL` probe).
//
// Threshold (`<0.10` cosine distance) mirrors the established single-symbol
// `semantic_clones` tool (REQ-AXO-91518) — no new calibration invented here.

use crate::graph::GraphStore;
use serde_json::Value;

/// Cosine-distance threshold below which two symbols are considered
/// near-duplicates. Mirrors `axon_semantic_clones` (tools_governance.rs).
pub const DUPLICATION_CLONE_THRESHOLD: f64 = 0.10;

/// Per-row over-fetch from the HNSW index before the project/threshold
/// filter is applied — candidates outside the project or the caller's own
/// row are discarded, so this must be wide enough that within-project
/// neighbors aren't crowded out by a busy multi-project instance.
const OVERFETCH_K: i64 = 30;

/// Statement-local timeout override for the reconcile INSERT (ms). Generous
/// margin above the ~1m16s measured for AXO's ~9300 symbols — this is a batch
/// job, not the RAM hot path, so headroom matters more than tightness here.
const RECONCILE_STATEMENT_TIMEOUT_MS: u64 = 300_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicationScanReport {
    pub symbols_scanned: usize,
    pub pairs_found: usize,
}

// `query_json` renders every column as a JSON string (confirmed empirically: a
// bigint `count(*)` comes back as `"3"`, not `3`) — `.as_i64()` alone silently
// reads that as `None`/0. Accept both forms.
pub(crate) fn scalar_count(raw: &str) -> usize {
    let rows: Vec<Vec<Value>> = serde_json::from_str(raw).unwrap_or_default();
    rows.first()
        .and_then(|row| row.first())
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok())))
        .unwrap_or(0) as usize
}

/// REQ-AXO-902185 slice 2 — pure decision: is the vectorization pipeline idle
/// enough to run a duplication reconcile without competing with live embedding
/// work? `pending_chunks` = count of `ist.chunk` rows not yet embedded
/// (`embed_status != 'done'`) across the whole instance, the same "is there
/// still work queued" signal `embedding_status` already surfaces. Zero
/// pending = safe to spend the ~1-2min/project HNSW scan cost.
pub fn duplication_scan_due(pending_chunks: usize) -> bool {
    pending_chunks == 0
}

impl GraphStore {
    /// REQ-AXO-902185 slice 2 — instance-wide count of chunks still queued for
    /// embedding (`embed_status = 'pending'`; terminal states are `embedded`
    /// and `failed`, mirroring the drain queries in `graph_ingestion.rs`).
    /// Feeds `duplication_scan_due`.
    pub fn pending_embed_chunk_count(&self) -> Result<usize, String> {
        let raw = self
            .query_json("SELECT count(*) FROM ist.chunk WHERE embed_status = 'pending'")
            .map_err(|e| e.to_string())?;
        Ok(scalar_count(&raw))
    }

    /// Rescans `project_code` for near-duplicate symbol pairs and replaces its
    /// entire `SIMILAR_TO` edge set with the fresh result (full reconcile —
    /// see module docs for why this must not be incremental). Returns how many
    /// representative-chunk symbols were scanned and how many pairs survived
    /// the threshold. Idempotent: re-running with unchanged embeddings
    /// reproduces the same edge set (delete-then-reinsert, `ON CONFLICT DO
    /// NOTHING` guards the rare exact-tie race, not correctness).
    pub fn reconcile_duplication_edges(
        &self,
        project_code: &str,
    ) -> Result<DuplicationScanReport, String> {
        let proj = project_code.replace('\'', "''");

        self.execute(&format!(
            "DELETE FROM ist.edge WHERE relation_type = 'SIMILAR_TO' AND project_code = '{proj}'"
        ))
        .map_err(|e| e.to_string())?;

        let scanned_raw = self
            .query_json(&format!(
                "SELECT count(*) FROM ist.chunk c JOIN ist.chunkembedding ce ON ce.chunk_id = c.id \
                 WHERE c.source_type = 'symbol' AND c.chunk_part_index = 1 AND c.project_code = '{proj}'"
            ))
            .map_err(|e| e.to_string())?;
        let symbols_scanned = scalar_count(&scanned_raw);

        // Data-modifying CTE (WITH … INSERT … SELECT): deliberately routed
        // through `execute`, NOT `execute_raw_sql_gateway` — that gateway's
        // `is_read_only_sql` classifier only scans statements AFTER the first
        // `;`, so a single WITH-prefixed writable-CTE statement (no semicolon)
        // is misclassified as read-only. Logged as REQ-AXO-902207 (latent
        // gateway gap, no current caller exercises it) rather than fixed here
        // — out of scope for this slice.
        //
        // `SET statement_timeout` prefix: the connection-wide default is 30s
        // (AXON_PG_STATEMENT_TIMEOUT_MS, REQ-AXO-91494) — far below the
        // ~1m10-1m16s measured for a ~9300-symbol project. `apply_session_setup`
        // re-applies the 30s default at the START of every `run_execute` call
        // (before ours runs), so this override is scoped to THIS call only —
        // the next `execute()` on any connection resets it automatically.
        // Discovered via a real dev-test failure (`57014: canceling statement
        // due to statement timeout` on AXO/CDV), not invented speculatively.
        self.execute(&format!(
            "SET statement_timeout = '{RECONCILE_STATEMENT_TIMEOUT_MS}'; \
             WITH reps AS ( \
                 SELECT c.source_id AS id, ce.chunk_id AS chunk_id \
                 FROM ist.chunk c JOIN ist.chunkembedding ce ON ce.chunk_id = c.id \
                 WHERE c.source_type = 'symbol' AND c.chunk_part_index = 1 AND c.project_code = '{proj}' \
             ) \
             INSERT INTO ist.edge (source_id, target_id, relation_type, project_code, metadata, created_at_ms) \
             SELECT a.id, b.id, 'SIMILAR_TO', '{proj}', jsonb_build_object('distance', nn.dist), \
                    (extract(epoch from clock_timestamp()) * 1000)::bigint \
             FROM reps a \
             JOIN LATERAL ( \
                 SELECT ce.chunk_id, \
                        (ce.embedding <=> (SELECT embedding FROM ist.chunkembedding WHERE chunk_id = a.chunk_id)) AS dist \
                 FROM ist.chunkembedding ce \
                 ORDER BY (SELECT embedding FROM ist.chunkembedding WHERE chunk_id = a.chunk_id) <=> ce.embedding \
                 LIMIT {k} \
             ) nn ON true \
             JOIN reps b ON b.chunk_id = nn.chunk_id \
             WHERE nn.dist < {threshold} AND b.id > a.id \
             ON CONFLICT DO NOTHING",
            proj = proj,
            k = OVERFETCH_K,
            threshold = DUPLICATION_CLONE_THRESHOLD,
        ))
        .map_err(|e| e.to_string())?;

        let pairs_raw = self
            .query_json(&format!(
                "SELECT count(*) FROM ist.edge WHERE relation_type = 'SIMILAR_TO' AND project_code = '{proj}'"
            ))
            .map_err(|e| e.to_string())?;
        let pairs_found = scalar_count(&pairs_raw);

        Ok(DuplicationScanReport { symbols_scanned, pairs_found })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_count_reads_number_and_string_encoded_forms() {
        assert_eq!(scalar_count("[[3]]"), 3);
        assert_eq!(scalar_count(r#"[["3"]]"#), 3);
        assert_eq!(scalar_count("[]"), 0);
        assert_eq!(scalar_count("not json"), 0);
    }

    #[test]
    fn duplication_scan_due_only_when_no_pending_embeds() {
        assert!(duplication_scan_due(0));
        assert!(!duplication_scan_due(1));
        assert!(!duplication_scan_due(4_000));
    }
}
