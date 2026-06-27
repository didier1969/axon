//! REQ-AXO-902118 (MBX-6) — Agent Cards: A2A capability discovery MCP surface.
//!
//! `mcp_agent_card` with action ∈ {set, get, list}:
//! - `set`  — the OWNER publishes its A2A AgentCard (project resolved from `from`
//!            / cwd; owner-write ACL — a project can never write another's card).
//!            Signed via [`mailbox::canonical_card`] + HMAC, then UPSERT.
//! - `get`  — fetch one project's card + `signature_verified` (HMAC re-check).
//! - `list` — discover peers, optionally `skill`-filtered via the GIN containment
//!            index on `card->'skills'` → `[{project_code, name, skills}]`.
//!
//! A2A v1.0 well-known path = `/.well-known/agent-card.json`. The signature reuses
//! the mailbox HMAC for internal interop; true A2A integrity is JWS — that remains
//! a deliberate MVP gap (the `sig` column is forward-compatible with a JWS swap).
//! Crypto + canonicalisation live in [`crate::mailbox`]; this is the DB surface.

use serde_json::{json, Value};

use super::McpServer;
use crate::mailbox;

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

fn card_err(msg: &str, status: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": msg }],
        "isError": true,
        "data": { "status": status }
    })
}

impl McpServer {
    /// REQ-AXO-902118 (MBX-6) — publish / fetch / discover A2A Agent Cards.
    pub(crate) fn axon_mcp_agent_card(&self, args: &Value) -> Option<Value> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("get");
        match action {
            "set" => self.agent_card_set(args),
            "get" => self.agent_card_get(args),
            "list" => self.agent_card_list(args),
            other => Some(card_err(
                &format!("mcp_agent_card: unknown action `{other}` (set|get|list)."),
                "input_invalid",
            )),
        }
    }

    /// `set` — owner publishes its card. project = `from` arg or cwd-resolution.
    /// Owner-write ACL: an explicit `project` target that differs from the resolved
    /// owner is rejected (a project may only write its OWN card).
    fn agent_card_set(&self, args: &Value) -> Option<Value> {
        let owner = args
            .get("from")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| self.auto_resolve_project_code_str())
            .unwrap_or_default();
        if owner.is_empty() {
            return Some(card_err(
                "owner project unresolved — pass `from` (cwd-resolution found none).",
                "input_invalid",
            ));
        }
        // Owner-write ACL: refuse to write another project's card.
        if let Some(target) = args.get("project").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            if target != owner {
                return Some(card_err(
                    &format!(
                        "owner-write only: `{owner}` cannot publish the card of `{target}` (set your own card)."
                    ),
                    "owner_write_denied",
                ));
            }
        }
        let card = match args.get("card") {
            Some(c) if c.is_object() => c.clone(),
            _ => {
                return Some(card_err(
                    "mcp_agent_card set requires `card` (A2A AgentCard object).",
                    "input_invalid",
                ))
            }
        };

        // Denormalised projections from the card (for indexing/listing).
        let name = card.get("name").and_then(Value::as_str).unwrap_or(&owner).to_string();
        let description = card.get("description").and_then(Value::as_str).unwrap_or("").to_string();
        let version = card.get("version").and_then(Value::as_str).unwrap_or("1.0.0").to_string();

        // Deterministic canonical → HMAC. Re-serialisation can never change bytes.
        let canonical = mailbox::canonical_card(&owner, &card);
        let sig = mailbox::sign(&owner, &canonical);

        let card_lit = esc(&serde_json::to_string(&card).unwrap_or_default());
        let sql = format!(
            "INSERT INTO axon.agent_card \
             (project_code, name, description, version, card, sig, updated_at) \
             VALUES ('{pc}','{name}','{desc}','{ver}','{card}'::jsonb,'{sig}', now()) \
             ON CONFLICT (project_code) DO UPDATE SET \
               name = EXCLUDED.name, description = EXCLUDED.description, \
               version = EXCLUDED.version, card = EXCLUDED.card, \
               sig = EXCLUDED.sig, updated_at = now() \
             RETURNING project_code",
            pc = esc(&owner),
            name = esc(&name),
            desc = esc(&description),
            ver = esc(&version),
            card = card_lit,
            sig = esc(&sig),
        );
        if let Err(e) = self.graph_store.query_json_writer(&sql) {
            return Some(card_err(&format!("agent_card set failed: {e}"), "degraded"));
        }
        let skill_count = card.get("skills").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        let report = format!(
            "### 🪪 mcp_agent_card set\n\n`{owner}` · {name} v{version} · {skill_count} skill(s) · published + signed"
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "ok",
                "project_code": owner,
                "name": name,
                "version": version,
                "skills": skill_count,
                "sig": sig,
            }
        }))
    }

    /// `get` — fetch one project's card + HMAC `signature_verified`.
    fn agent_card_get(&self, args: &Value) -> Option<Value> {
        let project = args
            .get("project")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| self.auto_resolve_project_code_str())
            .unwrap_or_default();
        if project.is_empty() {
            return Some(card_err("agent_card get: pass `project`.", "input_invalid"));
        }
        let sql = format!(
            "SELECT project_code, name, version, card, sig, updated_at \
             FROM axon.agent_card WHERE project_code='{}'",
            esc(&project)
        );
        let rows: Vec<Vec<Value>> = match self.graph_store.query_json(&sql) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(card_err(&format!("agent_card get failed: {e}"), "degraded")),
        };
        let row = match rows.first() {
            Some(r) => r,
            None => {
                return Some(card_err(
                    &format!("no agent card published for `{project}`."),
                    "not_found",
                ))
            }
        };
        let g = |i: usize| row.get(i).and_then(Value::as_str).unwrap_or("");
        let name = g(1).to_string();
        let version = g(2).to_string();
        // card column may arrive as an object or as a JSON string depending on the
        // driver's JSONB rendering; accept both.
        let card: Value = match row.get(3) {
            Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
            Some(other) => other.clone(),
            None => Value::Null,
        };
        let sig = g(4);
        let canonical = mailbox::canonical_card(&project, &card);
        let verified = mailbox::verify(&project, &canonical, sig);
        let report = format!(
            "### 🪪 mcp_agent_card get\n\n`{project}` · {name} v{version} · signature_verified={verified}"
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "ok",
                "project_code": project,
                "name": name,
                "version": version,
                "card": card,
                "signature_verified": verified,
                "updated_at": g(5),
                "well_known": "/.well-known/agent-card.json",
            }
        }))
    }

    /// `list` — discover published cards, optionally filtered by `skill` tag via the
    /// GIN containment index on `card->'skills'`.
    fn agent_card_list(&self, args: &Value) -> Option<Value> {
        let skill = args.get("skill").and_then(Value::as_str).filter(|s| !s.trim().is_empty());
        let mut filter = String::new();
        if let Some(tag) = skill {
            // Containment: any skill element whose `tags` array contains the tag.
            filter = format!(
                " WHERE card -> 'skills' @> '[{{\"tags\":[\"{}\"]}}]'::jsonb",
                esc(tag)
            );
        }
        let sql = format!(
            "SELECT project_code, name, card -> 'skills' \
             FROM axon.agent_card{} ORDER BY project_code ASC",
            filter
        );
        let rows: Vec<Vec<Value>> = match self.graph_store.query_json(&sql) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(card_err(&format!("agent_card list failed: {e}"), "degraded")),
        };
        let cards: Vec<Value> = rows
            .iter()
            .map(|row| {
                let g = |i: usize| row.get(i).and_then(Value::as_str).unwrap_or("");
                let skills: Value = match row.get(2) {
                    Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(json!([])),
                    Some(other) => other.clone(),
                    None => json!([]),
                };
                json!({
                    "project_code": g(0),
                    "name": g(1),
                    "skills": skills,
                })
            })
            .collect();
        let report = format!(
            "### 🪪 mcp_agent_card list\n\n{} card(s){}",
            cards.len(),
            skill.map(|s| format!(" · skill=`{s}`")).unwrap_or_default()
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "ok",
                "count": cards.len(),
                "skill": skill,
                "cards": cards,
            }
        }))
    }
}
