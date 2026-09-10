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
        .filter_map(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
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
    assert!(sent["data"]["message_id"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
    assert_eq!(sent["data"]["deduped"].as_bool(), Some(false));

    let inbox = read(&server, json!({ "project": TO, "mode": "all" }));
    let msgs = inbox["data"]["messages"]
        .as_array()
        .expect("messages array");
    assert_eq!(msgs.len(), 1, "exactly one message delivered");
    let m = &msgs[0];
    for key in [
        "message_id",
        "context_id",
        "from",
        "subject",
        "body_dense",
        "signature_verified",
    ] {
        assert!(
            m.get(key).is_some(),
            "A2A envelope key `{key}` must be present"
        );
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
    let msgs = inbox["data"]["messages"]
        .as_array()
        .expect("messages array");
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
    assert_eq!(
        first["data"]["deduped"].as_bool(),
        Some(false),
        "first send delivers"
    );
    assert_eq!(inbox_count(&server, TO), 1);

    // Re-send with the SAME (from, idempotency_key): idempotent no-op.
    let again = send(&server, args.clone());
    assert_eq!(
        again["data"]["deduped"].as_bool(),
        Some(true),
        "re-send is deduped"
    );
    assert_eq!(
        again["data"]["message_id"].as_str(),
        first["data"]["message_id"].as_str(),
        "dedup yields the same stable message_id"
    );
    assert_eq!(
        inbox_count(&server, TO),
        1,
        "row count is unchanged after re-send"
    );
}

// ── C4 — threading: context_id filters, cursor not advanced ────────────────
#[test]
fn c4_context_id_filters_thread_and_does_not_advance_cursor() {
    let server = create_test_server();
    for (i, ctx) in [
        ("t1a", "thread-1"),
        ("t1b", "thread-1"),
        ("t2a", "thread-2"),
    ] {
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
        msgs.iter()
            .all(|m| m["context_id"].as_str() == Some("thread-1")),
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
    assert_eq!(
        cursor, 0,
        "a thread view must not create/advance the read cursor"
    );

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
    assert_eq!(
        first["data"]["count"].as_i64(),
        Some(3),
        "first unread drains all three"
    );
    let cursor = first["data"]["cursor"].as_i64().unwrap_or(0);
    assert!(cursor > 0, "cursor advanced past floor");

    let second = read(&server, json!({ "project": TO, "mode": "unread" }));
    assert_eq!(
        second["data"]["count"].as_i64(),
        Some(0),
        "second unread sees nothing new"
    );
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

    send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c8-ordinary", "subject": "avis",
            "body_dense": "transitoire", "priority": "low"
        }),
    );
    send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c8-important", "subject": "décision",
            "body_dense": "ref REQ-AXO-902306", "priority": "high"
        }),
    );

    let first = read(&server, json!({ "project": TO, "mode": "unread" }));
    assert_eq!(
        first["data"]["count"].as_i64(),
        Some(2),
        "les deux sont livrés"
    );

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
    send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c8-view", "subject": "vue",
            "body_dense": "ne doit pas être archivé par une vue", "priority": "low"
        }),
    );

    read(&server, json!({ "project": TO, "mode": "all" }));
    read(
        &server,
        json!({ "project": TO, "mode": "since", "since_id": 0 }),
    );
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

    send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c9-important", "subject": "décision",
            "body_dense": "ref REQ-AXO-902308", "priority": "high",
            "ttl_hours": 1
        }),
    );

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
        .execute_tool_direct(
            "mcp_inbox_archive",
            &json!({ "project": TO, "message_ids": ids }),
        )
        .expect("mcp_inbox_archive returns a result");
    assert_eq!(archived["data"]["archived"].as_i64(), Some(1));
    assert_eq!(inbox_count(&server, TO), 0, "nommé, il sort");

    // Idempotent : re-nommer n'invente pas un second retrait.
    let again = server
        .execute_tool_direct(
            "mcp_inbox_archive",
            &json!({ "project": TO, "message_ids": ids }),
        )
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
    send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c9-mine", "subject": "à moi",
            "body_dense": "reste", "priority": "high"
        }),
    );
    let mine = message_ids(&server, TO);
    let mut mixed = mine.clone();
    mixed.push(999_999);

    let refused = server
        .execute_tool_direct(
            "mcp_inbox_archive",
            &json!({ "project": TO, "message_ids": mixed }),
        )
        .expect("mcp_inbox_archive returns a result");
    assert_eq!(
        refused["isError"].as_bool(),
        Some(true),
        "l'appel entier échoue"
    );
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
    assert_eq!(
        inbox_count(&server, TO),
        0,
        "a refused send delivers nothing"
    );

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

