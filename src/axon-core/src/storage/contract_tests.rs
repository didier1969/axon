use super::*;

#[test]
fn test_storage_engine_mode_resolution() {
    // Par défaut sans variable d'environnement -> Postgres
    std::env::remove_var("AXON_STORAGE");
    assert_eq!(StorageEngineMode::from_env(), StorageEngineMode::Postgres);

    // Mode embedded explicite
    std::env::set_var("AXON_STORAGE", "embedded");
    assert_eq!(StorageEngineMode::from_env(), StorageEngineMode::Embedded);

    // Mode sqlite alias
    std::env::set_var("AXON_STORAGE", "sqlite");
    assert_eq!(StorageEngineMode::from_env(), StorageEngineMode::Embedded);

    // Nettoyage de l'environnement pour sanctuariser le mode nominal
    std::env::remove_var("AXON_STORAGE");
    assert_eq!(StorageEngineMode::from_env(), StorageEngineMode::Postgres);
}

#[test]
fn test_embedded_storage_engine_bootstrap_and_queries() {
    let engine = EmbeddedStorageEngine::new_in_memory()
        .expect("EmbeddedStorageEngine in-memory bootstrap failed");

    // Vérification initiale : table vide
    let count = engine.run_query_count("SELECT COUNT(*) FROM soll.entity;");
    assert_eq!(count, 0);

    // Insertion d'une entité SOLL
    let insert_sql = r#"
        INSERT INTO soll.entity (id, kind, title, description, status, created_at, updated_at)
        VALUES ('REQ-AXO-TEST-001', 'requirement', 'Dual Engine Test', 'Test description', 'draft', 1000, 1000);
    "#;
    engine
        .run_execute(insert_sql)
        .expect("Failed to insert into soll.entity");

    // Vérification du comptage après insertion
    let count_after = engine.run_query_count("SELECT COUNT(*) FROM soll.entity;");
    assert_eq!(count_after, 1);

    // Requêtage structuré par table
    let table = engine
        .run_query_table("SELECT id, kind, title FROM soll.entity WHERE id = 'REQ-AXO-TEST-001';")
        .expect("Query table failed");
    assert_eq!(table.row_count, 1);
    assert_eq!(table.columns, vec!["id", "kind", "title"]);
    assert_eq!(
        table.rows[0],
        vec!["REQ-AXO-TEST-001", "requirement", "Dual Engine Test"]
    );

    // Requêtage JSON
    let json_str = engine.run_query_json("SELECT id, kind FROM soll.entity;");
    let parsed: Vec<Vec<String>> =
        serde_json::from_str(&json_str).expect("Failed to parse JSON rows");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0], vec!["REQ-AXO-TEST-001", "requirement"]);

    // Vérification des tables IST
    let ist_count = engine.run_query_count("SELECT COUNT(*) FROM ist.symbol;");
    assert_eq!(ist_count, 0);

    let insert_symbol = r#"
        INSERT INTO ist.symbol (id, name, kind, file_path, start_line, end_line)
        VALUES ('sym-1', 'StorageEngine', 'trait', 'src/storage/mod.rs', 10, 50);
    "#;
    engine
        .run_execute(insert_symbol)
        .expect("Failed to insert into ist.symbol");
    assert_eq!(
        engine.run_query_count("SELECT COUNT(*) FROM ist.symbol;"),
        1
    );

    // Erreur de syntaxe gérée proprement dans run_query_json
    let err_json = engine.run_query_json("SELECT * FROM table_inexistante;");
    assert!(err_json.contains("_axon_plugin_error"));
    assert!(err_json.contains("EMBEDDED_SQLITE_ERR"));
}

#[test]
fn test_embedded_storage_engine_flush_batch_and_embeddings() {
    let engine = EmbeddedStorageEngine::new_in_memory()
        .expect("EmbeddedStorageEngine in-memory bootstrap failed");

    // Construction d'un PgBulkBatch complet
    let batch = PgBulkBatch {
        symbols: vec![crate::graph_ingestion::rows::SymbolRow {
            symbol_id: "sym::test::1".to_string(),
            name: "test_fn".to_string(),
            kind: "function".to_string(),
            tested: true,
            is_public: true,
            is_nif: false,
            is_unsafe: false,
            is_entry_point: false,
            project_code: "AXO".to_string(),
            embedding: None,
            cyclomatic_complexity: Some(3),
        }],
        chunks: vec![],
        contains: vec![crate::graph_ingestion::rows::RelationRow {
            source_id: "file::test.rs".to_string(),
            target_id: "sym::test::1".to_string(),
            project_code: "AXO".to_string(),
        }],
        calls: vec![crate::graph_ingestion::rows::RelationRow {
            source_id: "sym::test::1".to_string(),
            target_id: "sym::target::2".to_string(),
            project_code: "AXO".to_string(),
        }],
        calls_nif: vec![],
        other_edges: vec![],
        indexed_files: vec![(
            "src/test.rs".to_string(),
            "hash123".to_string(),
            1000,
            500,
            2000,
        )],
        skipped_files: vec![],
        security_files: vec![],
        security_findings: vec![],
        project_code: "AXO".to_string(),
    };

    engine
        .flush_batch_copy(&batch)
        .expect("flush_batch_copy failed");

    assert_eq!(
        engine.run_query_count("SELECT COUNT(*) FROM ist.indexedfile;"),
        1
    );
    assert_eq!(
        engine.run_query_count("SELECT COUNT(*) FROM ist.symbol;"),
        1
    );
    assert_eq!(engine.run_query_count("SELECT COUNT(*) FROM ist.edge;"), 2);

    // Test de flush_chunk_embeddings_copy
    let embeddings = vec![
        ChunkEmbeddingPersistRow {
            chunk_id: "chk-001".to_string(),
            source_hash: "hash001".to_string(),
            embedding: vec![0.1, 0.2, 0.3],
        },
        ChunkEmbeddingPersistRow {
            chunk_id: "chk-002".to_string(),
            source_hash: "hash002".to_string(),
            embedding: vec![0.4, 0.5, 0.6],
        },
    ];

    engine
        .flush_chunk_embeddings_copy("AXO", "bge-large", &embeddings, 5000)
        .expect("flush_chunk_embeddings_copy failed");

    assert_eq!(
        engine.run_query_count("SELECT COUNT(*) FROM ist.ChunkEmbedding;"),
        2
    );
}

