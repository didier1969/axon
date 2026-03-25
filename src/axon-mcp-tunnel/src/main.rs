use std::io::{self, BufRead, Write};
use reqwest::blocking::Client;
use serde_json::Value;

fn main() {
    let client = Client::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        match line {
            Ok(req_str) => {
                if req_str.trim().is_empty() {
                    continue;
                }
                
                match serde_json::from_str::<Value>(&req_str) {
                    Ok(json_payload) => {
                        match client.post("http://127.0.0.1:44129/mcp")
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
                                        if let Err(_) = stdout.write_all(formatted_res.as_bytes()) {
                                            break; 
                                        }
                                        let _ = stdout.flush();
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("Error communicating with Axon Core HTTP: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Invalid JSON received on stdin: {}", e);
                    }
                }
            }
            Err(_) => break, 
        }
    }
}
