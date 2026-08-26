//! REQ-AXO-902112 / DEC-AXO-901663 — MAILBOX MVP MCP surface (MBX-1/2).
//!
//! `mcp_outbox_send` (build A2A envelope, HMAC-sign, idempotent UPSERT) and
//! `mcp_inbox_read` (per-recipient cursor, verify signatures, advance cursor).
//! Crypto + envelope live in [`crate::mailbox`]; this is the DB-bound surface.

use serde_json::{json, Value};

use super::McpServer;
use crate::mailbox;

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

/// REQ-AXO-902509 — lire un compte rendu par `query_json`.
///
/// Le pont SQL rend les entiers tantôt en nombres JSON, tantôt en CHAÎNES selon
/// le type PG de la colonne (`count(*)` → `bigint` → chaîne). Un `as_i64()` seul
/// échoue alors en SILENCE et retombe sur 0 : c'est ce qui faisait dire à
/// `mcp_inbox_read` « 2 sur 0 · id max 0 » alors que la boîte portait 70
/// messages non archivés. Le repli existait déjà — à QUATRE endroits de ce
/// fichier sur cinq. Une règle recopiée est une règle qu'on finit par oublier ;
/// elle vit désormais ici, et [`lecture_des_comptes_tests`] refuse qu'on la
/// ré-écrive ailleurs.
fn entier_json(v: &Value) -> i64 {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

/// REQ-AXO-902386 — nearest canonical project code to a mistyped one.
///
/// Pure so the matching rules are testable without a registry. Deliberately
/// CONSERVATIVE: it suggests only when the answer is fairly obvious, because a
/// wrong suggestion on a mailbox recipient is worse than none — it would invite
/// the sender to deliver to the wrong project.
///
/// Codes are 3 characters (DEC-AXO-085), so the realistic typos are: extra
/// character (`AXON` → `AXO`), missing one, wrong case, or one substituted letter.
fn nearest_project_code(supplied: &str, known: &[String]) -> Option<String> {
    let needle = supplied.trim().to_ascii_uppercase();
    if needle.is_empty() {
        return None;
    }
    // 1. Case only.
    if let Some(hit) = known.iter().find(|c| c.eq_ignore_ascii_case(&needle)) {
        return Some(hit.clone());
    }
    // 2. Prefix either way — the APS case (`AXON` starts with `AXO`).
    if let Some(hit) = known
        .iter()
        .find(|c| needle.starts_with(c.as_str()) || c.starts_with(&needle))
    {
        return Some(hit.clone());
    }
    // 3. A single substituted character, same length. Two differences on a
    //    3-character code is not a typo, it is a different code — no guess.
    known
        .iter()
        .find(|c| {
            c.len() == needle.len()
                && c.chars()
                    .zip(needle.chars())
                    .filter(|(a, b)| !a.eq_ignore_ascii_case(b))
                    .count()
                    == 1
        })
        .cloned()
}

pub(crate) fn mbx_err(msg: &str, status: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": msg }],
        "isError": true,
        "data": { "status": status }
    })
}

/// Result of materialising a single mailbox row (one recipient).
pub(crate) struct SentMessage {
    pub message_id: String,
    pub context_id: String,
    pub deduped: bool,
    pub sig: String,
}


/// REQ-AXO-902494 (doléance APS #238) — TOUS les manquements d'un coup, et jamais un
/// paramètre inconnu avalé en silence.
///
/// Cas rapporté : un appel portait `to` (au lieu de `to_project`), `body` (au lieu de
/// `body_dense`) et n'avait pas d'`idempotency_key`. Le refus ne parlait QUE de
/// `body_dense`. `to` a été **ignoré sans un mot** — et c'est le plus grave :
///
/// > « Le destinataire est le paramètre dont une erreur ne se rattrape pas. Ici l'appel a
/// > échoué pour une autre raison, donc rien n'est parti au mauvais endroit — mais un
/// > appel par ailleurs valide avec `to` au lieu de `to_project` aurait vu son
/// > destinataire tomber dans le vide. »
///
/// Un paramètre silencieusement jeté est un contrat rompu SANS erreur.
///
/// Retourne `None` si l'appel est recevable, sinon le message de refus complet.
fn valider_arguments_outbox(args: &Value) -> Option<String> {
    const CONNUS: &[&str] = &[
        "to_project", "to_topic", "to_room", "from", "subject", "body_dense",
        "idempotency_key", "in_reply_to", "context_id", "kind", "priority",
        "ref_soll_ids", "ttl_hours",
    ];

    let Some(obj) = args.as_object() else { return None };

    // Distance d'édition bornée : on ne propose un voisin que s'il est PROCHE.
    // Suggérer n'importe quoi serait pire que se taire — le lecteur corrigerait
    // vers une valeur fausse en croyant suivre un conseil.
    fn voisin(inconnu: &str) -> Option<&'static str> {
        CONNUS
            .iter()
            .map(|c| (*c, distance_edition(inconnu, c)))
            .filter(|(c, d)| *d <= 4 && (c.starts_with(inconnu) || inconnu.starts_with(&c[..c.len().min(inconnu.len())]) || *d <= 2))
            .min_by_key(|(_, d)| *d)
            .map(|(c, _)| c)
    }

    let mut inconnus: Vec<String> = Vec::new();
    for cle in obj.keys() {
        if CONNUS.contains(&cle.as_str()) {
            continue;
        }
        match voisin(cle) {
            Some(v) => inconnus.push(format!("`{cle}` inconnu — vouliez-vous dire `{v}` ?")),
            None => inconnus.push(format!("`{cle}` inconnu")),
        }
    }

    let mut manquants: Vec<&str> = Vec::new();
    let vide = |k: &str| {
        obj.get(k)
            .and_then(Value::as_str)
            .map(|v| v.trim().is_empty())
            .unwrap_or(true)
    };
    if vide("idempotency_key") {
        manquants.push("`idempotency_key` (ancre la déduplication at-least-once)");
    }
    if vide("body_dense") {
        manquants.push(
            "`body_dense` (un sujet seul est une impasse : le destinataire lit la \
             revendication et ne peut pas agir dessus)",
        );
    }

    if inconnus.is_empty() && manquants.is_empty() {
        return None;
    }

    let mut msg = String::from("mcp_outbox_send : appel refusé — TOUS les écarts, pas le premier.\n");
    if !inconnus.is_empty() {
        msg.push_str(&format!(
            "\n⛔ Paramètre(s) INCONNU(S), qui seraient ignorés en silence :\n  - {}\n",
            inconnus.join("\n  - ")
        ));
    }
    if !manquants.is_empty() {
        msg.push_str(&format!(
            "\n⛔ Paramètre(s) REQUIS absent(s) :\n  - {}\n",
            manquants.join("\n  - ")
        ));
    }
    msg.push_str(&format!("\nAcceptés : {CONNUS:?}"));
    Some(msg)
}

/// Distance de Levenshtein, bornée à ce dont on a besoin (noms de paramètres courts).
fn distance_edition(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prec: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cour = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cout = usize::from(ca != cb);
            cour.push((prec[j + 1] + 1).min(cour[j] + 1).min(prec[j] + cout));
        }
        prec = cour;
    }
    prec[b.len()]
}

impl McpServer {
    /// REQ-AXO-902278 — refuse a send whose `body_dense` is absent or blank.
    ///
    /// Returns the rejection envelope, or `None` when the body is usable. Kept
    /// separate from the send path so the direct and fan-out branches cannot
    /// drift apart: both are gated by this single call.
    ///
    /// Deliberately NOT a length threshold — "dense" is a judgement the sender
    /// makes, and a minimum character count would only teach padding. The
    /// contract enforced here is the one a reader can act on: there IS a body.
    fn reject_body_less_send(args: &Value) -> Option<Value> {
        let body = args.get("body_dense").and_then(Value::as_str).unwrap_or("");
        if !body.trim().is_empty() {
            return None;
        }
        Some(mbx_err(
            "mcp_outbox_send requires a non-empty `body_dense`. A subject alone is a \
             dead-end: the recipient reads the claim and cannot act on it (PIL-AXO-002). \
             Write the body dense and pointer-bearing — SOLL ids, symbols, commit SHAs, \
             a measured value — rather than inlining content they can retrieve.",
            "input_invalid",
        ))
    }

