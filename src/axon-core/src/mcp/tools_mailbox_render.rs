//! REQ-AXO-902122 (MBX-10) — human render + observation tap. READ-ONLY.
//!
//! `mailbox_render` expands a message (or a whole thread) into bounded human
//! markdown: body_dense verbatim + `ref_soll_ids` (pulled from the A2A envelope)
//! resolved to SOLL titles. `mailbox_tap` reads a thread WITHOUT being a
//! recipient and WITHOUT advancing ANY cursor (pure observation, reuses the
//! non-destructive view path of `mcp_inbox_read`). Bounded AXO-internal for now;
//! a real ACL is MBX-5 (future). No DDL — projections over `axon.mailbox_message`
//! + `soll.Node`.

use serde_json::{json, Value};

use super::McpServer;
use crate::mailbox;

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

fn render_err(msg: &str, status: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": msg }],
        "isError": true,
        "data": { "status": status }
    })
}

/// Parse the JSONB `ref_soll_ids` cell (selected `::text`, column index 7) of a
/// rendered row into a `Vec<String>`. Empty / null / unparseable → `[]`.
fn refs_of(row: &[Value]) -> Vec<String> {
    let raw = row.get(7).and_then(Value::as_str).unwrap_or("[]");
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// Expansion bounds (cap the human render — a thread can be long).
const RENDER_MSG_CAP: u64 = 50;
const RENDER_REF_CAP: usize = 100;
const TAP_LIMIT_DEFAULT: u64 = 50;
const TAP_LIMIT_MAX: u64 = 200;

impl McpServer {
    /// REQ-AXO-902122 (MBX-10) — render a message/thread to bounded human
    /// markdown with SOLL pointers resolved to titles. Read-only.
    pub(crate) fn axon_mailbox_render(&self, args: &Value) -> Option<Value> {
        let id = args
            .get("id")
            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
        let context_id = args
            .get("context_id")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string());

        let selector = if let Some(i) = id {
            format!("id = {i}")
        } else if let Some(ctx) = &context_id {
            format!("context_id = '{}'", esc(ctx))
        } else {
            return Some(render_err(
                "mailbox_render requires `id` or `context_id`.",
                "input_invalid",
            ));
        };

        let sql = format!(
            "SELECT id, message_id, context_id, from_project, to_project, subject, body_dense, \
                    (envelope->'parts'->0->'data'->'ref_soll_ids')::text AS refs, created_at, priority, kind \
             FROM axon.mailbox_message \
             WHERE {selector} AND archived_at IS NULL \
             ORDER BY id ASC LIMIT {cap}",
            cap = RENDER_MSG_CAP,
        );
        let rows: Vec<Vec<Value>> = match self.graph_store.query_json(&sql) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(render_err(&format!("mailbox render failed: {e}"), "degraded")),
        };
        if rows.is_empty() {
            return Some(render_err("mailbox_render: no message matched.", "not_found"));
        }

        // Pass 1 — gather all referenced SOLL ids across the rendered messages
        // (bounded, deduped) and resolve their titles in one round-trip.
        let mut ref_ids: Vec<String> = Vec::new();
        for row in &rows {
            for r in refs_of(row) {
                if ref_ids.len() < RENDER_REF_CAP && !ref_ids.contains(&r) {
                    ref_ids.push(r);
                }
            }
        }

        let mut titles: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        if !ref_ids.is_empty() {
            let arr = ref_ids
                .iter()
                .map(|r| format!("'{}'", esc(r)))
                .collect::<Vec<_>>()
                .join(",");
            let tsql = format!(
                "SELECT id, title FROM soll.Node WHERE id = ANY(ARRAY[{arr}]::text[])"
            );
            if let Ok(s) = self.graph_store.query_json(&tsql) {
                let trows: Vec<Vec<Value>> = serde_json::from_str(&s).unwrap_or_default();
                for tr in &trows {
                    let id = tr.first().and_then(Value::as_str).unwrap_or("").to_string();
                    let title = tr.get(1).and_then(Value::as_str).unwrap_or("").to_string();
                    if !id.is_empty() {
                        titles.insert(id, title);
                    }
                }
            }
        }

        // Pass 2 — build bounded human markdown.
        let mut md = String::new();
        let thread_ctx = rows
            .first()
            .and_then(|r| r.get(2))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        md.push_str(&format!(
            "## 📨 Thread `{}` — {} message(s)\n",
            thread_ctx,
            rows.len()
        ));
        for row in &rows {
            let g = |i: usize| row.get(i).and_then(Value::as_str).unwrap_or("");
            let rid = row
                .first()
                .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or(0);
            let (message_id, from, to, subject, body, created_at, priority) =
                (g(1), g(3), g(4), g(5), g(6), g(8), g(9));
            md.push_str(&format!(
                "\n---\n### #{rid} · {from} → {to}{prio}\n**{subject}**  \n`message_id={message_id}` · {created_at}\n\n{body}\n",
                prio = if priority == "high" { " · ⚡high" } else { "" },
            ));
            let refs = refs_of(row);
            if !refs.is_empty() {
                md.push_str("\n**SOLL pointers:**\n");
                for r in &refs {
                    match titles.get(r) {
                        Some(t) if !t.is_empty() => md.push_str(&format!("- `{r}` — {t}\n")),
                        _ => md.push_str(&format!("- `{r}` — _(unresolved)_\n")),
                    }
                }
            }
        }

        Some(json!({
            "content": [{ "type": "text", "text": md }],
            "data": {
                "status": "ok",
                "context_id": thread_ctx,
                "rendered": rows.len(),
                "soll_resolved": titles.len(),
            }
        }))
    }

    /// REQ-AXO-902122 (MBX-10) — observation tap: read a thread (or from/to
    /// slice) WITHOUT being the recipient and WITHOUT advancing any cursor.
    /// Read-only, non-destructive. Bounded AXO-internal (ACL = MBX-5, future).
    pub(crate) fn axon_mailbox_tap(&self, args: &Value) -> Option<Value> {
        let context_id = args
            .get("context_id")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty());
        let from = args
            .get("from")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty());
        let to = args
            .get("to")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty());
        if context_id.is_none() && from.is_none() && to.is_none() {
            return Some(render_err(
                "mailbox_tap requires at least one of `context_id`, `from`, `to` (no full-exchange dump).",
                "input_invalid",
            ));
        }
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(TAP_LIMIT_DEFAULT)
            .clamp(1, TAP_LIMIT_MAX);

        let mut filters = String::new();
        if let Some(c) = context_id {
            filters.push_str(&format!(" AND context_id = '{}'", esc(c)));
        }
        if let Some(f) = from {
            filters.push_str(&format!(" AND from_project = '{}'", esc(f)));
        }
        if let Some(t) = to {
            filters.push_str(&format!(" AND to_project = '{}'", esc(t)));
        }

        // Pure view: id-ordered, archived excluded, NO cursor read or write.
        let sql = format!(
            "SELECT id, message_id, context_id, from_project, to_project, kind, idempotency_key, \
                    in_reply_to, subject, body_dense, sig, created_at, priority \
             FROM axon.mailbox_message WHERE archived_at IS NULL{filters} \
             ORDER BY id ASC LIMIT {limit}",
        );
        let rows: Vec<Vec<Value>> = match self.graph_store.query_json(&sql) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(render_err(&format!("mailbox tap failed: {e}"), "degraded")),
        };

        let mut messages: Vec<Value> = Vec::with_capacity(rows.len());
        for row in &rows {
            let id = row
                .first()
                .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or(0);
            let g = |i: usize| row.get(i).and_then(Value::as_str).unwrap_or("");
            let (message_id, ctx, from_p, to_p, kind, idem, irt, subject, body, sig) =
                (g(1), g(2), g(3), g(4), g(5), g(6), g(7), g(8), g(9), g(10));
            // Verify against the SENDER's HMAC; tap knows the true to_project per row.
            let canonical =
                mailbox::canonical(from_p, to_p, ctx, message_id, kind, idem, irt, subject, body);
            let verified = mailbox::verify(from_p, &canonical, sig);
            messages.push(json!({
                "id": id,
                "message_id": message_id,
                "context_id": ctx,
                "from": from_p,
                "to": to_p,
                "kind": kind,
                "in_reply_to": irt,
                "subject": subject,
                "body_dense": body,
                "priority": g(12),
                "created_at": g(11),
                "signature_verified": verified,
            }));
        }

        let report = format!(
            "### 👁️ mailbox_tap (observation, no cursor advanced)\n\n{} message(s){}{}{}",
            messages.len(),
            context_id.map(|c| format!(" · thread=`{c}`")).unwrap_or_default(),
            from.map(|f| format!(" · from=`{f}`")).unwrap_or_default(),
            to.map(|t| format!(" · to=`{t}`")).unwrap_or_default(),
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "ok",
                "count": messages.len(),
                "messages": messages,
            }
        }))
    }
}
