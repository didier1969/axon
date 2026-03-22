use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<serde_json::Value>,
    pub id: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
    pub id: Option<serde_json::Value>,
}

fn main() {
    let req_str = r#"{"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}"#;
    let req: JsonRpcRequest = serde_json::from_str(req_str).unwrap();
    println!("req.id.is_none() = {}", req.id.is_none());
    
    let res = JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        result: None,
        error: None,
        id: req.id,
    };
    println!("Response: {}", serde_json::to_string(&res).unwrap());
}
