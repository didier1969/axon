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

impl McpServer {
    /// REQ-AXO-902113 (MBX-1) — send a message to another project's inbox.
    ///
    /// REQ-AXO-902119 (MBX-7) — also the fan-out entry point: when `to_topic`,
    /// `to_room`, or `to_project='*'` is supplied (mutually exclusive with a
    /// concrete `to_project`), the recipient set is resolved AT SEND and one
    /// materialised row is delivered per recipient (see `outbox_fanout`). The
    /// concrete-`to_project` path below is the default point-to-point case and is
    /// preserved verbatim.
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
    ) -> Result<SentMessage, String> {
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
             (message_id, context_id, from_project, to_project, kind, subject, body_dense, envelope, idempotency_key, in_reply_to, priority, sig, topic, room_id) \
             VALUES ('{mid}','{ctx}','{from}','{to}','{kind}','{subj}','{body}','{env}'::jsonb,'{idem}','{irt}','{prio}','{sig}',NULLIF('{topic}','')::text,NULLIF('{room}','')::text) \
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
        let order_clause = if cursor_advances {
            "ORDER BY id ASC".to_string()
        } else {
            "ORDER BY CASE priority WHEN 'high' THEN 0 WHEN 'normal' THEN 1 ELSE 2 END, id ASC"
                .to_string()
        };
        let sql = format!(
            "SELECT id, message_id, context_id, from_project, kind, idempotency_key, in_reply_to, subject, body_dense, sig, created_at \
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

        let mut messages: Vec<Value> = Vec::with_capacity(rows.len());
        let mut max_id = floor;
        for row in &rows {
            let id = row
                .first()
                .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or(0);
            max_id = max_id.max(id);
            let g = |i: usize| row.get(i).and_then(Value::as_str).unwrap_or("");
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
                "signature_verified": verified,
            }));
        }

        // Advance the read cursor only in `unread` mode (so `since`/`all`/search/
        // thread are non-destructive views). UPSERT, monotonic.
        if mode == "unread" && !view_only && max_id > floor {
            let _ = self.graph_store.execute(&format!(
                "INSERT INTO axon.mailbox_cursor (project_code, last_read_id, updated_at) \
                 VALUES ('{p}', {mid}, now()) \
                 ON CONFLICT (project_code) DO UPDATE SET \
                   last_read_id = GREATEST(axon.mailbox_cursor.last_read_id, EXCLUDED.last_read_id), \
                   updated_at = now()",
                p = esc(&project),
                mid = max_id
            ));
        }

        let report = format!(
            "### 📥 mcp_inbox_read\n\n`{}` · mode={} · {} message(s){}",
            project,
            mode,
            messages.len(),
            if mode == "unread" && max_id > floor {
                format!(" · cursor advanced to {max_id}")
            } else {
                String::new()
            }
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
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
            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
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
