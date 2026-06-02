use super::GraphStore;

#[test]
fn brain_reader_only_refresh_opens_late_and_republished_ist_replica() {
    let temp = tempfile::tempdir().unwrap();
    let db_root = temp.path().join("graph_v2");
    std::fs::create_dir_all(&db_root).unwrap();
    let db_root_str = db_root.to_string_lossy().to_string();

    let brain = GraphStore::new_brain_reader_soll_writer(&db_root_str).unwrap();
    assert!(!brain.reader_snapshot_reader_available());
    assert!(matches!(
        brain.reader_snapshot_freshness_contract().state,
        crate::runtime_truth_contract::RuntimeFreshnessState::Degraded
    ));

    let indexer = GraphStore::new_indexer_ist_writer_without_soll(&db_root_str).unwrap();
    indexer
        .execute(
            "INSERT INTO ist.IndexedFile (path, content_hash, last_seen_ms)
             VALUES ('/tmp/late-reader.txt', 'hash-1', 1)",
        )
        .unwrap();
    indexer.refresh_reader_snapshot().unwrap();

    let refreshed = brain.refresh_reader_snapshot_if_needed().unwrap();
    assert!(refreshed, "brain should open the late-published IST reader");
    assert!(brain.reader_snapshot_reader_available());
    assert!(matches!(
        brain.reader_snapshot_freshness_contract().state,
        crate::runtime_truth_contract::RuntimeFreshnessState::Fresh
    ));
    let raw = brain
        .query_json_on_reader("SELECT count(*) FROM ist.IndexedFile")
        .unwrap();
    assert!(raw.contains("1"), "{raw}");

    indexer
        .execute(
            "INSERT INTO ist.IndexedFile (path, content_hash, last_seen_ms)
             VALUES ('/tmp/late-reader-2.txt', 'hash-2', 2)",
        )
        .unwrap();
    indexer.refresh_reader_snapshot().unwrap();
    let refreshed_again = brain.refresh_reader_snapshot_if_needed().unwrap();
    assert!(
        refreshed_again,
        "brain should reopen the IST reader after indexer republishes it"
    );
    let raw = brain
        .query_json_on_reader("SELECT count(*) FROM ist.IndexedFile")
        .unwrap();
    assert!(raw.contains("2"), "{raw}");

    let before = brain.reader_snapshot_diagnostics();
    brain
        .execute(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_name, project_path)
             VALUES ('PUPPY', 'PuppyGraph Roadmap Probe', '/tmp/puppy')
             ON CONFLICT (project_code) DO NOTHING",
        )
        .unwrap();
    let after = brain.reader_snapshot_diagnostics();
    assert_eq!(
        after.commit_epoch, before.commit_epoch,
        "SOLL writes in split brain mode must not create false IST reader lag"
    );
}
