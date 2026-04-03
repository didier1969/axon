// Copyright (c) Didier Stadelmann. All rights reserved.

use crate::graph::GraphStore;
use crate::parser;
use crate::parser::elixir::ElixirParser;
use crate::parser::Parser;
use crate::queue::ProcessingMode;
use crate::queue::QueueStore;
use crate::worker::DbWriteTask;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_ingress_guard::{FileIngressGuard, GuardDecision};
    use crate::ingress_buffer::{
        IngressBuffer, IngressCause, IngressDrainBatch, IngressFileEvent, IngressSource,
        SharedIngressBuffer,
    };
    use once_cell::sync::Lazy;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    static FILE_INGRESS_GUARD_ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    fn lock_file_ingress_guard_env() -> std::sync::MutexGuard<'static, ()> {
        FILE_INGRESS_GUARD_ENV_LOCK.lock().unwrap()
    }

    fn shared_ingress_buffer() -> SharedIngressBuffer {
        Arc::new(Mutex::new(IngressBuffer::default()))
    }

    // --- MAILLON 1: LE SCANNER (Discovery) ---
    #[test]
    fn test_maillon_1_scanner_discovery() {
        let store = GraphStore::new(":memory:").unwrap();
        // Simuler un scan manuel
        let files = vec![("/tmp/test.rs".to_string(), "proj".to_string(), 100, 12345)];
        store.bulk_insert_files(&files).expect("Maillon 1 failed");

        let count = store
            .query_count("SELECT count(*) FROM File WHERE status = 'pending'")
            .unwrap();
        assert_eq!(
            count, 1,
            "Le scanner doit insérer les fichiers en status 'pending'"
        );

        let lifecycle = store
            .query_json("SELECT file_stage, graph_ready, vector_ready FROM File WHERE path = '/tmp/test.rs'")
            .unwrap();
        assert!(lifecycle.contains("promoted"), "{lifecycle}");
        assert!(lifecycle.contains("false"), "{lifecycle}");
    }

    #[test]
    fn test_maillon_1c_scanner_with_ingress_buffer_defers_canonical_write_until_promotion() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("buffered.ex");
        std::fs::write(&file_path, "defmodule Buffered do\nend\n").unwrap();

        let store = Arc::new(GraphStore::new(":memory:").unwrap());
        let ingress = shared_ingress_buffer();
        let scanner = crate::scanner::Scanner::new(&root.to_string_lossy());
        scanner.scan_with_guard_and_ingress(store.clone(), None, Some(&ingress));

        let pre_flush = store
            .query_count("SELECT count(*) FROM File WHERE path LIKE '%buffered.ex'")
            .unwrap();
        assert_eq!(
            pre_flush, 0,
            "Le scanner ne doit plus écrire canoniquement avant promotion"
        );

        let batch = ingress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .drain_batch(100);
        store.promote_ingress_batch(&batch).unwrap();

        let post_flush = store
            .query_count("SELECT count(*) FROM File WHERE path LIKE '%buffered.ex'")
            .unwrap();
        assert_eq!(
            post_flush, 1,
            "La promotion doit seule créer l'entrée canonique"
        );
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
        std::fs::write(
            root.join("docs").join(".axonignore"),
            "*.md\n!open/keep.md\n",
        )
        .unwrap();
        std::fs::write(root.join("docs").join("drop.md"), "# hidden").unwrap();
        std::fs::write(root.join("docs").join("open").join("keep.md"), "# visible").unwrap();

        let store = Arc::new(GraphStore::new(":memory:").unwrap());
        let scanner = crate::scanner::Scanner::new(&root.to_string_lossy());
        scanner.scan(store.clone());

        let files = store
            .query_json("SELECT path FROM File ORDER BY path")
            .unwrap();

        assert!(
            files.contains("kept.rs"),
            "Le scanner doit garder les fichiers autorisés"
        );
        assert!(
            files.contains("progress.md"),
            "Une ré-inclusion !pattern doit être respectée"
        );
        assert!(
            files.contains("keep.md"),
            "Une ré-ouverture locale doit être respectée"
        );
        assert!(
            !files.contains("lost.rs"),
            "Un répertoire ignoré par Axon Ignore ne doit pas être indexé"
        );
        assert!(
            !files.contains("drop.md"),
            "Une règle locale .axonignore doit exclure le fichier"
        );
    }

    // --- MAILLON 2: LE SÉLECTEUR (The Pull) ---
    #[test]
    fn test_maillon_2_selector_pull() {
        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[("/tmp/a.rs".to_string(), "p".to_string(), 10, 1)])
            .unwrap();

        let batch = store.fetch_pending_batch(10).expect("Maillon 2 failed");
        assert_eq!(
            batch.len(),
            1,
            "Le sélecteur doit être capable de tirer les fichiers pending"
        );

        let row = store
            .query_json(
                "SELECT status, status_reason, file_stage FROM File WHERE path = '/tmp/a.rs'",
            )
            .unwrap();
        assert!(row.contains("indexing"), "{row}");
        assert!(row.contains("claimed_for_indexing"), "{row}");
        assert!(row.contains("claimed"), "{row}");
    }

    #[test]
    fn test_file_ingress_guard_hydrates_and_skips_unchanged_file() {
        let _guard = lock_file_ingress_guard_env();
        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[("/tmp/unchanged.rs".to_string(), "proj".to_string(), 10, 1)])
            .unwrap();
        store
            .execute("UPDATE File SET status = 'indexed' WHERE path = '/tmp/unchanged.rs'")
            .unwrap();

        let guard = FileIngressGuard::hydrate_from_store(&store).unwrap();
        let decision = guard.should_stage(Path::new("/tmp/unchanged.rs"), 1, 10);

        assert_eq!(decision, GuardDecision::SkipUnchanged);
    }

    #[test]
    fn test_file_ingress_guard_stages_changed_file() {
        let _guard = lock_file_ingress_guard_env();
        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[("/tmp/changed.rs".to_string(), "proj".to_string(), 10, 1)])
            .unwrap();
        store
            .execute("UPDATE File SET status = 'indexed' WHERE path = '/tmp/changed.rs'")
            .unwrap();

        let guard = FileIngressGuard::hydrate_from_store(&store).unwrap();
        let decision = guard.should_stage(Path::new("/tmp/changed.rs"), 2, 10);

        assert_eq!(decision, GuardDecision::StageChanged);
    }

    #[test]
    fn test_file_ingress_guard_stages_unknown_file() {
        let _guard = lock_file_ingress_guard_env();
        let store = GraphStore::new(":memory:").unwrap();
        let guard = FileIngressGuard::hydrate_from_store(&store).unwrap();

        let decision = guard.should_stage(Path::new("/tmp/new.rs"), 1, 10);

        assert_eq!(decision, GuardDecision::StageNew);
    }

    #[test]
    fn test_file_ingress_guard_stages_indexing_file_with_changed_metadata() {
        let _guard = lock_file_ingress_guard_env();
        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[("/tmp/indexing.rs".to_string(), "proj".to_string(), 10, 1)])
            .unwrap();
        store
            .execute(
                "UPDATE File SET status = 'indexing', worker_id = 7 WHERE path = '/tmp/indexing.rs'",
            )
            .unwrap();

        let guard = FileIngressGuard::hydrate_from_store(&store).unwrap();
        let decision = guard.should_stage(Path::new("/tmp/indexing.rs"), 2, 10);

        assert_eq!(decision, GuardDecision::StageChanged);
    }

    #[test]
    fn test_file_ingress_guard_records_committed_tombstone_and_restages_recreated_file() {
        let _guard = lock_file_ingress_guard_env();
        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[("/tmp/recreated.rs".to_string(), "proj".to_string(), 10, 1)])
            .unwrap();
        store
            .execute("UPDATE File SET status = 'deleted' WHERE path = '/tmp/recreated.rs'")
            .unwrap();

        let mut guard = FileIngressGuard::hydrate_from_store(&store).unwrap();
        guard.record_tombstone(Path::new("/tmp/recreated.rs"));

        let decision = guard.should_stage(Path::new("/tmp/recreated.rs"), 2, 10);

        assert_eq!(decision, GuardDecision::StageChanged);
    }

    #[test]
    fn test_file_ingress_guard_kill_switch_disables_guard_path() {
        let _guard = FILE_INGRESS_GUARD_ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("AXON_ENABLE_FILE_INGRESS_GUARD", "0");
        }
        let store = GraphStore::new(":memory:").unwrap();
        let guard = FileIngressGuard::hydrate_from_store(&store).unwrap();

        assert!(!guard.is_enabled());

        unsafe {
            std::env::remove_var("AXON_ENABLE_FILE_INGRESS_GUARD");
        }
    }

    #[test]
    fn test_ingress_buffer_collapses_repeated_file_events_for_same_path() {
        let mut buffer = IngressBuffer::default();

        buffer.record_file(IngressFileEvent::new(
            "/tmp/collapse.ex",
            "proj",
            10,
            1,
            100,
            IngressSource::Scan,
            IngressCause::Discovered,
        ));
        buffer.record_file(IngressFileEvent::new(
            "/tmp/collapse.ex",
            "proj",
            12,
            2,
            100,
            IngressSource::Scan,
            IngressCause::Modified,
        ));

        let batch = buffer.drain_batch(100);

        assert_eq!(batch.files.len(), 1);
        assert_eq!(batch.files[0].path, "/tmp/collapse.ex");
        assert_eq!(batch.files[0].mtime, 2);
        assert_eq!(batch.files[0].size, 12);
        assert!(batch.collapsed_events >= 1);
    }

    #[test]
    fn test_ingress_buffer_keeps_highest_priority_for_same_path() {
        let mut buffer = IngressBuffer::default();

        buffer.record_file(IngressFileEvent::new(
            "/tmp/priority.ex",
            "proj",
            10,
            1,
            100,
            IngressSource::Scan,
            IngressCause::Discovered,
        ));
        buffer.record_file(IngressFileEvent::new(
            "/tmp/priority.ex",
            "proj",
            10,
            1,
            900,
            IngressSource::Watcher,
            IngressCause::Modified,
        ));

        let batch = buffer.drain_batch(100);

        assert_eq!(batch.files.len(), 1);
        assert_eq!(batch.files[0].priority, 900);
        assert_eq!(batch.files[0].source, IngressSource::Watcher);
    }

    #[test]
    fn test_ingress_buffer_tombstone_beats_stale_file_observation() {
        let mut buffer = IngressBuffer::default();

        buffer.record_file(IngressFileEvent::new(
            "/tmp/deleted.ex",
            "proj",
            10,
            1,
            100,
            IngressSource::Scan,
            IngressCause::Modified,
        ));
        buffer.record_tombstone("/tmp/deleted.ex", IngressSource::Watcher);

        let batch = buffer.drain_batch(100);

        assert!(batch.files.is_empty());
        assert_eq!(batch.tombstones, vec!["/tmp/deleted.ex".to_string()]);
    }

    #[test]
    fn test_ingress_buffer_records_subtree_hints_without_staging_files() {
        let mut buffer = IngressBuffer::default();

        buffer.record_subtree_hint("/tmp/project/tmp", 900, IngressSource::Watcher);

        let batch = buffer.drain_batch(100);

        assert!(batch.files.is_empty());
        assert!(batch.tombstones.is_empty());
        assert_eq!(batch.subtree_hints.len(), 1);
        assert_eq!(batch.subtree_hints[0].path, "/tmp/project/tmp");
    }

    #[test]
    fn test_ingress_buffer_subtree_hint_enters_in_flight_until_completed() {
        let mut buffer = IngressBuffer::default();

        buffer.record_subtree_hint("/tmp/project/runtime", 900, IngressSource::Watcher);

        let first = buffer.drain_batch(100);
        assert_eq!(first.subtree_hints.len(), 1);
        assert_eq!(buffer.metrics_snapshot().subtree_hint_in_flight, 1);

        let second = buffer.drain_batch(100);
        assert!(second.subtree_hints.is_empty());

        buffer.complete_subtree_hint("/tmp/project/runtime");
        assert_eq!(buffer.metrics_snapshot().subtree_hint_in_flight, 0);
    }

    #[test]
    fn test_ingress_buffer_subtree_hint_cooldown_blocks_immediate_requeue() {
        let mut buffer = IngressBuffer::default();

        buffer.record_subtree_hint("/tmp/project/cooling", 900, IngressSource::Watcher);
        let first = buffer.drain_batch(100);
        assert_eq!(first.subtree_hints.len(), 1);

        buffer.complete_subtree_hint("/tmp/project/cooling");
        buffer.record_subtree_hint("/tmp/project/cooling", 900, IngressSource::Watcher);

        let second = buffer.drain_batch(100);
        assert!(second.subtree_hints.is_empty());
        assert!(
            buffer.metrics_snapshot().subtree_hint_blocked_total >= 1,
            "Un hint immédiat pendant le cooldown doit être bloqué"
        );
    }

    #[test]
    fn test_ingress_buffer_partial_drain_keeps_remaining_entries() {
        let mut buffer = IngressBuffer::default();

        buffer.record_file(IngressFileEvent::new(
            "/tmp/a.ex",
            "proj",
            10,
            1,
            100,
            IngressSource::Scan,
            IngressCause::Discovered,
        ));
        buffer.record_file(IngressFileEvent::new(
            "/tmp/b.ex",
            "proj",
            20,
            2,
            100,
            IngressSource::Scan,
            IngressCause::Discovered,
        ));
        buffer.record_file(IngressFileEvent::new(
            "/tmp/c.ex",
            "proj",
            30,
            3,
            100,
            IngressSource::Scan,
            IngressCause::Discovered,
        ));

        let batch = buffer.drain_batch(2);

        assert_eq!(batch.files.len(), 2);
        assert_eq!(buffer.buffered_entries(), 1);

        let remaining = buffer.drain_batch(10);
        assert_eq!(remaining.files.len(), 1);
        assert_eq!(buffer.buffered_entries(), 0);
    }

    #[test]
    fn test_ingress_promoter_batch_writes_single_canonical_pending_update() {
        let store = GraphStore::new(":memory:").unwrap();
        let mut buffer = IngressBuffer::default();

        buffer.record_file(IngressFileEvent::new(
            "/tmp/promote.ex",
            "proj",
            10,
            1,
            100,
            IngressSource::Scan,
            IngressCause::Discovered,
        ));
        buffer.record_file(IngressFileEvent::new(
            "/tmp/promote.ex",
            "proj",
            20,
            2,
            100,
            IngressSource::Scan,
            IngressCause::Modified,
        ));

        let batch: IngressDrainBatch = buffer.drain_batch(100);
        let promoted = store.promote_ingress_batch(&batch).unwrap();

        assert_eq!(promoted.promoted_files, 1);
        assert_eq!(promoted.promoted_tombstones, 0);

        let row = store
            .query_json(
                "SELECT status, status_reason, size, mtime FROM File WHERE path = '/tmp/promote.ex'",
            )
            .unwrap();
        assert!(row.contains("pending"), "{row}");
        assert!(row.contains("20"), "{row}");
        assert!(row.contains("2"), "{row}");
    }

    #[test]
    fn test_boot_guard_hydrates_after_indexing_recovery() {
        let _guard = lock_file_ingress_guard_env();
        let db_root = std::env::temp_dir().join(format!(
            "axon-file-ingress-boot-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&db_root);
        std::fs::create_dir_all(&db_root).unwrap();

        let db_root_str = db_root.to_string_lossy().to_string();
        let store = GraphStore::new(&db_root_str).unwrap();
        store
            .bulk_insert_files(&[(
                "/tmp/recover_guard.rs".to_string(),
                "proj".to_string(),
                10,
                1,
            )])
            .unwrap();
        store
            .execute("UPDATE File SET status = 'indexing', worker_id = 3 WHERE path = '/tmp/recover_guard.rs'")
            .unwrap();
        drop(store);

        let reopened = GraphStore::new(&db_root_str).unwrap();
        let row = reopened
            .query_json("SELECT status FROM File WHERE path = '/tmp/recover_guard.rs'")
            .unwrap();
        assert!(row.contains("pending"));

        let guard = FileIngressGuard::hydrate_from_store(&reopened).unwrap();
        let decision = guard.should_stage(Path::new("/tmp/recover_guard.rs"), 1, 10);
        assert_eq!(decision, GuardDecision::SkipUnchanged);

        let _ = std::fs::remove_dir_all(&db_root);
    }

    #[test]
    fn test_scanner_requeue_records_metadata_changed_scan_reason() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/requeue_scan.rs".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 10, 1)])
            .unwrap();
        store
            .execute("UPDATE File SET status = 'indexed' WHERE path = '/tmp/requeue_scan.rs'")
            .unwrap();

        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 20, 2)])
            .unwrap();

        let row = store
            .query_json(
                "SELECT status, status_reason FROM File WHERE path = '/tmp/requeue_scan.rs'",
            )
            .unwrap();

        assert!(row.contains("pending"), "{row}");
        assert!(row.contains("metadata_changed_scan"), "{row}");
    }

    #[test]
    fn test_hot_delta_requeue_records_metadata_changed_hot_reason() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/requeue_hot.rs";
        store
            .bulk_insert_files(&[(path.to_string(), "proj".to_string(), 10, 1)])
            .unwrap();
        store
            .execute("UPDATE File SET status = 'indexed' WHERE path = '/tmp/requeue_hot.rs'")
            .unwrap();

        store.upsert_hot_file(path, "proj", 20, 2, 900).unwrap();

        let row = store
            .query_json("SELECT status, status_reason FROM File WHERE path = '/tmp/requeue_hot.rs'")
            .unwrap();

        assert!(row.contains("pending"), "{row}");
        assert!(row.contains("metadata_changed_hot_delta"), "{row}");
    }

    #[test]
    fn test_recovery_marks_requeued_reason_on_reopen() {
        let db_root = std::env::temp_dir().join(format!(
            "axon-status-reason-recovery-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&db_root);
        std::fs::create_dir_all(&db_root).unwrap();

        let db_root_str = db_root.to_string_lossy().to_string();
        let store = GraphStore::new(&db_root_str).unwrap();
        store
            .bulk_insert_files(&[(
                "/tmp/recover_reason.rs".to_string(),
                "proj".to_string(),
                10,
                1,
            )])
            .unwrap();
        store
            .execute("UPDATE File SET status = 'indexing', worker_id = 3 WHERE path = '/tmp/recover_reason.rs'")
            .unwrap();
        drop(store);

        let reopened = GraphStore::new(&db_root_str).unwrap();
        let row = reopened
            .query_json(
                "SELECT status, status_reason FROM File WHERE path = '/tmp/recover_reason.rs'",
            )
            .unwrap();

        assert!(row.contains("pending"), "{row}");
        assert!(row.contains("recovered_interrupted_indexing"), "{row}");

        let _ = std::fs::remove_dir_all(&db_root);
    }

    #[test]
    fn test_maillon_2d1_rust_watcher_with_guard_skips_unchanged_delta() {
        let _guard = lock_file_ingress_guard_env();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("unchanged_live.ex");
        std::fs::write(&file_path, "defmodule Live do\nend\n").unwrap();
        let metadata = std::fs::metadata(&file_path).unwrap();
        let size = metadata.len() as i64;
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[(
                file_path.to_string_lossy().to_string(),
                "proj".to_string(),
                size,
                mtime,
            )])
            .unwrap();
        store
            .execute(&format!(
                "UPDATE File SET status = 'indexed', priority = 10 WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        let guard = Arc::new(Mutex::new(
            FileIngressGuard::hydrate_from_store(&store).unwrap(),
        ));
        let staged = crate::fs_watcher::stage_hot_delta_with_guard(
            &store,
            root,
            &file_path,
            crate::fs_watcher::HOT_PRIORITY,
            &guard,
        )
        .unwrap();

        assert!(!staged, "Le guard doit filtrer un delta inchangé");

        let row = store
            .query_json(&format!(
                "SELECT status, priority FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert!(row.contains("indexed"));
        assert!(row.contains("10"));
    }

    #[test]
    fn test_maillon_2d3_watcher_ingress_buffer_defers_canonical_write_until_promotion() {
        let _guard = lock_file_ingress_guard_env();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("buffered_live.ex");
        std::fs::write(&file_path, "defmodule BufferedLive do\nend\n").unwrap();

        let ingress = shared_ingress_buffer();
        let guard = Arc::new(Mutex::new(FileIngressGuard::default()));

        let staged = crate::fs_watcher::enqueue_hot_delta_with_guard(
            root,
            &file_path,
            crate::fs_watcher::HOT_PRIORITY,
            &guard,
            &ingress,
        )
        .unwrap();
        assert!(staged);

        let store = GraphStore::new(":memory:").unwrap();
        let pre_flush = store
            .query_count("SELECT count(*) FROM File WHERE path LIKE '%buffered_live.ex'")
            .unwrap();
        assert_eq!(pre_flush, 0);

        let batch = ingress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .drain_batch(100);
        store.promote_ingress_batch(&batch).unwrap();

        let row = store
            .query_json("SELECT status, priority FROM File WHERE path LIKE '%buffered_live.ex'")
            .unwrap();
        assert!(row.contains("pending"), "{row}");
        assert!(row.contains("900"), "{row}");
    }

    #[test]
    fn test_maillon_2d2_scanner_with_guard_skips_unchanged_file() {
        let _guard = lock_file_ingress_guard_env();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("keep.ex");
        std::fs::write(&file_path, "defmodule Keep do\nend\n").unwrap();

        let metadata = std::fs::metadata(&file_path).unwrap();
        let size = metadata.len() as i64;
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let store = Arc::new(GraphStore::new(":memory:").unwrap());
        store
            .bulk_insert_files(&[(
                file_path.to_string_lossy().to_string(),
                "proj".to_string(),
                size,
                mtime,
            )])
            .unwrap();
        store
            .execute(&format!(
                "UPDATE File SET status = 'indexed' WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        let guard = Arc::new(Mutex::new(
            FileIngressGuard::hydrate_from_store(&store).unwrap(),
        ));
        let scanner = crate::scanner::Scanner::new(&root.to_string_lossy());
        scanner.scan_with_guard(store.clone(), Some(&guard));

        let pending = store
            .query_count(&format!(
                "SELECT count(*) FROM File WHERE path = '{}' AND status = 'pending'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert_eq!(pending, 0, "Le scan guardé ne doit pas rouvrir le fichier");
    }

    #[test]
    fn test_maillon_2b_rescan_requeues_changed_file() {
        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[("/tmp/a.rs".to_string(), "p".to_string(), 10, 1)])
            .unwrap();
        let _ = store.fetch_pending_batch(10).unwrap();
        store
            .execute(
                "UPDATE File SET status = 'indexed', worker_id = NULL WHERE path = '/tmp/a.rs'",
            )
            .unwrap();

        store
            .bulk_insert_files(&[("/tmp/a.rs".to_string(), "p".to_string(), 20, 2)])
            .unwrap();

        let status = store
            .query_json("SELECT status, size, mtime FROM File WHERE path = '/tmp/a.rs'")
            .unwrap();
        assert!(
            status.contains("pending"),
            "Un fichier modifié doit être remis en pending"
        );
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
            .bulk_insert_files(&[(
                "/tmp/reader_writer.ex".to_string(),
                "proj".to_string(),
                100,
                12345,
            )])
            .unwrap();

        let pending = store.fetch_pending_batch(10).unwrap();
        assert_eq!(
            pending.len(),
            1,
            "Le writer doit voir immédiatement le fichier pending"
        );

        let visible_now = store
            .query_count("SELECT count(*) FROM File WHERE path = '/tmp/reader_writer.ex'")
            .unwrap();
        assert_eq!(
            visible_now, 1,
            "Le reader doit voir immédiatement l'écriture"
        );

        drop(store);

        let reopened = GraphStore::new(&db_root_str).unwrap();
        let visible_after_restart = reopened
            .query_count("SELECT count(*) FROM File WHERE path = '/tmp/reader_writer.ex'")
            .unwrap();
        assert_eq!(
            visible_after_restart, 1,
            "La donnée doit survivre au redémarrage"
        );

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

        store.execute("DROP TABLE File;").unwrap();
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

        let row = reopened
            .query_json(
                "SELECT status, needs_reindex FROM File WHERE path = '/tmp/legacy_reopen.ex'",
            )
            .unwrap();
        assert!(row.contains("indexed"));
        assert!(
            row.contains("false"),
            "La colonne needs_reindex doit etre disponible apres reopen"
        );

        let _ = std::fs::remove_dir_all(&db_root);
    }

    #[test]
    fn test_maillon_2c_legacy_soll_reopen_adds_vision_goal_column() {
        let db_root = std::env::temp_dir().join(format!(
            "axon-legacy-soll-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&db_root);
        std::fs::create_dir_all(&db_root).unwrap();

        let db_root_str = db_root.to_string_lossy().to_string();
        let store = GraphStore::new(&db_root_str).unwrap();

        store.execute("DROP TABLE soll.Vision;").unwrap();
        store
            .execute(
                "CREATE TABLE soll.Vision (id VARCHAR PRIMARY KEY DEFAULT 'VIS-AXO-001', title VARCHAR, description VARCHAR, metadata VARCHAR)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.Vision (id, title, description, metadata) VALUES ('VIS-AXO-001', 'Legacy Vision', 'Desc', '{}')",
            )
            .unwrap();
        drop(store);

        let reopened = GraphStore::new(&db_root_str).unwrap();
        reopened
            .execute(
                "UPDATE soll.Vision SET goal = 'Truthful context', metadata = '{\"migrated\":true}' WHERE id = 'VIS-AXO-001'",
            )
            .unwrap();

        let row = reopened
            .query_json("SELECT title, description, goal, metadata FROM soll.Vision WHERE id = 'VIS-AXO-001'")
            .unwrap();
        assert!(row.contains("Legacy Vision"), "{row}");
        assert!(row.contains("Truthful context"), "{row}");
        assert!(row.contains("migrated"), "{row}");

        let _ = std::fs::remove_dir_all(&db_root);
    }

    #[test]
    fn test_maillon_2c_legacy_soll_reopen_adds_decision_rationale_column() {
        let db_root = std::env::temp_dir().join(format!(
            "axon-legacy-soll-decision-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&db_root);
        std::fs::create_dir_all(&db_root).unwrap();

        let db_root_str = db_root.to_string_lossy().to_string();
        let store = GraphStore::new(&db_root_str).unwrap();

        store.execute("DROP TABLE soll.Decision;").unwrap();
        store
            .execute(
                "CREATE TABLE soll.Decision (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR, status VARCHAR, context VARCHAR, metadata VARCHAR)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.Decision (id, title, description, status, context, metadata) VALUES ('DEC-AXO-001', 'Legacy Decision', 'Desc', 'accepted', 'Ctx', '{}')",
            )
            .unwrap();
        drop(store);

        let reopened = GraphStore::new(&db_root_str).unwrap();
        reopened
            .execute(
                "UPDATE soll.Decision SET rationale = 'Because truth matters', metadata = '{\"migrated\":true}' WHERE id = 'DEC-AXO-001'",
            )
            .unwrap();

        let row = reopened
            .query_json("SELECT title, description, context, rationale, metadata FROM soll.Decision WHERE id = 'DEC-AXO-001'")
            .unwrap();
        assert!(row.contains("Legacy Decision"), "{row}");
        assert!(row.contains("Because truth matters"), "{row}");
        assert!(row.contains("migrated"), "{row}");

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
            .bulk_insert_files(&[(
                "/tmp/embed_reset.ex".to_string(),
                "proj".to_string(),
                100,
                1,
            )])
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
        store.execute("DELETE FROM RuntimeMetadata;").unwrap();
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
            reopened
                .query_count("SELECT count(*) FROM ChunkEmbedding")
                .unwrap(),
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
            .bulk_insert_files(&[(
                "/tmp/ingestion_reset.ex".to_string(),
                "proj".to_string(),
                100,
                1,
            )])
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
        store.execute("DELETE FROM RuntimeMetadata;").unwrap();
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
            reopened
                .query_count("SELECT count(*) FROM CONTAINS")
                .unwrap(),
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
                .query_count("SELECT count(*) FROM RuntimeMetadata WHERE key = 'schema_version' AND value = '3'")
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

        assert!(
            row.contains("pending"),
            "Le delta doit remettre le fichier en pending"
        );
        assert!(
            row.contains("900"),
            "Le delta chaud doit imposer une priorité élevée"
        );
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

        assert!(
            !staged,
            "Un chemin ignoré par Axon Ignore ne doit pas être staged"
        );

        let count = store
            .query_count(&format!(
                "SELECT count(*) FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert_eq!(
            count, 0,
            "Le fichier ignoré ne doit pas apparaître dans IST"
        );
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

        assert_eq!(
            staged, 1,
            "Une rafale d'evenements identiques ne doit stager qu'une fois"
        );

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
        let nested = project.join("nested");
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

        assert!(
            staged,
            "Un evenement de repertoire doit pouvoir remonter un fichier imbrique"
        );

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
    fn test_maillon_2h2_watcher_ignores_noisy_directory_event_before_subtree_hint() {
        let _guard = lock_file_ingress_guard_env();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        let noisy = project.join("node_modules").join("pkg");
        std::fs::create_dir_all(&noisy).unwrap();
        std::fs::write(noisy.join("ignored.ex"), "defmodule Ignored do\nend\n").unwrap();

        let ingress = shared_ingress_buffer();
        let guard = Arc::new(Mutex::new(FileIngressGuard::default()));

        let staged = crate::fs_watcher::enqueue_hot_delta_with_guard(
            root,
            &project.join("node_modules"),
            crate::fs_watcher::HOT_PRIORITY,
            &guard,
            &ingress,
        )
        .unwrap();

        assert!(
            !staged,
            "Un repertoire bruité ne doit pas produire de subtree_hint"
        );

        let locked = ingress.lock().unwrap_or_else(|poison| poison.into_inner());
        assert_eq!(locked.subtree_hint_entries(), 0);
        assert_eq!(locked.buffered_entries(), 0);
    }

    #[test]
    fn test_maillon_2h3_watcher_allows_project_local_worktree_directory_when_root_rule_is_anchored()
    {
        let _guard = lock_file_ingress_guard_env();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join(".axonignore"), "/.worktrees/\n").unwrap();

        let project_worktree = root.join("proj").join(".worktrees").join("feature");
        std::fs::create_dir_all(&project_worktree).unwrap();
        std::fs::write(
            project_worktree.join("keep.ex"),
            "defmodule ProjectWorktree do\nend\n",
        )
        .unwrap();

        let ingress = shared_ingress_buffer();
        let guard = Arc::new(Mutex::new(FileIngressGuard::default()));

        let staged = crate::fs_watcher::enqueue_hot_delta_with_guard(
            root,
            &project_worktree,
            crate::fs_watcher::HOT_PRIORITY,
            &guard,
            &ingress,
        )
        .unwrap();

        assert!(
            staged,
            "Une worktree locale au projet doit rester eligible si seule la worktree racine est ignoree"
        );

        let locked = ingress.lock().unwrap_or_else(|poison| poison.into_inner());
        assert_eq!(locked.subtree_hint_entries(), 1);
    }

    #[test]
    fn test_maillon_2h4_watcher_blocks_subtree_hint_for_build_tree_even_if_directory_exists() {
        let _guard = lock_file_ingress_guard_env();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let blocked = root.join("proj").join("_build").join("dev").join("lib");
        std::fs::create_dir_all(&blocked).unwrap();

        let ingress = shared_ingress_buffer();
        let guard = Arc::new(Mutex::new(FileIngressGuard::default()));

        let staged = crate::fs_watcher::enqueue_hot_delta_with_guard(
            root,
            &root.join("proj").join("_build"),
            crate::fs_watcher::HOT_PRIORITY,
            &guard,
            &ingress,
        )
        .unwrap();

        assert!(
            !staged,
            "Un arbre de build ne doit pas produire de subtree_hint"
        );

        let locked = ingress.lock().unwrap_or_else(|poison| poison.into_inner());
        assert_eq!(locked.subtree_hint_entries(), 0);
        assert_eq!(locked.buffered_entries(), 0);
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
        assert_eq!(
            first_batch.len(),
            1,
            "Le premier claim doit prendre le fichier"
        );

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

        assert!(
            row.contains("indexing"),
            "Le fichier doit rester en cours d'indexation"
        );
        assert!(
            !row.contains("null"),
            "Le worker actif doit rester attache au fichier"
        );
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
        assert_eq!(
            first_batch.len(),
            1,
            "Le premier claim doit prendre le fichier"
        );

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
                reservation_id: "res-live-changed".to_string(),
                path: file_path.to_string_lossy().to_string(),
                content: Some("defmodule LiveChanged do\nend\n".to_string()),
                extraction,
                processing_mode: ProcessingMode::Full,
                trace_id: "trace".to_string(),
                observed_cost_bytes: 0,
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
        assert!(
            row.contains("900"),
            "La priorite chaude doit etre preservee pour la seconde passe"
        );
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
                reservation_id: "res-deleted-live".to_string(),
                path: file_path.to_string_lossy().to_string(),
                content: Some("defmodule DeletedLive do\nend\n".to_string()),
                extraction,
                processing_mode: ProcessingMode::Full,
                trace_id: "trace".to_string(),
                observed_cost_bytes: 0,
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
        assert!(
            row.contains("deleted"),
            "Le fichier supprimé doit être tombstoné"
        );
        assert!(
            row.contains("null"),
            "Le worker doit être libéré après tombstone"
        );

        let contains_count = store
            .query_count(&format!(
                "SELECT count(*) FROM CONTAINS WHERE source_id = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert_eq!(
            contains_count, 0,
            "Le lien CONTAINS du fichier supprimé doit disparaître"
        );

        let symbol_count = store.query_count("SELECT count(*) FROM Symbol").unwrap();
        assert_eq!(
            symbol_count, 0,
            "Les symboles du fichier supprimé doivent disparaître"
        );
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
                reservation_id: "res-rename-old".to_string(),
                path: old_path.to_string_lossy().to_string(),
                content: Some("defmodule RenameOld do\nend\n".to_string()),
                extraction,
                processing_mode: ProcessingMode::Full,
                trace_id: "trace".to_string(),
                observed_cost_bytes: 0,
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

        assert_eq!(
            staged, 2,
            "Un rename doit tombstoner l'ancien chemin et stager le nouveau"
        );

        let old_row = store
            .query_json(&format!(
                "SELECT status FROM File WHERE path = '{}'",
                old_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert!(
            old_row.contains("deleted"),
            "L'ancien chemin doit être tombstoné"
        );

        let new_row = store
            .query_json(&format!(
                "SELECT status, priority FROM File WHERE path = '{}'",
                new_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert!(
            new_row.contains("pending"),
            "Le nouveau chemin doit être staged en pending"
        );
        assert!(
            new_row.contains("900"),
            "Le nouveau chemin doit garder la priorité chaude"
        );

        let old_contains_count = store
            .query_count(&format!(
                "SELECT count(*) FROM CONTAINS WHERE source_id = '{}'",
                old_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert_eq!(
            old_contains_count, 0,
            "L'ancien chemin ne doit pas garder de vérité dérivée"
        );
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
        assert_eq!(
            claimed.len(),
            1,
            "Le fichier doit d'abord être pris par un claim actif"
        );

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
                "SELECT status, worker_id, file_stage FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert!(
            replay_row.contains("pending"),
            "Un fichier resté indexing après crash doit être rejoué au redémarrage"
        );
        assert!(
            replay_row.contains("null"),
            "Le worker orphelin doit être libéré au redémarrage"
        );
        assert!(replay_row.contains("promoted"), "{replay_row}");

        let replay_batch = reopened.fetch_pending_batch(10).unwrap();
        assert_eq!(
            replay_batch.len(),
            1,
            "Le fichier doit redevenir claimable après redémarrage"
        );

        let _ = std::fs::remove_dir_all(&db_root);
    }

    #[test]
    fn test_maillon_2o_oversized_file_status_is_explicit_and_reversible_on_new_scan() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/oversized_file.rs".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 10_000, 1)])
            .unwrap();

        store.mark_file_oversized_for_current_budget(&path).unwrap();

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
    fn test_maillon_2q_degraded_commit_preserves_structure_without_chunk_materialization() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/degraded_file.rs".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 128, 1)])
            .unwrap();

        let extraction = parser::ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![parser::Symbol {
                name: "degraded_file".to_string(),
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
                reservation_id: "res-degraded-file".to_string(),
                path: path.clone(),
                content: None,
                extraction,
                processing_mode: ProcessingMode::StructureOnly,
                trace_id: "trace".to_string(),
                observed_cost_bytes: 0,
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        let row = store
            .query_json(
                "SELECT status, last_error_reason, file_stage, graph_ready, vector_ready FROM File WHERE path = '/tmp/degraded_file.rs'",
            )
            .unwrap();
        assert!(
            row.contains("indexed_degraded"),
            "unexpected degraded row payload: {}",
            row
        );
        assert!(row.contains("degraded_structure_only"));
        assert!(row.contains("graph_indexed"), "{row}");
        assert!(row.contains("true"), "{row}");

        let symbol_count = store
            .query_count("SELECT count(*) FROM Symbol WHERE project_slug = 'proj'")
            .unwrap();
        assert_eq!(
            symbol_count, 1,
            "degraded mode must still preserve the structural symbol truth"
        );

        let chunk_count = store.query_count("SELECT count(*) FROM Chunk").unwrap();
        assert_eq!(
            chunk_count, 0,
            "degraded mode must avoid heavy chunk materialization"
        );
    }

    #[test]
    fn test_maillon_2r_full_commit_records_success_reason() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/full_success.rs".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 128, 1)])
            .unwrap();

        let extraction = parser::ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![parser::Symbol {
                name: "full_success".to_string(),
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
                reservation_id: "res-full-success".to_string(),
                path: path.clone(),
                content: Some("fn full_success() {}".to_string()),
                extraction,
                processing_mode: ProcessingMode::Full,
                trace_id: "trace".to_string(),
                observed_cost_bytes: 0,
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        let row = store
            .query_json(
                "SELECT status, status_reason, file_stage, graph_ready, vector_ready FROM File WHERE path = '/tmp/full_success.rs'",
            )
            .unwrap();
        assert!(row.contains("indexed"), "{row}");
        assert!(row.contains("indexed_success_full"), "{row}");
        assert!(row.contains("graph_indexed"), "{row}");
        assert!(row.contains("true"), "{row}");
    }

    #[test]
    fn test_maillon_2r2_skipped_commit_marks_terminal_file_stage_without_graph_ready() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/skipped_file.rs".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 32, 1)])
            .unwrap();

        store
            .insert_file_data_batch(&[DbWriteTask::FileSkipped {
                reservation_id: "res-skipped-file".to_string(),
                path: path.clone(),
                reason: "unsupported".to_string(),
                trace_id: "trace".to_string(),
                observed_cost_bytes: None,
                t0: 0,
                t1: 0,
                t2: 0,
            }])
            .unwrap();

        let row = store
            .query_json(
                "SELECT status, file_stage, graph_ready, vector_ready FROM File WHERE path = '/tmp/skipped_file.rs'",
            )
            .unwrap();
        assert!(row.contains("skipped"), "{row}");
        assert!(row.contains("false"), "{row}");
    }

    #[test]
    fn test_maillon_2r3_bootstrap_adds_lifecycle_columns() {
        let store = GraphStore::new(":memory:").unwrap();
        let columns = store
            .query_json("SELECT name FROM pragma_table_info('File')")
            .unwrap();

        assert!(columns.contains("file_stage"), "{columns}");
        assert!(columns.contains("graph_ready"), "{columns}");
        assert!(columns.contains("vector_ready"), "{columns}");
    }

    #[test]
    fn test_maillon_2r4_vector_ready_flips_true_after_chunk_embeddings_land() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/vector_ready.rs".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 128, 1)])
            .unwrap();

        let extraction = parser::ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![parser::Symbol {
                name: "vector_ready".to_string(),
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
                reservation_id: "res-vector-ready".to_string(),
                path: path.clone(),
                content: Some("fn vector_ready() {}".to_string()),
                extraction,
                processing_mode: ProcessingMode::Full,
                trace_id: "trace".to_string(),
                observed_cost_bytes: 0,
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        let before = store
            .query_json(
                "SELECT graph_ready, vector_ready FROM File WHERE path = '/tmp/vector_ready.rs'",
            )
            .unwrap();
        assert!(before.contains("true"), "{before}");
        assert!(before.contains("false"), "{before}");

        let chunk_rows = store
            .query_json("SELECT id, content_hash FROM Chunk WHERE project_slug = 'proj'")
            .unwrap();
        let rows: Vec<Vec<String>> = serde_json::from_str(&chunk_rows).unwrap();
        let chunk_id = rows[0][0].clone();
        let content_hash = rows[0][1].clone();

        store
            .update_chunk_embeddings("test-model", &[(chunk_id, content_hash, vec![0.0; 384])])
            .unwrap();

        let after = store
            .query_json(
                "SELECT graph_ready, vector_ready FROM File WHERE path = '/tmp/vector_ready.rs'",
            )
            .unwrap();
        let after_rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&after).unwrap();
        assert_eq!(after_rows.len(), 1);
        assert_eq!(after_rows[0].len(), 2);
        assert_eq!(after_rows[0][0].as_str(), Some("true"));
        assert_eq!(after_rows[0][1].as_str(), Some("true"));
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
            .query_json("SELECT defer_count, last_deferred_at_ms, status_reason FROM File WHERE path = '/tmp/deferred_file.rs'")
            .unwrap();
        assert!(
            deferred_row.contains("2"),
            "Le déferrement doit construire une dette de fairness persistante"
        );
        assert!(
            !deferred_row.contains("null"),
            "Le timestamp de dernier déferrement doit être renseigné"
        );
        assert!(
            deferred_row.contains("deferred_by_scheduler"),
            "Le déferrement doit aussi exposer une cause opératoire"
        );

        let claimed = store
            .claim_pending_paths(std::slice::from_ref(&path))
            .unwrap();
        assert_eq!(claimed.len(), 1, "Le fichier différé doit rester claimable");

        let claimed_row = store
            .query_json("SELECT status, defer_count, last_deferred_at_ms, status_reason FROM File WHERE path = '/tmp/deferred_file.rs'")
            .unwrap();
        assert!(claimed_row.contains("indexing"));
        assert!(
            claimed_row.contains("0"),
            "Une claim effective doit remettre à zéro la dette de fairness"
        );
        assert!(
            claimed_row.contains("null"),
            "Le timestamp de déferrement doit être purgé après claim"
        );
        assert!(
            claimed_row.contains("claimed_for_indexing"),
            "Une claim effective doit remplacer la raison de backlog par une raison d'exécution"
        );
    }

    #[test]
    fn test_requeue_claimed_file_with_specific_reason_updates_status_reason() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/requeue_specific_reason.ex".to_string();

        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 10, 1)])
            .unwrap();
        let claimed = store
            .claim_pending_paths(std::slice::from_ref(&path))
            .unwrap();
        assert_eq!(claimed.len(), 1);

        store
            .requeue_claimed_file_with_reason(&path, "requeued_after_queue_push_failure")
            .unwrap();

        let row = store
            .query_json(&format!(
                "SELECT status, status_reason FROM File WHERE path = '{}'",
                path.replace('\'', "''")
            ))
            .unwrap();
        assert!(row.contains("pending"), "{row}");
        assert!(row.contains("requeued_after_queue_push_failure"), "{row}");
    }

    #[test]
    fn test_tombstoned_late_writer_update_keeps_deleted_reason() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/tombstoned_late.rs".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 128, 1)])
            .unwrap();
        store
            .tombstone_missing_path(Path::new(&path))
            .expect("tombstone should succeed");

        store
            .insert_file_data_batch(&[DbWriteTask::FileSkipped {
                reservation_id: "res-tombstoned-late".to_string(),
                path: path.clone(),
                reason: "Read Error: vanished".to_string(),
                trace_id: "trace-late".to_string(),
                observed_cost_bytes: None,
                t0: 0,
                t1: 0,
                t2: 0,
            }])
            .unwrap();

        let row = store
            .query_json(
                "SELECT status, status_reason FROM File WHERE path = '/tmp/tombstoned_late.rs'",
            )
            .unwrap();
        assert!(row.contains("deleted"), "{row}");
        assert!(row.contains("tombstoned_missing"), "{row}");
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
        assert!(
            staged,
            "Le delete doit tombstoner pendant qu'un worker est encore en vol"
        );

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
                reservation_id: "res-late-deleted".to_string(),
                path: file_path.to_string_lossy().to_string(),
                content: Some("defmodule LateDeleted do\nend\n".to_string()),
                extraction,
                processing_mode: ProcessingMode::Full,
                trace_id: "trace".to_string(),
                observed_cost_bytes: 0,
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
        assert!(
            row.contains("deleted"),
            "Un commit tardif ne doit pas ressusciter un tombstone"
        );

        let symbol_count = store.query_count("SELECT count(*) FROM Symbol").unwrap();
        assert_eq!(
            symbol_count, 0,
            "Aucune vérité dérivée ne doit réapparaître après tombstone"
        );
    }

    // --- MAILLON 3: LA SOCKET (Le Protocole) ---
    #[tokio::test]
    async fn test_maillon_3_socket_protocol() {
        use std::fs;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::{UnixListener, UnixStream};

        let sock_path = "/tmp/test-maillon-3.sock";
        if std::path::Path::new(sock_path).exists() {
            let _ = fs::remove_file(sock_path);
        }

        let listener = match UnixListener::bind(sock_path) {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "Skipping socket protocol test in sandboxed environment: {}",
                    err
                );
                return;
            }
            Err(err) => panic!("Failed to bind unix socket: {}", err),
        };
        let store = Arc::new(GraphStore::new(":memory:").unwrap());

        // Simuler un fichier en attente
        store
            .bulk_insert_files(&[("/tmp/test.ex".to_string(), "proj".to_string(), 10, 1)])
            .unwrap();

        // Spawn Server Loop (Simulé de main.rs)
        let server_store = store.clone();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = socket.into_split();
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();

            // Welcome
            writer
                .write_all(b"Axon Telemetry Ready\n{\"SystemReady\":{}}\n")
                .await
                .unwrap();

            if let Ok(_) = buf_reader.read_line(&mut line).await {
                let command = line.trim();
                if command.starts_with("PULL_PENDING ") {
                    let count = command[13..].parse::<usize>().unwrap_or(1);
                    let files = server_store.fetch_pending_batch(count).unwrap();
                    let response =
                        serde_json::json!({"event": "PENDING_BATCH_READY", "files": files});
                    writer
                        .write_all((serde_json::to_string(&response).unwrap() + "\n").as_bytes())
                        .await
                        .unwrap();
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

        assert!(
            response.contains("PENDING_BATCH_READY"),
            "Le serveur doit répondre avec le batch de fichiers"
        );
        assert!(
            response.contains("/tmp/test.ex"),
            "Le batch doit contenir le fichier attendu"
        );

        let _ = fs::remove_file(sock_path);
    }

    // --- MAILLON 5: LA TRANSFORMATION (AST Parser) ---
    #[test]
    fn test_maillon_5_ast_parser() {
        let content = "defmodule T, do: def h, do: :ok";
        let parser = ElixirParser::new();
        let result = parser.parse(content);

        assert!(
            result.symbols.len() > 0,
            "Le parser doit extraire au moins un symbole"
        );
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
        queue
            .push(path.to_string_lossy().as_ref(), 1, "trace", 0, 0, false)
            .unwrap();

        let task = queue.pop().expect("Maillon 6 failed");
        assert_eq!(
            task.path,
            path.to_string_lossy(),
            "La queue doit restituer les tâches dans l'ordre"
        );
    }

    // --- MAILLON 7: LE COMMITTER (Writer Actor) ---
    #[test]
    fn test_maillon_7_writer_commit() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/test.rs".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 100, 12345)])
            .unwrap();

        let extraction = parser::ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![parser::Symbol {
                name: "test".to_string(),
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

        let task = DbWriteTask::FileExtraction {
            reservation_id: "res-maillon-7".to_string(),
            path: path.clone(),
            content: Some("fn test() {}".to_string()),
            extraction,
            processing_mode: ProcessingMode::Full,
            trace_id: "t".to_string(),
            observed_cost_bytes: 0,
            t0: 0,
            t1: 0,
            t2: 0,
            t3: 0,
        };

        store
            .insert_file_data_batch(&[task])
            .expect("Maillon 7 failed");

        let status_json = store.query_json("SELECT status FROM File").unwrap();
        assert!(
            status_json.contains("indexed"),
            "Le committer doit passer le statut à 'indexed'"
        );

        let chunk_count = store.query_count("SELECT count(*) FROM Chunk").unwrap();
        assert_eq!(
            chunk_count, 1,
            "Le committer doit aussi matérialiser un chunk dérivé"
        );
    }

    #[test]
    fn test_maillon_7b_chunk_embedding_storage() {
        let store = GraphStore::new(":memory:").unwrap();
        let path = "/tmp/test.rs".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 100, 12345)])
            .unwrap();

        let extraction = parser::ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![parser::Symbol {
                name: "test".to_string(),
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

        let task = DbWriteTask::FileExtraction {
            reservation_id: "res-maillon-7b".to_string(),
            path: path.clone(),
            content: Some("fn test() {}".to_string()),
            extraction,
            processing_mode: ProcessingMode::Full,
            trace_id: "t".to_string(),
            observed_cost_bytes: 0,
            t0: 0,
            t1: 0,
            t2: 0,
            t3: 0,
        };

        store
            .insert_file_data_batch(&[task])
            .expect("Chunk setup failed");
        store
            .ensure_embedding_model(
                "chunk-bge-small-en-v1.5-384",
                "chunk",
                "BAAI/bge-small-en-v1.5",
                384,
                "1",
            )
            .unwrap();

        let pending = store
            .fetch_unembedded_chunks("chunk-bge-small-en-v1.5-384", 10)
            .unwrap();
        assert_eq!(
            pending.len(),
            1,
            "Le store doit détecter le chunk non vectorisé"
        );

        let vector = vec![0.0_f32; 384];
        store
            .update_chunk_embeddings(
                "chunk-bge-small-en-v1.5-384",
                &[(pending[0].0.clone(), pending[0].2.clone(), vector)],
            )
            .unwrap();

        let stored = store
            .query_count("SELECT count(*) FROM ChunkEmbedding")
            .unwrap();
        assert_eq!(
            stored, 1,
            "Le vector store dérivé doit persister l'embedding du chunk"
        );
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

        let extraction_for = |project: &str, name: &str, _body: &str, docstring: Option<&str>| {
            parser::ExtractionResult {
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
            }
        };

        store
            .insert_file_data_batch(&[
                DbWriteTask::FileExtraction {
                    reservation_id: "res-alpha-1".to_string(),
                    path: path_a.clone(),
                    content: Some("fn alpha() {\n    old_body();\n}\n".to_string()),
                    extraction: extraction_for("proj", "alpha", "old_body", None),
                    processing_mode: ProcessingMode::Full,
                    trace_id: "alpha-1".to_string(),
                    observed_cost_bytes: 0,
                    t0: 0,
                    t1: 0,
                    t2: 0,
                    t3: 0,
                },
                DbWriteTask::FileExtraction {
                    reservation_id: "res-beta-1".to_string(),
                    path: path_b.clone(),
                    content: Some("fn beta() {\n    stable_body();\n}\n".to_string()),
                    extraction: extraction_for("proj", "beta", "stable_body", None),
                    processing_mode: ProcessingMode::Full,
                    trace_id: "beta-1".to_string(),
                    observed_cost_bytes: 0,
                    t0: 0,
                    t1: 0,
                    t2: 0,
                    t3: 0,
                },
                DbWriteTask::FileExtraction {
                    reservation_id: "res-gamma-1".to_string(),
                    path: path_c.clone(),
                    content: Some("fn gamma() {\n    foreign_project();\n}\n".to_string()),
                    extraction: extraction_for("other", "gamma", "foreign_project", None),
                    processing_mode: ProcessingMode::Full,
                    trace_id: "gamma-1".to_string(),
                    observed_cost_bytes: 0,
                    t0: 0,
                    t1: 0,
                    t2: 0,
                    t3: 0,
                },
            ])
            .unwrap();

        store
            .ensure_embedding_model(
                "chunk-bge-small-en-v1.5-384",
                "chunk",
                "BAAI/bge-small-en-v1.5",
                384,
                "1",
            )
            .unwrap();

        let initial_pending = store
            .fetch_unembedded_chunks("chunk-bge-small-en-v1.5-384", 10)
            .unwrap();
        assert_eq!(
            initial_pending.len(),
            3,
            "Tous les chunks initiaux doivent etre vectorisables"
        );

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
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM ChunkEmbedding")
                .unwrap(),
            3
        );

        store
            .insert_file_data_batch(&[DbWriteTask::FileExtraction {
                reservation_id: "res-alpha-2".to_string(),
                path: path_a.clone(),
                content: Some("fn alpha() {\n    new_body();\n}\n".to_string()),
                extraction: extraction_for(
                    "proj",
                    "alpha",
                    "new_body",
                    Some("routes the new behavior without replaying all semantic work"),
                ),
                processing_mode: ProcessingMode::Full,
                trace_id: "alpha-2".to_string(),
                observed_cost_bytes: 0,
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        assert_eq!(
            store
                .query_count("SELECT count(*) FROM ChunkEmbedding")
                .unwrap(),
            2,
            "Seul le chunk du fichier change doit perdre son embedding derive"
        );

        let pending = store
            .fetch_unembedded_chunks("chunk-bge-small-en-v1.5-384", 10)
            .unwrap();
        assert_eq!(
            pending.len(),
            1,
            "Le delta ne doit revectoriser que le chunk modifie"
        );
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
            .ensure_embedding_model(
                "chunk-bge-small-en-v1.5-384",
                "chunk",
                "BAAI/bge-small-en-v1.5",
                384,
                "1",
            )
            .unwrap();
        store
            .execute("INSERT INTO ChunkEmbedding (chunk_id, model_id, source_hash) VALUES ('chunk-drift', 'chunk-bge-small-en-v1.5-384', 'hash-stale')")
            .unwrap();

        let pending = store
            .fetch_unembedded_chunks("chunk-bge-small-en-v1.5-384", 10)
            .unwrap();
        assert_eq!(
            pending.len(),
            1,
            "Un hash derive stale doit etre revectorise"
        );
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
                    reservation_id: "res-a".to_string(),
                    path: path_a.clone(),
                    content: Some("def send_cypher(query):\n    return query\n".to_string()),
                    extraction: extraction_a,
                    processing_mode: ProcessingMode::Full,
                    trace_id: "a".to_string(),
                    observed_cost_bytes: 0,
                    t0: 0,
                    t1: 0,
                    t2: 0,
                    t3: 0,
                },
                DbWriteTask::FileExtraction {
                    reservation_id: "res-b".to_string(),
                    path: path_b.clone(),
                    content: Some("def send_cypher(query):\n    return query\n".to_string()),
                    extraction: extraction_b,
                    processing_mode: ProcessingMode::Full,
                    trace_id: "b".to_string(),
                    observed_cost_bytes: 0,
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
                reservation_id: "res-multi-clause".to_string(),
                path: path.clone(),
                content: Some("defmodule AxonDashboardWeb.StatusLive do\nend\n".to_string()),
                extraction,
                processing_mode: ProcessingMode::Full,
                trace_id: "trace".to_string(),
                observed_cost_bytes: 0,
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
            .execute(
                "INSERT INTO CONTAINS (source_id, target_id) VALUES \
                ('/tmp/graph/a.rs', 'proj::A'), \
                ('/tmp/graph/a.rs', 'proj::B'), \
                ('/tmp/graph/other.rs', 'proj::C'), \
                ('/tmp/graph/other.rs', 'proj::X')",
            )
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
            .execute(
                "INSERT INTO CONTAINS (source_id, target_id) VALUES \
                ('/tmp/graph/a.rs', 'proj::A'), \
                ('/tmp/graph/a.rs', 'proj::B'), \
                ('/tmp/graph/other.rs', 'proj::C'), \
                ('/tmp/graph/other.rs', 'proj::X')",
            )
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
            .execute(
                "INSERT INTO CONTAINS (source_id, target_id) VALUES \
                ('/tmp/graph/file_anchor.rs', 'proj::FileAlpha'), \
                ('/tmp/graph/file_anchor.rs', 'proj::FileBeta'), \
                ('/tmp/graph/helper.rs', 'proj::Helper')",
            )
            .unwrap();
        store
            .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('proj::FileAlpha', 'proj::Helper')")
            .unwrap();

        store.refresh_file_projection(file_path, 2).unwrap();
        let first_count = store
            .query_count("SELECT count(*) FROM GraphProjection WHERE anchor_type = 'file' AND anchor_id = '/tmp/graph/file_anchor.rs' AND radius = 2")
            .unwrap();
        let first_projection = store.query_graph_projection("file", file_path, 2).unwrap();

        store.refresh_file_projection(file_path, 2).unwrap();
        let second_count = store
            .query_count("SELECT count(*) FROM GraphProjection WHERE anchor_type = 'file' AND anchor_id = '/tmp/graph/file_anchor.rs' AND radius = 2")
            .unwrap();
        let second_projection = store.query_graph_projection("file", file_path, 2).unwrap();

        assert_eq!(
            first_count, second_count,
            "La projection dérivée ne doit pas se dupliquer"
        );
        assert_eq!(
            first_projection, second_projection,
            "La même ancre et le même rayon doivent rester stables"
        );
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
        assert_ne!(
            first_state, "[]",
            "L'etat de projection doit etre materialise"
        );
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
        assert_ne!(
            second_state, "[]",
            "L'etat de projection doit rester disponible"
        );
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
        assert_ne!(
            before_d_state, "[]",
            "Le voisinage non touche doit avoir un etat materialise"
        );

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
        assert_ne!(
            after_d_state, "[]",
            "Le voisinage non touche doit rester materialise"
        );

        assert!(projection_a.contains("proj::C"));
        assert!(!projection_a.contains("proj::B"));
        assert_eq!(
            before_d_state, after_d_state,
            "Le refresh d'une ancre modifiée ne doit pas réécrire les voisinages non touchés"
        );
    }

    #[test]
    fn test_graph_projection_symbol_includes_calls_nif_edges() {
        let store = GraphStore::new(":memory:").unwrap();
        store
            .execute("INSERT INTO File (path, project_slug) VALUES ('/tmp/graph/a.rs', 'proj'), ('/tmp/graph/b.rs', 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES \
                ('proj::A', 'A', 'function', true, true, false, false, 'proj'), \
                ('proj::B', 'B', 'function', true, true, true, false, 'proj')")
            .unwrap();
        store
            .execute(
                "INSERT INTO CONTAINS (source_id, target_id) VALUES \
                ('/tmp/graph/a.rs', 'proj::A'), \
                ('/tmp/graph/b.rs', 'proj::B')",
            )
            .unwrap();
        store
            .execute("INSERT INTO CALLS_NIF (source_id, target_id) VALUES ('proj::A', 'proj::B')")
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
    }

    #[test]
    fn test_tombstone_missing_path_invalidates_dependent_graph_derivations() {
        let store = GraphStore::new(":memory:").unwrap();
        store
            .execute("INSERT INTO File (path, project_slug) VALUES ('/tmp/graph/deleted.rs', 'proj'), ('/tmp/graph/keeper.rs', 'proj')")
            .unwrap();
        store
            .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES \
                ('proj::Deleted', 'Deleted', 'function', true, true, false, false, 'proj'), \
                ('proj::Keeper', 'Keeper', 'function', true, true, false, false, 'proj')")
            .unwrap();
        store
            .execute(
                "INSERT INTO CONTAINS (source_id, target_id) VALUES \
                ('/tmp/graph/deleted.rs', 'proj::Deleted'), \
                ('/tmp/graph/keeper.rs', 'proj::Keeper')",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO CALLS (source_id, target_id) VALUES ('proj::Keeper', 'proj::Deleted')",
            )
            .unwrap();

        let keeper_anchor = store
            .refresh_symbol_projection("Keeper", 1)
            .unwrap()
            .expect("anchor should resolve");
        store
            .execute(&format!(
                "INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) \
                 VALUES ('symbol', '{}', 1, 'graph-bge-small-en-v1.5-384', 'sig-keeper', '1', CAST([1.0] || repeat([0.0], 383) AS FLOAT[384]), 1000)",
                keeper_anchor
            ))
            .unwrap();

        let projection_before = store
            .query_graph_projection("symbol", &keeper_anchor, 1)
            .unwrap();
        assert!(projection_before.contains("proj::Deleted"));

        let affected = store
            .tombstone_missing_path(std::path::Path::new("/tmp/graph/deleted.rs"))
            .unwrap();
        assert_eq!(affected, 1);

        let projection_count = store
            .query_count(&format!(
                "SELECT count(*) FROM GraphProjection WHERE anchor_type = 'symbol' AND anchor_id = '{}'",
                keeper_anchor
            ))
            .unwrap();
        let state_count = store
            .query_count(&format!(
                "SELECT count(*) FROM GraphProjectionState WHERE anchor_type = 'symbol' AND anchor_id = '{}'",
                keeper_anchor
            ))
            .unwrap();
        let embedding_count = store
            .query_count(&format!(
                "SELECT count(*) FROM GraphEmbedding WHERE anchor_type = 'symbol' AND anchor_id = '{}'",
                keeper_anchor
            ))
            .unwrap();

        assert_eq!(projection_count, 0);
        assert_eq!(state_count, 0);
        assert_eq!(embedding_count, 0);
    }
}
