// REQ-AXO-902123 (MBX-11) — MAILBOX conformance harness.
//
// Golden valid-positive / valid-negative cases that pin the MAILBOX MVP wire
// contract end-to-end through the real MCP surface (`execute_tool_direct`) on a
// live PG TestDb clone:
//   C1 envelope well-formed        — A2A keys present; missing field → input_invalid
//   C2 HMAC integrity              — signature_verified=true; DB tamper → false
//   C3 dedup idempotent            — re-send same idempotency_key → deduped, count steady
//   C4 threading                   — context_id filters; cursor NOT advanced (view)
//   C5 cursor monotone             — unread advances; second unread → 0
//
// Reads route through the single PG pool (query_json == query_json_writer), so
// there is no reader/writer staleness on the clone — a send is immediately
// visible to the following read.

use super::*;

const FROM: &str = "PJA";
const TO: &str = "PJB";

fn send(server: &McpServer, args: Value) -> Value {
    server
        .execute_tool_direct("mcp_outbox_send", &args)
        .expect("mcp_outbox_send returns a result")
}

fn read(server: &McpServer, args: Value) -> Value {
    server
        .execute_tool_direct("mcp_inbox_read", &args)
        .expect("mcp_inbox_read returns a result")
}

/// Count of (non-archived) inbox rows for a recipient, straight from PG —
/// independent of the read cursor, so it is a stable dedup oracle.
fn inbox_count(server: &McpServer, to: &str) -> i64 {
    server
        .graph_store
        .query_single_i64_writer(&format!(
            "SELECT count(*) FROM axon.mailbox_message WHERE to_project='{to}' AND archived_at IS NULL"
        ))
        .ok()
        .flatten()
        .unwrap_or(-1)
}

// ── C1 — envelope well-formed (VP + VN) ────────────────────────────────────
#[test]
fn c1_envelope_wellformed_vp_and_missing_field_vn() {
    let server = create_test_server();

    // VP — a well-formed send is accepted and round-trips with the A2A keys.
    let sent = send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c1-k1",
            "subject": "hello", "body_dense": "ref SOLL-X",
            "context_id": "c1-thread"
        }),
    );
    assert_eq!(sent["data"]["status"].as_str(), Some("ok"));
    assert!(sent["data"]["message_id"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(sent["data"]["deduped"].as_bool(), Some(false));

    let inbox = read(&server, json!({ "project": TO, "mode": "all" }));
    let msgs = inbox["data"]["messages"].as_array().expect("messages array");
    assert_eq!(msgs.len(), 1, "exactly one message delivered");
    let m = &msgs[0];
    for key in ["message_id", "context_id", "from", "subject", "body_dense", "signature_verified"] {
        assert!(m.get(key).is_some(), "A2A envelope key `{key}` must be present");
    }
    assert_eq!(m["from"].as_str(), Some(FROM));
    assert_eq!(m["context_id"].as_str(), Some("c1-thread"));

    // VN — a missing required field (`to_project`) is rejected as input_invalid,
    // and so is a missing `idempotency_key`.
    let no_to = send(&server, json!({ "from": FROM, "idempotency_key": "c1-k2" }));
    assert_eq!(no_to["isError"].as_bool(), Some(true));
    assert_eq!(no_to["data"]["status"].as_str(), Some("input_invalid"));

    let no_idem = send(&server, json!({ "from": FROM, "to_project": TO }));
    assert_eq!(no_idem["isError"].as_bool(), Some(true));
    assert_eq!(no_idem["data"]["status"].as_str(), Some("input_invalid"));
}

// ── C2 — HMAC integrity (VP verified + VN tampered) ────────────────────────
#[test]
fn c2_hmac_verified_then_db_tamper_breaks_signature() {
    let server = create_test_server();
    send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c2-k1",
            "subject": "integrity", "body_dense": "original"
        }),
    );

    // VP — the freshly signed envelope verifies.
    let before = read(&server, json!({ "project": TO, "mode": "all" }));
    assert_eq!(
        before["data"]["messages"][0]["signature_verified"].as_bool(),
        Some(true),
        "a freshly signed message must verify"
    );

    // VN — tamper a canonical field (`body_dense`) directly in the store without
    // re-signing; the HMAC over the canonical envelope must now fail.
    server
        .graph_store
        .execute(&format!(
            "UPDATE axon.mailbox_message SET body_dense='EVIL' WHERE to_project='{TO}'"
        ))
        .expect("tamper update");

    let after = read(&server, json!({ "project": TO, "mode": "all" }));
    assert_eq!(
        after["data"]["messages"][0]["signature_verified"].as_bool(),
        Some(false),
        "a DB-tampered message must fail signature verification"
    );
}

