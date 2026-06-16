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

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        match line {
            Ok(req_str) => {
                if req_str.trim().is_empty() {
                    continue;
                }

                let req_val: Result<Value, _> = serde_json::from_str(&req_str);
                let request_id = req_val
                    .as_ref()
                    .ok()
                    .and_then(|v| v.get("id"))
                    .cloned()
                    .unwrap_or(json!(1));

                match req_val {
                    Ok(json_payload) => {
                        let timeout =
                            timeout_for_payload(&json_payload, default_timeout, heavy_timeout);
                        match client
                            .post(&mcp_url)
                            .timeout(timeout)
                            .json(&json_payload)
                            .send()
                        {
                            Ok(res) => {
                                if let Ok(res_text) = res.text() {
                                    if res_text.trim() != "null" && !res_text.trim().is_empty() {
                                        let formatted_res = if res_text.ends_with('\n') {
                                            res_text
                                        } else {
                                            format!("{}\n", res_text)
                                        };
                                        let _ = stdout.write_all(formatted_res.as_bytes());
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
                            "id": request_id,
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
}
