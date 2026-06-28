// REQ-AXO-902143 (MBX réactivité niveau-2) — awareness piggyback.
//
// The unread mailbox *signal* is attached to every tool envelope at the central
// dispatch chokepoint so an actively-working session learns it has mail on its
// next tool call, instead of only at `status` / `axon_init_project`. These tests
// pin the targeting contract decided with the operator:
//   R1 no mail              → banner is None (the zero-token no-op fast path)
//   R2 recipient sees it    → Some{unread, from, pointer}, SIGNAL ONLY (no body)
//   R3 non-recipient blind  → a sibling project never sees another's mail
//   R4 envelope injection   → generic tool gains data.mailbox + a text line
//   R5 SKIP surfaces        → status/inbox_read keep a clean envelope
//   R6 read clears it        → once the cursor advances, the banner disappears

use super::*;

const FROM: &str = "PJA";
const TO: &str = "PJB";

fn send(server: &McpServer, args: Value) -> Value {
    server
        .execute_tool_direct("mcp_outbox_send", &args)
        .expect("mcp_outbox_send returns a result")
}

fn send_one(server: &McpServer, idem: &str, subject: &str) {
    let sent = send(
        server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": idem,
            "subject": subject, "body_dense": "ref SOLL-X"
        }),
    );
    assert_eq!(sent["data"]["status"].as_str(), Some("ok"), "send must succeed");
}

// ── R1 / R2 / R3 — targeting + signal-only ─────────────────────────────────
#[test]
fn r1_r2_r3_banner_targets_recipient_signal_only() {
    let server = create_test_server();

    // R1 — empty inbox → no banner, no token cost.
    assert!(
        server.mailbox_unread_banner(TO).is_none(),
        "no mail must yield no banner (zero-token no-op)"
    );

    send_one(&server, "r2-k1", "hello");
    send_one(&server, "r2-k2", "again");

    // R2 — the recipient gets the signal: count + sender, and NO message body.
    let banner = server
        .mailbox_unread_banner(TO)
        .expect("recipient with mail must get a banner");
    assert_eq!(banner["unread"].as_i64(), Some(2), "counts both unread messages");
    let from = banner["from"].as_array().expect("from is an array");
    assert_eq!(from.len(), 1, "one distinct sender");
    assert_eq!(from[0].as_str(), Some(FROM));
    assert!(banner["latest_id"].as_i64().unwrap_or(0) > 0, "pointer carries newest id");
    assert_eq!(
        banner["pointer"]["tool"].as_str(),
        Some("mcp_inbox_read"),
        "pointer routes to the explicit pull"
    );
    // REQ-AXO-902145 — no dead-end (PIL-AXO-002) : the banner must carry the
    // recovery for a stale client binding (the read tool missing from the
    // session's catalogue) so "N non-lus" is never a terminal state.
    assert!(
        banner["on_tool_absent"].as_str().unwrap_or("").contains("reconnect"),
        "banner must tell a stale client how to recover (reconnect MCP)"
    );
    assert!(
        banner["banner"].as_str().unwrap_or("").contains("reconnecte"),
        "human banner line must name the reconnect recovery"
    );
    // SIGNAL ONLY — the body must never leak into the banner.
    let serialized = serde_json::to_string(&banner).unwrap();
    assert!(
        !serialized.contains("ref SOLL-X"),
        "banner must not inline the message body"
    );

    // R3 — a non-recipient project sees nothing of TO's mail.
    assert!(
        server.mailbox_unread_banner(FROM).is_none(),
        "non-recipient project must never see another project's mail"
    );
}

// ── R4 / R5 — envelope injection vs skip surfaces ──────────────────────────
#[test]
fn r4_r5_attach_injects_generic_envelope_and_skips_surfaces() {
    let server = create_test_server();
    send_one(&server, "r4-k1", "hello");

    let base = || {
        json!({
            "content": [{ "type": "text", "text": "original tool output" }],
            "data": { "status": "ok" }
        })
    };
    let args = json!({ "project": TO });

    // R4 — a generic tool envelope gains the structured banner + a text line.
    let injected = server.attach_mailbox_unread_banner("query", &args, base());
    assert_eq!(injected["data"]["mailbox"]["unread"].as_i64(), Some(1));
    let text = injected["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("original tool output"), "original text preserved");
    assert!(text.contains("📬"), "banner line appended to the text channel");

    // R5 — surfaces that already show the inbox keep a clean envelope.
    for skip in ["status", "mcp_inbox_read", "mailbox_render"] {
        let untouched = server.attach_mailbox_unread_banner(skip, &args, base());
        assert!(
            untouched["data"].get("mailbox").is_none(),
            "`{skip}` must not get a redundant banner"
        );
        assert_eq!(
            untouched["content"][0]["text"].as_str(),
            Some("original tool output"),
            "`{skip}` text channel must be untouched"
        );
    }

    // A non-recipient project's generic envelope stays clean too.
    let other = server.attach_mailbox_unread_banner("query", &json!({ "project": FROM }), base());
    assert!(
        other["data"].get("mailbox").is_none(),
        "non-recipient envelope must carry no banner"
    );
}

// ── R6 — reading clears the signal ─────────────────────────────────────────
#[test]
fn r6_banner_clears_after_read_advances_cursor() {
    let server = create_test_server();
    send_one(&server, "r6-k1", "hello");
    assert!(server.mailbox_unread_banner(TO).is_some(), "mail present before read");

    // Pull the inbox in `unread` mode → cursor advances past the message.
    server
        .execute_tool_direct("mcp_inbox_read", &json!({ "project": TO, "mode": "unread" }))
        .expect("inbox_read returns a result");

    assert!(
        server.mailbox_unread_banner(TO).is_none(),
        "banner must disappear once the message has been read"
    );
}