    /// REQ-AXO-902113 (MBX-1) — send a message to another project's inbox.
    ///
    /// REQ-AXO-902119 (MBX-7) — also the fan-out entry point: when `to_topic`,
    /// `to_room`, or `to_project='*'` is supplied (mutually exclusive with a
    /// concrete `to_project`), the recipient set is resolved AT SEND and one
    /// materialised row is delivered per recipient (see `outbox_fanout`). The
    /// concrete-`to_project` path below is the default point-to-point case and is
    /// preserved verbatim.
    /// REQ-AXO-902386 — reject a `to_project` absent from `ProjectCodeRegistry`,
    /// naming the field and suggesting the nearest canonical code.
    ///
    /// Returns `None` when the recipient is valid, or when the registry cannot be
    /// read at all — an unreachable registry must not silently block the mailbox,
    /// which is the channel used to REPORT that kind of outage.
    fn reject_unknown_recipient(&self, to: &str) -> Option<Value> {
        let known = self.all_project_codes();
        if known.is_empty() || known.iter().any(|c| c == to) {
            return None;
        }
        let suggestion = nearest_project_code(to, &known);
        let hint = match suggestion.as_deref() {
            Some(near) => format!(
                "`{to}` is not a canonical project code. Did you mean `{near}`? \
                 Confirm with `project_registry_lookup project_code=\"{near}\"`."
            ),
            None => format!(
                "`{to}` is not a canonical project code. List the valid ones with \
                 `project_registry_lookup`, or use `to_project=\"*\"` to broadcast."
            ),
        };
        let mut repair = json!({
            "invalid_field": "to_project",
            "supplied_value": to,
            "follow_up_tools": ["project_registry_lookup"],
            "hint": hint,
        });
        if let Some(near) = &suggestion {
            repair["did_you_mean"] = json!(near);
        }
        Some(json!({
            "content": [{ "type": "text", "text": hint }],
            "isError": true,
            "data": {
                "status": "input_invalid",
                "delivered": false,
                "parameter_repair": repair,
            }
        }))
    }

    pub(crate) fn axon_mcp_outbox_send(&self, args: &Value) -> Option<Value> {
        let from = args
            .get("from")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| self.auto_resolve_project_code_str())
            .unwrap_or_default();
        if from.is_empty() {
            return Some(mbx_err(
                "sender project unresolved — pass `from` (cwd-resolution found none).",
                "input_invalid",
            ));
        }

        // REQ-AXO-902278 — a message without a body is a dead-end (PIL-AXO-002).
        //
        // Message #5855 was delivered with `body_dense=""` under the subject
        // "l'index est périmé de 2 jours et les outils structurels rendent FAUX".
        // The recipient could read the alarm and act on nothing: no project, no
        // magnitude, no cure. The sender was not at fault — THIS contract accepted
        // it. `idempotency_key` and `to_project` were already refused when empty;
        // the one field carrying the message's reason to exist was not.
        //
        // Checked BEFORE the fan-out branch on purpose: a broadcast is where a
        // body-less message wastes the most readers.
        // REQ-AXO-902494 — valider TOUT l'appel avant d'en traiter une partie.
        if let Some(msg) = valider_arguments_outbox(args) {
            return Some(mbx_err(&msg, "input_invalid"));
        }
        if let Some(err) = Self::reject_body_less_send(args) {
            return Some(err);
        }

        // MBX-7 fan-out detection. `to_topic` / `to_room` are mutually exclusive with
        // a concrete `to_project`; `to_project='*'` is a registry-wide broadcast.
        let to_topic = args.get("to_topic").and_then(Value::as_str).filter(|s| !s.trim().is_empty());
        let to_room = args.get("to_room").and_then(Value::as_str).filter(|s| !s.trim().is_empty());
        let to_project_raw = args.get("to_project").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        if (to_topic.is_some() || to_room.is_some()) && to_project_raw.is_some() {
            return Some(mbx_err(
                "mcp_outbox_send: `to_topic`/`to_room` are exclusive of `to_project`.",
                "input_invalid",
            ));
        }
        if to_topic.is_some() || to_room.is_some() || to_project_raw == Some("*") {
            return self.outbox_fanout(&from, to_topic, to_room, to_project_raw == Some("*"), args);
        }

        let to = match to_project_raw {
            Some(t) => t.to_string(),
            None => return Some(mbx_err("mcp_outbox_send requires `to_project` (or `to_topic`/`to_room`).", "input_invalid")),
        };

        // REQ-AXO-902386 — the recipient must EXIST before we answer `delivered`.
        //
        // APS addressed two messages to `to_project="AXON"` (a typo for the canonical
        // `AXO`). The server answered `delivered` twice, with no warning, and stored
        // them in a mailbox for a project ABSENT from the registry. Worse, the same
        // response then advertised "📬 2 unread from APS — read with mcp_inbox_read
        // project=AXON" while `project_registry_lookup` said no such project existed:
        // two surfaces of one server contradicting each other in a single reply.
        //
        // Those two lost messages were their most serious findings of the session.
        // A sender who makes this typo believes they have communicated, and nothing
        // tells them otherwise — the dead end PIL-AXO-002 forbids, on the channel VPC
        // made the SINGLE route for inter-project requests on 2026-08-20.
        //
        // The bricks existed on both sides: the registry knows how to say no, and
        // `parameter_repair` knows how to suggest. Only the wiring at the entry point
        // was missing.
        if let Some(err) = self.reject_unknown_recipient(&to) {
            return Some(err);
        }

        // REQ-AXO-902117 (MBX-5) — ACL gate (MECHANISM, default-open). A `deny`
        // rule for (from → to) blocks the send ONLY when AXON_MAILBOX_ACL_ENFORCE
        // is on; otherwise the deny is observe-only (logged) and the message is
        // still delivered. Absence of a deny rule authorises (default-open). The
        // POLICY (default open vs closed, who-may-write) stays operator-owned.
        if self.mailbox_acl_denied(&from, &to) {
            let enforce = acl_enforce_enabled();
            if acl_should_block(enforce, true) {
                return Some(mbx_err(
                    &format!("mcp_outbox_send: ACL denies `{from}` → `{to}` (AXON_MAILBOX_ACL_ENFORCE=1)."),
                    "acl_denied",
                ));
            }
            eprintln!("[mbx5-acl] observe-only: deny rule for {from} → {to} (enforce off); delivering anyway.");
        }
        let idempotency_key = match args.get("idempotency_key").and_then(Value::as_str) {
            Some(k) if !k.trim().is_empty() => k.trim().to_string(),
            _ => {
                return Some(mbx_err(
                    "mcp_outbox_send requires `idempotency_key` (anchors at-least-once dedup).",
                    "input_invalid",
                ))
            }
        };
        let subject = args.get("subject").and_then(Value::as_str).unwrap_or("").to_string();
        let body_dense = args
            .get("body_dense")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let in_reply_to = args
            .get("in_reply_to")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let kind = args.get("kind").and_then(Value::as_str).unwrap_or("message").to_string();
        let priority = args
            .get("priority")
            .and_then(Value::as_str)
            .unwrap_or("normal")
            .to_string();
        let ref_soll_ids = args.get("ref_soll_ids").cloned().unwrap_or_else(|| json!([]));
        let context_in = args.get("context_id").and_then(Value::as_str).unwrap_or("");
        // REQ-AXO-902304 — retention declared by the sender.
        let ttl_hours = args.get("ttl_hours").and_then(Value::as_i64);

        let sent = match self.outbox_send_one(
            &from,
            &to,
            &idempotency_key,
            &subject,
            &body_dense,
            &in_reply_to,
            &kind,
            &priority,
            context_in,
            &ref_soll_ids,
            "",
            "",
            ttl_hours,
        ) {
            Ok(s) => s,
            Err(e) => return Some(mbx_err(&format!("mailbox send failed: {e}"), "degraded")),
        };

