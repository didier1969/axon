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

/// Ids of the still-live inbox rows for a recipient (ascending).
fn message_ids(server: &McpServer, to: &str) -> Vec<i64> {
    let raw = server
        .graph_store
        .query_json_writer(&format!(
            "SELECT id FROM axon.mailbox_message WHERE to_project='{to}' AND archived_at IS NULL ORDER BY id"
        ))
        .expect("ids query");
    serde_json::from_str::<Vec<Vec<Value>>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| r.into_iter().next())
        .filter_map(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .collect()
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

/// REQ-AXO-902413 — signalé par VPC. `mcp_outbox_send` accepte `priority`, la
/// requête de lecture TRIE dessus (`ORDER BY CASE priority WHEN 'high'…`), le
/// champ gouverne l'archivage (`priority='high'` échappe à l'archivage auto et
/// au balayage TTL) — et il n'était **pas publié**. Un automate ne pouvait donc
/// que tout notifier (interdit par l'opérateur de VPC) ou deviner l'urgence par
/// mots-clés. VPC n'a livré aucune surveillance, délibérément.
///
/// Même classe que REQ-AXO-902409 : le writer persiste, le reader ne restitue
/// pas — ici sur un champ qui DÉCIDE.
#[test]
fn c6_priority_is_published_in_data_and_text() {
    let server = create_test_server();

    for (key, subject, priority) in [
        ("c6-high", "incident en cours", "high"),
        ("c6-low", "note de routine", "low"),
    ] {
        let sent = send(
            &server,
            json!({
                "from": FROM, "to_project": TO,
                "idempotency_key": key,
                "subject": subject, "body_dense": "corps",
                "priority": priority
            }),
        );
        assert_eq!(sent["data"]["status"].as_str(), Some("ok"), "{key} envoyé");
    }

    let inbox = read(&server, json!({ "project": TO, "mode": "all" }));
    let msgs = inbox["data"]["messages"].as_array().expect("messages array");
    assert_eq!(msgs.len(), 2);

    let high = msgs
        .iter()
        .find(|m| m["subject"].as_str() == Some("incident en cours"))
        .expect("le message haute priorité est là");
    assert_eq!(
        high["priority"].as_str(),
        Some("high"),
        "la priorité doit être PUBLIÉE : elle gouverne l'archivage et le tri, \
         et sans elle un automate ne peut que deviner l'urgence"
    );

    let text = inbox["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("HAUTE"),
        "la priorité haute doit être visible dans le TEXTE : le tri se fait \
         dessus, un lecteur qui ne la voit pas ne comprend pas l'ordre servi.\n---\n{text}"
    );
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

// ── C8 — read empties the inbox, important messages survive (REQ-AXO-902306) ──
//
// Demande opérateur : « il faudrait que les messages lus disparaissent. les
// messages importants ne doivent pas être enlevés [sans] lecture par le
// destinataire. »
//
// Avancer le curseur ne retirait rien : seul le TTL finissait par archiver, et le
// TTL est une horloge ABSOLUE — un projet dormant plus longtemps que l'horizon
// perdait un avis jamais lu. La règle retenue satisfait les deux lectures de la
// demande : un message important ne disparaît JAMAIS tout seul.
#[test]
fn c8_reading_archives_ordinary_messages_but_never_important_ones() {
    let server = create_test_server();

    send(&server, json!({
        "from": FROM, "to_project": TO,
        "idempotency_key": "c8-ordinary", "subject": "avis",
        "body_dense": "transitoire", "priority": "low"
    }));
    send(&server, json!({
        "from": FROM, "to_project": TO,
        "idempotency_key": "c8-important", "subject": "décision",
        "body_dense": "ref REQ-AXO-902306", "priority": "high"
    }));

    let first = read(&server, json!({ "project": TO, "mode": "unread" }));
    assert_eq!(first["data"]["count"].as_i64(), Some(2), "les deux sont livrés");

    // L'inbox active ne garde que l'important.
    assert_eq!(
        inbox_count(&server, TO),
        1,
        "le message ordinaire lu doit sortir de l'inbox, l'important doit rester"
    );
    let remaining = read(&server, json!({ "project": TO, "mode": "all" }));
    let subjects: Vec<&str> = remaining["data"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|m| m["subject"].as_str())
        .collect();
    assert_eq!(
        subjects,
        vec!["décision"],
        "seul l'important survit à la lecture : {subjects:?}"
    );
}

#[test]
fn c8_non_destructive_views_archive_nothing() {
    // `all` / `since` / thread servent à RELIRE. Le contrat non-destructif est
    // déjà pinné par C4 pour le curseur ; il vaut aussi pour l'archivage.
    let server = create_test_server();
    send(&server, json!({
        "from": FROM, "to_project": TO,
        "idempotency_key": "c8-view", "subject": "vue",
        "body_dense": "ne doit pas être archivé par une vue", "priority": "low"
    }));

    read(&server, json!({ "project": TO, "mode": "all" }));
    read(&server, json!({ "project": TO, "mode": "since", "since_id": 0 }));
    // `search` NE passe PAS `mode` : le défaut est `unread`, et c'est précisément
    // le cas qui a menti en live (bandeau « cursor advanced » sur une vue qui
    // n'avait rien consommé). Le contrat non-destructif tient au `view_only`, pas
    // au mode déclaré — donc c'est CE cas qu'il faut épingler.
    let searched = read(&server, json!({ "project": TO, "search": "archivé" }));

    assert_eq!(
        inbox_count(&server, TO),
        1,
        "une vue non destructive ne doit rien archiver"
    );

    // Et elle ne doit pas non plus PRÉTENDRE l'avoir fait : le rapport est la
    // seule chose que l'appelant voit.
    let text = searched["content"][0]["text"]
        .as_str()
        .expect("le rapport porte un texte");
    assert!(
        !text.contains("cursor advanced") && !text.contains("archivé(s)"),
        "une vue ne doit annoncer ni avance de curseur ni archivage : {text}"
    );

    // Le pendant positif : la lecture destructive, elle, DIT ce qu'elle retire.
    let consumed = read(&server, json!({ "project": TO, "mode": "unread" }));
    let consumed_text = consumed["content"][0]["text"]
        .as_str()
        .expect("le rapport porte un texte");
    assert!(
        consumed_text.contains("1 archivé(s)"),
        "la lecture destructive doit annoncer son archivage : {consumed_text}"
    );
}

// ── C9 — le retrait délibéré (REQ-AXO-902308) ──────────────────────────────
//
// C8 a rendu `high` inarchivable par les DEUX sorties automatiques. Sans un verbe
// nommé, il devenait inretirable tout court — l'accumulation de REQ-AXO-902304
// déplacée d'un cran — et le bandeau de lecture annonçait une issue inexistante.
#[test]
fn c9_an_important_message_survives_both_automatic_exits_then_leaves_when_named() {
    let server = create_test_server();

    send(&server, json!({
        "from": FROM, "to_project": TO,
        "idempotency_key": "c9-important", "subject": "décision",
        "body_dense": "ref REQ-AXO-902308", "priority": "high",
        "ttl_hours": 1
    }));

    // Sortie automatique 1 : la lecture. Sortie automatique 2 : le balayage TTL
    // (horizon déjà passé — forcé en base pour ne pas dépendre de l'horloge).
    read(&server, json!({ "project": TO, "mode": "unread" }));
    server
        .graph_store
        .execute(&format!(
            "UPDATE axon.mailbox_message SET ttl_at = now() - interval '1 hour' WHERE to_project='{TO}'"
        ))
        .expect("ttl backdate");
    server
        .execute_tool_direct("mailbox_sweep", &json!({}))
        .expect("mailbox_sweep returns a result");

    assert_eq!(
        inbox_count(&server, TO),
        1,
        "ni la lecture ni l'expiration ne retirent un message important"
    );

    // Le geste délibéré, lui, le retire.
    let ids = message_ids(&server, TO);
    let archived = server
        .execute_tool_direct("mcp_inbox_archive", &json!({ "project": TO, "message_ids": ids }))
        .expect("mcp_inbox_archive returns a result");
    assert_eq!(archived["data"]["archived"].as_i64(), Some(1));
    assert_eq!(inbox_count(&server, TO), 0, "nommé, il sort");

    // Idempotent : re-nommer n'invente pas un second retrait.
    let again = server
        .execute_tool_direct("mcp_inbox_archive", &json!({ "project": TO, "message_ids": ids }))
        .expect("mcp_inbox_archive returns a result");
    assert_eq!(again["data"]["archived"].as_i64(), Some(0));
    assert_eq!(again["data"]["already_archived"].as_i64(), Some(1));
}

#[test]
fn c9_archiving_refuses_ids_that_belong_to_another_inbox() {
    // Un id étranger doit faire ÉCHOUER l'appel entier, pas être sauté en
    // silence : un archivage partiel qui rapporte « ok » est la façon dont un
    // appelant apprend à ne plus croire le compte.
    let server = create_test_server();
    send(&server, json!({
        "from": FROM, "to_project": TO,
        "idempotency_key": "c9-mine", "subject": "à moi",
        "body_dense": "reste", "priority": "high"
    }));
    let mine = message_ids(&server, TO);
    let mut mixed = mine.clone();
    mixed.push(999_999);

    let refused = server
        .execute_tool_direct("mcp_inbox_archive", &json!({ "project": TO, "message_ids": mixed }))
        .expect("mcp_inbox_archive returns a result");
    assert_eq!(refused["isError"].as_bool(), Some(true), "l'appel entier échoue");
    assert_eq!(
        inbox_count(&server, TO),
        1,
        "et RIEN n'est archivé — pas même la part légitime"
    );
}

// ── C7 — retention horizon (REQ-AXO-902304) ────────────────────────────────
//
// `axon.mailbox_sweep()` archives on `ttl_at < now()` and had existed all along,
// but nothing ever wrote that column: a purge wired to a field nobody filled.
// 8217 promote broadcasts piled up since 2026-07-03 — 118 per project, 100% of
// the inbox for four of them, none ever purgeable.
#[test]
fn c7_ttl_is_recorded_when_declared_and_absent_otherwise() {
    let server = create_test_server();

    send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c7-perishable",
            "subject": "maintenance", "body_dense": "coupure brève",
            "ttl_hours": 24
        }),
    );
    send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c7-durable",
            "subject": "décision", "body_dense": "ref REQ-AXO-902304"
        }),
    );

    let ttl_of = |key: &str| -> Option<String> {
        server
            .graph_store
            .query_json_writer(&format!(
                "SELECT COALESCE(ttl_at::text,'') FROM axon.mailbox_message \
                 WHERE idempotency_key='{key}'"
            ))
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
            .and_then(|rows| rows.first().and_then(|r| r.first()).cloned())
            .and_then(|v| v.as_str().map(str::to_string))
    };

    assert!(
        ttl_of("c7-perishable").is_some_and(|t| !t.is_empty()),
        "a declared ttl_hours must land in ttl_at, or the sweep can never reach it"
    );
    assert!(
        ttl_of("c7-durable").is_some_and(|t| t.is_empty()),
        "no ttl_hours means keep indefinitely — the right default for anything \
         actionable later; only time-bound notices should expire"
    );
}

