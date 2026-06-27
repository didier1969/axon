//! REQ-AXO-902119 (MBX-7) — MAILBOX pub/sub + broadcast/multicast + rooms.
//!
//! Decouples the emitter from N subscribers and supports multi-party rooms. The
//! management surface (subscribe/unsubscribe, room create/join) lives here; the
//! fan-out itself ([`McpServer::outbox_fanout`]) resolves the recipient set AT
//! SEND and materialises one [`crate::mailbox`]-signed row per recipient via
//! [`McpServer::outbox_send_one`] — so inbox_read / inbox_unread / cursor /
//! LISTEN-NOTIFY are reused verbatim. Store: `db/ddl/20_mailbox_pubsub.sql`.

use serde_json::{json, Value};

use super::tools_mailbox::mbx_err;
use super::McpServer;
use crate::mailbox;

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

impl McpServer {
    /// REQ-AXO-902119 (MBX-7) — subscribe a project to a topic. The topic row is
    /// lazily created (first subscriber declares it). `project` defaults to the
    /// cwd-resolved code. Idempotent (PK on (topic, project_code)).
    pub(crate) fn axon_mailbox_topic_subscribe(&self, args: &Value) -> Option<Value> {
        let topic = match args.get("topic").and_then(Value::as_str) {
            Some(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => return Some(mbx_err("mailbox_topic_subscribe requires `topic`.", "input_invalid")),
        };
        let project = match self.pubsub_resolve_project(args) {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let sql = format!(
            "INSERT INTO axon.mailbox_topic (topic, created_by) VALUES ('{t}','{p}') \
               ON CONFLICT (topic) DO NOTHING; \
             INSERT INTO axon.mailbox_subscription (topic, project_code) VALUES ('{t}','{p}') \
               ON CONFLICT (topic, project_code) DO NOTHING",
            t = esc(&topic),
            p = esc(&project),
        );
        if let Err(e) = self.graph_store.execute(&sql) {
            return Some(mbx_err(&format!("topic subscribe failed: {e}"), "degraded"));
        }
        let report = format!("### 📡 mailbox_topic_subscribe\n\n`{project}` subscribed to topic `{topic}`");
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": { "status": "ok", "topic": topic, "project": project }
        }))
    }

    /// REQ-AXO-902119 (MBX-7) — unsubscribe a project from a topic (no-op if not
    /// subscribed). The topic row itself is left in place.
    pub(crate) fn axon_mailbox_topic_unsubscribe(&self, args: &Value) -> Option<Value> {
        let topic = match args.get("topic").and_then(Value::as_str) {
            Some(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => return Some(mbx_err("mailbox_topic_unsubscribe requires `topic`.", "input_invalid")),
        };
        let project = match self.pubsub_resolve_project(args) {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let sql = format!(
            "DELETE FROM axon.mailbox_subscription WHERE topic='{t}' AND project_code='{p}'",
            t = esc(&topic),
            p = esc(&project),
        );
        if let Err(e) = self.graph_store.execute(&sql) {
            return Some(mbx_err(&format!("topic unsubscribe failed: {e}"), "degraded"));
        }
        let report = format!("### 📡 mailbox_topic_unsubscribe\n\n`{project}` unsubscribed from topic `{topic}`");
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": { "status": "ok", "topic": topic, "project": project }
        }))
    }

    /// REQ-AXO-902119 (MBX-7) — create a multi-party room and seat its members. The
    /// creator (cwd-resolved or `from`) is always seated. `members` is an optional
    /// array of project codes. Idempotent (PKs dedup re-create / re-seat).
    pub(crate) fn axon_mailbox_room_create(&self, args: &Value) -> Option<Value> {
        let room_id = match args.get("room_id").and_then(Value::as_str) {
            Some(r) if !r.trim().is_empty() => r.trim().to_string(),
            _ => return Some(mbx_err("mailbox_room_create requires `room_id`.", "input_invalid")),
        };
        let creator = match self.pubsub_resolve_project(args) {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let mut members: Vec<String> = args
            .get("members")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        members.push(creator.clone());
        members.sort();
        members.dedup();

        let mut sql = format!(
            "INSERT INTO axon.mailbox_room (room_id, created_by) VALUES ('{r}','{c}') \
               ON CONFLICT (room_id) DO NOTHING",
            r = esc(&room_id),
            c = esc(&creator),
        );
        for m in &members {
            sql.push_str(&format!(
                "; INSERT INTO axon.mailbox_room_member (room_id, project_code) VALUES ('{r}','{m}') \
                   ON CONFLICT (room_id, project_code) DO NOTHING",
                r = esc(&room_id),
                m = esc(m),
            ));
        }
        if let Err(e) = self.graph_store.execute(&sql) {
            return Some(mbx_err(&format!("room create failed: {e}"), "degraded"));
        }
        let report = format!(
            "### 🏛️ mailbox_room_create\n\nroom `{room_id}` (owner `{creator}`) · {} member(s): {}",
            members.len(),
            members.join(", ")
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": { "status": "ok", "room_id": room_id, "created_by": creator, "members": members }
        }))
    }

    /// REQ-AXO-902119 (MBX-7) — join an existing room. The room must exist (create
    /// it first). `project` defaults to the cwd-resolved code. Idempotent.
    pub(crate) fn axon_mailbox_room_join(&self, args: &Value) -> Option<Value> {
        let room_id = match args.get("room_id").and_then(Value::as_str) {
            Some(r) if !r.trim().is_empty() => r.trim().to_string(),
            _ => return Some(mbx_err("mailbox_room_join requires `room_id`.", "input_invalid")),
        };
        let project = match self.pubsub_resolve_project(args) {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let exists = self
            .graph_store
            .query_single_i64_writer(&format!(
                "SELECT count(*) FROM axon.mailbox_room WHERE room_id='{}'",
                esc(&room_id)
            ))
            .ok()
            .flatten()
            .unwrap_or(0);
        if exists == 0 {
            return Some(mbx_err(
                &format!("room `{room_id}` does not exist — create it first."),
                "not_found",
            ));
        }
        let sql = format!(
            "INSERT INTO axon.mailbox_room_member (room_id, project_code) VALUES ('{r}','{p}') \
               ON CONFLICT (room_id, project_code) DO NOTHING",
            r = esc(&room_id),
            p = esc(&project),
        );
        if let Err(e) = self.graph_store.execute(&sql) {
            return Some(mbx_err(&format!("room join failed: {e}"), "degraded"));
        }
        let report = format!("### 🏛️ mailbox_room_join\n\n`{project}` joined room `{room_id}`");
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": { "status": "ok", "room_id": room_id, "project": project }
        }))
    }

    /// REQ-AXO-902119 (MBX-7) — broadcast / multicast fan-out (called by
    /// `axon_mcp_outbox_send` when `to_topic` / `to_room` / `to_project='*'`).
    /// Resolves the recipient set, then materialises one signed row per recipient
    /// (excluding the sender). All rows share one `context_id` so the broadcast is a
    /// single thread. Per-recipient dedup is anchored by the widened UNIQUE key
    /// (from_project, to_project, idempotency_key) — re-broadcasting the same key is
    /// idempotent per recipient.
    pub(crate) fn outbox_fanout(
        &self,
        from: &str,
        to_topic: Option<&str>,
        to_room: Option<&str>,
        is_broadcast: bool,
        args: &Value,
    ) -> Option<Value> {
        let idempotency_key = match args.get("idempotency_key").and_then(Value::as_str) {
            Some(k) if !k.trim().is_empty() => k.trim().to_string(),
            _ => {
                return Some(mbx_err(
                    "mcp_outbox_send (fan-out) requires `idempotency_key`.",
                    "input_invalid",
                ))
            }
        };
        let subject = args.get("subject").and_then(Value::as_str).unwrap_or("").to_string();
        let body_dense = args.get("body_dense").and_then(Value::as_str).unwrap_or("").to_string();
        let in_reply_to = args.get("in_reply_to").and_then(Value::as_str).unwrap_or("").to_string();
        let kind = args.get("kind").and_then(Value::as_str).unwrap_or("message").to_string();
        let priority = args.get("priority").and_then(Value::as_str).unwrap_or("normal").to_string();
        let ref_soll_ids = args.get("ref_soll_ids").cloned().unwrap_or_else(|| json!([]));

        // Resolve the recipient set + the fan-out scope key (for the shared thread).
        let (recipients, scope_key, scope_label, topic_stamp, room_stamp) =
            match (to_topic, to_room, is_broadcast) {
                (Some(t), _, _) => (
                    self.topic_subscribers(t),
                    format!("topic:{t}"),
                    format!("topic `{t}`"),
                    t.to_string(),
                    String::new(),
                ),
                (_, Some(r), _) => (
                    self.room_members(r),
                    format!("room:{r}"),
                    format!("room `{r}`"),
                    String::new(),
                    r.to_string(),
                ),
                (_, _, true) => (
                    self.all_project_codes(),
                    "*".to_string(),
                    "broadcast `*`".to_string(),
                    String::new(),
                    String::new(),
                ),
                _ => return Some(mbx_err("mcp_outbox_send: no fan-out target resolved.", "input_invalid")),
            };

        // Exclude the sender from its own broadcast; dedup recipients.
        let mut recipients: Vec<String> = recipients.into_iter().filter(|r| r != from).collect();
        recipients.sort();
        recipients.dedup();

        // One shared thread for the whole fan-out (override via explicit context_id).
        let context_in = args.get("context_id").and_then(Value::as_str).unwrap_or("");
        let shared_context = if context_in.is_empty() {
            mailbox::message_id(from, &scope_key, &idempotency_key)
        } else {
            context_in.to_string()
        };

        let mut delivered = 0usize;
        let mut deduped = 0usize;
        let mut fanned: Vec<Value> = Vec::with_capacity(recipients.len());
        for to in &recipients {
            match self.outbox_send_one(
                from,
                to,
                &idempotency_key,
                &subject,
                &body_dense,
                &in_reply_to,
                &kind,
                &priority,
                &shared_context,
                &ref_soll_ids,
                &topic_stamp,
                &room_stamp,
            ) {
                Ok(s) => {
                    if s.deduped {
                        deduped += 1;
                    } else {
                        delivered += 1;
                    }
                    fanned.push(json!({
                        "to": to,
                        "message_id": s.message_id,
                        "deduped": s.deduped,
                    }));
                }
                Err(e) => return Some(mbx_err(&format!("fan-out send to `{to}` failed: {e}"), "degraded")),
            }
        }

        let report = format!(
            "### 📡 mcp_outbox_send (fan-out)\n\n{from} → {scope_label} · context=`{shared_context}` · {} recipient(s) · {delivered} delivered · {deduped} idempotent no-op",
            recipients.len()
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "ok",
                "from": from,
                "scope": scope_label,
                "context_id": shared_context,
                "recipients": recipients.len(),
                "delivered": delivered,
                "deduped": deduped,
                "fanned": fanned,
            }
        }))
    }

    /// Resolve the acting project for a pub/sub management call: explicit `project`
    /// / `from`, else cwd-resolution.
    fn pubsub_resolve_project(&self, args: &Value) -> Result<String, Value> {
        let p = args
            .get("project")
            .or_else(|| args.get("from"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| self.auto_resolve_project_code_str())
            .unwrap_or_default();
        if p.is_empty() {
            Err(mbx_err(
                "project unresolved — pass `project` (cwd-resolution found none).",
                "input_invalid",
            ))
        } else {
            Ok(p)
        }
    }

    /// All project_codes subscribed to `topic`.
    fn topic_subscribers(&self, topic: &str) -> Vec<String> {
        self.pubsub_codes(&format!(
            "SELECT project_code FROM axon.mailbox_subscription WHERE topic='{}' ORDER BY project_code ASC",
            esc(topic)
        ))
    }

    /// All project_codes seated in `room_id`.
    fn room_members(&self, room_id: &str) -> Vec<String> {
        self.pubsub_codes(&format!(
            "SELECT project_code FROM axon.mailbox_room_member WHERE room_id='{}' ORDER BY project_code ASC",
            esc(room_id)
        ))
    }

    /// All registered project codes (registry-wide broadcast target).
    fn all_project_codes(&self) -> Vec<String> {
        self.pubsub_codes(
            "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_code <> '' ORDER BY project_code ASC",
        )
    }

    /// Run a single-column `project_code` query and collect the codes (writer ctx,
    /// so freshly-written subscriptions/members are visible in the same call).
    fn pubsub_codes(&self, sql: &str) -> Vec<String> {
        let raw = self.graph_store.query_json_writer(sql).unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| r.into_iter().next())
            .filter_map(|v| v.as_str().map(str::to_string))
            .filter(|s| !s.is_empty())
            .collect()
    }
}
