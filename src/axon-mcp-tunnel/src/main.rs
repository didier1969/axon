use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::env;
use std::io::{self, BufRead, Write};
use std::time::Duration;

// REQ-AXO-902004 — the stdio↔HTTP tunnel previously used a single hard-coded
// 10 s reqwest timeout. A batch SOLL mutation (e.g. `axon_apply_guidelines` over
// 20 rules, `soll_apply_plan` over 16 items) routinely runs longer than 10 s on
// a loaded single-host brain, so the tunnel timed out and reported
// "Axon Backend is unavailable or timed out" to the LLM client *while the brain
// kept going and committed successfully*. The false failure forced systematic
// `sql` re-verification before retry — pure token cost (originator: consumer
// project SVZ, 2026-06-15).
//
// Note the original "fail fast if backend is down" rationale was only half
// right: a *down* backend yields an immediate connection-refused error, NOT a
// timeout. The request timeout only fires when the brain is reachable but slow —
// exactly the legitimate-load case we must NOT misreport. We therefore use a
// generous default and a longer bucket for the known-heavy batch/scan tools,
// both overridable via env. A genuinely hung (connected, non-responding) brain
// is still caught, just at a realistic ceiling.
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const HEAVY_TIMEOUT_SECS: u64 = 180;

/// REQ-AXO-902555 — Extract client identity from initialize request params.
fn extract_client_name(payload: &Value) -> Option<String> {
    if payload.get("method").and_then(Value::as_str) == Some("initialize") {
        payload
            .get("params")
            .and_then(|p| p.get("clientInfo"))
            .and_then(|ci| ci.get("name"))
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    }
}

/// Tools whose server-side work (batch SOLL writes, revision ops, full-project
/// scans / doc generation) legitimately exceeds the default ceiling under load.
const HEAVY_TOOLS: &[&str] = &[
    "soll_apply_plan",
    "axon_apply_guidelines",
    "axon_apply_methodology_bundle",
    "soll_commit_revision",
    "soll_rollback_revision",
    "restore_soll",
    "soll_generate_docs",
    "rescan_project",
    "audit",
];

/// Extract the invoked tool name from an MCP JSON-RPC payload, i.e. the
/// `params.name` of a `tools/call` request. Returns `None` for any other method
/// (`initialize`, `tools/list`, …) or a malformed payload.
fn tool_name(payload: &Value) -> Option<&str> {
    if payload.get("method").and_then(Value::as_str) == Some("tools/call") {
        payload
            .get("params")
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
    } else {
        None
    }
}

/// Per-request timeout: heavy tools get `heavy`, everything else `default`.
/// Pure (env already resolved into the two durations) so it is unit-testable
/// without a network or environment.
fn timeout_for_payload(payload: &Value, default: Duration, heavy: Duration) -> Duration {
    match tool_name(payload) {
        Some(name) if HEAVY_TOOLS.contains(&name) => heavy,
        _ => default,
    }
}

/// Parse a positive-integer seconds override from `key`, falling back to
/// `default` when absent, unparsable, or zero.
fn env_timeout(key: &str, default: u64) -> Duration {
    let secs = env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default);
    Duration::from_secs(secs)
}

