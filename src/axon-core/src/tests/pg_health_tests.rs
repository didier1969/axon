// REQ-AXO-284 Slice 2 — live PG health helpers (`pg_database_size_bytes`,
// `pg_chunkembedding_total_bytes`, `pg_buffer_hit_ratio`). Each query
// degrades gracefully to `None` on catalog error, so these tests assert
// the helpers either return a sensible value OR `None` — never panic.
//
// The shared dev PG instance is presumed reachable (cargo test invokes
// `AXON_DEV_DATABASE_URL` set by callers). The helpers are deliberately
// cheap catalog reads, safe to run in parallel.

#[cfg(test)]
mod tests {
    use crate::tests::test_helpers::create_test_db;

    #[test]
    fn pg_database_size_bytes_returns_positive_value_on_live_pg() {
        let store = create_test_db().expect("create test db");
        let result = store.pg_database_size_bytes();
        match result {
            Some(bytes) => assert!(
                bytes > 0,
                "pg_database_size should be positive on a live database (got {bytes})"
            ),
            None => panic!(
                "pg_database_size_bytes returned None on the dev PG ; \
                 catalog access should not fail in normal test conditions"
            ),
        }
    }

    #[test]
    fn pg_chunkembedding_total_bytes_returns_value_or_none() {
        let store = create_test_db().expect("create test db");
        // The ChunkEmbedding table is bootstrapped on store init ; size
        // should resolve. We only require the helper not to panic + to
        // return a non-negative number when Some.
        if let Some(bytes) = store.pg_chunkembedding_total_bytes() {
            assert!(
                bytes >= 0,
                "pg_total_relation_size returned negative value: {bytes}"
            );
        }
    }

    #[test]
    fn pg_buffer_hit_ratio_returns_ratio_or_none() {
        let store = create_test_db().expect("create test db");
        match store.pg_buffer_hit_ratio() {
            Some(ratio) => {
                assert!(
                    (0.0..=1.0).contains(&ratio),
                    "pg_buffer_hit_ratio out of [0.0, 1.0]: {ratio}"
                );
            }
            // Acceptable: fresh DB with no blks_hit + blks_read activity.
            None => {}
        }
    }
}
