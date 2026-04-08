use crate::embedder::default_embedding_profile;
use crate::graph::GraphStore;
use crate::graph_bootstrap::embedding_column_type_sql;
use crate::graph_ingestion::embedding_cast_sql;

fn table_column_type(store: &GraphStore, table: &str, column: &str) -> String {
    let raw = store
        .query_json(&format!(
            "SELECT type FROM pragma_table_info('{}') WHERE name = '{}'",
            table, column
        ))
        .unwrap();
    let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap();
    rows.into_iter()
        .next()
        .and_then(|row| row.into_iter().next())
        .expect("column type should exist")
}

#[test]
fn test_embedding_column_type_sql_tracks_requested_dimension() {
    assert_eq!(embedding_column_type_sql(384), "FLOAT[384]");
    assert_eq!(embedding_column_type_sql(768), "FLOAT[768]");
}

#[test]
fn test_embedding_cast_sql_tracks_requested_dimension() {
    assert_eq!(
        embedding_cast_sql(&[0.0_f32, 1.0_f32], 768),
        "CAST([0.0, 1.0] AS FLOAT[768])"
    );
}

#[test]
fn test_runtime_metadata_records_embedding_profile_contract() {
    let temp = tempfile::tempdir().unwrap();
    let profile = default_embedding_profile();
    let store = GraphStore::new(temp.path().to_str().unwrap()).unwrap();

    assert_eq!(
        store
            .query_count(&format!(
                "SELECT count(*) FROM RuntimeMetadata WHERE key = 'embedding_dimension' AND value = '{}'",
                profile.dimension
            ))
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .query_count(&format!(
                "SELECT count(*) FROM RuntimeMetadata WHERE key = 'embedding_model_name' AND value = '{}'",
                profile.model_name
            ))
            .unwrap(),
        1
    );
}

#[test]
fn test_embedding_dimension_drift_soft_invalidates_embedding_layers() {
    let temp = tempfile::tempdir().unwrap();
    let profile = default_embedding_profile();
    let db_root = temp.path().to_str().unwrap().to_string();

    let store = GraphStore::new(&db_root).unwrap();
    store
        .bulk_insert_files(&[("/tmp/embed_dim_reset.ex".to_string(), "proj".to_string(), 100, 1)])
        .unwrap();
    store
        .execute("INSERT INTO Symbol (id, name, kind, project_slug) VALUES ('sym-embed-dim-reset', 'embed_dim_reset', 'function', 'proj')")
        .unwrap();
    store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_slug, kind, content, content_hash, start_line, end_line) VALUES ('chunk-embed-dim-reset', 'symbol', 'sym-embed-dim-reset', 'proj', 'function', 'content', 'hash-1', 1, 1)")
        .unwrap();
    store
        .execute(&format!(
            "INSERT INTO EmbeddingModel (id, kind, model_name, dimension, version, created_at) VALUES ('model-embed-dim-reset', 'chunk', '{}', {}, '1', 1)",
            profile.model_name,
            profile.dimension
        ))
        .unwrap();
    store
        .execute("INSERT INTO ChunkEmbedding (chunk_id, model_id, source_hash) VALUES ('chunk-embed-dim-reset', 'model-embed-dim-reset', 'hash-1')")
        .unwrap();
    store.execute("DELETE FROM RuntimeMetadata;").unwrap();
    store
        .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('schema_version', '3')")
        .unwrap();
    store
        .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('ingestion_version', '3')")
        .unwrap();
    store
        .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('embedding_version', '2')")
        .unwrap();
    store
        .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('embedding_dimension', '999')")
        .unwrap();
    store
        .execute(&format!(
            "INSERT INTO RuntimeMetadata (key, value) VALUES ('embedding_model_name', '{}')",
            profile.model_name
        ))
        .unwrap();
    drop(store);

    let reopened = GraphStore::new(&db_root).unwrap();

    assert_eq!(reopened.query_count("SELECT count(*) FROM ChunkEmbedding").unwrap(), 0);
    assert_eq!(
        reopened
            .query_count(&format!(
                "SELECT count(*) FROM RuntimeMetadata WHERE key = 'embedding_dimension' AND value = '{}'",
                profile.dimension
            ))
            .unwrap(),
        1
    );
}