fn main() {
    // No client-level timeout: it is selected per-request (heavy vs default)
    // and applied via `RequestBuilder::timeout`, which overrides the client.
    let client = Client::builder()
        .build()
        .expect("Failed to create HTTP client");
    let mcp_url =
        env::var("AXON_MCP_URL").unwrap_or_else(|_| "http://127.0.0.1:44129/mcp".to_string());
    let default_timeout = env_timeout("AXON_MCP_TUNNEL_TIMEOUT_SECS", DEFAULT_TIMEOUT_SECS);
    let heavy_timeout = env_timeout("AXON_MCP_TUNNEL_HEAVY_TIMEOUT_SECS", HEAVY_TIMEOUT_SECS);

    // REQ-AXO-902286 — the tunnel is spawned by the MCP client (Claude Code) IN the
    // project directory, so its cwd IS the caller's project. The shared brain has no
    // other way to know which project the caller works in (its own cwd is always the
    // Axon repo). Forward the cwd on every request as `X-Axon-Client-Cwd`; the brain
    // resolves project_code against it. Captured once — a tunnel process serves one
    // client session from one directory.
    let client_cwd = env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .filter(|s| !s.trim().is_empty());
    // REQ-AXO-902555 — track caller client identity (e.g. claude-code, codex, gemini).
    let mut client_name = env::var("AXON_CLIENT_NAME")
        .ok()
        .filter(|s| !s.trim().is_empty());

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        match line {
            Ok(req_str) => {
                if req_str.trim().is_empty() {
                    continue;
                }

                let req_val: Result<Value, _> = serde_json::from_str(&req_str);
                match req_val {
                    Ok(json_payload) => {
                        let is_notification = json_payload.get("id").is_none();
                        let method = json_payload.get("method").and_then(Value::as_str);

                        // REQ-AXO-902555 — capture client identity on initialize handshake.
                        if let Some(extracted) = extract_client_name(&json_payload) {
                            client_name = Some(extracted);
                        }

                        // 1. Keepalive ping (MCP spec 2024-11-05).
                        // Standard MCP ping method requires an empty object result with caller's ID.
                        if method == Some("ping") {
                            if !is_notification {
                                let ping_id = json_payload.get("id").cloned().unwrap_or(Value::Null);
                                let ping_resp = json!({
                                    "jsonrpc": "2.0",
                                    "id": ping_id,
                                    "result": {}
                                });
                                let _ = stdout.write_all(format!("{}\n", ping_resp).as_bytes());
                                let _ = stdout.flush();
                            }
                            continue;
                        }

                        // 2. Notifications handling (JSON-RPC 2.0 Section 4.1 & 4.2).
                        // "The Server MUST NOT reply to a Notification".
                        // Strict clients (e.g. Go MCP client in Gemini CLI) terminate connections
                        // with "invalid request" if any unsolicited response frame is received.
                        if is_notification {
                            let mut request = client.post(&mcp_url).timeout(default_timeout);
                            if let Some(cwd) = client_cwd.as_deref() {
                                request = request.header("X-Axon-Client-Cwd", cwd);
                            }
                            if let Some(client) = client_name.as_deref() {
                                request = request.header("X-Axon-Client-Name", client);
                            }
                            // Fire-and-forget delivery to backend; never write anything to stdout.
                            let _ = request.json(&json_payload).send();
                            continue;
                        }

                        // 3. Regular Request (must always produce a response with matching ID).
                        let request_id = json_payload.get("id").cloned().unwrap_or(Value::Null);
                        let timeout =
                            timeout_for_payload(&json_payload, default_timeout, heavy_timeout);
                        let mut request = client.post(&mcp_url).timeout(timeout);
                        if let Some(cwd) = client_cwd.as_deref() {
                            // REQ-AXO-902286 — carry the caller's project directory.
                            request = request.header("X-Axon-Client-Cwd", cwd);
                        }
                        if let Some(client) = client_name.as_deref() {
                            // REQ-AXO-902555 — carry the caller's client identity.
                            request = request.header("X-Axon-Client-Name", client);
                        }

                        match request.json(&json_payload).send() {
                            Ok(res) => {
                                let status = res.status();
                                if let Ok(res_text) = res.text() {
                                    let trimmed = res_text.trim();
                                    if status.is_success() && !trimmed.is_empty() && trimmed != "null" {
                                        // Ensure the returned JSON-RPC payload carries the caller's request_id.
                                        let formatted = if let Ok(mut parsed) = serde_json::from_str::<Value>(trimmed) {
                                            if parsed.get("id").is_none() || parsed.get("id") == Some(&Value::Null) {
                                                parsed["id"] = request_id.clone();
                                            }
                                            format!("{}\n", parsed)
                                        } else if res_text.ends_with('\n') {
                                            res_text
                                        } else {
                                            format!("{}\n", res_text)
                                        };
                                        let _ = stdout.write_all(formatted.as_bytes());
                                        let _ = stdout.flush();
                                    } else {
                                        // Non-2xx status or empty response for a request: MUST return JSON-RPC error with request_id
                                        let error_msg = if !trimmed.is_empty() && trimmed != "null" {
                                            trimmed.to_string()
                                        } else {
                                            format!("Axon Backend returned HTTP {}", status)
                                        };
                                        let error_resp = json!({
                                            "jsonrpc": "2.0",
                                            "id": request_id,
                                            "error": {
                                                "code": -32603,
                                                "message": error_msg
                                            }
                                        });
                                        let _ = stdout.write_all(format!("{}\n", error_resp).as_bytes());
                                        let _ = stdout.flush();
                                    }
                                }
                            }
                            Err(e) => {
                                // BACKEND DOWN OR TIMEOUT: Return JSON-RPC Error to avoid hanging client
                                eprintln!("Error communicating with Axon Core: {}", e);
                                let error_resp = json!({
                                    "jsonrpc": "2.0",
                                    "id": request_id,
                                    "error": {
                                        "code": -32000,
                                        "message": format!("Axon Backend is unavailable or timed out: {}", e)
                                    }
                                });
                                let _ = stdout.write_all(format!("{}\n", error_resp).as_bytes());
                                let _ = stdout.flush();
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Invalid JSON received on stdin: {}", e);
                        let error_resp = json!({
                            "jsonrpc": "2.0",
                            "id": Value::Null,
                            "error": {
                                "code": -32700,
                                "message": format!("Parse error: {}", e)
                            }
                        });
                        let _ = stdout.write_all(format!("{}\n", error_resp).as_bytes());
                        let _ = stdout.flush();
                    }
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn heavy_batch_tool_gets_heavy_timeout() {
        for name in ["soll_apply_plan", "axon_apply_guidelines", "audit"] {
            let payload = json!({"method": "tools/call", "params": {"name": name}});
            assert_eq!(
                timeout_for_payload(&payload, secs(60), secs(180)),
                secs(180),
                "{name} must get the heavy timeout"
            );
        }
    }

    #[test]
    fn light_tool_gets_default_timeout() {
        let payload = json!({"method": "tools/call", "params": {"name": "query"}});
        assert_eq!(timeout_for_payload(&payload, secs(60), secs(180)), secs(60));
    }

    #[test]
    fn non_tools_call_methods_get_default() {
        for method in ["initialize", "tools/list", "notifications/initialized"] {
            let payload = json!({"method": method});
            assert_eq!(tool_name(&payload), None);
            assert_eq!(timeout_for_payload(&payload, secs(60), secs(180)), secs(60));
        }
    }

    #[test]
    fn malformed_payload_falls_back_to_default() {
        let payload = json!({"foo": "bar"});
        assert_eq!(tool_name(&payload), None);
        assert_eq!(timeout_for_payload(&payload, secs(60), secs(180)), secs(60));
    }

    #[test]
    fn env_timeout_parses_overrides_and_rejects_garbage() {
        // Unset / absent → default.
        assert_eq!(
            env_timeout("AXON_MCP_TUNNEL_TIMEOUT_SECS_UNSET_XYZ", 60),
            secs(60)
        );
    }

    #[test]
    fn notification_detection_identifies_missing_id() {
        let notif = json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 1}});
        assert!(notif.get("id").is_none());

        let req = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
        assert!(req.get("id").is_some());
    }

    #[test]
    fn extract_client_name_finds_name_in_initialize_handshake() {
        let init_payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "claude-code",
                    "version": "1.0.0"
                }
            }
        });
        assert_eq!(
            extract_client_name(&init_payload),
            Some("claude-code".to_string())
        );

        let non_init = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "query"
            }
        });
        assert_eq!(extract_client_name(&non_init), None);
    }
}
