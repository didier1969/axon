// REQ-AXO-902548 — Réveiller fiablement les sessions actives lors d'une livraison mailbox
//
// Validation TDD :
// 1. Une session active destinataire reçoit un signal observable dans les 5 secondes suivant la livraison p95.
// 2. Les états livré, notification tentée, lu et acquitté sont mesurables séparément par contexte.
// 3. Une session inactive conserve le message et le découvre au prochain init/status sans perte.
// 4. Un test multi-session reproduit la campagne VPC et obtient au moins un réveil automatique sans polling applicatif.

use super::*;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tokio_postgres::{AsyncMessage, NoTls};

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

fn ack(server: &McpServer, args: Value) -> Value {
    server
        .execute_tool_direct("mcp_inbox_ack", &args)
        .expect("mcp_inbox_ack returns a result")
}

fn tap(server: &McpServer, args: Value) -> Value {
    server
        .execute_tool_direct("mailbox_tap", &args)
        .expect("mailbox_tap returns a result")
}

#[tokio::test]
async fn vpc_campaign_active_session_wakes_and_tracks_quad_state() {
    let (server, db_url) = create_test_server_with_url();

    let emitter = "AXO";
    let active_recipient = "VPC";
    let inactive_recipient = "APS";
    let campaign_context = "csat-campaign-2026-08-28";

    // Enrol VPC and APS in ProjectCodeRegistry so recipient validation succeeds
    server
        .graph_store
        .execute(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_name, project_path) \
             VALUES ('VPC', 'VPC Project', '/tmp/VPC'), ('APS', 'APS Project', '/tmp/APS') \
             ON CONFLICT (project_code) DO NOTHING",
        )
        .expect("seed VPC and APS in registry");

    // 1. Active session setup: Connects to PostgreSQL and issues LISTEN to wake channels.
    let (client, mut connection) = tokio_postgres::connect(&db_url, NoTls)
        .await
        .expect("active session connect to PG");

    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel::<tokio_postgres::Notification>(64);

    let driver = tokio::spawn(async move {
        let stream = futures_util::stream::poll_fn(move |cx| connection.poll_message(cx));
        tokio::pin!(stream);
        while let Some(msg) = stream.next().await {
            if let Ok(AsyncMessage::Notification(n)) = msg {
                let _ = notify_tx.send(n).await;
            }
        }
    });

    client
        .batch_execute("LISTEN axon_mailbox; LISTEN axon_mailbox_wake;")
        .await
        .expect("LISTEN to mailbox wake channel");

    // 2. Emitter broadcasts or sends messages to VPC and APS under the same context_id.
    let sent_vpc = send(
        &server,
        json!({
            "from": emitter,
            "to_project": active_recipient,
            "context_id": campaign_context,
            "idempotency_key": "vpc-csat-msg-1",
            "subject": "CSAT Survey 2026-08-28",
            "body_dense": "Please confirm receipt + feasibility + ETA for CSAT survey",
            "priority": "high"
        }),
    );
    assert_eq!(sent_vpc["data"]["status"].as_str(), Some("ok"));

    let sent_aps = send(
        &server,
        json!({
            "from": emitter,
            "to_project": inactive_recipient,
            "context_id": campaign_context,
            "idempotency_key": "aps-csat-msg-1",
            "subject": "CSAT Survey 2026-08-28",
            "body_dense": "Please confirm receipt + feasibility + ETA for CSAT survey",
            "priority": "normal"
        }),
    );
    assert_eq!(sent_aps["data"]["status"].as_str(), Some("ok"));

    // 3. Acceptance Criterion 1 & 4: Active session wakes up via observable signal without polling << 5s.
    let wake_start = std::time::Instant::now();
    let mut received_wake_payload: Option<Value> = None;

    while let Ok(Some(notif)) = tokio::time::timeout(Duration::from_secs(3), notify_rx.recv()).await
    {
        let parsed: Value = serde_json::from_str(notif.payload()).unwrap_or(Value::Null);
        if parsed.get("to").and_then(Value::as_str) == Some(active_recipient) {
            received_wake_payload = Some(parsed);
            break;
        }
    }

    let elapsed = wake_start.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "active session must receive wake signal within 5 seconds p95 (got {:?})",
        elapsed
    );

    let wake = received_wake_payload.expect("active session must receive wake signal payload");
    assert_eq!(wake["to"].as_str(), Some(active_recipient));
    assert_eq!(wake["from"].as_str(), Some(emitter));
    assert_eq!(wake["context_id"].as_str(), Some(campaign_context));
    assert_eq!(wake["priority"].as_str(), Some("high"));
    assert!(wake.get("message_id").is_some());
    assert!(wake.get("id").is_some());

    // 4. Before reading: measure context state via mailbox_tap.
    // Delivered = 2, Notified = 2, Read = 0, Acknowledged = 0.
    let tap_pre_read = tap(&server, json!({ "context_id": campaign_context }));
    let pre_data = &tap_pre_read["data"];
    assert_eq!(pre_data["delivered_count"].as_i64(), Some(2));
    assert_eq!(pre_data["notified_count"].as_i64(), Some(2));
    assert_eq!(pre_data["read_count"].as_i64(), Some(0));
    assert_eq!(pre_data["acknowledged_count"].as_i64(), Some(0));

    // 5. Active session wakes up and reads its inbox.
    let vpc_inbox = read(
        &server,
        json!({ "project": active_recipient, "mode": "unread" }),
    );
    let vpc_msgs = vpc_inbox["data"]["messages"]
        .as_array()
        .expect("messages array");
    assert_eq!(vpc_msgs.len(), 1);
    let vpc_msg_id = vpc_msgs[0]["id"].as_i64().expect("message id");

    // After VPC read: Delivered = 2, Notified = 2, Read = 1, Acknowledged = 0.
    let tap_post_read = tap(&server, json!({ "context_id": campaign_context }));
    let post_read_data = &tap_post_read["data"];
    assert_eq!(post_read_data["delivered_count"].as_i64(), Some(2));
    assert_eq!(post_read_data["notified_count"].as_i64(), Some(2));
    assert_eq!(post_read_data["read_count"].as_i64(), Some(1));
    assert_eq!(post_read_data["acknowledged_count"].as_i64(), Some(0));

    // 6. Active session acknowledges receipt via mcp_inbox_ack.
    let ack_res = ack(
        &server,
        json!({
            "project": active_recipient,
            "message_ids": [vpc_msg_id],
            "context_id": campaign_context,
            "ack_note": "Confirme réception + faisabilité OK sous 24h"
        }),
    );
    assert_eq!(ack_res["data"]["status"].as_str(), Some("ok"));
    assert_eq!(ack_res["data"]["acknowledged_count"].as_i64(), Some(1));

    // After VPC ack: Delivered = 2, Notified = 2, Read = 1, Acknowledged = 1.
    let tap_post_ack = tap(&server, json!({ "context_id": campaign_context }));
    let post_ack_data = &tap_post_ack["data"];
    assert_eq!(post_ack_data["delivered_count"].as_i64(), Some(2));
    assert_eq!(post_ack_data["notified_count"].as_i64(), Some(2));
    assert_eq!(post_ack_data["read_count"].as_i64(), Some(1));
    assert_eq!(post_ack_data["acknowledged_count"].as_i64(), Some(1));

    // Check recipients breakdown:
    let recs = post_ack_data["recipients"]
        .as_array()
        .expect("recipients breakdown array");
    assert_eq!(recs.len(), 2);
    let vpc_rec = recs
        .iter()
        .find(|r| r["to_project"].as_str() == Some(active_recipient))
        .expect("vpc recipient");
    assert_eq!(vpc_rec["status"].as_str(), Some("acknowledged"));
    assert!(vpc_rec["notified_at"].as_str().is_some());
    assert!(vpc_rec["read_at"].as_str().is_some());
    assert!(vpc_rec["acknowledged_at"].as_str().is_some());

    let aps_rec = recs
        .iter()
        .find(|r| r["to_project"].as_str() == Some(inactive_recipient))
        .expect("aps recipient");
    assert_eq!(aps_rec["status"].as_str(), Some("notified"));
    assert!(aps_rec["notified_at"].as_str().is_some());
    assert!(aps_rec["read_at"].is_null());
    assert!(aps_rec["acknowledged_at"].is_null());

    // 7. Acceptance Criterion 3: Inactive session preserves message without loss and discovers it on next read.
    let aps_inbox = read(
        &server,
        json!({ "project": inactive_recipient, "mode": "unread" }),
    );
    let aps_msgs = aps_inbox["data"]["messages"]
        .as_array()
        .expect("aps messages array");
    assert_eq!(
        aps_msgs.len(),
        1,
        "inactive session must find its message preserved without loss"
    );
    assert_eq!(aps_msgs[0]["context_id"].as_str(), Some(campaign_context));

    // Cleanup client connection
    drop(client);
    let _ = driver.await;
}

