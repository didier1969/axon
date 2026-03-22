use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<Value>,
    pub id: Option<Value>,
}

fn main() {
    let s = r#"{"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}"#;
    let req: JsonRpcRequest = serde_json::from_str(s).unwrap();
    println!("No id: is_none() = {}", req.id.is_none());

    let s2 = r#"{"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}, "id": null}"#;
    let req2: JsonRpcRequest = serde_json::from_str(s2).unwrap();
    println!("Null id: is_none() = {}, is_null() = {}", req2.id.is_none(), req2.id.as_ref().unwrap().is_null());
}