// ── C6 — a body-less send is refused on every path (REQ-AXO-902278) ────────
//
// Message #5855 shipped an alarming subject ("l'index est périmé de 2 jours,
// les outils structurels rendent FAUX") with `body_dense=""`. The recipient
// could see the alarm and do nothing with it — the dead-end PIL-AXO-002 exists
// to forbid. The defect is not in whoever sent it: the CONTRACT accepted it.
// `idempotency_key` and `to_project` were already refused when empty; the one
// field carrying the message's reason to exist was not.
#[test]
fn c6_body_less_send_is_refused_on_direct_and_fanout_paths() {
    let server = create_test_server();

    // VN — body_dense absent entirely.
    let missing = send(
        &server,
        json!({ "from": FROM, "to_project": TO, "idempotency_key": "c6-k1", "subject": "alarm" }),
    );
    assert_eq!(missing["isError"].as_bool(), Some(true));
    assert_eq!(missing["data"]["status"].as_str(), Some("input_invalid"));

    // VN — body_dense present but whitespace-only: same dead-end for the reader.
    let blank = send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c6-k2", "subject": "alarm", "body_dense": "   \n  "
        }),
    );
    assert_eq!(blank["isError"].as_bool(), Some(true));
    assert_eq!(blank["data"]["status"].as_str(), Some("input_invalid"));

    // Nothing was delivered by either rejected send.
    assert_eq!(inbox_count(&server, TO), 0, "a refused send delivers nothing");

    // VN — the fan-out path must refuse too, or the gate is half-built: a
    // broadcast is the case where a body-less message wastes the most readers.
    let broadcast = send(
        &server,
        json!({ "from": FROM, "to_project": "*", "idempotency_key": "c6-k3", "subject": "alarm" }),
    );
    assert_eq!(broadcast["isError"].as_bool(), Some(true));
    assert_eq!(broadcast["data"]["status"].as_str(), Some("input_invalid"));

    // VP — the same send with a dense body goes through unchanged.
    let ok = send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c6-k4", "subject": "alarm",
            "body_dense": "BKS index périmé 2j — ref REQ-AXO-902264 ; cure: axon --instance live stop && start"
        }),
    );
    assert_eq!(ok["data"]["status"].as_str(), Some("ok"));
    assert_eq!(inbox_count(&server, TO), 1);
}