#[test]
fn inbox_ack_validation_and_idempotence() {
    let server = create_test_server();

    // 1. Validation: project unresolved or missing
    let no_proj = ack(&server, json!({ "message_ids": [1] }));
    assert_eq!(no_proj["isError"].as_bool(), Some(true));
    assert_eq!(no_proj["data"]["status"].as_str(), Some("input_invalid"));

    // 2. Validation: neither message_ids nor context_id
    let no_targets = ack(&server, json!({ "project": "PJA" }));
    assert_eq!(no_targets["isError"].as_bool(), Some(true));
    assert_eq!(no_targets["data"]["status"].as_str(), Some("input_invalid"));

    // 3. Send message to PJA
    let sent = send(
        &server,
        json!({
            "from": "PJB",
            "to_project": "PJA",
            "idempotency_key": "ack-test-k1",
            "subject": "task request",
            "body_dense": "deploy update",
            "context_id": "thread-ack-1"
        }),
    );
    assert_eq!(sent["data"]["status"].as_str(), Some("ok"));

    // Read to get row ID
    let inbox = read(&server, json!({ "project": "PJA", "mode": "all" }));
    let msg_id = inbox["data"]["messages"][0]["id"].as_i64().expect("id");

    // 4. Foreign ownership rejection: PJB tries to acknowledge PJA's message
    let wrong_proj = ack(
        &server,
        json!({
            "project": "PJB",
            "message_ids": [msg_id]
        }),
    );
    assert_eq!(wrong_proj["isError"].as_bool(), Some(true));
    assert_eq!(wrong_proj["data"]["status"].as_str(), Some("input_invalid"));

    // 5. Valid ack by PJA
    let valid_ack = ack(
        &server,
        json!({
            "project": "PJA",
            "message_ids": [msg_id],
            "ack_note": "Acknowledged by PJA operator"
        }),
    );
    assert_eq!(valid_ack["data"]["status"].as_str(), Some("ok"));
    assert_eq!(valid_ack["data"]["acknowledged_count"].as_i64(), Some(1));

    // 6. Idempotency: re-ack does not fail and preserves acknowledgment
    let re_ack = ack(
        &server,
        json!({
            "project": "PJA",
            "message_ids": [msg_id]
        }),
    );
    assert_eq!(re_ack["data"]["status"].as_str(), Some("ok"));
    assert_eq!(re_ack["data"]["acknowledged_count"].as_i64(), Some(1));
}