#[test]
fn test_embedding_model_drift_soft_invalidates_full_semantic_state() {
    let temp = tempfile::tempdir().unwrap();
    let profile = default_embedding_profile();
    let db_root = temp.path().to_str().unwrap().to_string();
    let vector = vec![0.0_f32; profile.dimension];

    let store = GraphStore::new(&db_root).unwrap();
    store
        .bulk_insert_files(&[(
            "/tmp/embed_model_reset.ex".to_string(),
            "proj".to_string(),
            100,
            1,
        )])
        .unwrap();
    store
        .execute("UPDATE File SET status = 'indexed', file_stage = 'graph_indexed', graph_ready = TRUE, vector_ready = TRUE WHERE path = '/tmp/embed_model_reset.ex'")
        .unwrap();
    store
        .execute("INSERT INTO Symbol (id, name, kind, project_slug) VALUES ('sym-embed-model-reset', 'embed_model_reset', 'function', 'proj')")
        .unwrap();
    store
        .execute(&format!(
            "UPDATE Symbol SET embedding = {} WHERE id = 'sym-embed-model-reset'",
            embedding_cast_sql(&vector, profile.dimension)
        ))
        .unwrap();
    store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_slug, kind, content, content_hash, start_line, end_line) VALUES ('chunk-embed-model-reset', 'symbol', 'sym-embed-model-reset', 'proj', 'function', 'content', 'hash-1', 1, 1)")
        .unwrap();
    store
        .execute(&format!(
            "INSERT INTO EmbeddingModel (id, kind, model_name, dimension, version, created_at) VALUES ('model-embed-model-reset', 'chunk', '{}', {}, '1', 1)",
            profile.model_name,
            profile.dimension
        ))
        .unwrap();
    store
        .execute(&format!(
            "INSERT INTO ChunkEmbedding (chunk_id, model_id, embedding, source_hash) VALUES ('chunk-embed-model-reset', 'model-embed-model-reset', {}, 'hash-1')",
            embedding_cast_sql(&vector, profile.dimension)
        ))
        .unwrap();
    store
        .execute(&format!(
            "INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES ('file', '/tmp/embed_model_reset.ex', 2, '{}', 'sig-1', '1', {}, 1)",
            profile.graph.model_id,
            embedding_cast_sql(&vector, profile.dimension)
        ))
        .unwrap();
    store
        .execute("INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES ('/tmp/embed_model_reset.ex', 'queued', 1)")
        .unwrap();
    let file_state_before = store
        .query_json(
            "SELECT graph_ready, vector_ready FROM File WHERE path = '/tmp/embed_model_reset.ex'",
        )
        .unwrap();
    assert!(
        file_state_before.contains("true"),
        "precondition invalide, graph_ready devrait etre vrai avant reopen: {file_state_before}"
    );
    assert!(
        file_state_before.matches("true").count() >= 2,
        "precondition invalide, vector_ready devrait etre vrai avant reopen: {file_state_before}"
    );
    store.execute("DELETE FROM RuntimeMetadata;").unwrap();
    store
        .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('schema_version', '3')")
        .unwrap();
    store
        .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('ingestion_version', '3')")
        .unwrap();
    store
        .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('embedding_version', '2')")
        .unwrap();
    store
        .execute(&format!(
            "INSERT INTO RuntimeMetadata (key, value) VALUES ('embedding_dimension', '{}')",
            profile.dimension
        ))
        .unwrap();
    store
        .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('embedding_model_name', 'legacy-model')")
        .unwrap();
    drop(store);

    let reopened = GraphStore::new(&db_root).unwrap();
    let file_state = reopened
        .query_json(
            "SELECT graph_ready, vector_ready FROM File WHERE path = '/tmp/embed_model_reset.ex'",
        )
        .unwrap();

    assert_eq!(
        reopened
            .query_count("SELECT count(*) FROM File WHERE path = '/tmp/embed_model_reset.ex'")
            .unwrap(),
        1
    );
    assert!(
        file_state.contains("false"),
        "vector_ready doit etre remis a false apres invalidation, etat observe: {file_state}"
    );
    assert!(
        file_state.contains("true"),
        "graph_ready doit rester vrai apres invalidation, etat observe: {file_state}"
    );
    assert_eq!(
        reopened
            .query_count("SELECT count(*) FROM Symbol WHERE id = 'sym-embed-model-reset'")
            .unwrap(),
        1
    );
    assert_eq!(
        reopened
            .query_count("SELECT count(*) FROM Symbol WHERE id = 'sym-embed-model-reset' AND embedding IS NULL")
            .unwrap(),
        1
    );
    assert_eq!(
        reopened
            .query_count("SELECT count(*) FROM Chunk WHERE id = 'chunk-embed-model-reset'")
            .unwrap(),
        1
    );
    assert_eq!(reopened.query_count("SELECT count(*) FROM ChunkEmbedding").unwrap(), 0);
    assert_eq!(reopened.query_count("SELECT count(*) FROM GraphEmbedding").unwrap(), 0);
    assert_eq!(reopened.query_count("SELECT count(*) FROM EmbeddingModel").unwrap(), 0);
    assert_eq!(
        reopened
            .query_count("SELECT count(*) FROM FileVectorizationQueue")
            .unwrap(),
        0
    );
    assert_eq!(
        reopened
            .query_count(&format!(
                "SELECT count(*) FROM RuntimeMetadata WHERE key = 'embedding_model_name' AND value = '{}'",
                profile.model_name
            ))
            .unwrap(),
        1
    );
}