        let report = format!(
            "### 📤 mcp_outbox_send\n\n{} → `{}` · message_id=`{}` · context=`{}`{}",
            from,
            to,
            sent.message_id,
            sent.context_id,
            if sent.deduped {
                " · (idempotent no-op: already sent)"
            } else {
                " · delivered"
            }
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "ok",
                "message_id": sent.message_id,
                "context_id": sent.context_id,
                "from": from,
                "to": to,
                "deduped": sent.deduped,
                "sig": sent.sig,
            }
        }))
    }

    /// Materialise ONE mailbox row for a single recipient (build the A2A envelope,
    /// HMAC-sign over the canonical form, idempotent UPSERT). Shared by the
    /// point-to-point path and the MBX-7 fan-out path. `context_in` empty → the
    /// message's own `message_id` becomes the thread id. `topic` / `room_id` empty
    /// → stored NULL (point-to-point); otherwise stamps the fan-out provenance.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn outbox_send_one(
        &self,
        from: &str,
        to: &str,
        idempotency_key: &str,
        subject: &str,
        body_dense: &str,
        in_reply_to: &str,
        kind: &str,
        priority: &str,
        context_in: &str,
        ref_soll_ids: &Value,
        topic: &str,
        room_id: &str,
        ttl_hours: Option<i64>,
    ) -> Result<SentMessage, String> {
        // REQ-AXO-902304 — retention horizon. `axon.mailbox_sweep()` archives on
        // `ttl_at < now()` and has existed all along, but NOTHING ever wrote that
        // column: a purge wired to a field nobody fills. Result: 8217 promote
        // broadcasts accumulated since 2026-07-03, none archived, and twelve
        // projects each carrying 118 of them — 100% of the inbox for four of them.
        //
        // A "MCP goes down in 3 minutes" notice is worthless an hour later, so the
        // sender declares how long its message stays relevant. Absent (`None`), the
        // message is kept indefinitely, which is the right default for anything a
        // human or agent might act on later.
        let ttl_sql = match ttl_hours.filter(|h| *h > 0) {
            Some(hours) => format!("now() + interval '{hours} hours'"),
            None => "NULL".to_string(),
        };
        let message_id = mailbox::message_id(from, to, idempotency_key);
        let context_id = if context_in.is_empty() {
            message_id.clone()
        } else {
            context_in.to_string()
        };

        let canonical = mailbox::canonical(
            from,
            to,
            &context_id,
            &message_id,
            kind,
            idempotency_key,
            in_reply_to,
            subject,
            body_dense,
        );
        // REQ-AXO-902117 (MBX-5) — provision a per-project signing token on first
        // use, then sign under the RESOLVED token (stored secret, else derived
        // fallback). Mechanism only — the HMAC scheme is unchanged.
        self.ensure_project_secret(from);
        let (token, _stored) = self.mailbox_signing_token(from);
        let sig = mailbox::sign_with_token(&token, &canonical);

        // A2A-aligned envelope (DEC-AXO-901663): the dense Axon body rides in a
        // `data` part so A2A interop (Agent Cards, MBX-6) is free later. Fan-out
        // provenance (topic/room_id) rides alongside but is OUT of the signed
        // canonical form, so a recipient's signature check is unaffected.
        let envelope = json!({
            "messageId": message_id,
            "contextId": context_id,
            "role": "agent",
            "kind": kind,
            "from": from,
            "to": to,
            "parts": [{ "kind": "data", "data": {
                "subject": subject,
                "body_dense": body_dense,
                "ref_soll_ids": ref_soll_ids,
            }}],
            "idempotencyKey": idempotency_key,
            "inReplyTo": in_reply_to,
            "topic": if topic.is_empty() { Value::Null } else { json!(topic) },
            "roomId": if room_id.is_empty() { Value::Null } else { json!(room_id) },
            "sig": sig,
        });
        let envelope_lit = esc(&serde_json::to_string(&envelope).unwrap_or_default());

        let sql = format!(
            "INSERT INTO axon.mailbox_message \
             (message_id, context_id, from_project, to_project, kind, subject, body_dense, envelope, idempotency_key, in_reply_to, priority, sig, topic, room_id, ttl_at) \
             VALUES ('{mid}','{ctx}','{from}','{to}','{kind}','{subj}','{body}','{env}'::jsonb,'{idem}','{irt}','{prio}','{sig}',NULLIF('{topic}','')::text,NULLIF('{room}','')::text,{ttl}) \
             ON CONFLICT (from_project, to_project, idempotency_key) DO NOTHING RETURNING id",
            mid = esc(&message_id),
            ctx = esc(&context_id),
            from = esc(from),
            to = esc(to),
            kind = esc(kind),
            subj = esc(subject),
            body = esc(body_dense),
            env = envelope_lit,
            idem = esc(idempotency_key),
            irt = esc(in_reply_to),
            prio = esc(priority),
            sig = esc(&sig),
            ttl = ttl_sql,
            topic = esc(topic),
            room = esc(room_id),
        );
        let rows: Vec<Vec<Value>> = self
            .graph_store
            .query_json_writer(&sql)
            .map(|s| serde_json::from_str(&s).unwrap_or_default())
            .map_err(|e| e.to_string())?;
        Ok(SentMessage {
            message_id,
            context_id,
            deduped: rows.is_empty(),
            sig,
        })
    }

    /// REQ-AXO-902114 (MBX-1/2) — read a project's inbox: `unread` (since the read
    /// cursor, advancing it), `since` (since an explicit id), or `all`.
    pub(crate) fn axon_mcp_inbox_read(&self, args: &Value) -> Option<Value> {
        let project = args
            .get("project")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| self.auto_resolve_project_code_str())
            .unwrap_or_default();
        if project.is_empty() {
            return Some(mbx_err("inbox project unresolved — pass `project`.", "input_invalid"));
        }
        // REQ-AXO-902287 (M1) — disclose when the project was inferred from the cwd
        // (no explicit `project=`), so a cross-project inbox read is never silently
        // scoped to the caller's own project without a visible cue.
        let project_inferred = !args
            .get("project")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty());
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20).clamp(1, 100);
        let mode = args.get("mode").and_then(Value::as_str).unwrap_or("unread");
        let since = args.get("since_id").and_then(Value::as_i64);

        // REQ-AXO-902116 (MBX-4) — searchable threads. `context_id` filters to one
        // thread; `search` is FTS over subject+body. Both are NON-DESTRUCTIVE views
        // across the whole inbox (ignore the cursor, never advance it).
        let thread = args.get("context_id").and_then(Value::as_str).filter(|s| !s.is_empty());
        let search = args.get("search").and_then(Value::as_str).filter(|s| !s.trim().is_empty());
        let view_only = thread.is_some() || search.is_some();

        let floor = if view_only || mode == "all" {
            -1
        } else if let Some(s) = since {
            s
        } else {
            self.graph_store
                .query_single_i64_writer(&format!(
                    "SELECT last_read_id FROM axon.mailbox_cursor WHERE project_code='{}'",
                    esc(&project)
                ))
                .ok()
                .flatten()
                .unwrap_or(0)
        };

        let mut filters = String::new();
        if let Some(t) = thread {
            filters.push_str(&format!(" AND context_id = '{}'", esc(t)));
        }
        if let Some(q) = search {
            filters.push_str(&format!(
                " AND to_tsvector('simple', subject || ' ' || body_dense) @@ plainto_tsquery('simple', '{}')",
                esc(q)
            ));
        }

        // REQ-AXO-902121 (MBX-7) — priority-ordered read: `high` first, then
        // `normal`, then everything else; ties break by id ASC. CURSOR SAFETY: when
        // the read advances the cursor to max(id) of the page (`unread` mode), a
        // priority reorder over a LIMITed page could skip lower-id lower-priority
        // messages that fall below that max(id) → they would be marked read unseen.
        // So priority-ordering is applied ONLY to non-cursor-advancing reads
        // (all/since/thread/search views); `unread` stays strictly id-ordered so the
        // monotone max(id) cursor never skips a message. Archived rows (TTL-swept,
        // see axon.mailbox_sweep) are excluded from the live inbox view.
        let cursor_advances = mode == "unread" && !view_only;
        // REQ-AXO-902495 (doléance VPC #240) — `mode=all` rendait les N plus ANCIENS.
        //
        // Sur une boîte de plusieurs milliers de messages, c'est l'inverse de ce qu'on
        // cherche à une reprise de session. Le rapporteur a dû faire TROIS appels, chacun
        // très volumineux, avec des bornes DEVINÉES — rien dans la réponse ne disait où
        // l'on se situait dans la boîte. Et le risque dépasse le coût : « un LLM pressé
        // qui lit la première page prend des messages de trois semaines pour l'état du
        // jour. J'ai failli traiter des demandes TE2/OPV déjà résolues. »
        //
        // ⚠️ `since` garde l'ordre ASCENDANT : on y avance DEPUIS un point, la progression
        // est le sens même du mode. Seul `all` — la lecture « montre-moi la boîte » — passe
        // aux plus récents. `unread` reste strictement id ASC (sécurité du curseur, voir
        // ci-dessus) : l'inverser ferait sauter des messages sous le max(id).
        let recents_dabord = mode == "all";
        let order_clause = if cursor_advances {
            "ORDER BY id ASC".to_string()
        } else if recents_dabord {
            "ORDER BY CASE priority WHEN 'high' THEN 0 WHEN 'normal' THEN 1 ELSE 2 END, id DESC"
                .to_string()
        } else {
            "ORDER BY CASE priority WHEN 'high' THEN 0 WHEN 'normal' THEN 1 ELSE 2 END, id ASC"
                .to_string()
        };
        let sql = format!(
            // REQ-AXO-902413 — `priority` est SÉLECTIONNÉ parce qu'il est publié.
            // Il était déjà utilisé par l'`ORDER BY` juste au-dessus : le tri se
            // faisait dessus et la valeur restait invisible. Le champ gouverne aussi
            // l'archivage (`priority='high'` échappe à l'archivage auto et au
            // balayage TTL) — un champ qui décide du comportement doit être lisible.
            "SELECT id, message_id, context_id, from_project, kind, idempotency_key, in_reply_to, subject, body_dense, sig, created_at, priority \
             FROM axon.mailbox_message WHERE to_project='{}' AND id > {} AND archived_at IS NULL{} {} LIMIT {}",
            esc(&project),
            floor,
            filters,
            order_clause,
            limit
        );
        let rows: Vec<Vec<Value>> = match self.graph_store.query_json(&sql) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(mbx_err(&format!("inbox read failed: {e}"), "degraded")),
        };

        // REQ-AXO-902419 (doléance TE2 #184, jumelle VPC #181 `blocking`) — `limit`
        // borne le NOMBRE de messages, jamais le VOLUME, et le volume est
        // inconnaissable AVANT l'appel.
        //
        // Mesuré : `limit=31` a rendu **62 390 caractères sur 563 lignes**, au-delà du
        // plafond du client, dérouté vers un fichier que le rapporteur a dû relire en
        // trois passes de `sed`. Sur ces 31 messages, la taille allait de deux lignes
        // (notifications de promote) à quatre-vingts (rapports d'incident) : **aucune
        // valeur de `limit` ne pouvait deviner ça.**
        //
        // Et l'échec tombe au pire moment : `GUI-PRO-102` place la relève d'inbox à
        // l'étape 3c de l'init, donc AVANT tout travail utile, sur la surface dont le
        // rôle est justement d'orienter la session.
        //
        // ⚠️ **La sûreté du curseur tient à un détail** : `max_id` n'est mis à jour que
        // pour les messages RÉELLEMENT rendus. Le curseur et l'archivage travaillent
        // sur `id <= max_id` : un message écarté par le budget n'est donc ni consommé
        // ni archivé, il repasse au prochain appel. Casser la boucle AVANT d'avoir
        // touché `max_id` est ce qui rend cette troncature non destructive — un test
        // le verrouille.
        let budget_chars = args
            .get("budget_chars")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(20_000);
        let mut volume = 0usize;
        let mut ecartes_budget = 0usize;

        let mut messages: Vec<Value> = Vec::with_capacity(rows.len());
        let mut body_lines = String::new();
        let mut max_id = floor;
        for row in &rows {
            let id = row.first().map(entier_json).unwrap_or(0);
            let g = |i: usize| row.get(i).and_then(Value::as_str).unwrap_or("");
            // REQ-AXO-902419 — décider AVANT de toucher `max_id`. Le premier message
            // passe toujours, quelle que soit sa taille : rendre zéro message parce que
            // le premier dépasse le budget serait remplacer un débordement par un
            // blocage, et le lecteur n'aurait aucun moyen d'avancer.
            let taille = g(7).len() + g(8).len() + 120;
            if !messages.is_empty() && volume + taille > budget_chars {
                ecartes_budget = rows.len() - messages.len();
                break;
            }
            volume += taille;
            max_id = max_id.max(id);
            let (message_id, context_id, from, kind, idem, irt, subject, body, sig) =
                (g(1), g(2), g(3), g(4), g(5), g(6), g(7), g(8), g(9));
            let canonical =
                mailbox::canonical(from, &project, context_id, message_id, kind, idem, irt, subject, body);
            let verified = self.mailbox_verify(from, &canonical, sig);
            messages.push(json!({
                "id": id,
                "message_id": message_id,
                "context_id": context_id,
                "from": from,
                "kind": kind,
                "in_reply_to": irt,
                "subject": subject,
                "body_dense": body,
                "created_at": g(10),
                // REQ-AXO-902413 — signalé par VPC : le champ existe, il gouverne
                // l'archivage, il n'était pas publié. Sans lui un automate ne peut
                // que tout notifier (interdit) ou deviner l'urgence par mots-clés.
                "priority": g(11),
                "signature_verified": verified,
            }));
            // REQ-AXO-902145 — render each body into the TEXT channel (content[0].text),
            // not only structuredContent : LLM clients consume the text channel, so a
            // count-only text reads as "messages sans corps" even on a successful read.
            // The explicit pull is where the content is meant to land.
            let sig_mark = if verified { "✓" } else { "✗ sig" };
            let reply = if irt.is_empty() { String::new() } else { format!(" ↩ {irt}") };
            // REQ-AXO-902413 — la priorité dans le TEXTE aussi : le tri se fait
            // dessus, donc un lecteur qui ne la voit pas ne comprend pas l'ordre
            // qu'on lui sert.
            let prio = match g(11) {
                "high" => " · ⚠️ HAUTE",
                "low" => " · basse",
                _ => "",
            };
            body_lines.push_str(&format!(
                "\n\n**[{id}] {from} → {subject}** ({kind}, {sig_mark}{reply}{prio})\n{body}"
            ));
        }

        // Advance the read cursor only in `unread` mode (so `since`/`all`/search/
        // thread are non-destructive views). UPSERT, monotonic.
        let mut archived_count: i64 = 0;
        // REQ-AXO-902485 — déclaré au niveau du rapport, pas dans la branche : c'est
        // le rapport qui doit les nommer.
        let mut archives_nommes: Vec<(i64, String)> = Vec::new();
        if cursor_advances && max_id > floor {
            let _ = self.graph_store.execute(&format!(
                "INSERT INTO axon.mailbox_cursor (project_code, last_read_id, updated_at) \
                 VALUES ('{p}', {mid}, now()) \
                 ON CONFLICT (project_code) DO UPDATE SET \
                   last_read_id = GREATEST(axon.mailbox_cursor.last_read_id, EXCLUDED.last_read_id), \
                   updated_at = now()",
                p = esc(&project),
                mid = max_id
            ));

            // REQ-AXO-902306 — a message that has been READ leaves the inbox.
            //
            // Advancing the cursor alone left every read message sitting in the
            // active inbox, so only the TTL ever removed anything. And the TTL is an
            // ABSOLUTE clock: a project dormant for longer than the horizon lost a
            // notice it had never seen. Archiving on read fixes both — the inbox
            // reflects what is still to be dealt with.
            //
            // `priority='high'` is EXEMPT: an important message never disappears on
            // its own, neither by reading nor by expiry. It takes a deliberate
            // gesture. Wrong-way-safe by design — if this reading of the intent is
            // off, we keep too much, never too little.
            //
            // Only this branch archives: `all` / `since` / search / thread views are
            // non-destructive by contract (test C4) and must stay so.
            //
            // Counted BEFORE the UPDATE (same predicate) so the report can SAY what it
            // removed. Archiving on read is a state change the caller did not spell
            // out; leaving it silent would be the same trust loss any undisclosed
            // input normalisation is (cf. `disclose_cwd_provenance`, mcp.rs).
            // REQ-AXO-902485 (doléance DGD #266) — le COMPTE ne suffit pas : il faut
            // NOMMER ce qui disparaît. « Un effet de bord sur des données, réduit à un
            // compteur » — on sait que trois messages ont quitté la boîte, pas
            // lesquels, donc on ne peut ni les retrouver ni juger si c'était grave.
            //
            // `count(*) OVER ()` rend le total EXACT dans la même passe que
            // l'échantillon : une seule requête, un compte non tronqué, une liste
            // bornée qui le dit (invariant KKI #204 — un total et un échantillon ne
            // sont pas le même nombre et ne doivent pas se lire pareil).
            archives_nommes = self
                .graph_store
                .query_json(&format!(
                    "SELECT id, COALESCE(subject,'(sans sujet)'), count(*) OVER () \
                     FROM axon.mailbox_message \
                     WHERE to_project='{p}' AND id <= {mid} AND id > {floor} \
                       AND archived_at IS NULL AND COALESCE(priority,'') <> 'high' \
                     ORDER BY id DESC LIMIT 8",
                    p = esc(&project),
                    mid = max_id,
                    floor = floor
                ))
                .ok()
                .and_then(|s| serde_json::from_str::<Vec<Vec<Value>>>(&s).ok())
                .map(|rows| {
                    // Le total vient de la fenêtre, pas de `rows.len()` : la liste est
                    // plafonnée à 8, le compte ne l'est pas.
                    if let Some(first) = rows.first() {
                        archived_count = first
                            .get(2)
                            .and_then(|cell| match cell {
                                Value::Number(n) => n.as_i64(),
                                Value::String(s) => s.parse::<i64>().ok(),
                                _ => None,
                            })
                            .unwrap_or(0);
                    }
                    rows.into_iter()
                        .filter_map(|row| {
                            let id = row.first().and_then(|c| match c {
                                Value::Number(n) => n.as_i64(),
                                Value::String(s) => s.parse::<i64>().ok(),
                                _ => None,
                            })?;
                            let sujet = row
                                .get(1)
                                .and_then(|c| c.as_str())
                                .unwrap_or("(sans sujet)")
                                .to_string();
                            Some((id, sujet))
                        })
                        .collect()
                })
                .unwrap_or_default();

            let _ = self.graph_store.execute(&format!(
                "UPDATE axon.mailbox_message SET archived_at = now() \
                 WHERE to_project='{p}' AND id <= {mid} AND id > {floor} \
                   AND archived_at IS NULL AND COALESCE(priority,'') <> 'high'",
                p = esc(&project),
                mid = max_id,
                floor = floor
            ));
        }

        // REQ-AXO-902419 — un lot tronqué le DIT, avec le geste exact pour la suite.
        // Un troncage silencieux ici serait pire qu'ailleurs : le lecteur croirait sa
        // boîte vide et passerait à côté de rapports d'incident.
        let note_budget = if ecartes_budget > 0 {
            format!(
                "\n\n⚠️ **Lot borné en VOLUME** : {volume} caractères rendus (budget \
                 {budget_chars}), **{ecartes_budget} message(s) non rendus**. Ils ne sont \
                 NI consommés NI archivés — rappelle `mcp_inbox_read` pour la suite, ou \
                 passe `budget_chars=<N>` pour un lot plus gros. `limit` borne le nombre \
                 de messages, pas leur taille (REQ-AXO-902419)."
            )
        } else {
            String::new()
        };

        let report = format!(
            "### 📥 mcp_inbox_read\n\n`{}`{} · mode={} · {} message(s){}{}{}",
            project,
            if project_inferred {
                " _(déduit du cwd — passe `project=` pour un autre)_"
            } else {
                ""
            },
            mode,
            messages.len(),
            // REQ-AXO-902306 — the banner states what the read actually DID, and only
            // that. It used to key off `mode == "unread"` alone, so a search or thread
            // view (mode defaults to `unread`, `view_only` true) announced a cursor
            // advance that never happened — the reader believed the inbox consumed. It
            // now keys off `cursor_advances`, the same flag the write path uses, and
            // names the archived/kept split so "read = gone" is visible where it occurs.
            if cursor_advances && max_id > floor {
                let kept = (messages.len() as i64 - archived_count).max(0);
                let kept_note = if kept > 0 {
                    format!(
                        " · {kept} conservé(s) — important(s), à retirer par \
                         `mcp_inbox_archive message_ids=[…]`"
                    )
                } else {
                    String::new()
                };
                // REQ-AXO-902485 — les nommer, bornés, avec le total exact à côté.
                let liste = if archives_nommes.is_empty() {
                    String::new()
                } else {
                    let compte = crate::mcp::format::Compte::borne(
                        archived_count.max(0) as usize,
                        archives_nommes.len(),
                    );
                    format!(
                        "\n\nArchivés par cette lecture ({}) — récupérables par \
                         `mcp_inbox_read mode=all` :\n{}",
                        compte.rendre(),
                        archives_nommes
                            .iter()
                            .map(|(id, sujet)| format!("- [{id}] {sujet}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                };
                format!(
                    " · cursor advanced to {max_id} · {archived_count} archivé(s){kept_note}{liste}"
                )
            } else {
                // REQ-AXO-902495 (doléance VPC #240) — DIRE OÙ L'ON SE SITUE.
                //
                // Sans repère, le rapporteur a dû DEVINER ses bornes : trois appels, trois
                // sorties denses, pour atteindre le récent. « Rien dans la réponse ne dit où
                // on se situe dans la boîte (ni total, ni id max, ni "il reste N plus
                // récents") ». Avec cette ligne, le deuxième appel est EXACT au lieu d'être
                // deviné — et le coût tombe de trois appels à deux.
                let total_boite = self
                    .graph_store
                    .query_json(&format!(
                        "SELECT count(*), COALESCE(max(id),0) FROM axon.mailbox_message \
                         WHERE to_project='{}' AND archived_at IS NULL",
                        esc(&project)
                    ))
                    .ok()
                    .and_then(|r| serde_json::from_str::<Vec<Vec<Value>>>(&r).ok())
                    .and_then(|rows| rows.first().cloned());
                match total_boite {
                    Some(r) if r.len() >= 2 => {
                        // REQ-AXO-902509 — c'est CE site qui n'avait pas le repli,
                        // d'où « 2 sur 0 · id max 0 » sur une boîte de 70 messages.
                        let total = entier_json(&r[0]);
                        let id_max = entier_json(&r[1]);
                        let restants = (total - messages.len() as i64).max(0);
                        format!(
                            " · {} sur {total} · id max {id_max} · {restants} non listé(s)",
                            messages.len()
                        )
                    }
                    _ => String::new(),
                }
            },
            if messages.is_empty() {
                "\n\n(aucun message)".to_string()
            } else {
                body_lines
            },
            note_budget
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                // REQ-AXO-902419 — l'automate ne lit pas la prose : le débordement de
                // budget doit être une DONNÉE, pas seulement une phrase.
                "budget_chars": budget_chars,
                "volume_chars": volume,
                "messages_non_rendus_budget": ecartes_budget,
                "status": "ok",
                "project": project,
                "mode": mode,
                "count": messages.len(),
                "cursor": max_id,
                "messages": messages,
            }
        }))
    }

    /// MBX-2 helper — count of unread messages for `project` (id > read cursor).
    /// Surfaced by `status` / `axon_init_project` so a waking session sees its
    /// inbox without an explicit read.
    pub(crate) fn mailbox_unread_count(&self, project: &str) -> i64 {
        self.graph_store
            .query_single_i64_writer(&format!(
                "SELECT count(*) FROM axon.mailbox_message m \
                 LEFT JOIN axon.mailbox_cursor c ON c.project_code = m.to_project \
                 WHERE m.to_project='{p}' AND m.id > COALESCE(c.last_read_id, 0) \
                 AND m.archived_at IS NULL",
                p = esc(project)
            ))
            .ok()
            .flatten()
            .unwrap_or(0)
    }

    /// REQ-AXO-902143 (MBX réactivité niveau-2) — assemble the unread *signal*
    /// for `project` in a single cheap query: count + distinct senders + the
    /// newest message id (pointer). Returns `None` when there is nothing unread
    /// — the no-op fast path that keeps the dispatch banner zero-cost on
    /// sessions with no mail. SIGNAL ONLY: the message body is never returned;
    /// the recipient pulls it with an explicit `mcp_inbox_read`.
    pub(crate) fn mailbox_unread_banner(&self, project: &str) -> Option<Value> {
        if project.is_empty() {
            return None;
        }
        let sql = format!(
            "SELECT count(*)::bigint, \
                    COALESCE(string_agg(DISTINCT m.from_project, ','), ''), \
                    COALESCE(max(m.id), 0)::bigint \
             FROM axon.mailbox_message m \
             LEFT JOIN axon.mailbox_cursor c ON c.project_code = m.to_project \
             WHERE m.to_project='{p}' AND m.id > COALESCE(c.last_read_id, 0) \
             AND m.archived_at IS NULL",
            p = esc(project)
        );
        let json_str = self.graph_store.query_json(&sql).ok()?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&json_str).ok()?;
        let row = rows.into_iter().next()?;
        let as_i64 = |v: Option<&Value>| v.map(entier_json).unwrap_or(0);
        let count = as_i64(row.first());
        if count <= 0 {
            return None;
        }
        let senders: Vec<&str> = row
            .get(1)
            .and_then(Value::as_str)
            .unwrap_or("")
            .split(',')
            .filter(|s| !s.is_empty())
            .collect();
        let latest_id = as_i64(row.get(2));
        let from_label = if senders.is_empty() {
            String::new()
        } else {
            format!(" de {}", senders.join(", "))
        };
        // REQ-AXO-902145 — no dead-end (PIL-AXO-002) : the banner names the exact
        // read invocation AND the recovery when that tool is missing from a stale
        // client binding (the tool IS advertised server-side in catalog.rs, but the
        // session bound an older catalogue → reconnect re-fetches tools/list).
        // Without this, a stale client sees "N non-lus" with no reachable way to
        // read the bodies — exactly the reported friction.
        let banner = format!(
            "📬 {count} message(s) non-lu(s){from_label} — relève avec `mcp_inbox_read project={project}` (signal seul, corps non inliné). Si `mcp_inbox_read` est absent de ta session : reconnecte ton client MCP (binding de catalogue stale)."
        );
        Some(json!({
            "unread": count,
            "from": senders,
            "latest_id": latest_id,
            "pointer": { "tool": "mcp_inbox_read", "arguments": { "project": project } },
            "on_tool_absent": "reconnect MCP client to refresh the tool catalogue (stale binding) — the read tool is advertised server-side (catalog.rs)",
            "banner": banner,
        }))
    }

    /// REQ-AXO-902143 (MBX réactivité niveau-2) — awareness piggyback at the
    /// central dispatch chokepoint. Attaches the unread mailbox signal to EVERY
    /// tool envelope so an actively-working session learns it has mail on its
    /// next tool call, instead of only at `status` / `axon_init_project`.
    ///
    /// Contract (decided with operator, REQ-AXO-902143):
    /// - TARGETED: recipient = the session's project (explicit `project` /
    ///   `project_code` arg wins, else cwd auto-resolution). A session never
    ///   sees another project's mail → zero cross-project token cost.
    /// - unread>0 ONLY: no mail → no-op, zero added tokens (the nominal case).
    /// - SIGNAL ONLY: count + senders + pointer, never the body.
    /// - PROJECT granularity: all sessions of the recipient project see it;
    ///   double-processing is prevented by advisory leases (MBX-8), not here.
    ///
    /// Tools that already surface the inbox (`status` / wake) or that ARE the
    /// mailbox surface skip the banner to avoid redundant noise.
    pub(crate) fn attach_mailbox_unread_banner(
        &self,
        normalized_name: &str,
        arguments: &Value,
        mut response: Value,
    ) -> Value {
        const SKIP: &[&str] = &[
            "status",
            "axon_init_project",
            "mcp_inbox_read",
            "inbox_read",
            "mailbox_sweep",
            // REQ-AXO-902308 — an archive response already states the inbox state it
            // just changed; stapling "📬 N non-lu(s) — relève avec mcp_inbox_read"
            // onto it would invite the caller straight back into a destructive read.
            "mcp_inbox_archive",
            "mailbox_render",
            "mailbox_tap",
        ];
        if SKIP.contains(&normalized_name) {
            return response;
        }

        // REQ-AXO-902497 (doléances DVM #258 ET VPC #249 — convergence) — une
        // notification se justifie par un DELTA, pas par une occasion de parler.
        //
        // Mesuré chez les deux : le bandeau apparaissait sur 8 sorties (DVM) et une
        // VINGTAINE (VPC), avec un compte FIGÉ — « 17 » huit fois, « 3 » vingt fois. Soit
        // ~1 200 jetons pour une information qui en vaut 15, une seule fois. VPC nomme le
        // vrai coût, et ce n'est pas le prix :
        //
        // > « Le problème n'est pas le coût, c'est L'HABITUATION. Un signal qui se répète à
        // > l'identique quel que soit le contexte devient du décor : je l'ai filtré
        // > mentalement dès la troisième occurrence. Le jour où le compte passera de 3 à 40,
        // > il sera dans la même police, au même endroit, après un `sql` en échec. »
        //
        // Deux règles, chacune tirée d'une observation :
        //   1. JAMAIS sur une réponse d'ERREUR — « l'appelant est en train de réparer autre
        //      chose ; le courrier non lu est la dernière chose qui doit occuper la ligne
        //      suivante ».
        //   2. Seulement si le compte a AUGMENTÉ depuis la dernière émission de ce process.
        //      Un courrier qui ARRIVE pendant qu'on travaille est une nouvelle ;
        //      « toujours 3 » n'en est pas une.
        let est_erreur = response
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || response
                .pointer("/data/status")
                .and_then(Value::as_str)
                .map(|s| {
                    matches!(
                        s,
                        "input_invalid" | "input_not_found" | "wrong_project_scope"
                            | "degraded" | "error" | "rejected_all" | "writer_failed"
                    )
                })
                .unwrap_or(false);
        if est_erreur {
            return response;
        }
        let project = arguments
            .get("project")
            .or_else(|| arguments.get("project_code"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| self.auto_resolve_project_code_str())
            .unwrap_or_default();
        let Some(banner) = self.mailbox_unread_banner(&project) else {
            return response; // unread == 0 → no-op
        };

        // Le DELTA : n'émettre que si le compte a monté depuis la dernière émission.
        // État de PROCESS, volontairement : « depuis le début de cette session » est la
        // bonne granularité — c'est la session qui subit la répétition.
        {
            static DERNIER_COMPTE: std::sync::OnceLock<
                std::sync::Mutex<std::collections::HashMap<String, i64>>,
            > = std::sync::OnceLock::new();
            let compte = banner
                .get("unread")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let carte = DERNIER_COMPTE.get_or_init(|| {
                std::sync::Mutex::new(std::collections::HashMap::new())
            });
            let mut carte = match carte.lock() {
                Ok(c) => c,
                // Un mutex empoisonné ne doit pas TAIRE le bandeau : il échouerait
                // fermé, et une vraie arrivée passerait inaperçue. On reprend l'état.
                Err(p) => p.into_inner(),
            };
            match carte.get(&project) {
                Some(precedent) if compte <= *precedent => return response,
                _ => {
                    carte.insert(project.clone(), compte);
                }
            }
        }
        let line = banner.get("banner").and_then(Value::as_str).map(str::to_string);
        if let Some(obj) = response.as_object_mut() {
            // Structured channel.
            match obj.get_mut("data").and_then(Value::as_object_mut) {
                Some(data) => {
                    data.insert("mailbox".to_string(), banner);
                }
                None => {
                    obj.insert("data".to_string(), json!({ "mailbox": banner }));
                }
            }
            // Text channel — where the LLM (and HTTP/curl clients) read.
            if let Some(line) = line {
                if let Some(first) = obj
                    .get_mut("content")
                    .and_then(Value::as_array_mut)
                    .and_then(|c| c.first_mut())
                    .and_then(|c| c.as_object_mut())
                {
                    if let Some(text) = first.get("text").and_then(Value::as_str) {
                        let merged = format!("{text}\n\n{line}");
                        first.insert("text".to_string(), Value::String(merged));
                    }
                }
            }
        }
        response
    }

    /// REQ-AXO-902119 (MBX-7) — TTL / dead-letter sweep. Soft-archives every
    /// message whose retention horizon (`ttl_at`) has passed by stamping
    /// `archived_at = now()` (the append-only log is preserved — archived rows
    /// just drop out of the live inbox view). Idempotent: a second call within
    /// the same window archives nothing. Returns the count swept this pass.
    pub(crate) fn axon_mailbox_sweep(&self, _args: &Value) -> Option<Value> {
        let rows: Vec<Vec<Value>> = match self.graph_store.query_json_writer("SELECT axon.mailbox_sweep()") {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(mbx_err(&format!("mailbox sweep failed: {e}"), "degraded")),
        };
        let swept = rows
            .first()
            .and_then(|r| r.first())
            .map(entier_json)
            .unwrap_or(0);
        let report = format!(
            "### 🧹 mailbox_sweep\n\n{swept} expired message(s) archived (ttl_at < now)."
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "ok",
                "swept": swept,
            }
        }))
    }

    /// REQ-AXO-902308 — remove NAMED messages from the active inbox.
    ///
    /// The deliberate counterpart to REQ-AXO-902306's two automatic exits: reading
    /// archives ordinary messages, the TTL sweep archives expired ones, and
    /// `priority='high'` is exempt from BOTH. Without this verb `high` would be
    /// unremovable by any tool — the very accumulation REQ-AXO-902304 measured,
    /// displaced one notch. And the read banner already tells the caller an
    /// important message is "à retirer délibérément"; a contract that names an exit
    /// it does not offer is the dead end REQ-AXO-902278 closed elsewhere.
    ///
    /// Ids only, on purpose. A predicate ("archive everything read", "archive all
    /// high older than X") would re-create the automatic removal this rule exists to
    /// prevent: what is kept deliberately must be released deliberately.
    ///
    /// Ids that do not belong to `project` are REFUSED — the whole call, not a
    /// silent per-row skip. A partially-applied archive whose report says "ok" is
    /// how a caller learns to distrust the count.
    pub(crate) fn axon_mcp_inbox_archive(&self, args: &Value) -> Option<Value> {
        let project = args
            .get("project")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| self.auto_resolve_project_code_str())
            .unwrap_or_default();
        if project.is_empty() {
            return Some(mbx_err("inbox project unresolved — pass `project`.", "input_invalid"));
        }

        // Accept a bare integer as well as an array: a caller archiving ONE message
        // naturally writes `message_ids: 8879`. Rejecting that would be a round-trip
        // over a difference with no meaning (same normalisation family as
        // SCALAR_TO_ARRAY_PARAMS in mcp.rs).
        let raw = args.get("message_ids").or_else(|| args.get("ids"));
        let ids: Vec<i64> = match raw {
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_i64).collect(),
            Some(Value::Number(n)) => n.as_i64().into_iter().collect(),
            Some(Value::String(s)) => s.parse::<i64>().ok().into_iter().collect(),
            _ => Vec::new(),
        };
        if ids.is_empty() {
            return Some(mbx_err(
                "mcp_inbox_archive requires `message_ids` — the inbox row ids to remove, \
                 as shown between brackets by `mcp_inbox_read` (e.g. `[8879]` → 8879). \
                 There is no bulk form: an important message leaves one named gesture at a time.",
                "input_invalid",
            ));
        }

        let id_list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");

        // Ownership check BEFORE the write: an id addressed to another project is a
        // caller error worth naming, not a row to skip.
        let owned: Vec<Vec<Value>> = match self.graph_store.query_json_writer(&format!(
            "SELECT id FROM axon.mailbox_message WHERE id IN ({id_list}) AND to_project='{p}'",
            p = esc(&project)
        )) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(mbx_err(&format!("inbox archive failed: {e}"), "degraded")),
        };
        let owned_ids: Vec<i64> = owned
            .iter()
            .filter_map(|r| r.first())
            .map(entier_json)
            .filter(|id| *id > 0)
            .collect();
        let foreign: Vec<i64> = ids.iter().copied().filter(|i| !owned_ids.contains(i)).collect();
        if !foreign.is_empty() {
            let listed = foreign.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
            return Some(mbx_err(
                &format!(
                    "these ids are not in `{project}`'s inbox (unknown, or addressed to another \
                     project): {listed}. Nothing was archived — pass `project=` for the recipient \
                     that actually holds them."
                ),
                "input_invalid",
            ));
        }

        // Soft-archive only: the append-only log (PIL-AXO-9004) stays intact, the rows
        // just drop out of the live inbox view. `archived_at IS NULL` makes it
        // idempotent — re-archiving reports 0, it does not restamp.
        let before = self.mailbox_live_count(&project, &id_list);
        if let Err(e) = self.graph_store.execute(&format!(
            "UPDATE axon.mailbox_message SET archived_at = now() \
             WHERE to_project='{p}' AND id IN ({id_list}) AND archived_at IS NULL",
            p = esc(&project)
        )) {
            return Some(mbx_err(&format!("inbox archive failed: {e}"), "degraded"));
        }
        let after = self.mailbox_live_count(&project, &id_list);
        let archived = (before - after).max(0);
        let already = (ids.len() as i64 - before).max(0);

        let already_note = if already > 0 {
            format!(" · {already} déjà archivé(s), inchangé(s)")
        } else {
            String::new()
        };
        let report = format!(
            "### 🗄️ mcp_inbox_archive\n\n`{project}` · {archived} message(s) retiré(s) de l'inbox \
             active{already_note}.\n\nArchivage doux : les lignes restent dans le journal \
             (`mode=all` les rend encore), elles ne comptent plus comme à traiter."
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "ok",
                "project": project,
                "archived": archived,
                "already_archived": already,
                "message_ids": ids,
            }
        }))
    }

    /// Count of still-live (non-archived) rows among `id_list` for `project`.
    /// Writer-side so it observes the same transaction as the UPDATE around it.
    fn mailbox_live_count(&self, project: &str, id_list: &str) -> i64 {
        self.graph_store
            .query_json_writer(&format!(
                "SELECT count(*) FROM axon.mailbox_message \
                 WHERE to_project='{p}' AND id IN ({id_list}) AND archived_at IS NULL",
                p = esc(project)
            ))
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<Vec<Value>>>(&s).ok())
            .and_then(|rows| rows.into_iter().next())
            .and_then(|row| row.into_iter().next())
            .and_then(|cell| match cell {
                Value::Number(n) => n.as_i64(),
                Value::String(s) => s.parse::<i64>().ok(),
                _ => None,
            })
            .unwrap_or(0)
    }

    // ===================================================================
    // REQ-AXO-902117 (MBX-5) — per-project signing secret + ACL (MECHANISM).
    // Token resolution is kept DB-side here so `crate::mailbox` stays pure;
    // the HMAC scheme is unchanged (confidentiality/H1/JWS = deferred POLICY).
    // ===================================================================

    /// Provision a random 32-byte signing token for `project` on first use.
    /// Idempotent (`ON CONFLICT DO NOTHING`): an existing secret is NEVER rotated
    /// here. Best-effort — a provisioning failure leaves the derived-token
    /// fallback intact, so sends keep working.
    pub(crate) fn ensure_project_secret(&self, project: &str) {
        use rand::RngCore;
        let mut token = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut token);
        let hex: String = token.iter().map(|b| format!("{b:02x}")).collect();
        let _ = self.graph_store.execute(&format!(
            "INSERT INTO axon.project_secret (project_code, token) \
             VALUES ('{p}', decode('{h}','hex')) ON CONFLICT (project_code) DO NOTHING",
            p = esc(project),
            h = hex
        ));
    }

    /// Resolve the signing token for `project`: the stored per-project secret
    /// (`axon.project_secret`, projected as `encode(token,'hex')` and decoded), or
    /// the derived fallback when no row exists. Returns `(token, is_stored)`.
    pub(crate) fn mailbox_signing_token(&self, project: &str) -> (Vec<u8>, bool) {
        let stored = self
            .graph_store
            .query_json_writer(&format!(
                "SELECT encode(token,'hex') FROM axon.project_secret WHERE project_code='{}'",
                esc(project)
            ))
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<Vec<Value>>>(&s).ok())
            .and_then(|rows| rows.into_iter().next())
            .and_then(|row| row.into_iter().next())
            .and_then(|v| v.as_str().map(str::to_string))
            .and_then(|hex| mailbox::decode_hex(&hex));
        match stored {
            Some(tok) if !tok.is_empty() => (tok, true),
            _ => (mailbox::derived_project_token(project), false),
        }
    }

    /// Verify a sender's signature with retro-compat: try the resolved token
    /// (stored or derived); if a STORED token exists and the check fails, also try
    /// the derived token so messages signed BEFORE the project was provisioned
    /// still verify.
    pub(crate) fn mailbox_verify(&self, from: &str, canonical: &str, sig: &str) -> bool {
        let (token, is_stored) = self.mailbox_signing_token(from);
        if mailbox::verify_with_token(&token, canonical, sig) {
            return true;
        }
        if is_stored {
            return mailbox::verify_with_token(&mailbox::derived_project_token(from), canonical, sig);
        }
        false
    }

    /// Does a `deny` ACL rule exist for the (from → to) edge? Default-open: the
    /// ABSENCE of a deny row authorises.
    pub(crate) fn mailbox_acl_denied(&self, from: &str, to: &str) -> bool {
        self.graph_store
            .query_single_i64_writer(&format!(
                "SELECT count(*) FROM axon.mailbox_acl \
                 WHERE from_project='{f}' AND to_project='{t}' AND mode='deny'",
                f = esc(from),
                t = esc(to)
            ))
            .ok()
            .flatten()
            .unwrap_or(0)
            > 0
    }
}

