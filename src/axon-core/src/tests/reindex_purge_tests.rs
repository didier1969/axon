//! REQ-AXO-902011 — re-index-safe orphan purge (audit 901896 finding, Plane A).
//!
//! Editing a file in place (renamed/removed symbol, fewer chunk parts) must not
//! leave orphan Symbol/Chunk/Edge/ChunkEmbedding rows, AND must preserve inbound
//! edges owned by OTHER (caller) files that are not part of the re-index batch.

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::parser::Symbol;
    use crate::pipeline::types::ParsedFile;
    use crate::tests::test_helpers::create_test_db;

    fn sym(name: &str) -> Symbol {
        Symbol {
            name: name.to_string(),
            kind: "function".to_string(),
            start_line: 1,
            end_line: 2,
            docstring: None,
            is_entry_point: false,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: Default::default(),
            embedding: None,
        }
    }

    fn parsed_file(path: &str, content: &str, symbols: Vec<Symbol>) -> ParsedFile {
        ParsedFile {
            path: PathBuf::from(path),
            project_code: crate::pipeline::ProjectCode::parse("AXO").unwrap(),
            content: content.to_string(),
            content_hash: format!("hash-{}", content.len()),
            mtime_ms: 0,
            size_bytes: content.len() as u64,
            skip_reason: None,
            symbols,
            relations: Vec::new(),
            security_findings: Vec::new(),
        }
    }

    /// Re-indexing a file whose symbol was renamed purges the old symbol and its
    /// chunks — no orphan rows survive.
    #[test]
    fn reindex_purges_renamed_symbol_leaves_no_orphan() {
        let store = create_test_db().unwrap();
        let path = "/tmp/reindex_purge_test_a.rs";

        store
            .upsert_graph_batch(
                &[parsed_file(
                    path,
                    "fn func_alpha() {}",
                    vec![sym("func_alpha")],
                )],
                "AXO",
            )
            .unwrap();
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM ist.Symbol WHERE name = 'func_alpha'")
                .unwrap(),
            1,
            "v1 symbol indexed"
        );

        // Re-index the SAME file with the symbol renamed.
        store
            .upsert_graph_batch(
                &[parsed_file(
                    path,
                    "fn func_beta() {}",
                    vec![sym("func_beta")],
                )],
                "AXO",
            )
            .unwrap();

        assert_eq!(
            store
                .query_count("SELECT count(*) FROM ist.Symbol WHERE name = 'func_alpha'")
                .unwrap(),
            0,
            "renamed-away symbol must be purged (no orphan)"
        );
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM ist.Symbol WHERE name = 'func_beta'")
                .unwrap(),
            1,
            "current symbol present"
        );
        // No chunk for this file points at a symbol that no longer exists.
        assert_eq!(
            store
                .query_count(&format!(
                    "SELECT count(*) FROM ist.Chunk \
                     WHERE file_path = '{path}' \
                       AND source_id IS NOT NULL \
                       AND source_id NOT IN (SELECT id FROM ist.Symbol)"
                ))
                .unwrap(),
            0,
            "no orphan chunk after re-index"
        );
    }

    /// Re-indexing the callee file must NOT delete inbound CALLS edges that
    /// belong to a caller file absent from the batch — the narrow purge keeps
    /// `target_id`-side edges (only `source_id = path` outbound edges are dropped).
    #[test]
    fn reindex_preserves_inbound_edges_from_other_files() {
        let store = create_test_db().unwrap();
        let path = "/tmp/reindex_callee_test.rs";

        store
            .upsert_graph_batch(
                &[parsed_file(
                    path,
                    "fn callee_fn() {}",
                    vec![sym("callee_fn")],
                )],
                "AXO",
            )
            .unwrap();

        // A CALLS edge from ANOTHER file into this symbol (owned by the caller);
        // target_id resolved by subquery so the test needs no id plumbing.
        store
            .execute(
                "INSERT INTO ist.Edge \
                     (source_id, target_id, relation_type, project_code, created_at_ms) \
                 SELECT '/tmp/caller.rs::caller_fn', id, 'CALLS', 'AXO', 0 \
                 FROM ist.Symbol WHERE name = 'callee_fn' \
                 ON CONFLICT DO NOTHING",
            )
            .unwrap();
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM ist.Edge \
                     WHERE relation_type = 'CALLS' AND source_id = '/tmp/caller.rs::caller_fn'"
                )
                .unwrap(),
            1,
            "inbound edge seeded"
        );

        // Re-index the callee file (same symbol, edited body).
        store
            .upsert_graph_batch(
                &[parsed_file(
                    path,
                    "fn callee_fn() { let x = 1; }",
                    vec![sym("callee_fn")],
                )],
                "AXO",
            )
            .unwrap();

        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM ist.Edge \
                     WHERE relation_type = 'CALLS' AND source_id = '/tmp/caller.rs::caller_fn'"
                )
                .unwrap(),
            1,
            "inbound CALLS edge from another file must survive re-index of the callee"
        );
    }

    /// REQ-AXO-902012 — a chunk that fails embedding repeatedly is quarantined
    /// at the attempt cap (embed_status='failed') and leaves the sorted drain,
    /// instead of being re-`SELECT`ed forever (the poison-pill). Driven purely
    /// at the DB layer (no mock embedder — GUI-PRO-004): a real chunk is created
    /// via the ingest path, then `record_embed_failure` is exercised directly.
    #[test]
    fn embed_failure_quarantines_chunk_at_attempt_cap() {
        let store = create_test_db().unwrap();
        let path = "/tmp/embed_quarantine_test.rs";
        store
            .upsert_graph_batch(
                &[parsed_file(
                    path,
                    "fn embed_me() { let a = 1; let b = 2; let c = a + b; }",
                    vec![sym("embed_me")],
                )],
                "AXO",
            )
            .unwrap();

        let pending = store.select_chunks_needing_embedding(100).unwrap();
        assert!(!pending.is_empty(), "ingest created a pending chunk");
        let n = pending.len();

        // Below the cap (3): the chunk stays 'pending' and drainable.
        store.record_embed_failure(&pending, 3).unwrap();
        store.record_embed_failure(&pending, 3).unwrap();
        assert_eq!(
            store.select_chunks_needing_embedding(100).unwrap().len(),
            n,
            "below the cap the chunk is still retried (drainable)"
        );

        // The cap-th failure quarantines it → gone from the drain.
        store.record_embed_failure(&pending, 3).unwrap();
        assert_eq!(
            store.select_chunks_needing_embedding(100).unwrap().len(),
            0,
            "at the attempt cap the chunk leaves the drain (no poison-pill)"
        );
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM ist.Chunk WHERE embed_status = 'failed'")
                .unwrap() as usize,
            n,
            "quarantined chunk is marked failed"
        );
    }

    /// REQ-AXO-902277 — the B2 inference-HANG path must quarantine the culprit
    /// batch BEFORE it exits for a supervisor restart. Otherwise the poison
    /// chunk stays `pending` and is re-drained on every restart (the drain is a
    /// re-drainable reservoir, `WHERE embed_status='pending'` with no claim), so
    /// the same deterministic hang recurs and burns `max_restarts` in minutes —
    /// the class of the 2026-08-06 ~2-day silent outage. Same DB-layer proof as
    /// the REQ-902012 sibling (GUI-PRO-004, no mock embedder), driven through
    /// the hang path's own recorder `quarantine_hung_batch`.
    #[tokio::test]
    async fn hang_path_quarantines_poison_chunk_at_attempt_cap() {
        use crate::pipeline::stage_b1::ChunkForEmbedding;
        use crate::pipeline::stage_b2::quarantine_hung_batch;

        let store = std::sync::Arc::new(create_test_db().unwrap());
        let path = "/tmp/b2_hang_quarantine_test.rs";
        store
            .upsert_graph_batch(
                &[parsed_file(
                    path,
                    "fn embed_me() { let a = 1; let b = 2; let c = a + b; }",
                    vec![sym("embed_me")],
                )],
                "AXO",
            )
            .unwrap();

        let pending = store.select_chunks_needing_embedding(100).unwrap();
        assert!(!pending.is_empty(), "ingest created a pending chunk");
        let n = pending.len();
        // record_batch_failure only reads chunk_id; content/hash are irrelevant here.
        let batch: Vec<ChunkForEmbedding> = pending
            .iter()
            .map(|id| ChunkForEmbedding {
                chunk_id: id.clone(),
                content: String::new(),
                content_hash: String::new(),
            })
            .collect();

        // Two hangs below the cap (3): the chunk stays pending → re-drained on restart.
        quarantine_hung_batch(Some(&store), &batch).await;
        quarantine_hung_batch(Some(&store), &batch).await;
        assert_eq!(
            store.select_chunks_needing_embedding(100).unwrap().len(),
            n,
            "below the cap the hung chunk is still retried on the next restart"
        );

        // The cap-th hang quarantines it → gone from the drain, so the supervisor
        // restart no longer re-hangs on it.
        quarantine_hung_batch(Some(&store), &batch).await;
        assert_eq!(
            store.select_chunks_needing_embedding(100).unwrap().len(),
            0,
            "at the cap the poison chunk leaves the drain (restart won't re-hang)"
        );
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM ist.Chunk WHERE embed_status = 'failed'")
                .unwrap() as usize,
            n,
            "the hung chunk is marked failed"
        );
    }

    /// REQ-AXO-902277 — the hang path with no store handle (pure batching tests)
    /// must be a safe no-op, never a panic: the exit must still proceed.
    #[tokio::test]
    async fn hang_path_quarantine_is_noop_without_store() {
        use crate::pipeline::stage_b1::ChunkForEmbedding;
        use crate::pipeline::stage_b2::quarantine_hung_batch;

        let batch = vec![ChunkForEmbedding {
            chunk_id: "AXO::x::y::chunk".to_string(),
            content: String::new(),
            content_hash: String::new(),
        }];
        quarantine_hung_batch(None, &batch).await; // must not panic
    }

    /// REQ-AXO-902586 — Cascade file deletion must purge ALL edges involving the
    /// deleted file and its contained symbols (both inbound and outbound), leaving
    /// zero orphaned edges in ist.Edge.
    ///
    /// The real defect found in LLL:
    /// `examples/tmph5laa9_f.lll` was deleted from disk, but its outbound call edge
    /// `…fulfill --CALLS--> …stock_reserve` survived indefinitely in `ist.Edge`
    /// because `delete_file_cascade` never purged edges where the deleted file's
    /// symbols were the SOURCE (or incoming caller edges to the deleted symbols).
    #[test]
    fn delete_file_cascade_purges_all_contained_symbol_edges() {
        let store = create_test_db().unwrap();
        let path = "/tmp/delete_cascade_test_file.rs";
        let caller_external = "/tmp/external_caller.rs";

        // 1. Seed the file to delete with two internal symbols: `fulfill` and `stock_reserve`
        store
            .upsert_graph_batch(
                &[parsed_file(
                    path,
                    "fn fulfill() { stock_reserve(); } fn stock_reserve() {}",
                    vec![sym("fulfill"), sym("stock_reserve")],
                )],
                "AXO",
            )
            .unwrap();

        // 2. Add an internal CALLS edge between the file's symbols: fulfill --CALLS--> stock_reserve
        // and an external inbound CALLS edge: external_caller --CALLS--> fulfill
        // and an external outbound CALLS edge: stock_reserve --CALLS--> std::alloc
        store
            .execute(&format!(
                "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) \
                 SELECT s1.id, s2.id, 'CALLS', 'AXO', 0 \
                 FROM ist.Symbol s1, ist.Symbol s2 \
                 WHERE s1.name = 'fulfill' AND s2.name = 'stock_reserve'; \
                 \
                 INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) \
                 SELECT '{caller_external}::caller_fn', id, 'CALLS', 'AXO', 0 \
                 FROM ist.Symbol WHERE name = 'fulfill'; \
                 \
                 INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) \
                 SELECT id, 'std::alloc::alloc', 'CALLS', 'AXO', 0 \
                 FROM ist.Symbol WHERE name = 'stock_reserve';"
            ))
            .unwrap();

        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM ist.Edge WHERE project_code = 'AXO' AND relation_type = 'CALLS'"
                )
                .unwrap(),
            3,
            "3 CALLS edges seeded (internal, inbound from external, outbound to external)"
        );

        // 3. Delete the file via delete_file_cascade
        store.delete_file_cascade(path).unwrap();

        // 4. Assertions:
        // - IndexedFile must be gone
        assert_eq!(
            store
                .query_count(&format!(
                    "SELECT count(*) FROM ist.IndexedFile WHERE path = '{path}'"
                ))
                .unwrap(),
            0,
            "IndexedFile must be purged"
        );
        // - Symbols must be gone
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM ist.Symbol WHERE name IN ('fulfill', 'stock_reserve')"
                )
                .unwrap(),
            0,
            "Contained symbols must be purged"
        );
        // - Chunks must be gone
        assert_eq!(
            store
                .query_count(&format!(
                    "SELECT count(*) FROM ist.Chunk WHERE file_path = '{path}'"
                ))
                .unwrap(),
            0,
            "Chunks must be purged"
        );
        // - ALL edges referencing the file OR its contained symbols must be gone!
        assert_eq!(
            store
                .query_count(&format!(
                    "SELECT count(*) FROM ist.Edge WHERE source_id = '{path}' OR target_id = '{path}'"
                ))
                .unwrap(),
            0,
            "File edges must be purged"
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM ist.Edge WHERE project_code = 'AXO' AND relation_type = 'CALLS'"
                )
                .unwrap(),
            0,
            "ALL 3 CALLS edges referencing deleted symbols must be completely purged from ist.Edge"
        );
    }

    /// REQ-AXO-902586 — Deleting a symbol-less file or data artifact must succeed cleanly
    /// and purge its IndexedFile and file edges without error.
    #[test]
    fn delete_file_cascade_symbolless_file_succeeds_cleanly() {
        let store = create_test_db().unwrap();
        let path = "/tmp/empty_manifest.json";

        store
            .upsert_graph_batch(&[parsed_file(path, "{}", vec![])], "AXO")
            .unwrap();

        store.delete_file_cascade(path).unwrap();

        assert_eq!(
            store
                .query_count(&format!(
                    "SELECT count(*) FROM ist.IndexedFile WHERE path = '{path}'"
                ))
                .unwrap(),
            0,
            "IndexedFile must be purged for empty/data file"
        );
    }

    /// REQ-AXO-902586 — audit_stale_edge_residues correctly categorizes unindexed targets
    /// into stale residues (belonging to deleted files) vs legitimate unindexed calls
    /// (belonging to existing indexed files), and prune_stale_edge_residues purges only the residues.
    #[test]
    fn test_audit_and_prune_stale_edge_residues() {
        let store = create_test_db().unwrap();
        let alive_path = "/tmp/repo/src/alive.rs";

        // Seed an indexed alive file with one symbol
        store
            .upsert_graph_batch(
                &[parsed_file(
                    alive_path,
                    "pub fn alive_fn() {}",
                    vec![sym("alive_fn")],
                )],
                "AXO",
            )
            .unwrap();

        // Seed two edges:
        // 1. Legitimate unindexed: callee in alive.rs (e.g. macro or local item without symbol row)
        // 2. Stale residue: callee in deleted file examples/ghost.rs (which has no IndexedFile)
        store
            .execute(
                "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) \
                 VALUES \
                 ('AXO::src::alive.rs::alive_fn', 'AXO::src::alive.rs::helper_macro', 'CALLS', 'AXO', 0), \
                 ('AXO::src::alive.rs::alive_fn', 'AXO::examples::ghost.rs::ghost_fn', 'CALLS', 'AXO', 0);"
            )
            .unwrap();

        let (stale, legitimate) = store.audit_stale_edge_residues(Some("AXO")).unwrap();
        assert_eq!(
            stale, 1,
            "Expected 1 stale residue for deleted examples/ghost.rs"
        );
        assert_eq!(
            legitimate, 1,
            "Expected 1 legitimate unindexed call in existing src/alive.rs"
        );

        // Prune the residues
        let purged = store.prune_stale_edge_residues(Some("AXO")).unwrap();
        assert_eq!(purged, 1, "Expected exactly 1 edge purged");

        // Re-audit to verify clean state
        let (stale_after, legitimate_after) = store.audit_stale_edge_residues(Some("AXO")).unwrap();
        assert_eq!(stale_after, 0, "No stale residues should remain");
        assert_eq!(
            legitimate_after, 1,
            "Legitimate unindexed call must be preserved intact"
        );
    }
}