// ── C10 — le lot est borné en VOLUME, et ce qu'il écarte survit (REQ-AXO-902419) ──
//
// Mesuré par TE2 (#184), jumelle VPC #181 marquée `blocking` : `limit=31` a rendu
// **62 390 caractères sur 563 lignes**, au-delà du plafond du client, dérouté vers un
// fichier relu en trois passes de `sed`. Sur ces 31 messages la taille allait de deux
// lignes à quatre-vingts : `limit` borne le NOMBRE, le débordement vient du VOLUME, et
// le volume est inconnaissable AVANT l'appel.
//
// L'échec tombe à l'étape 3c de `GUI-PRO-102` — donc avant tout travail utile.

/// La propriété qui rend la troncature acceptable : un message écarté par le budget
/// n'est **ni consommé ni archivé**. Sans elle, borner en volume ne serait pas une
/// protection mais une perte de courrier — bien pire que le débordement d'origine.
#[test]
fn c10_le_budget_de_volume_ne_consomme_ni_n_archive_ce_qu_il_n_a_pas_rendu() {
    let server = create_test_server();
    // Cinq messages d'environ 400 caractères de corps chacun.
    for i in 0..5 {
        send(
            &server,
            json!({
                "from": FROM, "to_project": TO,
                "idempotency_key": format!("c10-{i}"),
                "subject": format!("gros {i}"),
                "body_dense": "x".repeat(400)
            }),
        );
    }
    let avant = message_ids(&server, TO);
    assert_eq!(avant.len(), 5, "cinq messages en boîte");

    // Budget volontairement serré : deux messages doivent passer, pas cinq.
    let lot = read(
        &server,
        json!({ "project": TO, "mode": "unread", "budget_chars": 1100 }),
    );
    let rendus = lot["data"]["count"].as_i64().unwrap_or(-1);
    assert!(
        (1..5).contains(&rendus),
        "le budget doit tronquer sans tout jeter, got {rendus}"
    );
    assert!(
        lot["data"]["messages_non_rendus_budget"]
            .as_i64()
            .unwrap_or(0)
            > 0,
        "le débordement doit être une DONNÉE, pas seulement une phrase : {}",
        lot["data"]
    );
    assert!(
        lot["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("borné en VOLUME"),
        "et il doit être DIT dans le canal que l'agent lit"
    );

    // LE point : les non-rendus sont toujours là, non archivés.
    let apres = message_ids(&server, TO);
    assert_eq!(
        apres.len() as i64,
        5 - rendus,
        "seuls les messages RENDUS sont archivés — les autres survivent : {apres:?}"
    );

    // Et le rappel les délivre : rien n'est perdu, seulement différé.
    let suite = read(&server, json!({ "project": TO, "mode": "unread" }));
    assert_eq!(
        suite["data"]["count"].as_i64(),
        Some(5 - rendus),
        "le second appel rend EXACTEMENT ce que le premier avait écarté"
    );
    assert!(
        message_ids(&server, TO).is_empty(),
        "après les deux appels la boîte est vide — aucun message n'a été sauté"
    );
}

/// Contre-exemple : un premier message plus gros que le budget passe QUAND MÊME.
/// Rendre zéro message parce que le premier dépasse remplacerait un débordement par
/// un blocage, et le lecteur n'aurait aucun moyen d'avancer dans sa boîte.
#[test]
fn c10b_un_message_plus_gros_que_le_budget_passe_quand_meme() {
    let server = create_test_server();
    send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c10b-enorme",
            "subject": "rapport d'incident",
            "body_dense": "y".repeat(5_000)
        }),
    );
    let lot = read(
        &server,
        json!({ "project": TO, "mode": "unread", "budget_chars": 10 }),
    );
    assert_eq!(
        lot["data"]["count"].as_i64(),
        Some(1),
        "un budget minuscule ne doit jamais bloquer la boîte : {}",
        lot["data"]
    );
}

