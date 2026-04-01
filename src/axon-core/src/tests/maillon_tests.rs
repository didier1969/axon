use std::sync::Arc;
use crate::graph::GraphStore;
use crate::queue::QueueStore;
use crate::worker::DbWriteTask;
use crate::parser;
use crate::parser::elixir::ElixirParser;
use crate::parser::Parser;

#[cfg(test)]
mod tests {
    use super::*;

    // --- MAILLON 1: LE SCANNER (Discovery) ---
    #[test]
    fn test_maillon_1_scanner_discovery() {
        let store = GraphStore::new(":memory:").unwrap();
        // Simuler un scan manuel
        let files = vec![("/tmp/test.rs".to_string(), "proj".to_string(), 100, 12345)];
        store.bulk_insert_files(&files).expect("Maillon 1 failed");
        
        let count = store.query_count("SELECT count(*) FROM File WHERE status = 'pending'").unwrap();
        assert_eq!(count, 1, "Le scanner doit insérer les fichiers en status 'pending'");
    }

    #[test]
    fn test_maillon_1b_scanner_respects_hierarchical_axonignore() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        std::fs::write(root.join(".axonignore"), "ignored/\n*.md\n!progress.md\n").unwrap();
        std::fs::create_dir_all(root.join("ignored")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join("docs/open")).unwrap();

        std::fs::write(root.join("kept.rs"), "fn kept() {}").unwrap();
        std::fs::write(root.join("progress.md"), "keep me").unwrap();
        std::fs::write(root.join("ignored").join("lost.rs"), "fn lost() {}").unwrap();
        std::fs::write(root.join("docs").join(".axonignore"), "*.md\n!open/keep.md\n").unwrap();
        std::fs::write(root.join("docs").join("drop.md"), "# hidden").unwrap();
        std::fs::write(root.join("docs").join("open").join("keep.md"), "# visible").unwrap();

        let store = Arc::new(GraphStore::new(":memory:").unwrap());
        let scanner = crate::scanner::Scanner::new(&root.to_string_lossy());
        scanner.scan(store.clone());

        let files = store
            .query_json("SELECT path FROM File ORDER BY path")
            .unwrap();

