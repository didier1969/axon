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
}
