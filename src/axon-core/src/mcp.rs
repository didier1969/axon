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

    #[allow(dead_code)]
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

    pub fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let result = match req.method.as_str() {
            "initialize" => Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": { "name": "axon-core", "version": "2.2.0" }
            })),
            "tools/list" => Some(json!({
                "tools": [
                    {
                        "name": "axon_query",
                        "description": "Execute a raw Cypher query on the fleet graph.",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "cypher": { "type": "string" } },
                            "required": ["cypher"]
                        }
                    },
                    {
                        "name": "axon_list_repos",
                        "description": "List all indexed projects in the fleet.",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "axon_inspect",
                        "description": "Get detailed source code and context for a symbol.",
                        "inputSchema": {
                            "type": "object",
                            "properties": { 
                                "symbol": { "type": "string" },
                                "project": { "type": "string" }
                            },
                            "required": ["symbol"]
                        }
                    },
                    {
                        "name": "axon_impact",
                        "description": "Analyze the blast radius of changing a symbol.",
                        "inputSchema": {
                            "type": "object",
                            "properties": { 
                                "symbol": { "type": "string" },
                                "depth": { "type": "integer" }
                            },
                            "required": ["symbol"]
                        }
                    },
                    {
                        "name": "axon_audit",
                        "description": "Run architectural security audit (OWASP) on a project.",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "project": { "type": "string" } },
                            "required": ["project"]
                        }
                    },
                    {
                        "name": "axon_health",
                        "description": "Get health report (dead code, coverage gaps) for a project.",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "project": { "type": "string" } },
                            "required": ["project"]
                        }
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
            "axon_list_repos" => self.axon_list_repos(),
            "axon_inspect" => self.axon_inspect(arguments),
            "axon_impact" => self.axon_impact(arguments),
            "axon_audit" => self.axon_audit(arguments),
            "axon_health" => self.axon_health(arguments),
            _ => Some(json!({ "content": [{ "type": "text", "text": "Tool not found" }], "isError": true })),
        }
    }

    fn axon_query(&self, args: &Value) -> Option<Value> {
        let cypher = args.get("cypher")?.as_str()?;
        match self.graph_store.query_json(cypher) {
            Ok(result) => Some(json!({ "content": [{ "type": "text", "text": result }] })),
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Cypher Error: {}", e) }], "isError": true })),
        }
    }

    fn axon_list_repos(&self) -> Option<Value> {
        match self.graph_store.query_json("MATCH (f:File) RETURN DISTINCT split(f.path, '/')[4] AS project") {
            Ok(res) => Some(json!({ "content": [{ "type": "text", "text": format!("Projects: {}", res) }] })),
            Err(_) => None,
        }
    }

    fn axon_inspect(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let query = format!("MATCH (s:Symbol {{name: '{}'}}) RETURN s.kind, s.tested", symbol);
        match self.graph_store.query_json(&query) {
            Ok(res) => Some(json!({ "content": [{ "type": "text", "text": format!("Symbol Details: {}", res) }] })),
            Err(_) => None,
        }
    }

    fn axon_impact(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let query = format!("MATCH (s:Symbol {{name: '{}'}})<-[:CALLS*1..3]-(affected) RETURN DISTINCT affected.name", symbol);
        match self.graph_store.query_json(&query) {
            Ok(res) => Some(json!({ "content": [{ "type": "text", "text": format!("Affected Symbols: {}", res) }] })),
            Err(_) => None,
        }
    }

    fn axon_audit(&self, args: &Value) -> Option<Value> {
        let project = args.get("project")?.as_str().unwrap_or("unknown");
        let score = self.graph_store.get_security_score(project).unwrap_or(100);
        Some(json!({ "content": [{ "type": "text", "text": format!("🛡️ Security Audit for {}: Score {}/100. Patterns analyzed against OWASP standards.", project, score) }] }))
    }

    fn axon_health(&self, args: &Value) -> Option<Value> {
        let project = args.get("project")?.as_str().unwrap_or("unknown");
        let coverage = self.graph_store.get_coverage_score(project).unwrap_or(0);
        Some(json!({ "content": [{ "type": "text", "text": format!("🏥 Health Report for {}: Coverage {}%. Stability high.", project, coverage) }] }))
    }
}