// ── C11 — priorité complète ('low', 'normal', 'high', 'urgent') et préservation (REQ-AXO-902413) ──
#[test]
fn c11_priority_levels_published_and_preserved() {
    let server = create_test_server();

    for (key, subject, priority) in [
        ("c11-urgent", "panne critique", "urgent"),
        ("c11-high", "incident majeur", "high"),
        ("c11-normal", "tâche standard", "normal"),
        ("c11-low", "remarque mineure", "low"),
    ] {
        let sent = send(
            &server,
            json!({
                "from": FROM, "to_project": TO,
                "idempotency_key": key,
                "subject": subject, "body_dense": format!("corps de {subject}"),
                "priority": priority
            }),
        );
        assert_eq!(sent["data"]["status"].as_str(), Some("ok"));
    }

    // Lecture mode all: vérifie la publication du champ priority pour chaque message
    let inbox = read(&server, json!({ "project": TO, "mode": "all" }));
    let msgs = inbox["data"]["messages"]
        .as_array()
        .expect("messages array");
    assert_eq!(msgs.len(), 4);

    let urgent = msgs
        .iter()
        .find(|m| m["subject"].as_str() == Some("panne critique"))
        .expect("urgent");
    assert_eq!(urgent["priority"].as_str(), Some("urgent"));

    let high = msgs
        .iter()
        .find(|m| m["subject"].as_str() == Some("incident majeur"))
        .expect("high");
    assert_eq!(high["priority"].as_str(), Some("high"));

    let normal = msgs
        .iter()
        .find(|m| m["subject"].as_str() == Some("tâche standard"))
        .expect("normal");
    assert_eq!(normal["priority"].as_str(), Some("normal"));

    let low = msgs
        .iter()
        .find(|m| m["subject"].as_str() == Some("remarque mineure"))
        .expect("low");
    assert_eq!(low["priority"].as_str(), Some("low"));

    // En mode non-unread (ici all), le tri par priorité met urgent puis high en tête
    assert_eq!(msgs[0]["priority"].as_str(), Some("urgent"));
    assert_eq!(msgs[1]["priority"].as_str(), Some("high"));

    // Préservation: 'urgent' et 'high' survivent à l'auto-archivage d'une lecture unread
    let unread = read(&server, json!({ "project": TO, "mode": "unread" }));
    assert_eq!(unread["data"]["count"].as_i64(), Some(4));

    // Après lecture unread, 'normal' et 'low' ont été archivés, 'urgent' et 'high' restent en boîte
    let restants = message_ids(&server, TO);
    assert_eq!(
        restants.len(),
        2,
        "seuls urgent et high doivent survivre en boîte"
    );
}

// ── C12 — relèvement non destructif scopé projet (peek / summary_only) (REQ-AXO-902413) ──
#[test]
fn c12_peek_non_destructive_surveillance() {
    let server = create_test_server();

    // 0 messages: peek doit rendre 0 non-lus, age 0, et curseur durable 0
    let empty_peek = read(&server, json!({ "project": TO, "mode": "peek" }));
    assert_eq!(empty_peek["data"]["status"].as_str(), Some("ok"));
    assert_eq!(empty_peek["data"]["peek"].as_bool(), Some(true));
    assert_eq!(empty_peek["data"]["unread_count"].as_i64(), Some(0));
    assert_eq!(empty_peek["data"]["unread_high_count"].as_i64(), Some(0));
    assert_eq!(empty_peek["data"]["oldest_unread_age_s"].as_i64(), Some(0));
    assert_eq!(empty_peek["data"]["last_read_id"].as_i64(), Some(0));
    assert_eq!(empty_peek["data"]["read_cursor_id"].as_i64(), Some(0));
    assert_eq!(
        empty_peek["data"]["messages"].as_array().map(|a| a.len()),
        Some(0)
    );

    // Envoi de messages: 1 urgent, 1 high, 2 normal
    for (key, subject, prio) in [
        ("c12-1", "m1-urgent", "urgent"),
        ("c12-2", "m2-high", "high"),
        ("c12-3", "m3-normal", "normal"),
        ("c12-4", "m4-normal", "normal"),
    ] {
        send(
            &server,
            json!({
                "from": FROM, "to_project": TO,
                "idempotency_key": key,
                "subject": subject, "body_dense": "corps confidentiel",
                "priority": prio
            }),
        );
    }

    // Appel peek via mode="peek"
    let peek1 = read(&server, json!({ "project": TO, "mode": "peek" }));
    assert_eq!(peek1["data"]["unread_count"].as_i64(), Some(4));
    assert_eq!(
        peek1["data"]["unread_high_count"].as_i64(),
        Some(2),
        "1 urgent + 1 high = 2"
    );
    assert!(peek1["data"]["oldest_unread_age_s"].as_i64().unwrap_or(-1) >= 0);
    assert_eq!(peek1["data"]["last_read_id"].as_i64(), Some(0));
    assert_eq!(peek1["data"]["read_cursor_id"].as_i64(), Some(0));
    // Les corps de messages ne doivent PAS être rendus
    assert_eq!(
        peek1["data"]["messages"].as_array().map(|a| a.len()),
        Some(0)
    );
    let text = peek1["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        !text.contains("corps confidentiel"),
        "aucun corps inliné dans le texte en mode peek"
    );
    assert!(
        text.contains("peek") || text.contains("non-lu"),
        "le texte résume la surveillance"
    );

    // Vérification que read_at est toujours NULL en DB pour tous les messages
    let read_at_count = server
        .graph_store
        .query_single_i64_writer(&format!(
            "SELECT count(*) FROM axon.mailbox_message WHERE to_project='{TO}' AND read_at IS NOT NULL"
        ))
        .ok()
        .flatten()
        .unwrap_or(-1);
    assert_eq!(read_at_count, 0, "peek ne doit jamais marquer read_at");

    // Vérification que le curseur durable n'a pas bougé
    let cursor_count = server
        .graph_store
        .query_single_i64_writer(&format!(
            "SELECT count(*) FROM axon.mailbox_cursor WHERE project_code='{TO}'"
        ))
        .ok()
        .flatten()
        .unwrap_or(-1);
    assert_eq!(cursor_count, 0, "peek ne doit jamais avancer le curseur");

    // Appel alternatif via argument peek: true ou summary_only: true
    let peek2 = read(&server, json!({ "project": TO, "peek": true }));
    assert_eq!(peek2["data"]["unread_count"].as_i64(), Some(4));
    assert_eq!(peek2["data"]["unread_high_count"].as_i64(), Some(2));

    let peek3 = read(&server, json!({ "project": TO, "summary_only": true }));
    assert_eq!(peek3["data"]["unread_count"].as_i64(), Some(4));
    assert_eq!(peek3["data"]["unread_high_count"].as_i64(), Some(2));

    // Non destructif : un relèvement unread ultérieur voit toujours l'intégralité des 4 messages
    let unread = read(&server, json!({ "project": TO, "mode": "unread" }));
    assert_eq!(
        unread["data"]["count"].as_i64(),
        Some(4),
        "tous les messages sont consommables"
    );
    let new_cursor = unread["data"]["cursor"].as_i64().unwrap_or(0);
    assert!(new_cursor > 0);

    // Un nouveau peek après le drain unread voit 0 non-lus et le nouveau curseur durable
    let peek_after = read(&server, json!({ "project": TO, "mode": "peek" }));
    assert_eq!(peek_after["data"]["unread_count"].as_i64(), Some(0));
    assert_eq!(peek_after["data"]["unread_high_count"].as_i64(), Some(0));
    assert_eq!(
        peek_after["data"]["last_read_id"].as_i64(),
        Some(new_cursor)
    );
    assert_eq!(
        peek_after["data"]["read_cursor_id"].as_i64(),
        Some(new_cursor)
    );
}

