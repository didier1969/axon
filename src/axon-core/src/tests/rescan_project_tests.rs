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
            std::fs::write(&path, format!("// REQ-AXO-901676 fixture {idx}\nfn main() {{}}\n"))
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
            files_scheduled as usize, files.len(),
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
            .sync_project_registry_entry(
                &code,
                Some("rescan-full-fixture"),
                Some(&project_path),
            )
            .expect("register project");

        // Seed IndexedFile rows so we can assert the full sweep wipes them.
        let now_ms: i64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        for f in &files {
            let escaped = f.replace('\'', "''");
            store
                .execute_raw_sql_gateway(&format!(
                    "INSERT INTO public.IndexedFile (path, content_hash, last_seen_ms) \
                     VALUES ('{}', 'stale-hash', {}) \
                     ON CONFLICT (path) DO UPDATE SET content_hash = EXCLUDED.content_hash",
                    escaped, now_ms
                ))
                .expect("seed IndexedFile row");
        }

        // Pre-condition : rows present.
        let count_before = read_indexed_count(&store, &project_path);
        assert_eq!(count_before, files.len() as i64, "seed step must succeed");

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

        // Post-condition : IndexedFile rows wiped (forces full re-parse
        // on the next scanner pass).
        let count_after = read_indexed_count(&store, &project_path);
        assert_eq!(
            count_after, 0,
            "full=true must wipe IndexedFile rows under project_path"
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
        let escaped = project_path.replace('\'', "''");
        let raw = store
            .execute_raw_sql_gateway(&format!(
                "SELECT count(*) FROM public.IndexedFile WHERE path LIKE '{}/%'",
                escaped
            ))
            .expect("count indexed");
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
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
}