#[test]
fn test_embedding_storage_drift_retypes_physical_columns() {
    let temp = tempfile::tempdir().unwrap();
    let profile = default_embedding_profile();
    let db_root = temp.path().to_str().unwrap().to_string();
    let expected_type = embedding_column_type_sql(profile.dimension);

    let store = GraphStore::new(&db_root).unwrap();
    store
        .execute("ALTER TABLE Symbol ALTER COLUMN embedding TYPE FLOAT[16]")
        .unwrap();
    store
        .execute("DROP TABLE ChunkEmbedding")
        .unwrap();
    store
        .execute(
            "CREATE TABLE ChunkEmbedding (chunk_id VARCHAR, model_id VARCHAR, embedding FLOAT[16], source_hash VARCHAR)",
        )
        .unwrap();
    store
        .execute("DROP INDEX graph_embedding_anchor_model_idx")
        .unwrap();
    store
        .execute("DROP TABLE GraphEmbedding")
        .unwrap();
    store
        .execute(
            "CREATE TABLE GraphEmbedding (anchor_type VARCHAR, anchor_id VARCHAR, radius BIGINT, model_id VARCHAR, source_signature VARCHAR, projection_version VARCHAR, embedding FLOAT[16], updated_at BIGINT)",
        )
        .unwrap();
    store
        .execute("CREATE UNIQUE INDEX graph_embedding_anchor_model_idx ON GraphEmbedding(anchor_type, anchor_id, radius, model_id)")
        .unwrap();
    store.execute("DELETE FROM RuntimeMetadata;").unwrap();
    store
        .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('schema_version', '3')")
        .unwrap();
    store
        .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('ingestion_version', '3')")
        .unwrap();
    store
        .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('embedding_version', '2')")
        .unwrap();
    store
        .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('embedding_dimension', '16')")
        .unwrap();
    store
        .execute(&format!(
            "INSERT INTO RuntimeMetadata (key, value) VALUES ('embedding_model_name', '{}')",
            profile.model_name
        ))
        .unwrap();
    drop(store);

    let reopened = GraphStore::new(&db_root).unwrap();

    assert_eq!(table_column_type(&reopened, "Symbol", "embedding"), expected_type);
    assert_eq!(
        table_column_type(&reopened, "ChunkEmbedding", "embedding"),
        expected_type
    );
    assert_eq!(
        table_column_type(&reopened, "GraphEmbedding", "embedding"),
        expected_type
    );
}
