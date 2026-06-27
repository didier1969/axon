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

fn mbx_err(msg: &str, status: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": msg }],
        "isError": true,
        "data": { "status": status }
    })
}

impl McpServer {
    /// REQ-AXO-902113 (MBX-1) — send a message to another project's inbox.
    pub(crate) fn axon_mcp_outbox_send(&self, args: &Value) -> Option<Value> {
        let to = match args.get("to_project").and_then(Value::as_str) {
            Some(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => return Some(mbx_err("mcp_outbox_send requires `to_project`.", "input_invalid")),
        };
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

        let message_id = mailbox::message_id(&from, &to, &idempotency_key);
        let context_id = args
            .get("context_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| message_id.clone());

        let canonical = mailbox::canonical(
            &from,
            &to,
            &context_id,
            &message_id,
            &kind,
            &idempotency_key,
            &in_reply_to,
            &subject,
            &body_dense,
        );
        let sig = mailbox::sign(&from, &canonical);

        // A2A-aligned envelope (DEC-AXO-901663): the dense Axon body rides in a
        // `data` part so A2A interop (Agent Cards, MBX-6) is free later.
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
            "sig": sig,
        });
        let envelope_lit = esc(&serde_json::to_string(&envelope).unwrap_or_default());

        let sql = format!(
            "INSERT INTO axon.mailbox_message \
             (message_id, context_id, from_project, to_project, kind, subject, body_dense, envelope, idempotency_key, in_reply_to, priority, sig) \
             VALUES ('{mid}','{ctx}','{from}','{to}','{kind}','{subj}','{body}','{env}'::jsonb,'{idem}','{irt}','{prio}','{sig}') \
             ON CONFLICT (from_project, idempotency_key) DO NOTHING RETURNING id",
            mid = esc(&message_id),
            ctx = esc(&context_id),
            from = esc(&from),
            to = esc(&to),
            kind = esc(&kind),
            subj = esc(&subject),
            body = esc(&body_dense),
            env = envelope_lit,
            idem = esc(&idempotency_key),
            irt = esc(&in_reply_to),
            prio = esc(&priority),
            sig = esc(&sig),
        );
        let rows: Vec<Vec<Value>> = match self.graph_store.query_json_writer(&sql) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(mbx_err(&format!("mailbox send failed: {e}"), "degraded")),
        };
        let deduped = rows.is_empty();
        let report = format!(
            "### 📤 mcp_outbox_send\n\n{} → `{}` · message_id=`{}` · context=`{}`{}",
            from,
            to,
            message_id,
            context_id,
            if deduped {
                " · (idempotent no-op: already sent)"
            } else {
                " · delivered"
            }
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "ok",
                "message_id": message_id,
                "context_id": context_id,
                "from": from,
                "to": to,
                "deduped": deduped,
                "sig": sig,
            }
        }))
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

        let sql = format!(
            "SELECT id, message_id, context_id, from_project, kind, idempotency_key, in_reply_to, subject, body_dense, sig, created_at \
             FROM axon.mailbox_message WHERE to_project='{}' AND id > {}{} ORDER BY id ASC LIMIT {}",
            esc(&project),
            floor,
            filters,
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
            let verified = mailbox::verify(from, &canonical, sig);
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
                 WHERE m.to_project='{p}' AND m.id > COALESCE(c.last_read_id, 0)",
                p = esc(project)
            ))
            .ok()
            .flatten()
            .unwrap_or(0)
    }
}