// ── C13 — mode since distingue le curseur de lecture du max_id (REQ-AXO-902413) ──
#[test]
fn c13_since_distinguishes_durable_cursor_from_batch_max_id() {
    let server = create_test_server();

    // Envoi de message 1 (priority: high pour survivre à la lecture unread)
    send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c13-1",
            "subject": "m1", "body_dense": "corps 1",
            "priority": "high"
        }),
    );

    // Draine message 1 avec mode=unread: établit le curseur durable sur message 1
    let drain1 = read(&server, json!({ "project": TO, "mode": "unread" }));
    let cursor1 = drain1["data"]["cursor"].as_i64().expect("cursor 1");
    assert!(cursor1 > 0);

    // Envoi de 2 nouveaux messages
    send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c13-2",
            "subject": "m2", "body_dense": "corps 2"
        }),
    );
    send(
        &server,
        json!({
            "from": FROM, "to_project": TO,
            "idempotency_key": "c13-3",
            "subject": "m3", "body_dense": "corps 3"
        }),
    );

    // Appel mode=since avec since_id=0
    let since_res = read(
        &server,
        json!({ "project": TO, "mode": "since", "since_id": 0 }),
    );
    let msgs = since_res["data"]["messages"].as_array().expect("messages");
    assert_eq!(msgs.len(), 3);

    let max_id = since_res["data"]["max_id"].as_i64().expect("max_id");
    let last_read_id = since_res["data"]["last_read_id"]
        .as_i64()
        .expect("last_read_id");
    let read_cursor_id = since_res["data"]["read_cursor_id"]
        .as_i64()
        .expect("read_cursor_id");

    assert_eq!(
        last_read_id, cursor1,
        "last_read_id doit être le VRAI curseur durable"
    );
    assert_eq!(read_cursor_id, cursor1);
    assert!(
        max_id > cursor1,
        "max_id ({max_id}) doit être supérieur au curseur durable ({cursor1})"
    );
    assert_ne!(
        last_read_id, max_id,
        "last_read_id et max_id doivent être distincts"
    );

    // Vérifie que le curseur durable en DB n'a pas été écrasé par max_id
    let db_cursor = server
        .graph_store
        .query_single_i64_writer(&format!(
            "SELECT last_read_id FROM axon.mailbox_cursor WHERE project_code='{TO}'"
        ))
        .ok()
        .flatten()
        .unwrap_or(0);
    assert_eq!(
        db_cursor, cursor1,
        "le curseur durable en base ne doit pas avoir avancé"
    );
}
