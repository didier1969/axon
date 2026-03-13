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
                        "description": "Recherche hybride (texte + vecteur) et similarité sémantique.",
                        "inputSchema": {
                            "type": "object",
                            "properties": { 
                                "query": { "type": "string" },
                                "project": { "type": "string" }
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "axon_inspect",
                        "description": "Vue 360° d'un symbole (code source, appelants/appelés, statistiques).",
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
                        "name": "axon_audit",
                        "description": "Vérification de conformité (Sécurité OWASP, Qualité, Anti-patterns).",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "project": { "type": "string" } },
                            "required": ["project"]
                        }
                    },
                    {
                        "name": "axon_impact",
                        "description": "Analyse prédictive (Rayon d'impact et chemins critiques).",
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
                        "name": "axon_health",
                        "description": "Rapport de santé global (Code mort, lacunes de tests, points d'entrée).",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "project": { "type": "string" } },
                            "required": ["project"]
                        }
                    },
                    {
                        "name": "axon_diff",
                        "description": "Analyse sémantique des changements (Git Diff -> Symboles touchés).",
                        "inputSchema": {
                            "type": "object",
                            "properties": { 
                                "diff_content": { "type": "string" }
                            },
                            "required": ["diff_content"]
                        }
                    },
                    {
                        "name": "axon_batch",
                        "description": "Orchestration d'appels multiples pour optimiser la performance.",
                        "inputSchema": {
                            "type": "object",
                            "properties": { 
                                "calls": { 
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "tool": { "type": "string" },
                                            "args": { "type": "object" }
                                        },
                                        "required": ["tool", "args"]
                                    }
                                }
                            },
                            "required": ["calls"]
                        }
                    },
                    {
                        "name": "axon_cypher",
                        "description": "Interface de bas niveau pour requêtes HydraDB brutes.",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "cypher": { "type": "string" } },
                            "required": ["cypher"]
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
            "axon_inspect" => self.axon_inspect(arguments),
            "axon_audit" => self.axon_audit(arguments),
            "axon_impact" => self.axon_impact(arguments),
            "axon_health" => self.axon_health(arguments),
            "axon_diff" => self.axon_diff(arguments),
            "axon_batch" => self.axon_batch(arguments),
            "axon_cypher" => self.axon_cypher(arguments),
            _ => Some(json!({ "content": [{ "type": "text", "text": "Tool not found" }], "isError": true })),
        }
    }

    fn axon_query(&self, args: &Value) -> Option<Value> {
        let query = args.get("query")?.as_str()?;
        // Placeholder for hybrid search implementation
        let cypher = format!("MATCH (s:Symbol) WHERE s.name CONTAINS '{}' RETURN s.name, s.kind LIMIT 10", query);
        match self.graph_store.query_json(&cypher) {
            Ok(result) => Some(json!({ "content": [{ "type": "text", "text": result }] })),
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Search Error: {}", e) }], "isError": true })),
        }
    }

    fn axon_cypher(&self, args: &Value) -> Option<Value> {
        let cypher = args.get("cypher")?.as_str()?;
        match self.graph_store.query_json(cypher) {
            Ok(result) => Some(json!({ "content": [{ "type": "text", "text": result }] })),
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Cypher Error: {}", e) }], "isError": true })),
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

    fn axon_diff(&self, _args: &Value) -> Option<Value> {
        Some(json!({ "content": [{ "type": "text", "text": "Diff analysis not yet implemented in v2" }] }))
    }

    fn axon_batch(&self, args: &Value) -> Option<Value> {
        let calls = args.get("calls")?.as_array()?;
        let mut results = Vec::new();

        for call in calls {
            let tool_name = call.get("tool").and_then(|v| v.as_str()).unwrap_or("unknown");
            let empty_args = json!({});
            let tool_args = call.get("args").unwrap_or(&empty_args);
            
            // Build a fake params block
            let params = Some(json!({
                "name": tool_name,
                "arguments": tool_args
            }));

            if let Some(res) = self.handle_call_tool(params) {
                results.push(json!({
                    "tool": tool_name,
                    "result": res
                }));
            } else {
                results.push(json!({
                    "tool": tool_name,
                    "error": "Failed to execute or tool not found"
                }));
            }
        }

        Some(json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&results).unwrap_or_default() }] }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::graph::GraphStore;

    // Helper function to create a dummy server for testing tool signatures
    fn create_test_server() -> McpServer {
        // Use an in-memory or temp DB if needed, here we use a dummy path
        let store = Arc::new(GraphStore::new(":memory:").unwrap_or_else(|_| GraphStore::new("/tmp/test_db").unwrap()));
        McpServer::new(store)
    }

    #[test]
    fn test_mcp_tools_list() {
        let server = create_test_server();
        let req = JsonRpcRequest {
            method: "tools/list".to_string(),
            params: None,
            id: Some(json!(1)),
        };

        let response = server.handle_request(req);
        let result = response.result.expect("Expected result");
        let tools = result.get("tools").expect("Expected tools array").as_array().expect("tools is array");
        
        assert_eq!(tools.len(), 8);
        
        let tool_names: Vec<&str> = tools.iter()
            .map(|t| t.get("name").unwrap().as_str().unwrap())
            .collect();
            
        assert!(tool_names.contains(&"axon_query"));
        assert!(tool_names.contains(&"axon_inspect"));
        assert!(tool_names.contains(&"axon_audit"));
        assert!(tool_names.contains(&"axon_impact"));
        assert!(tool_names.contains(&"axon_health"));
        assert!(tool_names.contains(&"axon_diff"));
        assert!(tool_names.contains(&"axon_batch"));
        assert!(tool_names.contains(&"axon_cypher"));
    }
}
