//! MIL-AXO-015 P3 smoke test: GraphStore::new under AXON_DB_BACKEND=postgres.
//!
//! Marked `#[ignore]`. Requirements:
//!   1. Docker runtime available (testcontainers spawns axon-test/age-pgvector).
//!   2. `libaxon_plugin_postgres.so` already built — the test does not
//!      shell out to cargo. Build with:
//!
//!          cargo build --manifest-path src/axon-plugin-postgres/Cargo.toml --lib
//!
//! The test boots a fresh PG container with AGE preinstalled, points
//! axon-core at it via `AXON_LIVE_DATABASE_URL`, and asserts that
//! `GraphStore::new` runs through `bootstrap_global_pg_schema`
//! cleanly. Subsequent queries against `soll.Node` confirm the global
//! schema bootstrap actually wired the SOLL layer.

use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use axon_core::graph::GraphStore;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{GenericImage, ImageExt};

/// Serialise PG-backend tests: each one mutates `AXON_DB_BACKEND` +
/// the URL env vars, and they would race with the duckdb-default
/// tests if cargo runs them in parallel.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
#[ignore = "requires docker; opt-in via `cargo test -- --ignored`"]
fn graphstore_boots_under_postgres_backend() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let container = GenericImage::new("axon-test/age-pgvector", "pg17")
        .with_exposed_port(ContainerPort::Tcp(5432))
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_PASSWORD", "axon_test_pw")
        .with_env_var("POSTGRES_DB", "axon_test_db")
        .with_env_var("POSTGRES_USER", "postgres")
        .start()
        .expect("start container");
    let port = container
        .get_host_port_ipv4(5432)
        .expect("ephemeral host port");
    let url = format!("postgres://postgres:axon_test_pw@127.0.0.1:{port}/axon_test_db");

    std::env::set_var("AXON_DB_BACKEND", "postgres");
    std::env::set_var("AXON_LIVE_DATABASE_URL", &url);
    // Defensive: the resolver tries AXON_LIVE first, then DEV, then
    // generic DATABASE_URL. Clear DEV so a host-set value does not
    // leak in.
    std::env::remove_var("AXON_DEV_DATABASE_URL");

    // PG sometimes races between "ready" message and accepting TCP.
    let mut last_err = None;
    let mut store = None;
    for _ in 0..10 {
        match GraphStore::new("/tmp/axon-pg-smoke-unused") {
            Ok(s) => {
                store = Some(s);
                break;
            }
            Err(e) => {
                last_err = Some(e);
                sleep(Duration::from_millis(500));
            }
        }
    }
    let store = store.unwrap_or_else(|| {
        panic!(
            "GraphStore::new under PG failed after retries: {:?}",
            last_err
        )
    });

    // bootstrap_global_pg_schema should have created soll.Node.
    let soll_node_count = store
        .query_count("SELECT count(*)::BIGINT FROM soll.Node")
        .expect("query_count on soll.Node should succeed under PG backend");
    assert_eq!(
        soll_node_count, 0,
        "fresh PG database should have zero SOLL nodes"
    );

    // ProjectCodeRegistry should also exist in the soll schema (REQ-AXO-247:
    // PCR moved from public to soll to match the consumer code path that the
    // DuckDB-era init_schema established).
    let registry_count = store
        .query_count("SELECT count(*)::BIGINT FROM soll.ProjectCodeRegistry")
        .expect("query_count on soll.ProjectCodeRegistry should succeed under PG");
    assert_eq!(registry_count, 0);
    // Negative assertion: the legacy public.ProjectCodeRegistry must NOT exist —
    // had it been dual-created the registry sync paths would silently fork.
    let public_pcr_exists = store
        .query_count(
            "SELECT count(*)::BIGINT FROM information_schema.tables \
             WHERE table_schema = 'public' AND lower(table_name) = lower('ProjectCodeRegistry')",
        )
        .expect("information_schema lookup for legacy public.ProjectCodeRegistry");
    assert_eq!(
        public_pcr_exists, 0,
        "legacy public.ProjectCodeRegistry must not be created by bootstrap_global_pg_schema"
    );

    // soll.McpJob: REQ-AXO-247 also added the async-job mirror so
    // axon_commit_work + soll_apply_plan persist state under PG. Verify the
    // table + its two covering indexes are in place.
    let mcp_job_count = store
        .query_count("SELECT count(*)::BIGINT FROM soll.McpJob")
        .expect("query_count on soll.McpJob should succeed");
    assert_eq!(mcp_job_count, 0);
    let mcp_job_idx_count = store
        .query_count(
            "SELECT count(*)::BIGINT FROM pg_indexes \
             WHERE schemaname = 'soll' \
               AND lower(indexname) IN (lower('soll_mcp_job_status_idx'), \
                                        lower('soll_mcp_job_project_idx'))",
        )
        .expect("pg_indexes lookup for soll.McpJob");
    assert_eq!(
        mcp_job_idx_count, 2,
        "soll.McpJob covering indexes should be created"
    );

    // AGE extension should be loaded — confirm via pg_extension.
    let age_count = store
        .query_count("SELECT count(*)::BIGINT FROM pg_extension WHERE extname = 'age'")
        .expect("query_count on pg_extension should succeed under PG");
    assert_eq!(age_count, 1, "AGE extension should be installed");

    // Post-CPT-AXO-039 supersedure (2026-05-08) + option B: every IST
    // table lives in `public` (provisioned by bootstrap_global_pg_schema)
    // and AGE elabels are pre-declared for the writer migration. We
    // verify the multi-project File table is readable, then round-trip
    // a ChunkEmbedding via the project_code-aware upsert helper.
    let public_files_count = store
        .query_count("SELECT count(*)::BIGINT FROM public.File")
        .expect("query against multi-project File table should succeed");
    assert_eq!(public_files_count, 0);

    // generate_project_schema is still exposed for API stability but
    // emits zero DDL post-supersedure.
    let project_stmts = axon_core::postgres::ddl::generate_project_schema("TST")
        .expect("generate_project_schema('TST') should succeed");
    assert!(
        project_stmts.is_empty(),
        "generate_project_schema must be a no-op post-CPT-AXO-039 supersedure"
    );

    // P4 slice 4c (refactored): round-trip a single ChunkEmbedding via
    // the project_code-aware upsert helper. Validates pgvector text
    // serialisation + the HNSW-backed table accepts the row.
    use axon_core::postgres::vector::{upsert_chunk_embedding_sql, vector_literal};
    let mut sample = vec![0.0_f32; axon_core::embedding_contract::DIMENSION];
    sample[0] = 0.42;
    sample[1] = -0.13;
    let upsert = upsert_chunk_embedding_sql(
        "chunk-x",
        "code-1024",
        "TST",
        "hash-abc",
        &sample,
        1714999999000,
    )
    .expect("upsert SQL builds");
    store
        .execute(&upsert)
        .expect("pgvector upsert should succeed against combined image");
    let count = store
        .query_count(
            "SELECT count(*)::BIGINT FROM public.ChunkEmbedding WHERE project_code = 'TST'",
        )
        .expect("count ChunkEmbedding scoped by project_code");
    assert_eq!(count, 1, "upsert should land exactly one row");
    // Idempotence: re-issuing the same upsert under ON CONFLICT keeps
    // the row count at 1.
    store.execute(&upsert).expect("pgvector upsert idempotent");
    let count_after_replay = store
        .query_count(
            "SELECT count(*)::BIGINT FROM public.ChunkEmbedding WHERE project_code = 'TST'",
        )
        .expect("count after replay");
    assert_eq!(count_after_replay, 1);
    // Sanity: vector_literal parses back via the same helper.
    let _ = vector_literal(&sample).expect("literal builds for round-trip");

    // Option B: AGE labels (vlabel + elabel) are declared by
    // bootstrap_global_pg_schema for the writer migration that follows.
    // Verify each label exists in ag_catalog so the migration plan has
    // a stable foundation.
    let graph_count = store
        .query_count("SELECT count(*)::BIGINT FROM ag_catalog.ag_graph WHERE name = 'axon_graph'")
        .expect("ag_catalog.ag_graph readable");
    assert_eq!(graph_count, 1, "axon_graph must exist after bootstrap");
    for label in [
        "File",
        "Symbol",
        "Chunk",
        "CONTAINS",
        "CALLS",
        "CALLS_NIF",
        "IMPACTS",
        "SUBSTANTIATES",
    ] {
        let label_count = store
            .query_count(&format!(
                "SELECT count(*)::BIGINT FROM ag_catalog.ag_label l \
                 JOIN ag_catalog.ag_graph g ON g.graphid = l.graph \
                 WHERE g.name = 'axon_graph' AND l.name = '{label}'"
            ))
            .unwrap_or_else(|e| panic!("ag_label query for '{label}': {e:?}"));
        assert_eq!(label_count, 1, "AGE label '{label}' must be declared");
    }

    // MIL-AXO-015 P5: seed loader round-trip. Apply a synthetic
    // SeedDocument with one node, one edge, one registry row, and one
    // revision; confirm the rows land in the live SOLL layer and that
    // re-applying is a no-op (the empty-check on soll.Node guards
    // double-loading).
    let synthetic = serde_json::json!({
        "version": 1,
        "generated_at_ms": 1714999999000_i64,
        "nodes": [
            {"id": "VIS-TST-001", "type": "Vision", "project_code": "TST",
             "title": "Test vision", "description": "smoke", "status": "active",
             "metadata": {"tag": "smoke"}}
        ],
        "edges": [
            {"source_id": "VIS-TST-001", "target_id": "VIS-TST-001",
             "relation_type": "EPITOMIZES", "project_code": "TST"}
        ],
        "registry": [
            {"project_code": "TST", "id": "TST", "last_vis": 1, "last_pil": 0,
             "last_req": 0, "last_cpt": 0, "last_dec": 0, "last_mil": 0,
             "last_val": 0, "last_stk": 0, "last_gui": 0, "last_prv": 0,
             "last_rev": 0}
        ],
        "revisions": [
            {"revision_id": "REV-TST-001", "project_code": "TST",
             "author": "smoke-test", "summary": "initial",
             "created_at": 1714999999000_i64}
        ]
    });
    let doc: axon_core::postgres::seed::SeedDocument = serde_json::from_value(synthetic).unwrap();
    let inserted = axon_core::postgres::seed::apply_seed(&store, &doc)
        .expect("apply_seed should succeed against PG-backed store");
    assert_eq!(
        inserted, 4,
        "expected 1 registry + 1 node + 1 edge + 1 revision"
    );
    assert_eq!(
        store
            .query_count("SELECT count(*)::BIGINT FROM soll.Node")
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .query_count("SELECT count(*)::BIGINT FROM soll.Edge")
            .unwrap(),
        1
    );

    // Re-applying via load_seed_if_needed must no-op now that
    // soll.Node is non-empty. Use a tempfile so the empty-check fires.
    let tmpfile = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmpfile.path(), serde_json::to_string(&doc).unwrap()).unwrap();
    let inserted_again = axon_core::postgres::seed::load_seed_if_needed(&store, tmpfile.path())
        .expect("re-apply should be a no-op");
    assert_eq!(inserted_again, 0);

    // MIL-AXO-015 P4 4e seed: assert the axon schema + tables
    // were created by bootstrap_global_pg_schema. Indexer hot-path
    // writes (slice 4e steps 1-4) go through these.
    let runtime_schema_present = store
        .query_count(
            "SELECT count(*)::BIGINT FROM information_schema.schemata WHERE schema_name = 'axon'",
        )
        .expect("axon schema check");
    assert_eq!(
        runtime_schema_present, 1,
        "axon schema should exist"
    );
    for table in [
        "VectorWorkerFault",
        "VectorLaneState",
        "VectorPersistOutbox",
        "vector_batch_run",
    ] {
        let exists = store
            .query_count(&format!(
                "SELECT count(*)::BIGINT FROM information_schema.tables \
                 WHERE table_schema = 'axon' AND lower(table_name) = lower('{table}')",
            ))
            .unwrap_or_else(|e| panic!("table existence check for axon.{table}: {e:?}"));
        assert_eq!(
            exists, 1,
            "axon.{table} should be present after bootstrap"
        );
    }

    // ── REQ-AXO-247 AC #4 + REQ-AXO-249 closure: end-to-end MCP workflow
    // ── smoke. Mirrors the failing flip-rehearsal that surfaced both gaps:
    //
    //  - soll.ProjectCodeRegistry write (mirrors axon_init_project's
    //    sync_project_registry_entry path).
    //  - JSON helper rewrite for soll_validate (DuckDB-style
    //    `json_extract(metadata, '$.acceptance_criteria')` must rewrite
    //    to `metadata->'acceptance_criteria'` under PG; the JSON-encoded
    //    string fixup must yield a JSONB object so the `IN ('', '[]')`
    //    completeness check evaluates correctly).
    let synthetic_pcr = "INSERT INTO soll.ProjectCodeRegistry \
        (project_code, project_name, project_path) \
        VALUES ('AXO', 'axon', '/tmp/test-axo') \
        ON CONFLICT (project_code) DO UPDATE \
            SET project_name = EXCLUDED.project_name, \
                project_path = EXCLUDED.project_path";
    store
        .execute(synthetic_pcr)
        .expect("INSERT INTO soll.ProjectCodeRegistry should succeed (REQ-AXO-247)");
    let pcr_count = store
        .query_count(
            "SELECT count(*)::BIGINT FROM soll.ProjectCodeRegistry WHERE project_code = 'AXO'",
        )
        .expect("read back soll.ProjectCodeRegistry");
    assert_eq!(pcr_count, 1, "registry sync should land exactly one row");

    // soll.Node with metadata containing a JSON-encoded acceptance_criteria
    // value (mirroring soll-export-seed's to_json output before REQ-AXO-249's
    // unwrapping fix). After REQ-AXO-249 the seed loader unwraps the encoded
    // string into a real JSONB object, so completeness queries see arrays/
    // strings via metadata->>'acceptance_criteria' rather than JSONB string
    // scalars that always return NULL.
    let req_with_ac = serde_json::json!({
        "version": 1,
        "generated_at_ms": 1714999999000_i64,
        "nodes": [
            {"id": "REQ-AXO-9999", "type": "Requirement", "project_code": "AXO",
             "title": "Smoke req", "description": "REQ-247/249 smoke",
             "status": "open",
             // NB: metadata is a JSON OBJECT here. The seed-loader path that
             // gets exercised by load_seed_if_needed parses inner JSON-encoded
             // strings into objects before insert (REQ-AXO-249 fix), so this
             // shape covers the post-fix contract.
             "metadata": {"acceptance_criteria": ["x", "y"], "priority": "P1"}}
        ],
        "edges": [],
        "registry": [],
        "revisions": []
    });
    let doc_ac: axon_core::postgres::seed::SeedDocument =
        serde_json::from_value(req_with_ac).unwrap();
    let inserted_ac = axon_core::postgres::seed::apply_seed(&store, &doc_ac)
        .expect("apply_seed for REQ-247/249 smoke should succeed");
    assert_eq!(
        inserted_ac, 1,
        "expected exactly one new node from smoke seed"
    );

    // Sanity probe: jsonb_typeof confirms metadata is stored as an OBJECT
    // (not a JSONB string scalar — that would mean the pre-REQ-249
    // double-encoding leaked through).
    let metadata_typeof = store
        .query_json("SELECT jsonb_typeof(metadata) FROM soll.Node WHERE id = 'REQ-AXO-9999'")
        .expect("jsonb_typeof on soll.Node.metadata");
    assert!(
        metadata_typeof.contains("object"),
        "metadata must be a JSONB object (not a JSONB string scalar); got {metadata_typeof}"
    );

    // DuckDB-flavoured query rewritten by normalize_attached_soll_query
    // (REQ-AXO-249). The MCP tool surface emits this exact shape from
    // completeness_coverage.rs:174 — must round-trip cleanly under PG.
    let dbduck_style = store
        .query_count(
            "SELECT COUNT(*)::BIGINT FROM soll.Node r \
             WHERE r.id = 'REQ-AXO-9999' \
               AND COALESCE(CAST(json_extract(r.metadata, '$.acceptance_criteria') AS VARCHAR), '') NOT IN ('', '[]')",
        )
        .expect("DuckDB-style json_extract should be rewritten to JSONB operator under PG");
    assert_eq!(
        dbduck_style, 1,
        "REQ-AXO-9999 has acceptance_criteria=['x','y'] so the query should match it"
    );
    let dbduck_style_zero = store
        .query_count(
            "SELECT COUNT(*)::BIGINT FROM soll.Node r \
             WHERE r.id = 'REQ-AXO-9999' \
               AND COALESCE(CAST(json_extract(r.metadata, '$.nonexistent_key') AS VARCHAR), '') IN ('', '[]')",
        )
        .expect("rewriter should also handle the negative case");
    assert_eq!(
        dbduck_style_zero, 1,
        "missing key should fall through the COALESCE empty-string check"
    );

    // ── REQ-AXO-247 AC #4 final: sync_project_registry_entry must be
    // ── idempotent under conflict (mirrors axon_init_project re-runs).
    store
        .execute(synthetic_pcr)
        .expect("re-running registry insert under ON CONFLICT must be a no-op");
    let pcr_count_after = store
        .query_count(
            "SELECT count(*)::BIGINT FROM soll.ProjectCodeRegistry WHERE project_code = 'AXO'",
        )
        .expect("read back soll.ProjectCodeRegistry after replay");
    assert_eq!(pcr_count_after, 1, "registry replay must remain idempotent");

    // MIL-AXO-017 slice 6B Phase D: AGE cascade smoke retired (AGE removed).
    drop(store);

    // Reset env so subsequent test runs in the same `cargo test`
    // invocation do not inherit the postgres backend selection.
    std::env::remove_var("AXON_DB_BACKEND");
    std::env::remove_var("AXON_LIVE_DATABASE_URL");
}