        assert!(files.contains("kept.rs"), "Le scanner doit garder les fichiers autorisés");
        assert!(files.contains("progress.md"), "Une ré-inclusion !pattern doit être respectée");
        assert!(files.contains("keep.md"), "Une ré-ouverture locale doit être respectée");
        assert!(!files.contains("lost.rs"), "Un répertoire ignoré par Axon Ignore ne doit pas être indexé");
        assert!(!files.contains("drop.md"), "Une règle locale .axonignore doit exclure le fichier");
    }

    // --- MAILLON 2: LE SÉLECTEUR (The Pull) ---
    #[test]
    fn test_maillon_2_selector_pull() {
        let store = GraphStore::new(":memory:").unwrap();
        store.bulk_insert_files(&[("/tmp/a.rs".to_string(), "p".to_string(), 10, 1)]).unwrap();
        
        let batch = store.fetch_pending_batch(10).expect("Maillon 2 failed");
        assert_eq!(batch.len(), 1, "Le sélecteur doit être capable de tirer les fichiers pending");
    }

    #[test]
    fn test_maillon_2b_rescan_requeues_changed_file() {
        let store = GraphStore::new(":memory:").unwrap();
        store.bulk_insert_files(&[("/tmp/a.rs".to_string(), "p".to_string(), 10, 1)]).unwrap();
        let _ = store.fetch_pending_batch(10).unwrap();
        store.execute("UPDATE File SET status = 'indexed', worker_id = NULL WHERE path = '/tmp/a.rs'").unwrap();

        store.bulk_insert_files(&[("/tmp/a.rs".to_string(), "p".to_string(), 20, 2)]).unwrap();

        let status = store.query_json("SELECT status, size, mtime FROM File WHERE path = '/tmp/a.rs'").unwrap();
        assert!(status.contains("pending"), "Un fichier modifié doit être remis en pending");
        assert!(status.contains("20"), "La taille doit être mise à jour");
        assert!(status.contains("2"), "Le mtime doit être mis à jour");
    }

    #[test]
    fn test_maillon_2c_reader_writer_consistency_after_bulk_insert_and_reopen() {
        let db_root = std::env::temp_dir().join(format!(
            "axon-reader-writer-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&db_root);
        std::fs::create_dir_all(&db_root).unwrap();

        let db_root_str = db_root.to_string_lossy().to_string();
        let store = GraphStore::new(&db_root_str).unwrap();

        store
            .bulk_insert_files(&[("/tmp/reader_writer.ex".to_string(), "proj".to_string(), 100, 12345)])
            .unwrap();

        let pending = store.fetch_pending_batch(10).unwrap();
        assert_eq!(pending.len(), 1, "Le writer doit voir immédiatement le fichier pending");

        let visible_now = store
            .query_count("SELECT count(*) FROM File WHERE path = '/tmp/reader_writer.ex'")
            .unwrap();
        assert_eq!(visible_now, 1, "Le reader doit voir immédiatement l'écriture");

        drop(store);

        let reopened = GraphStore::new(&db_root_str).unwrap();
        let visible_after_restart = reopened
            .query_count("SELECT count(*) FROM File WHERE path = '/tmp/reader_writer.ex'")
            .unwrap();
        assert_eq!(visible_after_restart, 1, "La donnée doit survivre au redémarrage");

        let _ = std::fs::remove_dir_all(&db_root);
    }

    #[test]
    fn test_maillon_2c_legacy_ist_reopen_adds_needs_reindex_column() {
        let db_root = std::env::temp_dir().join(format!(
            "axon-legacy-ist-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&db_root);
        std::fs::create_dir_all(&db_root).unwrap();

        let db_root_str = db_root.to_string_lossy().to_string();
        let store = GraphStore::new(&db_root_str).unwrap();

        store
            .execute("DROP TABLE File;")
            .unwrap();
        store
            .execute(
                "CREATE TABLE File (path VARCHAR PRIMARY KEY, project_slug VARCHAR, status VARCHAR, size BIGINT, priority BIGINT, mtime BIGINT, worker_id BIGINT, trace_id VARCHAR)"
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_slug, status, size, priority, mtime, worker_id, trace_id) VALUES ('/tmp/legacy_reopen.ex', 'proj', 'indexed', 100, 1, 1, NULL, 'trace-legacy')"
            )
            .unwrap();
        store
            .execute("DELETE FROM RuntimeMetadata;")
            .unwrap();
        store
            .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('schema_version', '1')")
            .unwrap();
        store
            .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('ingestion_version', '3')")
            .unwrap();
        store
            .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('embedding_version', '1')")
            .unwrap();
        drop(store);

        let reopened = GraphStore::new(&db_root_str).unwrap();

        let row = reopened
            .query_json("SELECT status, needs_reindex FROM File WHERE path = '/tmp/legacy_reopen.ex'")
            .unwrap();
        assert!(row.contains("indexed"));
        assert!(row.contains("false"), "La colonne needs_reindex doit etre disponible apres reopen");

        let _ = std::fs::remove_dir_all(&db_root);
    }

    #[test]
    fn test_maillon_2c_embedding_version_drift_resets_only_embedding_layers() {
        let db_root = std::env::temp_dir().join(format!(
            "axon-embedding-soft-reset-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&db_root);
        std::fs::create_dir_all(&db_root).unwrap();

        let db_root_str = db_root.to_string_lossy().to_string();
        let store = GraphStore::new(&db_root_str).unwrap();

        store
            .bulk_insert_files(&[("/tmp/embed_reset.ex".to_string(), "proj".to_string(), 100, 1)])
            .unwrap();
        store
            .execute("INSERT INTO Symbol (id, name, kind, project_slug) VALUES ('sym-embed-reset', 'embed_reset', 'function', 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO Chunk (id, source_type, source_id, project_slug, kind, content, content_hash, start_line, end_line) VALUES ('chunk-embed-reset', 'symbol', 'sym-embed-reset', 'proj', 'function', 'content', 'hash-1', 1, 1)")
            .unwrap();
        store
            .execute("INSERT INTO EmbeddingModel (id, kind, model_name, dimension, version, created_at) VALUES ('model-embed-reset', 'chunk', 'bge-small-en-v1.5', 384, '0', 1)")
            .unwrap();
        store
            .execute("INSERT INTO ChunkEmbedding (chunk_id, model_id, source_hash) VALUES ('chunk-embed-reset', 'model-embed-reset', 'hash-1')")
            .unwrap();
        store
            .execute("DELETE FROM RuntimeMetadata;")
            .unwrap();
        store
            .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('schema_version', '2')")
            .unwrap();
        store
            .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('ingestion_version', '3')")
            .unwrap();
        store
            .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('embedding_version', '0')")
            .unwrap();
        drop(store);

        let reopened = GraphStore::new(&db_root_str).unwrap();

        assert_eq!(
            reopened
                .query_count("SELECT count(*) FROM File WHERE path = '/tmp/embed_reset.ex'")
                .unwrap(),
            1,
            "Le drift embedding ne doit pas purger File"
        );
        assert_eq!(
            reopened
                .query_count("SELECT count(*) FROM Symbol WHERE id = 'sym-embed-reset'")
                .unwrap(),
            1,
            "Le drift embedding ne doit pas purger Symbol"
        );
        assert_eq!(
            reopened
                .query_count("SELECT count(*) FROM Chunk WHERE id = 'chunk-embed-reset'")
                .unwrap(),
            1,
            "Le drift embedding ne doit pas purger Chunk"
        );
        assert_eq!(
            reopened.query_count("SELECT count(*) FROM ChunkEmbedding").unwrap(),
            0,
            "Le drift embedding doit purger uniquement ChunkEmbedding"
        );

        let _ = std::fs::remove_dir_all(&db_root);
    }

    #[test]
    fn test_maillon_2c_ingestion_version_drift_preserves_file_rows_and_requeues_them() {
        let db_root = std::env::temp_dir().join(format!(
            "axon-ingestion-soft-reset-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&db_root);
        std::fs::create_dir_all(&db_root).unwrap();

        let db_root_str = db_root.to_string_lossy().to_string();
        let store = GraphStore::new(&db_root_str).unwrap();

        store
            .bulk_insert_files(&[("/tmp/ingestion_reset.ex".to_string(), "proj".to_string(), 100, 1)])
            .unwrap();
        store
            .execute("UPDATE File SET status = 'indexed' WHERE path = '/tmp/ingestion_reset.ex'")
            .unwrap();
        store
            .execute("INSERT INTO Symbol (id, name, kind, project_slug) VALUES ('sym-ingestion-reset', 'ingestion_reset', 'function', 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('/tmp/ingestion_reset.ex', 'sym-ingestion-reset')")
            .unwrap();
        store
            .execute("DELETE FROM RuntimeMetadata;")
            .unwrap();
        store
            .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('schema_version', '2')")
            .unwrap();
        store
            .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('ingestion_version', '2')")
            .unwrap();
        store
            .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('embedding_version', '1')")
            .unwrap();
        drop(store);

        let reopened = GraphStore::new(&db_root_str).unwrap();

        assert_eq!(
            reopened
                .query_count("SELECT count(*) FROM File WHERE path = '/tmp/ingestion_reset.ex'")
                .unwrap(),
            1,
            "Le drift ingestion ne doit pas purger File"
        );
        let file_row = reopened
            .query_json("SELECT status FROM File WHERE path = '/tmp/ingestion_reset.ex'")
            .unwrap();
        assert!(
            file_row.contains("pending"),
            "Le drift ingestion doit remettre les fichiers en pending pour replay"
        );
        assert_eq!(
            reopened
                .query_count("SELECT count(*) FROM Symbol WHERE id = 'sym-ingestion-reset'")
                .unwrap(),
            0,
            "Le drift ingestion doit purger les dérivés structurels"
        );
        assert_eq!(
            reopened.query_count("SELECT count(*) FROM CONTAINS").unwrap(),
            0,
            "Le drift ingestion doit purger les relations dérivées"
        );

        let _ = std::fs::remove_dir_all(&db_root);
    }

    #[test]
    fn test_maillon_2c_incompatible_file_schema_triggers_hard_rebuild() {
        let db_root = std::env::temp_dir().join(format!(
            "axon-hard-rebuild-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&db_root);
        std::fs::create_dir_all(&db_root).unwrap();

        let db_root_str = db_root.to_string_lossy().to_string();
        let store = GraphStore::new(&db_root_str).unwrap();

        store.execute("DROP TABLE File;").unwrap();
        store
            .execute(
                "CREATE TABLE File (path VARCHAR PRIMARY KEY, project_slug VARCHAR, priority BIGINT)"
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_slug, priority) VALUES ('/tmp/hard_reset.ex', 'proj', 1)"
            )
            .unwrap();
        store.execute("DELETE FROM RuntimeMetadata;").unwrap();
        store
            .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('schema_version', '1')")
            .unwrap();
        store
            .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('ingestion_version', '3')")
            .unwrap();
        store
            .execute("INSERT INTO RuntimeMetadata (key, value) VALUES ('embedding_version', '1')")
            .unwrap();
        drop(store);

        let reopened = GraphStore::new(&db_root_str).unwrap();

        assert_eq!(
            reopened
                .query_count("SELECT count(*) FROM File WHERE path = '/tmp/hard_reset.ex'")
                .unwrap(),
            0,
            "Un schéma File incompatible doit déclencher un rebuild dur de IST"
        );
        assert_eq!(
            reopened
                .query_count("SELECT count(*) FROM RuntimeMetadata WHERE key = 'schema_version' AND value = '2'")
                .unwrap(),
            1,
            "Le metadata runtime doit être réaligné après rebuild"
        );

        let _ = std::fs::remove_dir_all(&db_root);
    }

    #[test]
    fn test_maillon_2d_rust_watcher_requeues_hot_delta() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("live.ex");
        std::fs::write(&file_path, "defmodule Live do\nend\n").unwrap();

        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[(
                file_path.to_string_lossy().to_string(),
                "proj".to_string(),
                10,
                1,
            )])
            .unwrap();
        store
            .execute(&format!(
                "UPDATE File SET status = 'indexed', priority = 10 WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        let staged = crate::fs_watcher::stage_hot_delta(
            &store,
            root,
            &file_path,
            crate::fs_watcher::HOT_PRIORITY,
        )
        .unwrap();

        assert!(staged, "Le watcher Rust doit ré-enqueuer un delta valide");

        let row = store
            .query_json(&format!(
                "SELECT status, priority, project_slug FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        assert!(row.contains("pending"), "Le delta doit remettre le fichier en pending");
        assert!(row.contains("900"), "Le delta chaud doit imposer une priorité élevée");
        assert!(row.contains("proj"), "Le slug projet doit être conservé");
    }

    #[test]
    fn test_maillon_2e_rust_watcher_respects_axonignore_for_delta() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        let ignored = project.join("ignored");
        std::fs::create_dir_all(&ignored).unwrap();
        std::fs::write(project.join(".axonignore"), "ignored/\n").unwrap();
        let file_path = ignored.join("skip.ex");
        std::fs::write(&file_path, "defmodule Skip do\nend\n").unwrap();

        let store = GraphStore::new(":memory:").unwrap();
        let staged = crate::fs_watcher::stage_hot_delta(
            &store,
            root,
            &file_path,
            crate::fs_watcher::HOT_PRIORITY,
        )
        .unwrap();

        assert!(!staged, "Un chemin ignoré par Axon Ignore ne doit pas être staged");

        let count = store
            .query_count(&format!(
                "SELECT count(*) FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert_eq!(count, 0, "Le fichier ignoré ne doit pas apparaître dans IST");
    }

    #[test]
    fn test_maillon_2f_rust_watcher_ignores_missing_delta_without_failing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let missing = root.join("proj").join("gone.ex");
        let store = GraphStore::new(":memory:").unwrap();

        let staged = crate::fs_watcher::stage_hot_delta(
            &store,
            root,
            &missing,
            crate::fs_watcher::HOT_PRIORITY,
        )
        .unwrap();

        assert!(!staged, "Un delta manquant doit etre ignore sans erreur");

        let count = store
            .query_count(&format!(
                "SELECT count(*) FROM File WHERE path = '{}'",
                missing.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert_eq!(count, 0, "Un fichier manquant ne doit pas etre staged");
    }

    #[test]
    fn test_maillon_2g_rust_watcher_deduplicates_burst_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("burst.ex");
        std::fs::write(&file_path, "defmodule Burst do\nend\n").unwrap();

        let store = GraphStore::new(":memory:").unwrap();

        let staged = crate::fs_watcher::stage_hot_deltas(
            &store,
            root,
            vec![file_path.clone(), file_path.clone(), file_path.clone()],
            crate::fs_watcher::HOT_PRIORITY,
        )
        .unwrap();

        assert_eq!(staged, 1, "Une rafale d'evenements identiques ne doit stager qu'une fois");

        let count = store
            .query_count(&format!(
                "SELECT count(*) FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert_eq!(count, 1, "Le fichier ne doit pas etre duplique dans IST");
    }

    #[test]
    fn test_maillon_2h_rust_watcher_directory_event_stages_nested_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        let nested = project.join("tmp");
        std::fs::create_dir_all(&nested).unwrap();
        let file_path = nested.join("dir_event.ex");
        std::fs::write(&file_path, "defmodule DirEvent do\nend\n").unwrap();

        let store = GraphStore::new(":memory:").unwrap();
        let staged = crate::fs_watcher::stage_hot_delta(
            &store,
            root,
            &nested,
            crate::fs_watcher::HOT_PRIORITY,
        )
        .unwrap();

        assert!(staged, "Un evenement de repertoire doit pouvoir remonter un fichier imbrique");

        let row = store
            .query_json(&format!(
                "SELECT path, status, priority FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        assert!(row.contains("dir_event.ex"));
        assert!(row.contains("pending"));
        assert!(row.contains("900"));
    }

    #[test]
    fn test_maillon_2i_hot_delta_does_not_reopen_file_already_indexing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("live_reopen.ex");
        std::fs::write(&file_path, "defmodule LiveReopen do\nend\n").unwrap();

        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[(
                file_path.to_string_lossy().to_string(),
                "proj".to_string(),
                10,
                1,
            )])
            .unwrap();

        let first_batch = store.fetch_pending_batch(10).unwrap();
        assert_eq!(first_batch.len(), 1, "Le premier claim doit prendre le fichier");

        store
            .upsert_hot_file(
                &file_path.to_string_lossy(),
                "proj",
                10,
                1,
                crate::fs_watcher::HOT_PRIORITY,
            )
            .unwrap();

        let second_batch = store.fetch_pending_batch(10).unwrap();
        assert!(
            second_batch.is_empty(),
            "Un hot delta ne doit pas re-ouvrir un fichier deja indexing sans changement reel"
        );

        let row = store
            .query_json(&format!(
                "SELECT status, worker_id FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        assert!(row.contains("indexing"), "Le fichier doit rester en cours d'indexation");
        assert!(!row.contains("null"), "Le worker actif doit rester attache au fichier");
    }

    #[test]
    fn test_maillon_2j_hot_delta_changed_during_indexing_requeues_after_commit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("live_changed.ex");
        std::fs::write(&file_path, "defmodule LiveChanged do\nend\n").unwrap();

        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[(
                file_path.to_string_lossy().to_string(),
                "proj".to_string(),
                10,
                1,
            )])
            .unwrap();

        let first_batch = store.fetch_pending_batch(10).unwrap();
        assert_eq!(first_batch.len(), 1, "Le premier claim doit prendre le fichier");

        store
            .upsert_hot_file(
                &file_path.to_string_lossy(),
                "proj",
                20,
                2,
                crate::fs_watcher::HOT_PRIORITY,
            )
            .unwrap();

        let second_batch = store.fetch_pending_batch(10).unwrap();
        assert!(
            second_batch.is_empty(),
            "Un changement reel pendant indexing ne doit pas dupliquer le claim immediatement"
        );

        let extraction = parser::ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![parser::Symbol {
                name: "live_changed".to_string(),
                kind: "func".to_string(),
                start_line: 1,
                end_line: 1,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: std::collections::HashMap::new(),
                embedding: None,
            }],
            relations: vec![],
        };

        store
            .insert_file_data_batch(&[DbWriteTask::FileExtraction {
                path: file_path.to_string_lossy().to_string(),
                content: "defmodule LiveChanged do\nend\n".to_string(),
                extraction,
                trace_id: "trace".to_string(),
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        let row = store
            .query_json(&format!(
                "SELECT status, priority FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        assert!(
            row.contains("pending"),
            "Le fichier doit etre replanifie apres le commit si un vrai changement est arrive pendant indexing"
        );
        assert!(row.contains("900"), "La priorite chaude doit etre preservee pour la seconde passe");
    }

    #[test]
    fn test_maillon_2k_rust_watcher_tombstones_deleted_file_and_purges_truth() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("deleted_live.ex");
        std::fs::write(&file_path, "defmodule DeletedLive do\nend\n").unwrap();

        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[(
                file_path.to_string_lossy().to_string(),
                "proj".to_string(),
                10,
                1,
            )])
            .unwrap();

        let extraction = parser::ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![parser::Symbol {
                name: "deleted_live".to_string(),
                kind: "func".to_string(),
                start_line: 1,
                end_line: 1,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: std::collections::HashMap::new(),
                embedding: None,
            }],
            relations: vec![],
        };

        store
            .insert_file_data_batch(&[DbWriteTask::FileExtraction {
                path: file_path.to_string_lossy().to_string(),
                content: "defmodule DeletedLive do\nend\n".to_string(),
                extraction,
                trace_id: "trace".to_string(),
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        std::fs::remove_file(&file_path).unwrap();

        let staged = crate::fs_watcher::stage_hot_delta(
            &store,
            root,
            &file_path,
            crate::fs_watcher::HOT_PRIORITY,
        )
        .unwrap();

        assert!(staged, "Une suppression doit modifier IST via un tombstone");

        let row = store
            .query_json(&format!(
                "SELECT status, worker_id FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert!(row.contains("deleted"), "Le fichier supprimé doit être tombstoné");
        assert!(row.contains("null"), "Le worker doit être libéré après tombstone");

        let contains_count = store
            .query_count(&format!(
                "SELECT count(*) FROM CONTAINS WHERE source_id = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert_eq!(contains_count, 0, "Le lien CONTAINS du fichier supprimé doit disparaître");

        let symbol_count = store.query_count("SELECT count(*) FROM Symbol").unwrap();
        assert_eq!(symbol_count, 0, "Les symboles du fichier supprimé doivent disparaître");
    }

    #[test]
    fn test_maillon_2l_rust_watcher_rename_tombstones_old_path_and_stages_new_one() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let old_path = project.join("rename_old.ex");
        let new_path = project.join("rename_new.ex");
        std::fs::write(&old_path, "defmodule RenameOld do\nend\n").unwrap();

        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[(
                old_path.to_string_lossy().to_string(),
                "proj".to_string(),
                10,
                1,
            )])
            .unwrap();

        let extraction = parser::ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![parser::Symbol {
                name: "rename_old".to_string(),
                kind: "func".to_string(),
                start_line: 1,
                end_line: 1,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: std::collections::HashMap::new(),
                embedding: None,
            }],
            relations: vec![],
        };

        store
            .insert_file_data_batch(&[DbWriteTask::FileExtraction {
                path: old_path.to_string_lossy().to_string(),
                content: "defmodule RenameOld do\nend\n".to_string(),
                extraction,
                trace_id: "trace".to_string(),
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        std::fs::rename(&old_path, &new_path).unwrap();

        let staged = crate::fs_watcher::stage_hot_deltas(
            &store,
            root,
            vec![old_path.clone(), new_path.clone()],
            crate::fs_watcher::HOT_PRIORITY,
        )
        .unwrap();

        assert_eq!(staged, 2, "Un rename doit tombstoner l'ancien chemin et stager le nouveau");

        let old_row = store
            .query_json(&format!(
                "SELECT status FROM File WHERE path = '{}'",
                old_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert!(old_row.contains("deleted"), "L'ancien chemin doit être tombstoné");

        let new_row = store
            .query_json(&format!(
                "SELECT status, priority FROM File WHERE path = '{}'",
                new_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert!(new_row.contains("pending"), "Le nouveau chemin doit être staged en pending");
        assert!(new_row.contains("900"), "Le nouveau chemin doit garder la priorité chaude");

        let old_contains_count = store
            .query_count(&format!(
                "SELECT count(*) FROM CONTAINS WHERE source_id = '{}'",
                old_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert_eq!(old_contains_count, 0, "L'ancien chemin ne doit pas garder de vérité dérivée");
    }

    #[test]
    fn test_maillon_2m_reopen_requeues_interrupted_indexing_after_crash() {
        let db_root = std::env::temp_dir().join(format!(
            "axon-crash-replay-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&db_root);
        std::fs::create_dir_all(&db_root).unwrap();

        let db_root_str = db_root.to_string_lossy().to_string();
        let file_path = db_root.join("proj").join("crash_replay.ex");
        std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        std::fs::write(&file_path, "defmodule CrashReplay do\nend\n").unwrap();

        let store = GraphStore::new(&db_root_str).unwrap();
        store
            .bulk_insert_files(&[(
                file_path.to_string_lossy().to_string(),
                "proj".to_string(),
                10,
                1,
            )])
            .unwrap();

        let claimed = store.fetch_pending_batch(10).unwrap();
        assert_eq!(claimed.len(), 1, "Le fichier doit d'abord être pris par un claim actif");

        let indexing_row = store
            .query_json(&format!(
                "SELECT status, worker_id FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert!(indexing_row.contains("indexing"));
        assert!(!indexing_row.contains("null"));

        drop(store);

        let reopened = GraphStore::new(&db_root_str).unwrap();
        let replay_row = reopened
            .query_json(&format!(
                "SELECT status, worker_id FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert!(
            replay_row.contains("pending"),
            "Un fichier resté indexing après crash doit être rejoué au redémarrage"
        );
        assert!(replay_row.contains("null"), "Le worker orphelin doit être libéré au redémarrage");

        let replay_batch = reopened.fetch_pending_batch(10).unwrap();
        assert_eq!(replay_batch.len(), 1, "Le fichier doit redevenir claimable après redémarrage");

        let _ = std::fs::remove_dir_all(&db_root);
    }

    #[test]
    fn test_maillon_2o_oversized_file_status_is_explicit_and_reversible_on_new_scan() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/oversized_file.rs".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 10_000, 1)])
            .unwrap();

        store
            .mark_file_oversized_for_current_budget(&path)
            .unwrap();

        let oversized_row = store
            .query_json("SELECT status FROM File WHERE path = '/tmp/oversized_file.rs'")
            .unwrap();
        assert!(
            oversized_row.contains("oversized_for_current_budget"),
            "an oversized file must keep an explicit status instead of a generic skip"
        );

        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 10_000, 2)])
            .unwrap();

        let replay_row = store
            .query_json("SELECT status, mtime FROM File WHERE path = '/tmp/oversized_file.rs'")
            .unwrap();
        assert!(replay_row.contains("pending"));
        assert!(replay_row.contains("2"));
    }

    #[test]
    fn test_maillon_2p_deferred_pending_file_builds_aging_debt_and_claim_reset() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/deferred_file.rs".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 4_096, 1)])
            .unwrap();

        store
            .mark_pending_files_deferred(std::slice::from_ref(&path))
            .unwrap();
        store
            .mark_pending_files_deferred(std::slice::from_ref(&path))
            .unwrap();

        let deferred_row = store
            .query_json("SELECT defer_count, last_deferred_at_ms FROM File WHERE path = '/tmp/deferred_file.rs'")
            .unwrap();
        assert!(deferred_row.contains("2"), "Le déferrement doit construire une dette de fairness persistante");
        assert!(!deferred_row.contains("null"), "Le timestamp de dernier déferrement doit être renseigné");

        let claimed = store
            .claim_pending_paths(std::slice::from_ref(&path))
            .unwrap();
        assert_eq!(claimed.len(), 1, "Le fichier différé doit rester claimable");

        let claimed_row = store
            .query_json("SELECT status, defer_count, last_deferred_at_ms FROM File WHERE path = '/tmp/deferred_file.rs'")
            .unwrap();
        assert!(claimed_row.contains("indexing"));
        assert!(claimed_row.contains("0"), "Une claim effective doit remettre à zéro la dette de fairness");
        assert!(claimed_row.contains("null"), "Le timestamp de déferrement doit être purgé après claim");
    }

    #[test]
    fn test_maillon_2n_late_commit_does_not_resurrect_tombstoned_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("late_deleted.ex");
        std::fs::write(&file_path, "defmodule LateDeleted do\nend\n").unwrap();

        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[(
                file_path.to_string_lossy().to_string(),
                "proj".to_string(),
                10,
                1,
            )])
            .unwrap();

        let first_batch = store.fetch_pending_batch(10).unwrap();
        assert_eq!(first_batch.len(), 1, "Le fichier doit d'abord être claimé");

        std::fs::remove_file(&file_path).unwrap();
        let staged = crate::fs_watcher::stage_hot_delta(
            &store,
            root,
            &file_path,
            crate::fs_watcher::HOT_PRIORITY,
        )
        .unwrap();
        assert!(staged, "Le delete doit tombstoner pendant qu'un worker est encore en vol");

        let extraction = parser::ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![parser::Symbol {
                name: "late_deleted".to_string(),
                kind: "func".to_string(),
                start_line: 1,
                end_line: 1,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: std::collections::HashMap::new(),
                embedding: None,
            }],
            relations: vec![],
        };

        store
            .insert_file_data_batch(&[DbWriteTask::FileExtraction {
                path: file_path.to_string_lossy().to_string(),
                content: "defmodule LateDeleted do\nend\n".to_string(),
                extraction,
                trace_id: "trace".to_string(),
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        let row = store
            .query_json(&format!(
                "SELECT status FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert!(row.contains("deleted"), "Un commit tardif ne doit pas ressusciter un tombstone");

        let symbol_count = store.query_count("SELECT count(*) FROM Symbol").unwrap();
        assert_eq!(symbol_count, 0, "Aucune vérité dérivée ne doit réapparaître après tombstone");
    }

    // --- MAILLON 3: LA SOCKET (Le Protocole) ---
    #[tokio::test]
    async fn test_maillon_3_socket_protocol() {
        use tokio::net::{UnixListener, UnixStream};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use std::fs;

        let sock_path = "/tmp/test-maillon-3.sock";
        if std::path::Path::new(sock_path).exists() { let _ = fs::remove_file(sock_path); }
        
        let listener = match UnixListener::bind(sock_path) {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("Skipping socket protocol test in sandboxed environment: {}", err);
                return;
            }
            Err(err) => panic!("Failed to bind unix socket: {}", err),
        };
        let store = Arc::new(GraphStore::new(":memory:").unwrap());
        
        // Simuler un fichier en attente
        store.bulk_insert_files(&[("/tmp/test.ex".to_string(), "proj".to_string(), 10, 1)]).unwrap();

        // Spawn Server Loop (Simulé de main.rs)
        let server_store = store.clone();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = socket.into_split();
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();
            
            // Welcome
            writer.write_all(b"Axon Telemetry Ready\n{\"SystemReady\":{}}\n").await.unwrap();

            if let Ok(_) = buf_reader.read_line(&mut line).await {
                let command = line.trim();
                if command.starts_with("PULL_PENDING ") {
                    let count = command[13..].parse::<usize>().unwrap_or(1);
                    let files = server_store.fetch_pending_batch(count).unwrap();
                    let response = serde_json::json!({"event": "PENDING_BATCH_READY", "files": files});
                    writer.write_all((serde_json::to_string(&response).unwrap() + "\n").as_bytes()).await.unwrap();
                }
            }
        });

        // Client Loop
        let client = UnixStream::connect(sock_path).await.unwrap();
        let mut client_reader = BufReader::new(client);
        let mut response = String::new();
        
        // Skip welcome
        client_reader.read_line(&mut response).await.unwrap(); // Axon Ready
        response.clear();
        client_reader.read_line(&mut response).await.unwrap(); // SystemReady JSON
        response.clear();

        // Send Command
        let mut stream = client_reader.into_inner();
        stream.write_all(b"PULL_PENDING 1\n").await.unwrap();
        
        let mut reader = BufReader::new(stream);
        reader.read_line(&mut response).await.unwrap();
        
        assert!(response.contains("PENDING_BATCH_READY"), "Le serveur doit répondre avec le batch de fichiers");
        assert!(response.contains("/tmp/test.ex"), "Le batch doit contenir le fichier attendu");
        
        let _ = fs::remove_file(sock_path);
    }

    // --- MAILLON 5: LA TRANSFORMATION (AST Parser) ---
    #[test]
    fn test_maillon_5_ast_parser() {
        let content = "defmodule T, do: def h, do: :ok";
        let parser = ElixirParser::new();
        let result = parser.parse(content);
        
        assert!(result.symbols.len() > 0, "Le parser doit extraire au moins un symbole");
        let sym = &result.symbols[0];
        // Test de la rigueur des 9 colonnes
        assert!(sym.is_public, "La métadonnée is_public doit être extraite");
    }

    // --- MAILLON 6: LE BUFFER INTERNE (Hopper) ---
    #[test]
    fn test_maillon_6_hopper_queue() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.rs");
        std::fs::write(&path, "fn test() {}").unwrap();
        let queue = QueueStore::new(10);
        queue.push(path.to_string_lossy().as_ref(), 1, "trace", 0, 0, false).unwrap();
        
        let task = queue.pop().expect("Maillon 6 failed");
        assert_eq!(task.path, path.to_string_lossy(), "La queue doit restituer les tâches dans l'ordre");
    }

    // --- MAILLON 7: LE COMMITTER (Writer Actor) ---
    #[test]
    fn test_maillon_7_writer_commit() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/test.rs".to_string();
        store.bulk_insert_files(&[(path.clone(), "proj".to_string(), 100, 12345)]).unwrap();
        
        let extraction = parser::ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![parser::Symbol {
                name: "test".to_string(), kind: "func".to_string(), start_line: 1, end_line: 1,
                docstring: None, is_entry_point: false, is_public: true, tested: false,
                is_nif: false, is_unsafe: false, properties: std::collections::HashMap::new(),
                embedding: None
            }],
            relations: vec![]
        };
        
        let task = DbWriteTask::FileExtraction {
            path: path.clone(), content: "fn test() {}".to_string(), extraction, trace_id: "t".to_string(), t0: 0, t1: 0, t2: 0, t3: 0
        };
        
        store.insert_file_data_batch(&[task]).expect("Maillon 7 failed");
        
        let status_json = store.query_json("SELECT status FROM File").unwrap();
        assert!(status_json.contains("indexed"), "Le committer doit passer le statut à 'indexed'");

        let chunk_count = store.query_count("SELECT count(*) FROM Chunk").unwrap();
        assert_eq!(chunk_count, 1, "Le committer doit aussi matérialiser un chunk dérivé");
    }

    #[test]
    fn test_maillon_7b_chunk_embedding_storage() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/test.rs".to_string();
        store.bulk_insert_files(&[(path.clone(), "proj".to_string(), 100, 12345)]).unwrap();

        let extraction = parser::ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![parser::Symbol {
                name: "test".to_string(), kind: "func".to_string(), start_line: 1, end_line: 1,
                docstring: None, is_entry_point: false, is_public: true, tested: false,
                is_nif: false, is_unsafe: false, properties: std::collections::HashMap::new(),
                embedding: None
            }],
            relations: vec![]
        };

        let task = DbWriteTask::FileExtraction {
            path: path.clone(), content: "fn test() {}".to_string(), extraction, trace_id: "t".to_string(), t0: 0, t1: 0, t2: 0, t3: 0
        };

        store.insert_file_data_batch(&[task]).expect("Chunk setup failed");
        store.ensure_embedding_model("chunk-bge-small-en-v1.5-384", "chunk", "BAAI/bge-small-en-v1.5", 384, "1").unwrap();

        let pending = store.fetch_unembedded_chunks("chunk-bge-small-en-v1.5-384", 10).unwrap();
        assert_eq!(pending.len(), 1, "Le store doit détecter le chunk non vectorisé");

        let vector = vec![0.0_f32; 384];
        store.update_chunk_embeddings("chunk-bge-small-en-v1.5-384", &[(
            pending[0].0.clone(),
            pending[0].2.clone(),
            vector
        )]).unwrap();

        let stored = store.query_count("SELECT count(*) FROM ChunkEmbedding").unwrap();
        assert_eq!(stored, 1, "Le vector store dérivé doit persister l'embedding du chunk");
    }

    #[test]
    fn test_maillon_7e_chunk_invalidation_requeues_only_changed_file_embeddings() {
        let store = GraphStore::new(":memory:").unwrap();
        let path_a = "/tmp/chunk/a.rs".to_string();
        let path_b = "/tmp/chunk/b.rs".to_string();
        let path_c = "/tmp/other/c.rs".to_string();
        store
            .bulk_insert_files(&[
                (path_a.clone(), "proj".to_string(), 100, 1),
                (path_b.clone(), "proj".to_string(), 100, 1),
                (path_c.clone(), "other".to_string(), 100, 1),
            ])
            .unwrap();

        let extraction_for = |project: &str, name: &str, _body: &str, docstring: Option<&str>| parser::ExtractionResult {
            project_slug: Some(project.to_string()),
            symbols: vec![parser::Symbol {
                name: name.to_string(),
                kind: "function".to_string(),
                start_line: 1,
                end_line: 3,
                docstring: docstring.map(|value| value.to_string()),
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: std::collections::HashMap::new(),
                embedding: None,
            }],
            relations: vec![],
        };

        store
            .insert_file_data_batch(&[
                DbWriteTask::FileExtraction {
                    path: path_a.clone(),
                    content: "fn alpha() {\n    old_body();\n}\n".to_string(),
                    extraction: extraction_for("proj", "alpha", "old_body", None),
                    trace_id: "alpha-1".to_string(),
                    t0: 0,
                    t1: 0,
                    t2: 0,
                    t3: 0,
                },
                DbWriteTask::FileExtraction {
                    path: path_b.clone(),
                    content: "fn beta() {\n    stable_body();\n}\n".to_string(),
                    extraction: extraction_for("proj", "beta", "stable_body", None),
                    trace_id: "beta-1".to_string(),
                    t0: 0,
                    t1: 0,
                    t2: 0,
                    t3: 0,
                },
                DbWriteTask::FileExtraction {
                    path: path_c.clone(),
                    content: "fn gamma() {\n    foreign_project();\n}\n".to_string(),
                    extraction: extraction_for("other", "gamma", "foreign_project", None),
                    trace_id: "gamma-1".to_string(),
                    t0: 0,
                    t1: 0,
                    t2: 0,
                    t3: 0,
                },
            ])
            .unwrap();

        store
            .ensure_embedding_model("chunk-bge-small-en-v1.5-384", "chunk", "BAAI/bge-small-en-v1.5", 384, "1")
            .unwrap();

        let initial_pending = store
            .fetch_unembedded_chunks("chunk-bge-small-en-v1.5-384", 10)
            .unwrap();
        assert_eq!(initial_pending.len(), 3, "Tous les chunks initiaux doivent etre vectorisables");

        let alpha_chunk_id = initial_pending
            .iter()
            .find(|(_, content, _)| content.contains("alpha"))
            .expect("alpha chunk missing")
            .0
            .clone();
        let updates: Vec<(String, String, Vec<f32>)> = initial_pending
            .iter()
            .map(|(id, _, hash)| (id.clone(), hash.clone(), vec![0.0_f32; 384]))
            .collect();
        store
            .update_chunk_embeddings("chunk-bge-small-en-v1.5-384", &updates)
            .unwrap();
        assert_eq!(store.query_count("SELECT count(*) FROM ChunkEmbedding").unwrap(), 3);

        store
            .insert_file_data_batch(&[DbWriteTask::FileExtraction {
                path: path_a.clone(),
                content: "fn alpha() {\n    new_body();\n}\n".to_string(),
                extraction: extraction_for(
                    "proj",
                    "alpha",
                    "new_body",
                    Some("routes the new behavior without replaying all semantic work"),
                ),
                trace_id: "alpha-2".to_string(),
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        assert_eq!(
            store.query_count("SELECT count(*) FROM ChunkEmbedding").unwrap(),
            2,
            "Seul le chunk du fichier change doit perdre son embedding derive"
        );

        let pending = store
            .fetch_unembedded_chunks("chunk-bge-small-en-v1.5-384", 10)
            .unwrap();
        assert_eq!(pending.len(), 1, "Le delta ne doit revectoriser que le chunk modifie");
        assert_eq!(pending[0].0, alpha_chunk_id);
        assert!(pending[0].1.contains("new_body") || pending[0].1.contains("new behavior"));
    }

    #[test]
    fn test_maillon_7f_fetch_unembedded_chunks_detects_source_hash_drift() {
        let store = GraphStore::new(":memory:").unwrap();
        store
            .execute("INSERT INTO Chunk (id, source_type, source_id, project_slug, kind, content, content_hash, start_line, end_line) VALUES ('chunk-drift', 'symbol', 'sym-drift', 'proj', 'function', 'fresh content', 'hash-fresh', 1, 1)")
            .unwrap();
        store
            .ensure_embedding_model("chunk-bge-small-en-v1.5-384", "chunk", "BAAI/bge-small-en-v1.5", 384, "1")
            .unwrap();
        store
            .execute("INSERT INTO ChunkEmbedding (chunk_id, model_id, source_hash) VALUES ('chunk-drift', 'chunk-bge-small-en-v1.5-384', 'hash-stale')")
            .unwrap();

        let pending = store
            .fetch_unembedded_chunks("chunk-bge-small-en-v1.5-384", 10)
            .unwrap();
        assert_eq!(pending.len(), 1, "Un hash derive stale doit etre revectorise");
        assert_eq!(pending[0].0, "chunk-drift");
        assert_eq!(pending[0].2, "hash-fresh");
    }

    #[test]
    fn test_maillon_7c_writer_keeps_distinct_top_level_symbols_from_different_files() {
        let store = GraphStore::new(":memory:").unwrap();
        let path_a = "/tmp/scripts/a.py".to_string();
        let path_b = "/tmp/scripts/b.py".to_string();
        store
            .bulk_insert_files(&[
                (path_a.clone(), "proj".to_string(), 100, 1),
                (path_b.clone(), "proj".to_string(), 100, 1),
            ])
            .unwrap();

        let extraction_a = parser::ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![parser::Symbol {
                name: "send_cypher".to_string(),
                kind: "function".to_string(),
                start_line: 1,
                end_line: 1,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: std::collections::HashMap::new(),
                embedding: None,
            }],
            relations: vec![],
        };

        let extraction_b = parser::ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![parser::Symbol {
                name: "send_cypher".to_string(),
                kind: "function".to_string(),
                start_line: 1,
                end_line: 1,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: std::collections::HashMap::new(),
                embedding: None,
            }],
            relations: vec![],
        };

        store
            .insert_file_data_batch(&[
                DbWriteTask::FileExtraction {
                    path: path_a.clone(),
                    content: "def send_cypher(query):\n    return query\n".to_string(),
                    extraction: extraction_a,
                    trace_id: "a".to_string(),
                    t0: 0,
                    t1: 0,
                    t2: 0,
                    t3: 0,
                },
                DbWriteTask::FileExtraction {
                    path: path_b.clone(),
                    content: "def send_cypher(query):\n    return query\n".to_string(),
                    extraction: extraction_b,
                    trace_id: "b".to_string(),
                    t0: 0,
                    t1: 0,
                    t2: 0,
                    t3: 0,
                },
            ])
            .unwrap();

        let symbols_json = store
            .query_json("SELECT id, name FROM Symbol ORDER BY id")
            .unwrap();
        let symbol_count = store
            .query_count("SELECT count(*) FROM Symbol WHERE name = 'send_cypher'")
            .unwrap();
        assert_eq!(
            symbol_count, 2,
            "Deux fichiers distincts ne doivent pas se partager le meme symbole top-level: {}",
            symbols_json
        );
    }

    #[test]
    fn test_maillon_7d_writer_coalesces_duplicate_symbol_names_inside_same_file() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/status_live.ex".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "axon".to_string(), 100, 1)])
            .unwrap();

        let extraction = parser::ExtractionResult {
            project_slug: Some("axon".to_string()),
            symbols: vec![
                parser::Symbol {
                    name: "AxonDashboardWeb.StatusLive.handle_info".to_string(),
                    kind: "function".to_string(),
                    start_line: 10,
                    end_line: 12,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: std::collections::HashMap::new(),
                    embedding: None,
                },
                parser::Symbol {
                    name: "AxonDashboardWeb.StatusLive.handle_info".to_string(),
                    kind: "function".to_string(),
                    start_line: 30,
                    end_line: 34,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: std::collections::HashMap::new(),
                    embedding: None,
                },
            ],
            relations: vec![],
        };

        store
            .insert_file_data_batch(&[DbWriteTask::FileExtraction {
                path: path.clone(),
                content: "defmodule AxonDashboardWeb.StatusLive do\nend\n".to_string(),
                extraction,
                trace_id: "trace".to_string(),
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        let symbol_count = store
            .query_count(
                "SELECT count(*) FROM Symbol WHERE name = 'AxonDashboardWeb.StatusLive.handle_info'"
            )
            .unwrap();
        assert_eq!(
            symbol_count, 1,
            "Les clauses multiples doivent etre coalescees en un symbole logique"
        );
    }

    #[test]
    fn test_graph_projection_symbol_radius_1_returns_useful_neighborhood() {
        let store = GraphStore::new(":memory:").unwrap();
        store
            .execute("INSERT INTO File (path, project_slug) VALUES ('/tmp/graph/a.rs', 'proj'), ('/tmp/graph/other.rs', 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES \
                ('proj::A', 'A', 'function', true, true, false, false, 'proj'), \
                ('proj::B', 'B', 'function', true, true, false, false, 'proj'), \
                ('proj::C', 'C', 'function', true, true, false, false, 'proj'), \
                ('proj::X', 'X', 'function', true, true, false, false, 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES \
                ('/tmp/graph/a.rs', 'proj::A'), \
                ('/tmp/graph/a.rs', 'proj::B'), \
                ('/tmp/graph/other.rs', 'proj::C'), \
                ('/tmp/graph/other.rs', 'proj::X')")
            .unwrap();
        store
            .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('proj::A', 'proj::B'), ('proj::B', 'proj::C'), ('proj::X', 'proj::C')")
            .unwrap();

        let anchor_id = store
            .refresh_symbol_projection("A", 1)
            .unwrap()
            .expect("anchor should resolve");
        let projection = store
            .query_graph_projection("symbol", &anchor_id, 1)
            .unwrap();

        assert!(projection.contains("proj::A"));
        assert!(projection.contains("proj::B"));
        assert!(!projection.contains("proj::C"));
        assert!(!projection.contains("proj::X"));
        assert!(projection.contains("call-neighborhood"));
    }

    #[test]
    fn test_graph_projection_symbol_radius_2_expands_but_stays_bounded() {
        let store = GraphStore::new(":memory:").unwrap();
        store
            .execute("INSERT INTO File (path, project_slug) VALUES ('/tmp/graph/a.rs', 'proj'), ('/tmp/graph/other.rs', 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES \
                ('proj::A', 'A', 'function', true, true, false, false, 'proj'), \
                ('proj::B', 'B', 'function', true, true, false, false, 'proj'), \
                ('proj::C', 'C', 'function', true, true, false, false, 'proj'), \
                ('proj::X', 'X', 'function', true, true, false, false, 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES \
                ('/tmp/graph/a.rs', 'proj::A'), \
                ('/tmp/graph/a.rs', 'proj::B'), \
                ('/tmp/graph/other.rs', 'proj::C'), \
                ('/tmp/graph/other.rs', 'proj::X')")
            .unwrap();
        store
            .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('proj::A', 'proj::B'), ('proj::B', 'proj::C'), ('proj::X', 'proj::C')")
            .unwrap();

        let anchor_id = store
            .refresh_symbol_projection("A", 2)
            .unwrap()
            .expect("anchor should resolve");
        let projection = store
            .query_graph_projection("symbol", &anchor_id, 2)
            .unwrap();

        assert!(projection.contains("proj::A"));
        assert!(projection.contains("proj::B"));
        assert!(projection.contains("proj::C"));
        assert!(!projection.contains("proj::X"));
    }

    #[test]
    fn test_graph_projection_file_anchor_is_stable_and_idempotent() {
        let store = GraphStore::new(":memory:").unwrap();
        let file_path = "/tmp/graph/file_anchor.rs";
        store
            .execute("INSERT INTO File (path, project_slug) VALUES ('/tmp/graph/file_anchor.rs', 'proj'), ('/tmp/graph/helper.rs', 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES \
                ('proj::FileAlpha', 'FileAlpha', 'function', true, true, false, false, 'proj'), \
                ('proj::FileBeta', 'FileBeta', 'function', true, true, false, false, 'proj'), \
                ('proj::Helper', 'Helper', 'function', true, true, false, false, 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES \
                ('/tmp/graph/file_anchor.rs', 'proj::FileAlpha'), \
                ('/tmp/graph/file_anchor.rs', 'proj::FileBeta'), \
                ('/tmp/graph/helper.rs', 'proj::Helper')")
            .unwrap();
        store
            .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('proj::FileAlpha', 'proj::Helper')")
            .unwrap();

        store.refresh_file_projection(file_path, 2).unwrap();
        let first_count = store
            .query_count("SELECT count(*) FROM GraphProjection WHERE anchor_type = 'file' AND anchor_id = '/tmp/graph/file_anchor.rs' AND radius = 2")
            .unwrap();
        let first_projection = store
            .query_graph_projection("file", file_path, 2)
            .unwrap();

        store.refresh_file_projection(file_path, 2).unwrap();
        let second_count = store
            .query_count("SELECT count(*) FROM GraphProjection WHERE anchor_type = 'file' AND anchor_id = '/tmp/graph/file_anchor.rs' AND radius = 2")
            .unwrap();
        let second_projection = store
            .query_graph_projection("file", file_path, 2)
            .unwrap();

        assert_eq!(first_count, second_count, "La projection dérivée ne doit pas se dupliquer");
        assert_eq!(first_projection, second_projection, "La même ancre et le même rayon doivent rester stables");
        assert!(second_projection.contains("contains"));
        assert!(second_projection.contains("proj::FileAlpha"));
        assert!(second_projection.contains("proj::FileBeta"));
    }

    #[test]
    fn test_graph_projection_refresh_reuses_unchanged_anchor_without_rebuild() {
        let store = GraphStore::new(":memory:").unwrap();
        store
            .execute("INSERT INTO File (path, project_slug) VALUES ('/tmp/graph/a.rs', 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES \
                ('proj::A', 'A', 'function', true, true, false, false, 'proj'), \
                ('proj::B', 'B', 'function', true, true, false, false, 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('/tmp/graph/a.rs', 'proj::A'), ('/tmp/graph/a.rs', 'proj::B')")
            .unwrap();
        store
            .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('proj::A', 'proj::B')")
            .unwrap();

        let anchor_id = store
            .refresh_symbol_projection("A", 1)
            .unwrap()
            .expect("anchor should resolve");
        let first_state = store
            .query_json("SELECT source_signature, updated_at FROM GraphProjectionState WHERE anchor_type = 'symbol' AND anchor_id = 'proj::A' AND radius = 1")
            .unwrap();
        assert_ne!(first_state, "[]", "L'etat de projection doit etre materialise");
        let first_projection_count = store
            .query_count("SELECT count(*) FROM GraphProjection WHERE anchor_type = 'symbol' AND anchor_id = 'proj::A' AND radius = 1")
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(2));
        let second_anchor = store
            .refresh_symbol_projection("A", 1)
            .unwrap()
            .expect("anchor should resolve");
        let second_state = store
            .query_json("SELECT source_signature, updated_at FROM GraphProjectionState WHERE anchor_type = 'symbol' AND anchor_id = 'proj::A' AND radius = 1")
            .unwrap();
        assert_ne!(second_state, "[]", "L'etat de projection doit rester disponible");
        let second_projection_count = store
            .query_count("SELECT count(*) FROM GraphProjection WHERE anchor_type = 'symbol' AND anchor_id = 'proj::A' AND radius = 1")
            .unwrap();

        assert_eq!(anchor_id, second_anchor);
        assert_eq!(first_projection_count, second_projection_count);
        assert_eq!(
            first_state, second_state,
            "Une projection inchangée doit être réutilisée sans réécriture"
        );
    }

    #[test]
    fn test_graph_projection_refresh_rebuilds_only_changed_anchor() {
        let store = GraphStore::new(":memory:").unwrap();
        store
            .execute("INSERT INTO File (path, project_slug) VALUES ('/tmp/graph/a.rs', 'proj'), ('/tmp/graph/d.rs', 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES \
                ('proj::A', 'A', 'function', true, true, false, false, 'proj'), \
                ('proj::B', 'B', 'function', true, true, false, false, 'proj'), \
                ('proj::C', 'C', 'function', true, true, false, false, 'proj'), \
                ('proj::D', 'D', 'function', true, true, false, false, 'proj'), \
                ('proj::E', 'E', 'function', true, true, false, false, 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES \
                ('/tmp/graph/a.rs', 'proj::A'), ('/tmp/graph/a.rs', 'proj::B'), ('/tmp/graph/a.rs', 'proj::C'), \
                ('/tmp/graph/d.rs', 'proj::D'), ('/tmp/graph/d.rs', 'proj::E')")
            .unwrap();
        store
            .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('proj::A', 'proj::B'), ('proj::D', 'proj::E')")
            .unwrap();

        store.refresh_symbol_projection("A", 2).unwrap();
        store.refresh_symbol_projection("D", 2).unwrap();
        let before_d_state = store
            .query_json("SELECT source_signature, updated_at FROM GraphProjectionState WHERE anchor_type = 'symbol' AND anchor_id = 'proj::D' AND radius = 2")
            .unwrap();
        assert_ne!(before_d_state, "[]", "Le voisinage non touche doit avoir un etat materialise");

        std::thread::sleep(std::time::Duration::from_millis(2));
        store
            .execute("DELETE FROM CALLS WHERE source_id = 'proj::A' AND target_id = 'proj::B'")
            .unwrap();
        store
            .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('proj::A', 'proj::C')")
            .unwrap();

        store.refresh_symbol_projection("A", 2).unwrap();

        let projection_a = store
            .query_graph_projection("symbol", "proj::A", 2)
            .unwrap();
        let after_d_state = store
            .query_json("SELECT source_signature, updated_at FROM GraphProjectionState WHERE anchor_type = 'symbol' AND anchor_id = 'proj::D' AND radius = 2")
            .unwrap();
        assert_ne!(after_d_state, "[]", "Le voisinage non touche doit rester materialise");

        assert!(projection_a.contains("proj::C"));
        assert!(!projection_a.contains("proj::B"));
        assert_eq!(
            before_d_state, after_d_state,
            "Le refresh d'une ancre modifiée ne doit pas réécrire les voisinages non touchés"
        );
    }
}