/// REQ-AXO-902117 (MBX-5) — pure ACL gate decision. A send is BLOCKED only when a
/// deny rule exists AND enforcement is on. Observe-only (`enforce=false`) never
/// blocks; default-open is the absence of a deny rule. POLICY (default open vs
/// closed, who-may-write) stays operator-owned via the rules table + the
/// `AXON_MAILBOX_ACL_ENFORCE` flag.
pub(crate) fn acl_should_block(enforce: bool, deny_rule_exists: bool) -> bool {
    enforce && deny_rule_exists
}

/// Read the MBX-5 ACL enforcement flag (`AXON_MAILBOX_ACL_ENFORCE`; default unset
/// = observe-only). Truthy = `1`/`true`/`yes`/`on` (case-insensitive).
fn acl_enforce_enabled() -> bool {
    matches!(
        std::env::var("AXON_MAILBOX_ACL_ENFORCE")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod mbx5_tests {
    use super::acl_should_block;

    #[test]
    fn acl_default_open_passes() {
        // No deny rule → never blocked, regardless of enforcement.
        assert!(!acl_should_block(true, false));
        assert!(!acl_should_block(false, false));
    }

    #[test]
    fn acl_deny_blocks_only_when_enforced() {
        // Deny rule present: blocks under enforce, observe-only when not.
        assert!(acl_should_block(true, true));
        assert!(!acl_should_block(false, true));
    }
}

#[cfg(test)]
mod req_902386_recipient_validation_tests {
    use super::nearest_project_code;

    fn registry() -> Vec<String> {
        ["AXO", "APS", "VPC", "NEX", "OPV", "KKI", "BKS"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn the_aps_typo_suggests_the_canonical_code() {
        // Le cas EXACT : APS a écrit `AXON`, le serveur a répondu « delivered », et
        // leurs deux signalements les plus graves ont disparu dans une boîte
        // fantôme (inbox 11934).
        assert_eq!(
            nearest_project_code("AXON", &registry()),
            Some("AXO".to_string())
        );
    }

    #[test]
    fn case_alone_resolves() {
        assert_eq!(nearest_project_code("axo", &registry()), Some("AXO".to_string()));
        assert_eq!(nearest_project_code("Vpc", &registry()), Some("VPC".to_string()));
    }

    #[test]
    fn a_truncated_code_resolves() {
        // L'autre sens du préfixe : `AX` est le début de `AXO`.
        assert_eq!(nearest_project_code("AX", &registry()), Some("AXO".to_string()));
    }

    #[test]
    fn one_substituted_character_resolves() {
        assert_eq!(nearest_project_code("AXQ", &registry()), Some("AXO".to_string()));
    }

    #[test]
    fn two_differences_suggest_nothing_at_all() {
        // LA garde. Sur un code de 3 caractères, deux différences ne sont pas une
        // faute de frappe : c'est un autre code. Suggérer ici enverrait le message
        // au MAUVAIS projet — pire que ne rien suggérer, puisque l'expéditeur
        // suivrait la suggestion avec confiance.
        assert_eq!(nearest_project_code("XYZ", &registry()), None);
        assert_eq!(nearest_project_code("ZZZ", &registry()), None);
    }

    #[test]
    fn an_exact_code_is_returned_by_the_case_rule() {
        // Un code valide n'atteint jamais cette fonction en production (l'appelant
        // vérifie l'appartenance d'abord), mais la fonction doit rester correcte
        // isolément — elle est publique dans le module et testable seule.
        assert_eq!(nearest_project_code("NEX", &registry()), Some("NEX".to_string()));
    }

    #[test]
    fn an_empty_registry_suggests_nothing() {
        // Registre illisible : l'appelant laisse passer le message plutôt que de
        // bloquer le canal qui sert justement à SIGNALER ce genre de panne.
        assert_eq!(nearest_project_code("AXON", &[]), None);
    }
}

#[cfg(test)]
mod tests_validation_outbox {
    use super::valider_arguments_outbox;
    use serde_json::json;

    /// REQ-AXO-902494 (doléance APS #238) — le cas EXACT du rapporteur.
    ///
    /// Appel fautif : `to` (au lieu de `to_project`), `body` (au lieu de
    /// `body_dense`), et pas d'`idempotency_key`. L'ancien refus ne parlait QUE
    /// de `body_dense` ; `to` était **ignoré sans un mot**.
    ///
    /// « Le destinataire est le paramètre dont une erreur ne se rattrape pas. »
    #[test]
    fn a_call_with_three_faults_is_refused_once_naming_all_three() {
        let msg = valider_arguments_outbox(&json!({
            "to": "AXO",
            "subject": "…",
            "body": "<mon message complet>"
        }))
        .expect("un appel à trois défauts doit être refusé");

        assert!(msg.contains("`to`"), "le destinataire fautif n'est pas nommé : {msg}");
        assert!(
            msg.contains("to_project"),
            "le voisin de `to` n'est pas proposé — la correction reste à deviner : {msg}"
        );
        assert!(msg.contains("`body`"), "`body` ignoré en silence : {msg}");
        assert!(msg.contains("body_dense"), "le voisin de `body` manque : {msg}");
        assert!(
            msg.contains("idempotency_key"),
            "le manquement requis n'est pas signalé dans le MÊME refus : {msg}"
        );
    }

    /// Un appel valide ne doit RIEN déclencher — sinon la garde bloque le travail
    /// qu'elle prétend protéger.
    #[test]
    fn a_valid_call_is_not_refused() {
        assert!(valider_arguments_outbox(&json!({
            "to_project": "AXO",
            "subject": "sujet",
            "body_dense": "corps",
            "idempotency_key": "k-1",
            "priority": "high",
            "ttl_hours": 6
        }))
        .is_none());
    }

    /// ⚠️ Ne pas suggérer n'importe quoi. Un voisin proposé à tort ferait corriger
    /// vers une valeur fausse en croyant suivre un conseil — pire que se taire.
    #[test]
    fn a_far_fetched_key_gets_no_bogus_suggestion() {
        let msg = valider_arguments_outbox(&json!({
            "to_project": "AXO",
            "body_dense": "corps",
            "idempotency_key": "k-1",
            "zzzz_totalement_inconnu": 1
        }))
        .expect("un paramètre inconnu doit être refusé");
        assert!(msg.contains("zzzz_totalement_inconnu"), "{msg}");
        assert!(
            !msg.contains("vouliez-vous dire"),
            "un voisin a été proposé pour une clé sans rapport : {msg}"
        );
    }
}

#[cfg(test)]
mod lecture_des_comptes_tests {
    use super::*;

    /// REQ-AXO-902509 — `query_json` rend les entiers en CHAÎNES.
    #[test]
    fn un_entier_rendu_en_chaine_est_lu_comme_un_entier() {
        assert_eq!(entier_json(&json!(70)), 70, "entier natif");
        assert_eq!(entier_json(&json!("70")), 70, "entier rendu en chaîne");
        assert_eq!(entier_json(&json!(null)), 0, "absent → 0, pas de panique");
        assert_eq!(entier_json(&json!("pas un nombre")), 0, "illisible → 0");
    }

    /// REQ-AXO-902509 — la garde qui vaut vraiment : ce n'est pas le repli qui
    /// manquait, c'est UN site sur cinq qui ne l'avait pas. Une règle qui vit à
    /// cinq endroits est appliquée à quatre tôt ou tard. Cette garde lit le CODE
    /// et refuse qu'un sixième site réinvente le repli à la main.
    #[test]
    fn aucune_lecture_de_compte_ne_reinvente_le_repli() {
        let source = include_str!("tools_mailbox.rs");
        // Motif composé à l'exécution : écrit en clair, cette garde se citerait
        // elle-même comme coupable.
        let motif = format!("{}{}", "as_str().and_then(|s|", " s.parse()");
        let corps = source
            .lines()
            .position(|l| l.contains("fn entier_json"))
            .expect("`entier_json` a disparu : la règle n'a plus de domicile");
        let coupables: Vec<(usize, &str)> = source
            .lines()
            .enumerate()
            .filter(|(i, l)| l.contains(&motif) && !(corps..corps + 5).contains(i))
            .map(|(i, l)| (i + 1, l.trim()))
            .collect();
        assert!(
            coupables.is_empty(),
            "{} site(s) ré-écrivent le repli chaîne au lieu d'appeler `entier_json` : {:?}",
            coupables.len(),
            coupables
        );
    }
}
