use std::io::{self, BufRead, Write};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::env;
use std::time::Duration;

fn main() {
    // We use a short timeout to fail fast if backend is stuck or down
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to create HTTP client");
    let mcp_url =
        env::var("AXON_MCP_URL").unwrap_or_else(|_| "http://127.0.0.1:44129/mcp".to_string());

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        match line {
            Ok(req_str) => {
                if req_str.trim().is_empty() {
                    continue;
                }
                
                let req_val: Result<Value, _> = serde_json::from_str(&req_str);
                let request_id = req_val.as_ref().ok()
                    .and_then(|v| v.get("id"))
                    .cloned()
                    .unwrap_or(json!(1));

                match req_val {
                    Ok(json_payload) => {
                        match client.post(&mcp_url)
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
