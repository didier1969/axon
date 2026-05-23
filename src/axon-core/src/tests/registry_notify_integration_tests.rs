// REQ-AXO-901675 (PIL-AXO-008) — integration tests for the
// `axon_registry_changed` LISTEN/NOTIFY plumbing.
//
// The tests below open a `tokio_postgres` client to the dev PG, issue
// `LISTEN axon_registry_changed`, mutate `soll.ProjectCodeRegistry` via
// the canonical helper, and assert that a notification arrives carrying
// the expected payload. This verifies the trigger installed by
// `db/ddl/07_registry_notify.sql` is wired correctly end-to-end.
//
// Skipped (returning Ok early) when `AXON_DEV_DATABASE_URL` is not set,
// so the test harness stays green on machines without a live dev PG.

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::tests::test_helpers::{create_test_db, unique_test_scope};
    use futures_util::stream::StreamExt;
    use tokio_postgres::{AsyncMessage, NoTls};

    fn dev_database_url() -> Option<String> {
        std::env::var("AXON_DEV_DATABASE_URL").ok()
    }

    /// Hash the unique scope tag into a 3-char [A-Z0-9] code that passes
    /// `project_meta::is_valid_project_code`. Collisions across parallel
    /// tests are extremely unlikely given the nanos suffix entropy in the
    /// scope tag ; if one occurs, the registry UPSERT path keeps a single
    /// row anyway and the assertion still matches the probe_code.
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

    #[tokio::test(flavor = "current_thread")]
    async fn registry_insert_emits_pg_notify_axon_registry_changed() {
        let Some(url) = dev_database_url() else {
            return;
        };
        let store = create_test_db().expect("create test db");

        // 1. Open dedicated listener connection.
        let (client, mut connection) = tokio_postgres::connect(&url, NoTls)
            .await
            .expect("connect listener");
        let (notify_tx, mut notify_rx) =
            tokio::sync::mpsc::channel::<tokio_postgres::Notification>(64);
        let driver = tokio::spawn(async move {
            let stream =
                futures_util::stream::poll_fn(move |cx| connection.poll_message(cx));
            tokio::pin!(stream);
            while let Some(msg) = stream.next().await {
                if let Ok(AsyncMessage::Notification(n)) = msg {
                    if notify_tx.send(n).await.is_err() {
                        return;
                    }
                }
            }
        });
        client
            .batch_execute("LISTEN axon_registry_changed")
            .await
            .expect("LISTEN");

        // 2. Mutate the registry via canonical helper. Use a unique
        //    project_code so we don't race with parallel tests on the
        //    shared dev PG.
        // is_valid_project_code requires EXACTLY 3 ASCII alphanumeric chars.
        // Derive a deterministic-per-invocation 3-char code from the unique
        // scope tag so we don't collide with parallel tests.
        let scope = unique_test_scope("rni");
        let probe_code = three_char_code_from_scope(&scope);
        let probe_path = format!("/tmp/{}-registry-notify", scope);
        store
            .sync_project_registry_entry(
                &probe_code,
                Some("registry-notify-fixture"),
                Some(&probe_path),
            )
            .expect("sync_project_registry_entry");

        // 3. Drain notifications until we see one matching our probe.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut matched = None;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(500), notify_rx.recv())
                .await
            {
                Ok(Some(n)) if n.channel() == "axon_registry_changed" => {
                    if n.payload().contains(&probe_code) {
                        matched = Some(n.payload().to_string());
                        break;
                    }
                }
                _ => {}
            }
        }
        drop(client);
        drop(driver);

        let payload = matched.unwrap_or_else(|| {
            panic!(
                "did not receive axon_registry_changed notification for {probe_code} \
                 within 5s — trigger or LISTEN/NOTIFY wiring broken"
            )
        });
        assert!(
            payload.contains(&probe_code),
            "payload missing project_code: {payload}"
        );
        assert!(
            payload.contains(&probe_path),
            "payload missing project_path: {payload}"
        );
        assert!(
            payload.contains("insert") || payload.contains("update"),
            "payload missing op: {payload}"
        );
    }
}