// ── C3 — dedup idempotent (re-send is a no-op) ─────────────────────────────
#[test]
fn c3_resend_same_idempotency_key_is_deduped_no_op() {
    let server = create_test_server();
    let args = json!({
        "from": FROM, "to_project": TO,
        "idempotency_key": "c3-fixed",
        "subject": "dup", "body_dense": "once"
    });

    let first = send(&server, args.clone());
    assert_eq!(first["data"]["deduped"].as_bool(), Some(false), "first send delivers");
    assert_eq!(inbox_count(&server, TO), 1);

    // Re-send with the SAME (from, idempotency_key): idempotent no-op.
    let again = send(&server, args.clone());
    assert_eq!(again["data"]["deduped"].as_bool(), Some(true), "re-send is deduped");
    assert_eq!(
        again["data"]["message_id"].as_str(),
        first["data"]["message_id"].as_str(),
        "dedup yields the same stable message_id"
    );
    assert_eq!(inbox_count(&server, TO), 1, "row count is unchanged after re-send");
}

// ── C4 — threading: context_id filters, cursor not advanced ────────────────
#[test]
fn c4_context_id_filters_thread_and_does_not_advance_cursor() {
    let server = create_test_server();
    for (i, ctx) in [("t1a", "thread-1"), ("t1b", "thread-1"), ("t2a", "thread-2")] {
        send(
            &server,
            json!({
                "from": FROM, "to_project": TO,
                "idempotency_key": i, "context_id": ctx,
                "subject": ctx, "body_dense": i
            }),
        );
    }

    // A thread view returns only that thread …
    let thread1 = read(&server, json!({ "project": TO, "context_id": "thread-1" }));
    let msgs = thread1["data"]["messages"].as_array().expect("messages");
    assert_eq!(msgs.len(), 2, "thread-1 has exactly two messages");
    assert!(
        msgs.iter().all(|m| m["context_id"].as_str() == Some("thread-1")),
        "every returned message belongs to thread-1"
    );

    // … and is NON-destructive: the read cursor must not have been written.
    let cursor = server
        .graph_store
        .query_single_i64_writer(&format!(
            "SELECT count(*) FROM axon.mailbox_cursor WHERE project_code='{TO}'"
        ))
        .ok()
        .flatten()
        .unwrap_or(-1);
    assert_eq!(cursor, 0, "a thread view must not create/advance the read cursor");

    // Proof the cursor is still at floor 0: a fresh `unread` read sees all three.
    let unread = read(&server, json!({ "project": TO, "mode": "unread" }));
    assert_eq!(unread["data"]["count"].as_i64(), Some(3));
}

// ── C5 — cursor monotone: unread advances, second unread is empty ──────────
#[test]
fn c5_unread_advances_cursor_then_second_read_is_empty() {
    let server = create_test_server();
    for i in 0..3 {
        send(
            &server,
            json!({
                "from": FROM, "to_project": TO,
                "idempotency_key": format!("c5-{i}"),
                "subject": "seq", "body_dense": format!("m{i}")
            }),
        );
    }

    let first = read(&server, json!({ "project": TO, "mode": "unread" }));
    assert_eq!(first["data"]["count"].as_i64(), Some(3), "first unread drains all three");
    let cursor = first["data"]["cursor"].as_i64().unwrap_or(0);
    assert!(cursor > 0, "cursor advanced past floor");

    let second = read(&server, json!({ "project": TO, "mode": "unread" }));
    assert_eq!(second["data"]["count"].as_i64(), Some(0), "second unread sees nothing new");
    assert_eq!(
        second["data"]["cursor"].as_i64(),
        Some(cursor),
        "cursor is monotone — it does not regress on an empty read"
    );
}
