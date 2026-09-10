// REQ-AXO-901676 — integration tests for the public MCP tool
// `rescan_project(project_code, full=false)`.
//
// The tool is the proportionate recovery surface for scenarios where the
// indexer's incremental state machine is suspected stale (git pull
// massif, backup restore, inotify drop, watcher crash). It must return
// in <500 ms with a `files_scheduled` count and `projection_eta_ms`
// estimate, and trigger an async re-scan via the existing
// `axon_registry_changed` NOTIFY plumbing (REQ-AXO-901675) so the
// indexer (when running) picks up the work without restart.
//
// Tests live behind `#[cfg(test)]` and require the dev PG (resolved
// from `AXON_DEV_DATABASE_URL`). They are skipped (early return Ok) on
// machines without a live dev PG so the test harness stays green.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::Value;

    use crate::mcp::McpServer;
    use crate::tests::test_helpers::{create_test_db, unique_test_scope};

    /// Build a temp project directory containing N source files so the
    /// scanner has something to enumerate. Returns the project root and
    /// the list of file paths created.
    fn make_temp_project(label: &str, file_count: usize) -> (std::path::PathBuf, Vec<String>) {
        let scope = unique_test_scope(label);
        let root = std::env::temp_dir().join(format!("rescan-{scope}"));
        std::fs::create_dir_all(&root).expect("create project root");
        let mut files = Vec::with_capacity(file_count);
        for idx in 0..file_count {
            let path = root.join(format!("file_{idx}.rs"));
            std::fs::write(
                &path,
                format!("// REQ-AXO-901676 fixture {idx}\nfn main() {{}}\n"),
            )
            .expect("write fixture file");
            files.push(path.to_string_lossy().to_string());
        }
        (root, files)
    }

    fn parse_structured(envelope: &Value) -> Value {
        envelope
            .get("structuredContent")
            .cloned()
            .unwrap_or_else(|| Value::Null)
    }

    #[test]
    fn rescan_project_delta_default_returns_files_scheduled_and_eta() {
        let store = Arc::new(create_test_db().expect("create test db"));
        let server = McpServer::new(store.clone());

        let (root, files) = make_temp_project("delta", 3);
        let scope = unique_test_scope("rpd");
        let code = three_char_code_from_scope(&scope);
        store
            .sync_project_registry_entry(
                &code,
                Some("rescan-delta-fixture"),
                Some(root.to_string_lossy().as_ref()),
            )
            .expect("register project");

        let args = serde_json::json!({ "project_code": code });
        let envelope = server
            .axon_rescan_project(&args)
            .expect("rescan_project must return Some envelope");
        let payload = parse_structured(&envelope);

        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("ok"),
            "envelope status: {envelope}"
        );
        let files_scheduled = payload
            .get("files_scheduled")
            .and_then(|v| v.as_u64())
            .expect("files_scheduled field");
        assert_eq!(
            files_scheduled as usize,
            files.len(),
            "files_scheduled must match enumerated count"
        );
        assert!(
            payload
                .get("projection_eta_ms")
                .and_then(|v| v.as_u64())
                .is_some(),
            "projection_eta_ms field missing"
        );
        assert_eq!(
            payload.get("project_code").and_then(|v| v.as_str()),
            Some(code.as_str())
        );
        assert_eq!(
            payload.get("mode").and_then(|v| v.as_str()),
            Some("delta"),
            "default mode must be delta"
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rescan_project_full_true_invalidates_indexed_file_rows() {
        let store = Arc::new(create_test_db().expect("create test db"));
        let server = McpServer::new(store.clone());

        let (root, files) = make_temp_project("full", 2);
        let scope = unique_test_scope("rpf");
        let code = three_char_code_from_scope(&scope);
        let project_path = root.to_string_lossy().to_string();
        store
            .sync_project_registry_entry(&code, Some("rescan-full-fixture"), Some(&project_path))
            .expect("register project");

        // Seed IndexedFile rows carrying a `stale-hash` so we can assert the
        // full sweep INVALIDATES the cache (drops that hash + re-enrols the
        // files as `discovered`), forcing a full re-parse.
        // REQ-AXO-901860 — IndexedFile.project_code is a NOT NULL FK to
        // axon.Project, so the parent row + an explicit project_code are
        // required (the legacy seed omitted both and broke post-901860).
        let safe_code = code.replace('\'', "''");
        store
            .execute_raw_sql_gateway(&format!(
                "INSERT INTO axon.Project (code) VALUES ('{}') ON CONFLICT (code) DO NOTHING",
                safe_code
            ))
            .expect("seed axon.Project parent");
        let now_ms: i64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        for f in &files {
            let escaped = f.replace('\'', "''");
            store
                .execute_raw_sql_gateway(&format!(
                    "INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms) \
                     VALUES ('{}', '{}', 'stale-hash', {}) \
                     ON CONFLICT (path) DO UPDATE SET content_hash = EXCLUDED.content_hash",
                    escaped, safe_code, now_ms
                ))
                .expect("seed IndexedFile row");
        }

        // REQ-AXO-902505 — a broad registry path may contain rows owned by a
        // more-specific tenant. A full rescan must invalidate by project_code,
        // never by path prefix alone.
        let foreign_scope = unique_test_scope("rpf-foreign");
        let foreign_code = three_char_code_from_scope(&foreign_scope);
        let safe_foreign_code = foreign_code.replace('\'', "''");
        let foreign_path = root.join("nested-tenant-only-in-index.rs");
        let escaped_foreign_path = foreign_path.to_string_lossy().replace('\'', "''");
        store
            .execute_raw_sql_gateway(&format!(
                "INSERT INTO axon.Project (code) VALUES ('{}') ON CONFLICT (code) DO NOTHING",
                safe_foreign_code
            ))
            .expect("seed foreign axon.Project parent");
        store
            .execute_raw_sql_gateway(&format!(
                "INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms) \
                 VALUES ('{}', '{}', 'foreign-hash', {})",
                escaped_foreign_path, safe_foreign_code, now_ms
            ))
            .expect("seed foreign IndexedFile row under the same path prefix");

        // Pre-condition : rows present.
        let count_before = read_indexed_count(&store, &project_path);
        assert_eq!(
            count_before,
            files.len() as i64 + 1,
            "seed step must include the foreign tenant row"
        );

        let args = serde_json::json!({ "project_code": code, "full": true });
        let envelope = server
            .axon_rescan_project(&args)
            .expect("rescan_project must return Some envelope");
        let payload = parse_structured(&envelope);

        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("ok"),
            "envelope: {envelope}"
        );
        assert_eq!(
            payload.get("mode").and_then(|v| v.as_str()),
            Some("full"),
            "mode must reflect full=true"
        );
        let files_scheduled = payload
            .get("files_scheduled")
            .and_then(|v| v.as_u64())
            .expect("files_scheduled field");
        assert_eq!(files_scheduled as usize, files.len());

        // Post-condition — `full=true` INVALIDATES the IndexedFile cache so
        // the next pipeline-A pass re-parses every file. Since REQ-AXO-901893
        // (LEGACY FEED PURGE) the tool no longer leaves the rows deleted: step 2
        // wipes them (dropping the cached `content_hash`), then step 4 runs a
        // synchronous scanner walk that re-enrols every on-disk file into the
        // durable work queue as `status='discovered'` with a blank
        // `content_hash`. The DBQ-A claim feeder (REQ-AXO-901897) drains those
        // rows into pipeline A by construction. The observable invalidation
        // contract is therefore: the seeded `stale-hash` is gone AND the rows
        // are back as `discovered` (enqueued for re-parse) — NOT row-absence,
        // which only held under the pre-901893 async-NOTIFY design.
        let stale_hash_rows =
            read_count_where(&store, &project_path, "content_hash = 'stale-hash'");
        assert_eq!(
            stale_hash_rows, 0,
            "full=true must invalidate the cached content_hash (seeded 'stale-hash' must be gone)"
        );
        let discovered_rows = read_count_where(
            &store,
            &project_path,
            &format!("status = 'discovered' AND project_code = '{}'", safe_code),
        );
        assert_eq!(
            discovered_rows,
            files.len() as i64,
            "full=true must re-enrol every file as status='discovered' for re-parse"
        );
        let foreign_rows = read_count_where(
            &store,
            &project_path,
            &format!("project_code = '{}'", safe_foreign_code),
        );
        assert_eq!(
            foreign_rows, 1,
            "full=true must preserve another tenant even when its path shares the prefix"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rescan_project_unknown_code_returns_structured_error() {
        let store = Arc::new(create_test_db().expect("create test db"));
        let server = McpServer::new(store);

        let args = serde_json::json!({ "project_code": "ZZ9" });
        let envelope = server
            .axon_rescan_project(&args)
            .expect("rescan_project must return Some envelope even for unknown code");

        assert_eq!(
            envelope.get("isError").and_then(|v| v.as_bool()),
            Some(true),
            "envelope: {envelope}"
        );
        let payload = parse_structured(&envelope);
        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("error"),
            "structured status must be 'error': {envelope}"
        );
    }

    #[test]
    fn rescan_project_missing_project_code_returns_structured_error() {
        let store = Arc::new(create_test_db().expect("create test db"));
        let server = McpServer::new(store);

        let args = serde_json::json!({});
        let envelope = server
            .axon_rescan_project(&args)
            .expect("rescan_project must return Some envelope even when arg missing");

        assert_eq!(
            envelope.get("isError").and_then(|v| v.as_bool()),
            Some(true),
            "envelope: {envelope}"
        );
        let payload = parse_structured(&envelope);
        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("error"),
            "envelope: {envelope}"
        );
    }

    /// Read the IndexedFile count for rows whose `path` is under the
    /// supplied project_path prefix.
    fn read_indexed_count(store: &crate::graph::GraphStore, project_path: &str) -> i64 {
        read_count_where(store, project_path, "TRUE")
    }

    /// Read the IndexedFile count for rows under `project_path` that also
    /// satisfy `extra_predicate` (a raw SQL boolean expression). Used to
    /// assert the `full=true` invalidation contract (stale hash cleared +
    /// rows re-enrolled as `discovered`).
    fn read_count_where(
        store: &crate::graph::GraphStore,
        project_path: &str,
        extra_predicate: &str,
    ) -> i64 {
        let escaped = project_path.replace('\'', "''");
        let raw = store
            .execute_raw_sql_gateway(&format!(
                "SELECT count(*) FROM ist.IndexedFile WHERE path LIKE '{}/%' AND ({})",
                escaped, extra_predicate
            ))
            .expect("count indexed");
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.first()
            .and_then(|r| r.first())
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0)
    }

    /// Hash a unique scope tag into a 3-char [A-Z0-9] code that passes
    /// `project_meta::is_valid_project_code`. Mirrors the helper used by
    /// `registry_notify_integration_tests`.
    fn three_char_code_from_scope(scope: &str) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let mut hash: u64 = 1469598103934665603;
        for b in scope.bytes() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(1099511628211);
        }
        let mut out = String::with_capacity(3);
        for i in 0..3 {
            let idx = ((hash >> (i * 12)) as usize) % ALPHABET.len();
            out.push(ALPHABET[idx] as char);
        }
        out
    }

    /// REQ-AXO-902512 — rescan_project in delta mode (full=false) must detect files enrolled in
    /// ist.IndexedFile that have NO chunks in ist.Chunk (and no terminal policy skip like minified/empty),
    /// invalidate their cache/hash, and re-enrol them as status='discovered'.
    #[test]
    fn rescan_project_delta_reconciles_enrolled_files_without_chunks_902512() {
        let store = Arc::new(create_test_db().expect("create test db"));
        let server = McpServer::new(store.clone());

        let (root, files) = make_temp_project("delta_rec", 3);
        let scope = unique_test_scope("rpdr");
        let code = three_char_code_from_scope(&scope);
        let project_path = root.to_string_lossy().to_string();
        store
            .sync_project_registry_entry(
                &code,
                Some("rescan-delta-chunkless-fixture"),
                Some(&project_path),
            )
            .expect("register project");

        let safe_code = code.replace('\'', "''");
        store
            .execute_raw_sql_gateway(&format!(
                "INSERT INTO axon.Project (code) VALUES ('{}') ON CONFLICT (code) DO NOTHING",
                safe_code
            ))
            .expect("seed axon.Project parent");

        let now_ms: i64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        // Seed 3 IndexedFile rows:
        // files[0]: has a chunk in ist.Chunk (healthy)
        // files[1]: 0 chunk in ist.Chunk (chunkless hole, e.g. KKI scenario)
        // files[2]: 0 chunk in ist.Chunk, status='skipped', skip_reason='parse_timeout' (transient failure)
        for (i, f) in files.iter().enumerate() {
            let escaped = f.replace('\'', "''");
            let md = std::fs::metadata(f).expect("file metadata");
            let mtime_sec = md
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let mtime_ms = mtime_sec * 1000;
            let size_bytes = md.len() as i64;
            let (status, reason) = if i == 2 {
                ("skipped", "'parse_timeout'")
            } else {
                ("indexed", "NULL")
            };
            store
                .execute_raw_sql_gateway(&format!(
                    "INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms, status, skip_reason, mtime_ms, size_bytes) \
                     VALUES ('{}', '{}', 'hash-{}', {}, '{}', {}, {}, {}) \
                     ON CONFLICT (path) DO UPDATE SET content_hash = EXCLUDED.content_hash, status = EXCLUDED.status, skip_reason = EXCLUDED.skip_reason, mtime_ms = EXCLUDED.mtime_ms, size_bytes = EXCLUDED.size_bytes",
                    escaped, safe_code, i, now_ms, status, reason, mtime_ms, size_bytes
                ))
                .expect("seed IndexedFile row");
        }

        // Seed a chunk only for files[0]
        let escaped_f0 = files[0].replace('\'', "''");
        store
            .execute_raw_sql_gateway(&format!(
                "INSERT INTO ist.Chunk \
                 (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line, chunk_part_index, chunk_part_count, chunk_path) VALUES \
                 ('c0','symbol','s0','{}','{}','fn','content','chash0',1,10,1,1,'1/1')",
                safe_code, escaped_f0
            ))
            .expect("seed chunk for files[0]");

        let args = serde_json::json!({ "project_code": code, "full": false });
        let envelope = server
            .axon_rescan_project(&args)
            .expect("rescan_project must return Some envelope");
        let payload = parse_structured(&envelope);

        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("ok")
        );
        assert_eq!(
            payload.get("mode").and_then(|v| v.as_str()),
            Some("delta")
        );

        // Invalidation in delta mode MUST report the 2 uncovered files
        let invalidated = payload
            .get("invalidated_rows")
            .and_then(|v| v.as_u64())
            .expect("invalidated_rows must be present in payload");
        assert_eq!(
            invalidated, 2,
            "delta mode must invalidate the 2 chunkless files"
        );

        let cache_inv = payload
            .get("cache_invalidation")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            cache_inv.contains("delta") && cache_inv.contains("2"),
            "cache_invalidation must describe delta invalidation of 2 files: {cache_inv}"
        );

        // Post-condition:
        // files[0] has its content_hash intact ('hash-0')
        let f0_intact = read_count_where(
            &store,
            &project_path,
            &format!("content_hash = 'hash-0' AND path = '{}'", escaped_f0),
        );
        assert_eq!(f0_intact, 1, "files[0] with chunk must remain untouched");

        // files[1] and files[2] have been re-enrolled with status='discovered' and empty content_hash
        let discovered_count = read_count_where(
            &store,
            &project_path,
            &format!("status = 'discovered' AND project_code = '{}'", safe_code),
        );
        assert_eq!(
            discovered_count, 2,
            "both chunkless files must be re-enrolled as discovered"
        );

        let _ = store.execute_raw_sql_gateway(&format!(
            "DELETE FROM ist.Chunk WHERE file_path LIKE '{}/%'",
            project_path.replace('\'', "''")
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    // =========================================================================
    // REQ-AXO-902613 — Targeted re-indexing by path
    // =========================================================================

    #[test]
    fn rescan_project_targeted_paths_invalidates_only_specified_files_902613() {
        let store = Arc::new(create_test_db().expect("create test db"));
        let server = McpServer::new(store.clone());

        let (root, files) = make_temp_project("targeted", 3);
        let scope = unique_test_scope("rpt");
        let code = three_char_code_from_scope(&scope);
        let project_path = root.to_string_lossy().to_string();
        store
            .sync_project_registry_entry(
                &code,
                Some("rescan-targeted-fixture"),
                Some(&project_path),
            )
            .expect("register project");

        let safe_code = code.replace('\'', "''");
        store
            .execute_raw_sql_gateway(&format!(
                "INSERT INTO axon.Project (code) VALUES ('{}') ON CONFLICT (code) DO NOTHING",
                safe_code
            ))
            .expect("seed axon.Project parent");

        let now_ms: i64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        // Seed 3 IndexedFile rows with stale-hash
        for (idx, f) in files.iter().enumerate() {
            let escaped = f.replace('\'', "''");
            let md = std::fs::metadata(f).expect("file metadata");
            let mtime_ms = md
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let size_bytes = md.len() as i64;
            store
                .execute_raw_sql_gateway(&format!(
                    "INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms, status, mtime_ms, size_bytes) \
                     VALUES ('{}', '{}', 'stale-hash-{}', {}, 'indexed', {}, {}) \
                     ON CONFLICT (path) DO UPDATE SET content_hash = EXCLUDED.content_hash, status = EXCLUDED.status",
                    escaped, safe_code, idx, now_ms, mtime_ms, size_bytes
                ))
                .expect("seed IndexedFile row");
        }

        // Also seed a foreign tenant file under the same prefix
        let foreign_scope = unique_test_scope("rpt-foreign");
        let foreign_code = three_char_code_from_scope(&foreign_scope);
        let safe_foreign_code = foreign_code.replace('\'', "''");
        let foreign_path = root.join("foreign-tenant.rs");
        std::fs::write(&foreign_path, "// foreign fixture\n").expect("write foreign fixture");
        let escaped_foreign_path = foreign_path.to_string_lossy().replace('\'', "''");
        store
            .execute_raw_sql_gateway(&format!(
                "INSERT INTO axon.Project (code) VALUES ('{}') ON CONFLICT (code) DO NOTHING",
                safe_foreign_code
            ))
            .expect("seed foreign axon.Project parent");
        store
            .execute_raw_sql_gateway(&format!(
                "INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms, status) \
                 VALUES ('{}', '{}', 'foreign-stale-hash', {}, 'indexed')",
                escaped_foreign_path, safe_foreign_code, now_ms
            ))
            .expect("seed foreign IndexedFile row");

        // Target ONLY files[0] and files[1]
        let targeted_paths = vec![files[0].clone(), files[1].clone()];
        let args = serde_json::json!({
            "project_code": code,
            "paths": targeted_paths,
        });

        let envelope = server
            .axon_rescan_project(&args)
            .expect("rescan_project must return Some envelope");
        let payload = parse_structured(&envelope);

        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("ok"),
            "envelope: {envelope}"
        );
        assert_eq!(
            payload.get("mode").and_then(|v| v.as_str()),
            Some("targeted"),
            "mode must be targeted"
        );
        assert_eq!(
            payload.get("paths_targeted").and_then(|v| v.as_u64()),
            Some(2),
            "paths_targeted must match input paths length"
        );
        assert_eq!(
            payload.get("files_scheduled").and_then(|v| v.as_u64()),
            Some(2),
            "files_scheduled must match scheduled count"
        );

        // Verification of targeted invalidation & enrolment:
        // files[0] and files[1] must have content_hash cleared and status = 'discovered'
        let escaped_f0 = files[0].replace('\'', "''");
        let escaped_f1 = files[1].replace('\'', "''");
        let escaped_f2 = files[2].replace('\'', "''");

        let f0_status = store
            .execute_raw_sql_gateway(&format!(
                "SELECT status, content_hash FROM ist.IndexedFile WHERE path = '{escaped_f0}'"
            ))
            .expect("read f0");
        assert!(
            f0_status.contains("discovered") && !f0_status.contains("stale-hash-0"),
            "f0 must be discovered with cleared hash: {f0_status}"
        );

        let f1_status = store
            .execute_raw_sql_gateway(&format!(
                "SELECT status, content_hash FROM ist.IndexedFile WHERE path = '{escaped_f1}'"
            ))
            .expect("read f1");
        assert!(
            f1_status.contains("discovered") && !f1_status.contains("stale-hash-1"),
            "f1 must be discovered with cleared hash: {f1_status}"
        );

        // files[2] must remain COMPLETELY UNTOUCHED: status='indexed', content_hash='stale-hash-2'
        let f2_status = store
            .execute_raw_sql_gateway(&format!(
                "SELECT status, content_hash FROM ist.IndexedFile WHERE path = '{escaped_f2}'"
            ))
            .expect("read f2");
        assert!(
            f2_status.contains("indexed") && f2_status.contains("stale-hash-2"),
            "f2 must be untouched: {f2_status}"
        );

        // Foreign tenant row must remain COMPLETELY UNTOUCHED
        let foreign_status = store
            .execute_raw_sql_gateway(&format!(
                "SELECT status, content_hash FROM ist.IndexedFile WHERE path = '{escaped_foreign_path}'"
            ))
            .expect("read foreign");
        assert!(
            foreign_status.contains("foreign-stale-hash"),
            "foreign file must be untouched: {foreign_status}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rescan_project_targeted_relative_paths_resolves_and_invalidates_902613() {
        let store = Arc::new(create_test_db().expect("create test db"));
        let server = McpServer::new(store.clone());

        let (root, files) = make_temp_project("targeted_rel", 2);
        let scope = unique_test_scope("rpr");
        let code = three_char_code_from_scope(&scope);
        let project_path = root.to_string_lossy().to_string();
        store
            .sync_project_registry_entry(
                &code,
                Some("rescan-targeted-rel-fixture"),
                Some(&project_path),
            )
            .expect("register project");

        let safe_code = code.replace('\'', "''");
        store
            .execute_raw_sql_gateway(&format!(
                "INSERT INTO axon.Project (code) VALUES ('{}') ON CONFLICT (code) DO NOTHING",
                safe_code
            ))
            .expect("seed axon.Project parent");

        for (idx, f) in files.iter().enumerate() {
            let escaped = f.replace('\'', "''");
            store
                .execute_raw_sql_gateway(&format!(
                    "INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms, status) \
                     VALUES ('{}', '{}', 'stale-hash-{}', 1000, 'indexed')",
                    escaped, safe_code, idx
                ))
                .expect("seed IndexedFile row");
        }

        // Relative path: "file_0.rs"
        let args = serde_json::json!({
            "project_code": code,
            "paths": ["file_0.rs"],
        });

        let envelope = server
            .axon_rescan_project(&args)
            .expect("rescan_project must return Some envelope");
        let payload = parse_structured(&envelope);

        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("ok")
        );
        assert_eq!(
            payload.get("mode").and_then(|v| v.as_str()),
            Some("targeted")
        );
        assert_eq!(
            payload.get("paths_targeted").and_then(|v| v.as_u64()),
            Some(1)
        );
        assert_eq!(
            payload.get("files_scheduled").and_then(|v| v.as_u64()),
            Some(1)
        );

        let escaped_f0 = files[0].replace('\'', "''");
        let escaped_f1 = files[1].replace('\'', "''");

        let f0_status = store
            .execute_raw_sql_gateway(&format!(
                "SELECT status, content_hash FROM ist.IndexedFile WHERE path = '{escaped_f0}'"
            ))
            .expect("read f0");
        assert!(f0_status.contains("discovered") && !f0_status.contains("stale-hash-0"));

        let f1_status = store
            .execute_raw_sql_gateway(&format!(
                "SELECT status, content_hash FROM ist.IndexedFile WHERE path = '{escaped_f1}'"
            ))
            .expect("read f1");
        assert!(f1_status.contains("indexed") && f1_status.contains("stale-hash-1"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rescan_project_targeted_path_outside_project_returns_structured_error_902613() {
        let store = Arc::new(create_test_db().expect("create test db"));
        let server = McpServer::new(store.clone());

        let (root, _files) = make_temp_project("targeted_out", 1);
        let scope = unique_test_scope("rpo");
        let code = three_char_code_from_scope(&scope);
        let project_path = root.to_string_lossy().to_string();
        store
            .sync_project_registry_entry(
                &code,
                Some("rescan-targeted-out-fixture"),
                Some(&project_path),
            )
            .expect("register project");

        let args = serde_json::json!({
            "project_code": code,
            "paths": ["/etc/passwd"],
        });

        let envelope = server
            .axon_rescan_project(&args)
            .expect("rescan_project must return Some envelope");

        assert_eq!(
            envelope.get("isError").and_then(|v| v.as_bool()),
            Some(true),
            "envelope: {envelope}"
        );
        let payload = parse_structured(&envelope);
        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("error")
        );
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("path_outside_project")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rescan_project_targeted_path_traversal_returns_structured_error_902613() {
        let store = Arc::new(create_test_db().expect("create test db"));
        let server = McpServer::new(store.clone());

        let (root, _files) = make_temp_project("targeted_trav", 1);
        let scope = unique_test_scope("rptr");
        let code = three_char_code_from_scope(&scope);
        let project_path = root.to_string_lossy().to_string();
        store
            .sync_project_registry_entry(
                &code,
                Some("rescan-targeted-trav-fixture"),
                Some(&project_path),
            )
            .expect("register project");

        let args = serde_json::json!({
            "project_code": code,
            "paths": ["../../outside.rs"],
        });

        let envelope = server
            .axon_rescan_project(&args)
            .expect("rescan_project must return Some envelope");

        assert_eq!(
            envelope.get("isError").and_then(|v| v.as_bool()),
            Some(true),
            "envelope: {envelope}"
        );
        let payload = parse_structured(&envelope);
        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("error")
        );
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("path_outside_project")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rescan_project_targeted_exceeding_max_paths_returns_error_902613() {
        let store = Arc::new(create_test_db().expect("create test db"));
        let server = McpServer::new(store.clone());

        let (root, _files) = make_temp_project("targeted_max", 1);
        let scope = unique_test_scope("rpex");
        let code = three_char_code_from_scope(&scope);
        let project_path = root.to_string_lossy().to_string();
        store
            .sync_project_registry_entry(
                &code,
                Some("rescan-targeted-max-fixture"),
                Some(&project_path),
            )
            .expect("register project");

        let excessive_paths: Vec<String> = (0..129).map(|i| format!("file_{i}.rs")).collect();
        let args = serde_json::json!({
            "project_code": code,
            "paths": excessive_paths,
        });

        let envelope = server
            .axon_rescan_project(&args)
            .expect("rescan_project must return Some envelope");

        assert_eq!(
            envelope.get("isError").and_then(|v| v.as_bool()),
            Some(true)
        );
        let payload = parse_structured(&envelope);
        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("error")
        );
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("too_many_paths")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rescan_project_targeted_empty_paths_returns_error_902613() {
        let store = Arc::new(create_test_db().expect("create test db"));
        let server = McpServer::new(store.clone());

        let (root, _files) = make_temp_project("targeted_empty", 1);
        let scope = unique_test_scope("rpem");
        let code = three_char_code_from_scope(&scope);
        let project_path = root.to_string_lossy().to_string();
        store
            .sync_project_registry_entry(
                &code,
                Some("rescan-targeted-empty-fixture"),
                Some(&project_path),
            )
            .expect("register project");

        let args = serde_json::json!({
            "project_code": code,
            "paths": [],
        });

        let envelope = server
            .axon_rescan_project(&args)
            .expect("rescan_project must return Some envelope");

        assert_eq!(
            envelope.get("isError").and_then(|v| v.as_bool()),
            Some(true)
        );
        let payload = parse_structured(&envelope);
        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("error")
        );
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("empty_paths")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rescan_project_targeted_path_not_found_returns_error_902613() {
        let store = Arc::new(create_test_db().expect("create test db"));
        let server = McpServer::new(store.clone());

        let (root, _files) = make_temp_project("targeted_nf", 1);
        let scope = unique_test_scope("rpnf");
        let code = three_char_code_from_scope(&scope);
        let project_path = root.to_string_lossy().to_string();
        store
            .sync_project_registry_entry(
                &code,
                Some("rescan-targeted-nf-fixture"),
                Some(&project_path),
            )
            .expect("register project");

        let args = serde_json::json!({
            "project_code": code,
            "paths": ["does_not_exist.rs"],
        });

        let envelope = server
            .axon_rescan_project(&args)
            .expect("rescan_project must return Some envelope");

        assert_eq!(
            envelope.get("isError").and_then(|v| v.as_bool()),
            Some(true)
        );
        let payload = parse_structured(&envelope);
        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("error")
        );
        assert_eq!(
            payload.get("code").and_then(|v| v.as_str()),
            Some("path_not_found")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// REQ-AXO-902655 (Feedback #425 DVM) — rescan_project must:
    /// 1. Auto-repair missing parent in axon.Project so ist.IndexedFile FK constraint is satisfied.
    /// 2. Measure actual persisted files: scanner must not report `enrolled:N` when DB insertion fails.
    #[test]
    fn rescan_project_ensures_axon_project_parent_exists_and_measures_enrolled_count() {
        let store = Arc::new(create_test_db().expect("create test db"));
        let server = McpServer::new(store.clone());

        let (root, files) = make_temp_project("fk_parent", 3);
        let scope = unique_test_scope("rpfk");
        let code = three_char_code_from_scope(&scope);
        let project_path = root.to_string_lossy().to_string();

        // 1. Enregistrer dans soll.ProjectCodeRegistry
        store
            .sync_project_registry_entry(
                &code,
                Some("rescan-fk-parent-fixture"),
                Some(&project_path),
            )
            .expect("register project");

        // Neutraliser l'auto-seed du harnais de test pour refléter la stricte réalité de production
        crate::test_support::test_db::neutraliser_autoseed_des_parents_fk(&store).unwrap();

        // 2. Simuler la désynchronisation : supprimer le parent dans axon.Project
        // (comme c'était le cas pour DVM, SWT, MRG, OPO avant la réconciliation).
        store.execute(&format!("DELETE FROM axon.Project WHERE code = '{code}'")).unwrap();

        let count_before = store.query_count(&format!("SELECT count(*) FROM axon.Project WHERE code = '{code}'")).unwrap();
        assert_eq!(count_before, 0, "parent must be absent before rescan");

        // 3. Appeler axon_rescan_project
        let args = serde_json::json!({
            "project_code": code,
            "full": true,
        });

        let envelope = server
            .axon_rescan_project(&args)
            .expect("rescan_project must return Some envelope");

        let payload = parse_structured(&envelope);
        assert_eq!(payload.get("status").and_then(|v| v.as_str()), Some("ok"));

        // Critère 1 : Le parent axon.Project DOIT avoir été restauré
        let count_parent = store.query_count(&format!("SELECT count(*) FROM axon.Project WHERE code = '{code}'")).unwrap();
        assert_eq!(count_parent, 1, "axon.Project parent row must be ensured by rescan_project");

        // Critère 2 : Les fichiers doivent être réellement présents dans ist.IndexedFile
        let indexed_count = store.query_count(&format!("SELECT count(*) FROM ist.IndexedFile WHERE project_code = '{code}'")).unwrap();
        assert_eq!(indexed_count as usize, files.len(), "actual rows in ist.IndexedFile must match files count");

        // Critère 3 : notify_outcome doit rapporter la vérité mesurée
        let notify_outcome = payload.get("notify_outcome").and_then(|v| v.as_str()).unwrap();
        assert_eq!(notify_outcome, format!("enrolled:{}", files.len()));

        let _ = std::fs::remove_dir_all(&root);
    }
}