#[test]
fn quad_state_full_lifecycle_progression() {
    let server = create_test_server();
    let ctx = "quad-thread-lifecycle";

    // 1. State 1: Delivered
    let sent = send(
        &server,
        json!({
            "from": "PJA",
            "to_project": "PJB",
            "context_id": ctx,
            "idempotency_key": "quad-key-1",
            "subject": "quad state test",
            "body_dense": "testing 4 states"
        }),
    );
    assert_eq!(sent["data"]["status"].as_str(), Some("ok"));

    let tap1 = tap(&server, json!({ "context_id": ctx }));
    assert_eq!(tap1["data"]["delivered_count"].as_i64(), Some(1));
    assert_eq!(tap1["data"]["notified_count"].as_i64(), Some(1));
    assert_eq!(tap1["data"]["read_count"].as_i64(), Some(0));
    assert_eq!(tap1["data"]["acknowledged_count"].as_i64(), Some(0));
    assert_eq!(
        tap1["data"]["recipients"][0]["status"].as_str(),
        Some("notified")
    );

    // 2. State 2: Read
    let read_res = read(&server, json!({ "project": "PJB", "mode": "unread" }));
    let msg_id = read_res["data"]["messages"][0]["id"].as_i64().expect("id");
    assert!(msg_id > 0);

    let tap2 = tap(&server, json!({ "context_id": ctx }));
    assert_eq!(tap2["data"]["delivered_count"].as_i64(), Some(1));
    assert_eq!(tap2["data"]["notified_count"].as_i64(), Some(1));
    assert_eq!(tap2["data"]["read_count"].as_i64(), Some(1));
    assert_eq!(tap2["data"]["acknowledged_count"].as_i64(), Some(0));
    assert_eq!(
        tap2["data"]["recipients"][0]["status"].as_str(),
        Some("read")
    );

    // 3. State 3: Acknowledged
    let ack_res = ack(
        &server,
        json!({
            "project": "PJB",
            "context_id": ctx
        }),
    );
    assert_eq!(ack_res["data"]["status"].as_str(), Some("ok"));

    let tap3 = tap(&server, json!({ "context_id": ctx }));
    assert_eq!(tap3["data"]["delivered_count"].as_i64(), Some(1));
    assert_eq!(tap3["data"]["notified_count"].as_i64(), Some(1));
    assert_eq!(tap3["data"]["read_count"].as_i64(), Some(1));
    assert_eq!(tap3["data"]["acknowledged_count"].as_i64(), Some(1));
    assert_eq!(
        tap3["data"]["recipients"][0]["status"].as_str(),
        Some("acknowledged")
    );
}
