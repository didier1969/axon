//! REQ-AXO-902120 (MBX-8) — advisory leases / cooperative edit locks.
//!
//! Anti-collision for multi-LLM editing. A project announces its INTENT to work
//! on a `resource` so peer agents see the conflict BEFORE they collide. Purely
//! cooperative: `acquire` ALWAYS grants (advisory, never blocks) but returns the
//! live conflicting holders so the caller decides. `release` drops a lease;
//! `check` is a read-only list of live holders. A crashed holder's lease is
//! reclaimed only by `expires_at` ageing out — there is no other auto-release.
//! Store + rationale live in `db/ddl/19_mailbox_lease.sql`.

use serde_json::{json, Value};

use super::McpServer;

/// Single-quote escape for inline SQL literals (mirrors `tools_mailbox::esc`).
fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

fn lease_err(msg: &str, status: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": msg }],
        "isError": true,
        "data": { "status": status }
    })
}

/// Default / bounds for a lease TTL (seconds). 15 min default, 1s..24h range.
const LEASE_TTL_DEFAULT_S: u64 = 900;
const LEASE_TTL_MIN_S: u64 = 1;
const LEASE_TTL_MAX_S: u64 = 86_400;

impl McpServer {
    /// REQ-AXO-902120 (MBX-8) — advisory lease surface. `action` ∈
    /// {acquire, release, check}.
    pub(crate) fn axon_mailbox_lease(&self, args: &Value) -> Option<Value> {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("check");
        match action {
            "acquire" => self.mailbox_lease_acquire(args),
            "release" => self.mailbox_lease_release(args),
            "check" => self.mailbox_lease_check(args),
            other => Some(lease_err(
                &format!("mailbox_lease: unknown action `{other}` (expected acquire|release|check)."),
                "input_invalid",
            )),
        }
    }

    /// Live holders of `resource` (expires_at > now()), optionally excluding one
    /// project. Returns the parsed rows as JSON objects.
    fn lease_live_holders(&self, resource: &str, exclude_project: Option<&str>) -> Vec<Value> {
        let mut filter = String::new();
        if let Some(p) = exclude_project {
            filter.push_str(&format!(" AND holder_project <> '{}'", esc(p)));
        }
        let sql = format!(
            "SELECT lease_id, holder_project, intent, acquired_at, expires_at \
             FROM axon.mailbox_lease \
             WHERE resource = '{res}' AND expires_at > now(){filter} \
             ORDER BY acquired_at ASC",
            res = esc(resource),
        );
        let rows: Vec<Vec<Value>> = self
            .graph_store
            .query_json_writer(&sql)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        rows.iter()
            .map(|row| {
                let lease_id = row
                    .first()
                    .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                    .unwrap_or(0);
                let g = |i: usize| row.get(i).and_then(Value::as_str).unwrap_or("");
                json!({
                    "lease_id": lease_id,
                    "holder_project": g(1),
                    "intent": g(2),
                    "acquired_at": g(3),
                    "expires_at": g(4),
                })
            })
            .collect()
    }

    fn mailbox_lease_acquire(&self, args: &Value) -> Option<Value> {
        let resource = match args.get("resource").and_then(Value::as_str) {
            Some(r) if !r.trim().is_empty() => r.trim().to_string(),
            _ => return Some(lease_err("mailbox_lease acquire requires `resource`.", "input_invalid")),
        };
        let holder = args
            .get("holder")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
            .or_else(|| self.auto_resolve_project_code_str())
            .unwrap_or_default();
        if holder.is_empty() {
            return Some(lease_err(
                "lease holder unresolved — pass `holder` (cwd-resolution found none).",
                "input_invalid",
            ));
        }
        let ttl_s = args
            .get("ttl_s")
            .and_then(Value::as_u64)
            .unwrap_or(LEASE_TTL_DEFAULT_S)
            .clamp(LEASE_TTL_MIN_S, LEASE_TTL_MAX_S);
        let intent = args.get("intent").and_then(Value::as_str).unwrap_or("").to_string();

        let sql = format!(
            "INSERT INTO axon.mailbox_lease (resource, holder_project, intent, expires_at) \
             VALUES ('{res}','{holder}','{intent}', now() + make_interval(secs => {ttl})) \
             RETURNING lease_id, expires_at",
            res = esc(&resource),
            holder = esc(&holder),
            intent = esc(&intent),
            ttl = ttl_s,
        );
        let rows: Vec<Vec<Value>> = match self.graph_store.query_json_writer(&sql) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(lease_err(&format!("lease acquire failed: {e}"), "degraded")),
        };
        let lease_id = rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(0);
        let expires_at = rows
            .first()
            .and_then(|r| r.get(1))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        // Conflicts = OTHER projects' live leases on the same resource (our own
        // freshly-inserted lease is excluded by holder).
        let conflicts = self.lease_live_holders(&resource, Some(&holder));
        let report = format!(
            "### 🔒 mailbox_lease acquire\n\n`{holder}` ⇒ `{resource}` · lease_id=`{lease_id}` · ttl={ttl_s}s · expires={expires_at}{c}",
            c = if conflicts.is_empty() {
                " · no live conflicts".to_string()
            } else {
                format!(" · ⚠️ {} conflicting holder(s) still live (advisory, granted anyway)", conflicts.len())
            }
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "ok",
                "granted": true,
                "lease_id": lease_id,
                "resource": resource,
                "holder_project": holder,
                "ttl_s": ttl_s,
                "expires_at": expires_at,
                "conflicts": conflicts,
            }
        }))
    }

    fn mailbox_lease_release(&self, args: &Value) -> Option<Value> {
        let lease_id = args
            .get("lease_id")
            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
        let resource = args
            .get("resource")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string());

        // Resource-scoped release is bounded to the caller's OWN leases so one
        // project can never drop another's advisory lease.
        let holder = args
            .get("holder")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
            .or_else(|| self.auto_resolve_project_code_str());

        let where_clause = if let Some(id) = lease_id {
            format!("lease_id = {id}")
        } else if let Some(res) = &resource {
            match &holder {
                Some(h) => format!("resource = '{}' AND holder_project = '{}'", esc(res), esc(h)),
                None => {
                    return Some(lease_err(
                        "lease release by `resource` needs a resolved `holder` (cwd found none) — pass `holder` or `lease_id`.",
                        "input_invalid",
                    ))
                }
            }
        } else {
            return Some(lease_err(
                "mailbox_lease release requires `lease_id` or `resource`.",
                "input_invalid",
            ));
        };

        let sql = format!(
            "DELETE FROM axon.mailbox_lease WHERE {where_clause} RETURNING lease_id"
        );
        let rows: Vec<Vec<Value>> = match self.graph_store.query_json_writer(&sql) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(lease_err(&format!("lease release failed: {e}"), "degraded")),
        };
        let released = rows.len();
        let report = format!(
            "### 🔓 mailbox_lease release\n\n{released} lease(s) released ({where_clause})."
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "ok",
                "released": released,
            }
        }))
    }

    fn mailbox_lease_check(&self, args: &Value) -> Option<Value> {
        let resource = match args.get("resource").and_then(Value::as_str) {
            Some(r) if !r.trim().is_empty() => r.trim().to_string(),
            _ => return Some(lease_err("mailbox_lease check requires `resource`.", "input_invalid")),
        };
        let holders = self.lease_live_holders(&resource, None);
        let report = format!(
            "### 🔎 mailbox_lease check\n\n`{resource}` · {} live holder(s).",
            holders.len()
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "ok",
                "resource": resource,
                "count": holders.len(),
                "holders": holders,
            }
        }))
    }
}
