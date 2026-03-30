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
        let queue = QueueStore::new(10);
        queue.push("/tmp/test.rs", 1, "trace", 0, 0, false).unwrap();
        
        let task = queue.pop().expect("Maillon 6 failed");
        assert_eq!(task.path, "/tmp/test.rs", "La queue doit restituer les tâches dans l'ordre");
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
}
