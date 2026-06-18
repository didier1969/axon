//! REQ-AXO-901757 slice B (sub-slice B1) — SOLL node-description embedding
//! storage + staleness selection. DB-level, no embedder (GUI-PRO-004): the
//! embedding is a synthetic vector; only the store/select/staleness logic is
//! exercised here (the batch_embed populate sweep + retrieve_context RRF land in
//! follow-up sub-slices).

#[cfg(test)]
mod tests {
    use crate::tests::test_helpers::create_test_db;

    fn insert_node(store: &crate::graph::GraphStore, id: &str, title: &str, desc: &str) {
        // `TST` is seeded in the test template registry; the id-segment trigger
        // requires split_part(id,'-',2) == project_code.
        store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                 VALUES ('{id}', 'Requirement', 'TST', '{title}', '{desc}', 'planned', '{{}}') \
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description"
            ))
            .unwrap();
    }

    #[test]
    fn soll_node_embedding_store_select_and_staleness() {
        let store = create_test_db().unwrap();
        let id = "REQ-TST-001";
        insert_node(&store, id, "GPU embedding throughput", "restore the embed rate");

        // Before embedding: the node is selected as needing one, with its hash.
        let needing = store.select_soll_nodes_needing_embedding(100).unwrap();
        let (_nid, pc, _text, hash) = needing
            .iter()
            .find(|(nid, ..)| nid == id)
            .expect("node selected as needing embedding")
            .clone();

        // Store a (synthetic) embedding under the computed hash.
        let vec = vec![0.05f32; crate::embedding_contract::DIMENSION];
        store
            .upsert_soll_node_embedding(id, &pc, &hash, &vec, 0)
            .unwrap();
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM soll.NodeEmbedding WHERE node_id = 'REQ-TST-001'")
                .unwrap(),
            1,
            "embedding row stored"
        );

        // Now the node is NOT re-selected (hash matches current body).
        assert!(
            store
                .select_soll_nodes_needing_embedding(100)
                .unwrap()
                .iter()
                .all(|(nid, ..)| nid != id),
            "embedded node must drop out of the needing-embedding set"
        );

        // Edit the description → hash drifts → the node is stale again.
        insert_node(&store, id, "GPU embedding throughput", "restore the embed rate ON THE LANE");
        assert!(
            store
                .select_soll_nodes_needing_embedding(100)
                .unwrap()
                .iter()
                .any(|(nid, ..)| nid == id),
            "a body edit must re-stale the embedding (hash drift)"
        );
    }

    /// REQ-AXO-902015 — the staleness `source_hash` must represent EXACTLY the
    /// embedded text (title `\n` description), separator included. Two nodes whose
    /// embedded texts differ must get distinct hashes. The pre-fix code hashed
    /// `title||description` WITHOUT the separator, so `('ab','c')` and `('a','bc')`
    /// both hashed `md5('abc')` while their embedded texts (`'ab\nc'` vs `'a\nbc'`)
    /// differ — a hash collision that could suppress a legitimate re-embed.
    #[test]
    fn soll_staleness_hash_includes_embed_separator_no_collision() {
        let store = create_test_db().unwrap();
        insert_node(&store, "REQ-TST-001", "ab", "c");
        insert_node(&store, "REQ-TST-002", "a", "bc");

        let needing = store.select_soll_nodes_needing_embedding(100).unwrap();
        let get = |id: &str| -> (String, String) {
            needing
                .iter()
                .find(|(nid, ..)| nid == id)
                .map(|(_, _, text, hash)| (text.clone(), hash.clone()))
                .unwrap_or_else(|| panic!("{id} selected as needing embedding"))
        };
        let (text_a, hash_a) = get("REQ-TST-001");
        let (text_b, hash_b) = get("REQ-TST-002");

        assert_eq!(text_a, "ab\nc", "embedded text keeps the title/description separator");
        assert_eq!(text_b, "a\nbc", "embedded text keeps the title/description separator");
        assert_ne!(
            hash_a, hash_b,
            "distinct embedded texts must yield distinct staleness hashes (no separator-stripping collision)"
        );
    }

    /// REQ-AXO-901757 slice B (sub-slice B3a) — ANN search returns the SOLL node
    /// whose embedding is nearest the query vector. Synthetic unit vectors on
    /// distinct axes (no embedder, GUI-PRO-004): a query on axis-k is closest to
    /// the node embedded on axis-k (cosine distance 0).
    #[test]
    fn soll_ann_returns_nearest_node_by_embedding() {
        let store = create_test_db().unwrap();
        fn axis_vec(axis: usize) -> Vec<f32> {
            let mut v = vec![0.0f32; crate::embedding_contract::DIMENSION];
            v[axis] = 1.0;
            v
        }
        for (id, axis) in [("REQ-TST-001", 0usize), ("REQ-TST-002", 1), ("REQ-TST-003", 2)] {
            insert_node(&store, id, &format!("node {id}"), "semantic body");
            store
                .upsert_soll_node_embedding(id, "TST", &format!("h-{axis}"), &axis_vec(axis), 0)
                .unwrap();
        }

        let hits = store.select_soll_nodes_by_ann(&axis_vec(1), 3).unwrap();
        assert!(!hits.is_empty(), "ANN returns results");
        assert_eq!(hits[0].0, "REQ-TST-002", "nearest node by embedding: {hits:?}");
        assert!(
            hits[0].1 < 0.01,
            "self-match cosine distance ~0, got {:?}",
            hits[0]
        );
    }
}
