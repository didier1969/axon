use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead};
use anyhow::Result;
use crate::graph::GraphStore;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub method: String,
    pub params: Option<Value>,
    pub id: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub result: Option<Value>,
    pub error: Option<Value>,
    pub id: Option<Value>,
}

pub struct McpServer {
    graph_store: Arc<GraphStore>,
}

impl McpServer {
    pub fn new(graph_store: Arc<GraphStore>) -> Self {
        Self { graph_store }
    }

    pub fn run(&self) -> Result<()> {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line = line?;
            if let Ok(request) = serde_json::from_str::<JsonRpcRequest>(&line) {
                let response = self.handle_request(request);
                println!("{}", serde_json::to_string(&response)?);
            }
        }
        Ok(())
    }

    fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let result = match req.method.as_str() {
            "initialize" => Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": { "name": "axon-core", "version": "2.0.0" }
            })),
            "tools/list" => Some(json!({
                "tools": [
                    {
                        "name": "axon_query",
                        "description": "Exécuter une requête Cypher sur la base de données de graphe Axon.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "cypher": { "type": "string" }
                            },
                            "required": ["cypher"]
                        }
                    },
                    {
                        "name": "axon_fleet_status",
                        "description": "Récupérer l'état actuel de l'indexation (fichiers, symboles).",
                        "inputSchema": { "type": "object", "properties": {} }
                    }
                ]
            })),
            "tools/call" => self.handle_call_tool(req.params),
            _ => None,
        };

        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result,
            error: None,
            id: req.id,
        }
    }

    fn handle_call_tool(&self, params: Option<Value>) -> Option<Value> {
        let params = params?;
        let name = params.get("name")?.as_str()?;
        let arguments = params.get("arguments")?;

        match name {
            "axon_query" => self.axon_query(arguments),
            "axon_fleet_status" => self.axon_fleet_status(),
            _ => Some(json!({ "content": [{ "type": "text", "text": "Tool not found" }], "isError": true })),
        }
    }

    fn axon_query(&self, args: &Value) -> Option<Value> {
        let cypher = args.get("cypher")?.as_str()?;
        match self.graph_store.get_connection() {
            Ok(conn) => {
                match conn.query(cypher) {
                    Ok(result) => {
                        // Pour le moment on renvoie une représentation textuelle simple
                        // Dans la Task 10, on formatera les résultats JSON-RPC proprement
                        Some(json!({ "content": [{ "type": "text", "text": format!("{:?}", result) }] }))
                    }
                    Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Cypher Error: {}", e) }], "isError": true })),
                }
            }
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Connection Error: {}", e) }], "isError": true })),
        }
    }

    fn axon_fleet_status(&self) -> Option<Value> {
        match self.graph_store.get_connection() {
            Ok(conn) => {
                let files = conn.query("MATCH (f:File) RETURN count(f)").ok();
                let symbols = conn.query("MATCH (s:Symbol) RETURN count(s)").ok();
                Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!("Fleet Status: Files: {:?}, Symbols: {:?}", files, symbols)
                    }]
                }))
            }
            Err(_) => None,
        }
    }
}
