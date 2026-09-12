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