#[test]
fn test_embedded_storage_engine_vector_similarity() {
    let engine = EmbeddedStorageEngine::new_in_memory()
        .expect("EmbeddedStorageEngine in-memory bootstrap failed");

    let table = engine
        .run_query_table("SELECT cosine_similarity('[1.0, 0.0]', '[1.0, 0.0]') as sim, cosine_distance('[1.0, 0.0]', '[1.0, 0.0]') as dist;")
        .expect("vector similarity function failed");

    assert_eq!(table.row_count, 1);
    let sim: f64 = table.rows[0][0].parse().unwrap();
    let dist: f64 = table.rows[0][1].parse().unwrap();
    assert!((sim - 1.0).abs() < 1e-4);
    assert!(dist.abs() < 1e-4);

    let ortho_table = engine
        .run_query_table("SELECT cosine_similarity('[1.0, 0.0]', '[0.0, 1.0]') as sim;")
        .expect("orthogonal vector query failed");
    let ortho_sim: f64 = ortho_table.rows[0][0].parse().unwrap();
    assert!(ortho_sim.abs() < 1e-4);
}

#[test]
fn test_embedded_storage_engine_persistence() {
    let temp_dir = std::env::temp_dir().join(format!("axon_test_db_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);

    {
        let engine = EmbeddedStorageEngine::open(temp_dir.to_str().unwrap())
            .expect("open persistent DB failed");
        let insert_sql = r#"
            INSERT INTO soll.entity (id, kind, title, description, status, created_at, updated_at)
            VALUES ('REQ-PERSIST-001', 'requirement', 'Persistent Title', 'Desc', 'approved', 1, 1);
        "#;
        engine.run_execute(insert_sql).expect("insert failed");
        assert_eq!(
            engine.run_query_count("SELECT COUNT(*) FROM soll.entity;"),
            1
        );
    }

    // Réouverture de la même base sur disque
    {
        let reopened = EmbeddedStorageEngine::open(temp_dir.to_str().unwrap())
            .expect("re-open persistent DB failed");
        assert_eq!(
            reopened.run_query_count("SELECT COUNT(*) FROM soll.entity;"),
            1
        );
        let table = reopened
            .run_query_table("SELECT id, title FROM soll.entity WHERE id = 'REQ-PERSIST-001';")
            .expect("query failed");
        assert_eq!(table.rows[0][0], "REQ-PERSIST-001");
        assert_eq!(table.rows[0][1], "Persistent Title");
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_graph_store_boot_with_embedded_engine() {
    std::env::set_var("AXON_STORAGE", "embedded");
    let store = crate::graph::GraphStore::new(":memory:")
        .expect("GraphStore failed to boot with AXON_STORAGE=embedded");

    // Exécution d'une écriture via le GraphStore unifié
    let insert_sql = r#"
        INSERT INTO soll.entity (id, kind, title, description, status, created_at, updated_at)
        VALUES ('DEC-AXO-DUAL-001', 'decision', 'Dual Engine Decision', 'Desc', 'approved', 1, 1);
    "#;
    store
        .execute(insert_sql)
        .expect("GraphStore.execute failed");

    // Requêtage par table
    let table = store
        .query_table("SELECT id, title FROM soll.entity WHERE id = 'DEC-AXO-DUAL-001';")
        .expect("GraphStore.query_table failed");
    assert_eq!(table.row_count, 1);
    assert_eq!(table.rows[0][0], "DEC-AXO-DUAL-001");
    assert_eq!(table.rows[0][1], "Dual Engine Decision");

    // Requêtage JSON
    let json = store
        .query_json("SELECT id FROM soll.entity WHERE id = 'DEC-AXO-DUAL-001';")
        .expect("GraphStore.query_json failed");
    assert!(json.contains("DEC-AXO-DUAL-001"));

    // Nettoyage impératif pour sanctuariser le mode nominal PostgreSQL
    std::env::remove_var("AXON_STORAGE");
}
