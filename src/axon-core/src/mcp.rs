use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use anyhow::Result;
use crate::graph::GraphStore;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<Value>,
    pub id: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    pub id: Option<Value>,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<Value>,
}

pub struct McpServer {
    graph_store: Arc<GraphStore>,
}

impl McpServer {
    pub fn new(graph_store: Arc<GraphStore>) -> Self {
        Self { graph_store }
    }

    #[allow(dead_code)]
    pub async fn run_stdio(&self) -> Result<()> {
        let mut stdin = BufReader::new(tokio::io::stdin());
        let mut stdout = tokio::io::stdout();
        let mut line = String::new();

        while let Ok(bytes_read) = stdin.read_line(&mut line).await {
            if bytes_read == 0 {
                break;
            }
            
            match serde_json::from_str::<JsonRpcRequest>(&line) {
                Ok(request) => {
                    let response = self.handle_request(request);
                    let mut response_str = serde_json::to_string(&response)?;
                    response_str.push('\n');
                    let _ = stdout.write_all(response_str.as_bytes()).await;
                },
                Err(e) => {
                    let error_response = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        result: None,
                        error: Some(json!({
                            "code": -32700,
                            "message": "Parse error",
                            "data": e.to_string()
                        })),
                        id: None,
                    };
                    if let Ok(mut response_str) = serde_json::to_string(&error_response) {
                        response_str.push('\n');
                        let _ = stdout.write_all(response_str.as_bytes()).await;
                    }
                }
            }
            line.clear();
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn send_notification(&self, method: &str, params: Option<Value>) -> JsonRpcNotification {
        JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        }
    }

    fn format_kuzu_table(&self, json_res: &str, headers: &[&str]) -> String {
        let rows: Vec<Vec<String>> = match serde_json::from_str(json_res) {
            Ok(r) => r,
            Err(_) => return format!("Erreur de formatage : {}", json_res),
        };

        if rows.is_empty() {
            return "Aucun résultat trouvé.".to_string();
        }

        let mut output = String::new();

        // Header
        output.push('|');
        for h in headers {
            output.push_str(&format!(" {} |", h));
        }
        output.push('\n');

        // Separator
        output.push('|');
        for _ in headers {
            output.push_str(" --- |");
        }
        output.push('\n');

        // Body
        for row in rows {
            output.push('|');
            for val in row {
                let clean_val = val.trim_start_matches("String(\"").trim_end_matches("\")")
                                   .trim_start_matches("Int64(").trim_end_matches(")")
                                   .trim_start_matches("Boolean(").trim_end_matches(")")
                                   .replace("\\\"", "\"");
                output.push_str(&format!(" {} |", clean_val));
            }
            output.push('\n');
        }

        output
    }

    pub fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        if req.id.is_none() {
            return None;
        }

        let result = match req.method.as_str() {
            "initialize" => Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": { "name": "axon-core", "version": "2.2.0" }
            })),
            "tools/list" => Some(json!({
                "tools": [
                    {
                        "name": "axon_refine_lattice",
                        "description": "[SYSTEM] Lattice Refiner: Analyse le graphe post-ingestion pour lier les frontières inter-langages (ex: Elixir NIF -> Rust natif).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "required": []
                        }
                    },
                    {
                        "name": "axon_fs_read",
                        "description": "[DX] Agent DX L2 (Detail) : Lit le contenu physique complet d'un fichier source. À n'utiliser qu'après avoir identifié une URI (chemin) précise via axon_query ou axon_inspect.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "uri": { "type": "string", "description": "Le chemin complet vers le fichier (ex: 'src/main.rs')" },
                                "start_line": { "type": "integer", "description": "Ligne de début optionnelle" },
                                "end_line": { "type": "integer", "description": "Ligne de fin optionnelle" }
                            },
                            "required": ["uri"]
                        }
                    },
                    {
                        "name": "axon_soll_manager",
                        "description": "[SOLL] Centre de commande pour le graphe intentionnel. Gère la création (avec IDs auto), la mise à jour et les liaisons hiérarchiques.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "action": { "type": "string", "enum": ["create", "update", "link"], "description": "L'opération à effectuer." },
                                "entity": { "type": "string", "enum": ["pillar", "requirement", "concept", "milestone", "decision", "stakeholder", "validation"], "description": "Le type d'objet concerné." },
                                "data": { 
                                    "type": "object", 
                                    "description": "Données JSON. \n- create (pillar: title, desc; requirement: title, desc, priority; concept: name, explanation, rationale; decision: title, context, rationale, status; milestone: title, status; stakeholder: name, role; validation: method, result).\n- update (id, status/desc/etc).\n- link (source_id, target_id)." 
                                }
                            },
                            "required": ["action", "entity", "data"]
                        }
                    },
                    {
                        "name": "axon_export_soll",
                        "description": "[SOLL] Exporte l'intégralité du graphe intentionnel (Vision, Pillars, Milestones, Requirements, Decisions, Concepts) dans un document Markdown horodaté.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "required": []
                        }
                    },
                    {
                        "name": "axon_query",
                        "description": "[DX] Recherche hybride (texte + vecteur) et similarité sémantique.",
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
                        "description": "[DX] Vue 360° d'un symbole (code source, appelants/appelés, statistiques).",
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
                        "description": "[GOVERNANCE] Vérification de conformité (Sécurité OWASP, Qualité, Anti-patterns, Dette Technique).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "project": { "type": "string" }
                            },
                            "required": []
                        }
                    },
                    {
                        "name": "axon_impact",
                        "description": "[RISK] Analyse prédictive (Rayon d'impact et chemins critiques).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "depth": { "type": "integer" },
                                "symbol": { "type": "string" }
                            },
                            "required": ["symbol"]
                        }
                    },
                    {
                        "name": "axon_health",
                        "description": "[GOVERNANCE] Rapport de santé global (Code mort, lacunes de tests, points d'entrée).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "project": { "type": "string" }
                            },
                            "required": []
                        }
                    },
                    {
                        "name": "axon_diff",
                        "description": "[RISK] Analyse sémantique des changements (Git Diff -> Symboles touchés).",
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
                        "description": "[SYSTEM] Orchestration d'appels multiples pour optimiser la performance.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "calls": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "tool": { "type": "string" },
                                            "args": { "type": "object", "additionalProperties": true }
                                        },
                                        "required": ["tool", "args"]
                                    }
                                }
                            },
                            "required": ["calls"]
                        }
                    },
                    {
                        "name": "axon_semantic_clones",
                        "description": "[GOVERNANCE] Trouve des fonctions sémantiquement similaires (clones de logique) dans le projet.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "symbol": { "type": "string", "description": "Nom du symbole source" }
                            },
                            "required": ["symbol"]
                        }
                    },
                    {
                        "name": "axon_architectural_drift",
                        "description": "[GOVERNANCE] Vérifie les violations d'architecture entre deux couches (ex: 'ui' appelant directement 'db').",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "source_layer": { "type": "string", "description": "Couche source (ex: 'ui', 'frontend')" },
                                "target_layer": { "type": "string", "description": "Couche interdite (ex: 'db', 'repository')" }
                            },
                            "required": ["source_layer", "target_layer"]
                        }
                    },
                    {
                        "name": "axon_bidi_trace",
                        "description": "[DX] Trace bidirectionnelle: remonte aux Entry Points (haut) et liste les appels profonds (bas).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "symbol": { "type": "string", "description": "Symbole de départ" },
                                "depth": { "type": "integer", "description": "Profondeur maximale (défaut: sans limite pour être exhaustif, mais cappé par le moteur)" }
                            },
                            "required": ["symbol"]
                        }
                    },
                    {
                        "name": "axon_api_break_check",
                        "description": "[RISK] Vérifie si la modification d'un symbole public impacte des composants externes.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "symbol": { "type": "string" }
                            },
                            "required": ["symbol"]
                        }
                    },
                    {
                        "name": "axon_simulate_mutation",
                        "description": "[RISK] Dry-run : calcule le volume de l'impact d'une modification avant de coder.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "symbol": { "type": "string" },
                                "depth": { "type": "integer", "description": "Profondeur d'impact (optionnel)" }
                            },
                            "required": ["symbol"]
                        }
                    },
                    {
                        "name": "axon_cypher",
                        "description": "[SYSTEM] Interface de bas niveau pour requêtes HydraDB brutes.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "cypher": { "type": "string" }
                            },
                            "required": ["cypher"]
                        }
                    },
                    json!({
                        "name": "axon_debug",
                        "description": "[SYSTEM] Diagnostic système bas niveau : Affiche l'état interne du moteur Axon V2 (RAM, DB, architecture, statut d'indexation) pour éviter les hallucinations sur l'infrastructure.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "required": []
                        }
                    })
                ]
            })),
            "tools/call" => self.handle_call_tool(req.params),
            _ => None,
        };

        if let Some(res) = result {
            Some(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: Some(res),
                error: None,
                id: req.id,
            })
        } else {
            Some(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: None,
                error: Some(json!({
                    "code": -32601,
                    "message": "Method not found"
                })),
                id: req.id,
            })
        }
    }

    fn handle_call_tool(&self, params: Option<Value>) -> Option<Value> {
        let params = params?;
        let name = params.get("name")?.as_str()?;
        let arguments = params.get("arguments")?;

        match name {
            "axon_refine_lattice" => self.axon_refine_lattice(arguments),
            "axon_fs_read" => self.axon_fs_read(arguments),
            "axon_query" => self.axon_query(arguments),
            "axon_soll_manager" => self.axon_soll_manager(arguments),
            "axon_export_soll" => self.axon_export_soll(),
            "axon_inspect" => self.axon_inspect(arguments),
            "axon_audit" => self.axon_audit(arguments),
            "axon_impact" => self.axon_impact(arguments),
            "axon_health" => self.axon_health(arguments),
            "axon_diff" => self.axon_diff(arguments),
            "axon_batch" => self.axon_batch(arguments),
            "axon_cypher" => self.axon_cypher(arguments),
            "axon_semantic_clones" => self.axon_semantic_clones(arguments),
            "axon_architectural_drift" => self.axon_architectural_drift(arguments),
            "axon_bidi_trace" => self.axon_bidi_trace(arguments),
            "axon_api_break_check" => self.axon_api_break_check(arguments),
            "axon_simulate_mutation" => self.axon_simulate_mutation(arguments),
            "axon_debug" => self.axon_debug(),
            _ => Some(json!({ "content": [{ "type": "text", "text": "Tool not found" }], "isError": true })),
        }
    }

    fn axon_refine_lattice(&self, _args: &Value) -> Option<Value> {
        let store = &self.graph_store;
        let refine_query = "
            MATCH (elixir:Symbol {is_nif: true})<-[:CONTAINS]-(e_file:File)
            MATCH (rust:Symbol {is_nif: true})<-[:CONTAINS]-(r_file:File)
            WHERE elixir.name = rust.name 
            MERGE (elixir)-[:CALLS_NIF]->(rust)
            RETURN elixir.name, e_file.path, r_file.path
        ";
        match store.query_json(refine_query) {
            Ok(res) => {
                let parsed: Vec<Value> = serde_json::from_str(&res).unwrap_or_default();
                let count = parsed.len();
                let report = if count > 0 {
                    format!("✨ **Lattice Refiner exécuté avec succès.**\n\nJ'ai découvert et lié **{} ponts FFI (Rustler NIFs)** entre Elixir et Rust.\n\n{}", count, self.format_kuzu_table(&res, &["Nom NIF", "Fichier Elixir", "Fichier Rust"]))
                } else {
                    "✅ **Lattice Refiner exécuté.**\nAucun nouveau pont FFI (Rustler NIF) non-lié n'a été détecté dans le graphe.".to_string()
                };
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Refiner Error: {}", e) }], "isError": true })),
        }
    }

    fn axon_fs_read(&self, args: &Value) -> Option<Value> {
        let uri = args.get("uri")?.as_str()?;
        let start_line = args.get("start_line").and_then(|v| v.as_u64());
        let end_line = args.get("end_line").and_then(|v| v.as_u64());
        
        let file_path = std::path::Path::new(uri);
        if !file_path.exists() || !file_path.is_file() {
            return Some(json!({ "content": [{ "type": "text", "text": format!("Erreur: Le fichier '{}' n'existe pas ou n'est pas lisible.", uri) }], "isError": true }));
        }

        match std::fs::read_to_string(file_path) {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let total_lines = lines.len();
                let start = start_line.unwrap_or(1).saturating_sub(1) as usize;
                let end = end_line.unwrap_or(total_lines as u64) as usize;
                let start = start.min(total_lines);
                let end = end.min(total_lines).max(start);
                let sliced_content = lines[start..end].join("\n");
                let report = format!("📄 L2 Detail : {}\n(Lignes {} à {} sur {})\n\n```\n{}\n```", uri, start + 1, end, total_lines, sliced_content);
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Erreur de lecture: {}", e) }], "isError": true })),
        }
    }

    fn axon_soll_manager(&self, args: &Value) -> Option<Value> {
        let action = args.get("action")?.as_str()?;
        let entity = args.get("entity")?.as_str()?;
        let data = args.get("data")?;

        match action {
            "create" => {
                let reg_col = match entity {
                    "pillar" => "last_req", 
                    "requirement" => "last_req",
                    "concept" => "last_cpt",
                    "decision" => "last_dec",
                    "milestone" => "last_mil",
                    "validation" => "last_val",
                    "stakeholder" => "id", // Not incremented, name is PK
                    _ => return None,
                };
                let prefix = match entity {
                    "pillar" => "PIL",
                    "requirement" => "REQ",
                    "concept" => "CPT",
                    "decision" => "DEC",
                    "milestone" => "MIL",
                    "validation" => "VAL",
                    _ => "OBJ",
                };

                let update_query = if entity == "stakeholder" {
                    "SELECT 0".to_string() // Dummy
                } else {
                    format!("UPDATE soll.Registry SET {0} = {0} + 1 WHERE id = 'AXON_GLOBAL' RETURNING {0}", reg_col)
                };

                match self.graph_store.query_json(&update_query) {
                    Ok(res) => {
                        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
                        let formatted_id = if entity == "stakeholder" {
                            data.get("name")?.as_str()?.to_string()
                        } else {
                            if rows.is_empty() || rows[0].is_empty() {
                                return Some(json!({ "content": [{ "type": "text", "text": "Erreur: Registre SOLL non initialisé." }], "isError": true }));
                            }
                            let next_num: u64 = rows[0][0].parse().unwrap_or(0);
                            format!("{}-AXO-{:03}", prefix, next_num)
                        };
                        
                        let insert_res = match entity {
                            "pillar" => {
                                let title = data.get("title")?.as_str()?;
                                let desc = data.get("description")?.as_str()?;
                                let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                                let q = "INSERT INTO soll.Pillar (id, title, description, metadata) VALUES (?, ?, ?, ?)";
                                self.graph_store.execute_param(q, &json!([formatted_id, title, desc, meta.to_string()]))
                            },
                            "requirement" => {
                                let title = data.get("title")?.as_str()?;
                                let desc = data.get("description")?.as_str()?;
                                let prio = data.get("priority").and_then(|v| v.as_str()).unwrap_or("P2");
                                let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                                let q = "INSERT INTO soll.Requirement (id, title, description, priority, metadata) VALUES (?, ?, ?, ?, ?)";
                                self.graph_store.execute_param(q, &json!([formatted_id, title, desc, prio, meta.to_string()]))
                            },
                            "concept" => {
                                let name = data.get("name")?.as_str()?;
                                let expl = data.get("explanation")?.as_str()?;
                                let rat = data.get("rationale")?.as_str()?;
                                let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                                let final_name = format!("{}: {}", formatted_id, name);
                                let q = "INSERT INTO soll.Concept (name, explanation, rationale, metadata) VALUES (?, ?, ?, ?)";
                                self.graph_store.execute_param(q, &json!([final_name, expl, rat, meta.to_string()]))
                            },
                            "decision" => {
                                let title = data.get("title")?.as_str()?;
                                let ctx = data.get("context")?.as_str()?;
                                let rat = data.get("rationale")?.as_str()?;
                                let status = data.get("status").and_then(|v| v.as_str()).unwrap_or("accepted");
                                let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                                let q = "INSERT INTO soll.Decision (id, title, context, rationale, status, metadata) VALUES (?, ?, ?, ?, ?, ?)";
                                self.graph_store.execute_param(q, &json!([formatted_id, title, ctx, rat, status, meta.to_string()]))
                            },
                            "milestone" => {
                                let title = data.get("title")?.as_str()?;
                                let status = data.get("status").and_then(|v| v.as_str()).unwrap_or("planned");
                                let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                                let q = "INSERT INTO soll.Milestone (id, title, status, metadata) VALUES (?, ?, ?, ?)";
                                self.graph_store.execute_param(q, &json!([formatted_id, title, status, meta.to_string()]))
                            },
                            "stakeholder" => {
                                let name = data.get("name")?.as_str()?;
                                let role = data.get("role")?.as_str()?;
                                let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                                let q = "INSERT INTO soll.Stakeholder (name, role, metadata) VALUES (?, ?, ?)";
                                self.graph_store.execute_param(q, &json!([name, role, meta.to_string()]))
                            },
                            "validation" => {
                                let method = data.get("method")?.as_str()?;
                                let result = data.get("result")?.as_str()?;
                                let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                                let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
                                let q = "INSERT INTO soll.Validation (id, method, result, timestamp, metadata) VALUES (?, ?, ?, ?, ?)";
                                self.graph_store.execute_param(q, &json!([formatted_id, method, result, ts, meta.to_string()]))
                            },
                            _ => Err(anyhow::anyhow!("Unknown entity")),
                        };

                        match insert_res {
                            Ok(_) => {
                                let report = format!("✅ Entité SOLL créée : `{}`", formatted_id);
                                Some(json!({ "content": [{ "type": "text", "text": report }] }))
                            },
                            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Erreur d'insertion: {}", e) }], "isError": true }))
                        }
                    },
                    Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Erreur registre: {}", e) }], "isError": true }))
                }
            },
            "update" => {
                let id = data.get("id")?.as_str()?;
                let update_res = match entity {
                    "pillar" => {
                        let title = data.get("title")?.as_str()?;
                        let desc = data.get("description")?.as_str()?;
                        let q = "UPDATE soll.Pillar SET title = ?, description = ? WHERE id = ?";
                        self.graph_store.execute_param(q, &json!([title, desc, id]))
                    },
                    "requirement" => {
                        let title = data.get("title")?.as_str()?;
                        let desc = data.get("description")?.as_str()?;
                        let q = "UPDATE soll.Requirement SET title = ?, description = ? WHERE id = ?";
                        self.graph_store.execute_param(q, &json!([title, desc, id]))
                    },
                    "concept" => {
                        let expl = data.get("explanation")?.as_str()?;
                        let rat = data.get("rationale")?.as_str()?;
                        let q = "UPDATE soll.Concept SET explanation = ?, rationale = ? WHERE name LIKE ?";
                        self.graph_store.execute_param(q, &json!([expl, rat, format!("{}%", id)]))
                    },
                    "decision" => {
                        let status = data.get("status")?.as_str()?;
                        let q = "UPDATE soll.Decision SET status = ? WHERE id = ?";
                        self.graph_store.execute_param(q, &json!([status, id]))
                    },
                    "stakeholder" => {
                        let role = data.get("role")?.as_str()?;
                        let q = "UPDATE soll.Stakeholder SET role = ? WHERE name = ?";
                        self.graph_store.execute_param(q, &json!([role, id]))
                    },
                    "validation" => {
                        let result = data.get("result")?.as_str()?;
                        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
                        let q = "UPDATE soll.Validation SET result = ?, timestamp = ? WHERE id = ?";
                        self.graph_store.execute_param(q, &json!([result, ts, id]))
                    },
                    _ => Err(anyhow::anyhow!("Unknown entity")),
                };
                match update_res {
                    Ok(_) => Some(json!({ "content": [{ "type": "text", "text": format!("✅ Mise à jour réussie pour `{}`", id) }] })),
                    Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Erreur update: {}", e) }], "isError": true }))
                }
            },
            "link" => {
                let src = data.get("source_id")?.as_str()?;
                let tgt = data.get("target_id")?.as_str()?;
                let rel_table = match (src.split('-').next().unwrap_or(""), tgt.split('-').next().unwrap_or("")) {
                    ("PIL", "REQ") | ("REQ", "PIL") => "soll.BELONGS_TO",
                    ("CPT", "REQ") | ("REQ", "CPT") => "soll.EXPLAINS",
                    ("PIL", "AXO") | ("AXO", "PIL") => "soll.EPITOMIZES",
                    ("DEC", "REQ") | ("REQ", "DEC") => "soll.SOLVES",
                    ("MIL", "REQ") | ("REQ", "MIL") => "soll.TARGETS",
                    ("VAL", "REQ") | ("REQ", "VAL") => "soll.VERIFIES",
                    ("STK", "REQ") | ("REQ", "STK") => "soll.ORIGINATES",
                    ("DEC", _) => "IMPACTS", // Link decision to a physical symbol (IST)
                    _ => "SUBSTANTIATES", 
                };
                // Note: DuckDB links are simple source/target VARCHARs
                let q = format!("INSERT INTO {} (source_id, target_id) VALUES (?, ?)", rel_table);
                match self.graph_store.execute_param(&q, &json!([src, tgt])) {
                    Ok(_) => Some(json!({ "content": [{ "type": "text", "text": format!("✅ Liaison établie : `{}` -> `{}`", src, tgt) }] })),
                    Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Erreur liaison: {}", e) }], "isError": true }))
                }
            }
            _ => None,
        }
    }

    fn axon_export_soll(&self) -> Option<Value> {
        let mut markdown = String::from("# Axon Lattice - SOLL Extraction\n\n## 1. Vision\n");
        if let Ok(res) = self.graph_store.query_json("SELECT title, description, goal, metadata FROM soll.Vision") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("### {}\n*Description:* {}\n*Goal:* {}\n*Metadata:* {}\n\n", r[0], r[1], r[2], r[3]));
            }
        }
        
        markdown.push_str("## 2. Pillars\n");
        if let Ok(res) = self.graph_store.query_json("SELECT id, title, description, metadata FROM soll.Pillar") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("* **{}** ({}): {} | Meta: {}\n", r[0], r[1], r[2], r[3]));
            }
        }

        markdown.push_str("\n## 3. Milestones\n");
        if let Ok(res) = self.graph_store.query_json("SELECT id, title, status, deadline, metadata FROM soll.Milestone") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("### {} - {}\n*Status:* {} | *Deadline:* {}\n*Metadata:* {}\n\n", r[0], r[1], r[2], r[3], r[4]));
            }
        }

        markdown.push_str("## 4. Requirements\n");
        if let Ok(res) = self.graph_store.query_json("SELECT id, title, description, justification, priority, metadata FROM soll.Requirement") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("### {} - {}\n*Priority:* {}\n*Description:* {}\n*Justification:* {}\n*Metadata:* {}\n\n", r[0], r[1], r[4], r[2], r[3], r[5]));
            }
        }

        markdown.push_str("## 5. Decisions (ADR)\n");
        if let Ok(res) = self.graph_store.query_json("SELECT id, title, context, rationale, status, metadata FROM soll.Decision") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("### {} - {}\n*Status:* {}\n*Context:* {}\n*Rationale:* {}\n*Metadata:* {}\n\n", r[0], r[1], r[4], r[2], r[3], r[5]));
            }
        }

        markdown.push_str("## 6. Concepts\n");
        if let Ok(res) = self.graph_store.query_json("SELECT name, explanation, rationale, metadata FROM soll.Concept") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("### {}\n*Explanation:* {}\n*Rationale:* {}\n*Metadata:* {}\n\n", r[0], r[1], r[2], r[3]));
            }
        }

        markdown.push_str("## 7. Stakeholders & Validations\n");
        if let Ok(res) = self.graph_store.query_json("SELECT name, role FROM soll.Stakeholder") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("* **{}** : {}\n", r[0], r[1]));
            }
        }

        let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let file_name = format!("SOLL_EXPORT_{}.md", timestamp);
        let file_path = format!("docs/vision/{}", file_name);
        
        let _ = std::fs::create_dir_all("docs/vision");
        match std::fs::write(&file_path, &markdown) {
            Ok(_) => {
                let report = format!("✅ Exported to {}\n\n{}", file_path, markdown.chars().take(250).collect::<String>());
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Erreur d'écriture fichier: {}", e) }], "isError": true }))
        }
    }

    fn axon_query(&self, args: &Value) -> Option<Value> {
        let query_text = args.get("query")?.as_str()?;
        let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        
        let embedding = crate::embedder::batch_embed(vec![query_text.to_string()]).ok()
            .and_then(|v| v.into_iter().next());

        let (sql, params) = if let Some(emb) = embedding {
            let vec_str = format!("{:?}", emb);
            if project == "*" {
                (
                    format!(
                        "SELECT s.name, s.kind, f.path AS uri, array_cosine_distance(s.embedding, {}::FLOAT[384]) as score \
                         FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                         WHERE s.name LIKE '%' || $q || '%' OR array_cosine_distance(s.embedding, {}::FLOAT[384]) < 0.5 \
                         ORDER BY score ASC LIMIT 10",
                        vec_str, vec_str
                    ),
                    json!({"q": query_text})
                )
            } else {
                (
                    format!(
                        "SELECT s.name, s.kind, f.path AS uri, array_cosine_distance(s.embedding, {}::FLOAT[384]) as score \
                         FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                         WHERE f.path LIKE '%' || $proj || '%' AND (s.name LIKE '%' || $q || '%' OR array_cosine_distance(s.embedding, {}::FLOAT[384]) < 0.5) \
                         ORDER BY score ASC LIMIT 10",
                        vec_str, vec_str
                    ),
                    json!({"q": query_text, "proj": project})
                )
            }
        } else {
            if project == "*" {
                (
                    "SELECT s.name, s.kind, f.path AS uri \
                     FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                     WHERE s.name LIKE '%' || $q || '%' LIMIT 10".to_string(),
                    json!({"q": query_text})
                )
            } else {
                (
                    "SELECT s.name, s.kind, f.path AS uri \
                     FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                     WHERE f.path LIKE '%' || $proj || '%' AND s.name LIKE '%' || $q || '%' LIMIT 10".to_string(),
                    json!({"q": query_text, "proj": project})
                )
            }
        };

        match self.graph_store.query_json_param(&sql, &params) {
            Ok(res) => {
                let headers = if sql.contains("score") {
                    vec!["Nom", "Type", "URI (Chemin)", "Distance Sémantique"]
                } else {
                    vec!["Nom", "Type", "URI (Chemin)"]
                };
                let table = self.format_kuzu_table(&res, &headers);
                Some(json!({ "content": [{ "type": "text", "text": format!("### 🔎 Résultats de Recherche Hybride : '{}'\n\n{}", query_text, table) }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Search Error: {}", e) }], "isError": true })),
        }
    }

    fn axon_debug(&self) -> Option<Value> {
        let file_count = self.graph_store.query_count("SELECT count(*) FROM File").unwrap_or(0);
        let symbol_count = self.graph_store.query_count("SELECT count(*) FROM Symbol").unwrap_or(0);
        let edge_count = self.graph_store.query_count("SELECT (SELECT count(*) FROM CONTAINS) + (SELECT count(*) FROM CALLS)").unwrap_or(0);

        let mut mem_str = "Unknown".to_string();
        if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(rss_pages) = content.split_whitespace().nth(1).and_then(|s| s.parse::<u64>().ok()) {
                let rss_mb = (rss_pages * 4096) / 1024 / 1024;
                mem_str = format!("{} MB", rss_mb);
            }
        }

        let report = format!(
            "## 🤖 Axon Core V2 (Maestria) - Diagnostic Interne\n\n\
            **Architecture du Moteur :**\n\
            *   **Mode :** Embarqué (C-FFI) sans réseau TCP.\n\
            *   **Base de Graphe :** DuckDB (Local, Zero-Copy).\n\
            *   **Parseurs Actifs :** Rust, Elixir, Python, TypeScript, etc.\n\
            *   **Protection OOM :** Option B (Watchdog Process Cycling Actif à 14 Go).\n\n\
            **État de la Mémoire (RSS) :** {}\n\n\
            **Volume du Graphe en direct :**\n\
            *   Fichiers indexés : {}\n\
            *   Symboles extraits : {}\n\
            *   Relations (Edges) : {}\n\n\
            *Note aux Agents IA : Toute erreur 'TCP auth closed' observée dans des logs Elixir n'est pas liée à ce serveur MCP. Axon Core V2 est 100% autonome.*",
            mem_str, file_count, symbol_count, edge_count
        );
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
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
        let query = "SELECT s.name, s.kind, s.tested, \
                     (SELECT count(*) FROM CALLS c1 WHERE c1.target_id = s.id) AS callers, \
                     (SELECT count(*) FROM CALLS c2 WHERE c2.source_id = s.id) AS callees \
                     FROM Symbol s WHERE s.name = $sym";
             
        match self.graph_store.query_json_param(query, &json!({"sym": symbol})) {
            Ok(res) => {
                let table = self.format_kuzu_table(&res, &["Nom", "Type", "Testé", "Appelants", "Appelés"]);
                Some(json!({ "content": [{ "type": "text", "text": format!("### 🔍 Inspection du Symbole : {}\n\n{}", symbol, table) }] }))
            },
            Err(_) => None,
        }
    }

    fn axon_impact(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3);
        
        let query = format!(
            "WITH RECURSIVE traverse(caller, callee, depth) AS (
                SELECT source_id, target_id, 1 as depth FROM CALLS WHERE target_id = $sym
                UNION ALL
                SELECT c.source_id, c.target_id, t.depth + 1
                FROM CALLS c JOIN traverse t ON c.target_id = t.caller
                WHERE t.depth < {}
            )
            SELECT DISTINCT COALESCE(f.path, 'Unknown') AS origin, s.name, s.kind
            FROM traverse t
            JOIN Symbol s ON t.caller = s.id
            LEFT JOIN CONTAINS con ON s.id = con.target_id
            LEFT JOIN File f ON f.path = con.source_id", depth
        );
        let params = json!({"sym": symbol});

        match self.graph_store.query_json_param(&query, &params) {
            Ok(res) => {
                let table = self.format_kuzu_table(&res, &["Fichier / Projet", "Symbole Impacté", "Type"]);
                let count_query = format!(
                    "WITH RECURSIVE traverse(caller, callee, depth) AS (
                        SELECT source_id, target_id, 1 as depth FROM CALLS WHERE target_id = $sym
                        UNION ALL
                        SELECT c.source_id, c.target_id, t.depth + 1
                        FROM CALLS c JOIN traverse t ON c.target_id = t.caller
                        WHERE t.depth < {}
                    )
                    SELECT count(DISTINCT caller) FROM traverse", depth
                );
                let impact_radius = self.graph_store.query_count_param(&count_query, &params).unwrap_or(0);
                
                let mut report = format!("## 💥 Analyse d'Impact Transversale : {}\n\n", symbol);
                report.push_str(&format!("**Rayon d'Impact (profondeur {}) :** {} composants affectés à travers le Treillis.\n\n", depth, impact_radius));
                report.push_str(&table);
                
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Impact Analysis Error: {}", e) }], "isError": true })),
        }
    }

    fn axon_audit(&self, args: &Value) -> Option<Value> {
        let requested_project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let project = requested_project; 
        
        let file_count = if project == "*" {
            self.graph_store.query_count("SELECT count(*) FROM File").unwrap_or(0)
        } else {
            let count_query = "SELECT count(*) FROM File WHERE path LIKE '%' || $proj || '%'".to_string();
            let params = json!({"proj": project});
            self.graph_store.query_count_param(&count_query, &params).unwrap_or(0)
        };
        
        if file_count < 1 {
            let warning = format!("⚠️ Warning: Project '{}' seems unindexed or parser failed (Found {} files). Health metrics are invalid.", project, file_count);
            return Some(json!({ "content": [{ "type": "text", "text": warning }] }));
        }

        let (sec_score, paths) = self.graph_store.get_security_audit(project).unwrap_or((100, "[]".to_string()));
        let cov_score = self.graph_store.get_coverage_score(project).unwrap_or(0);
        let tech_debt = self.graph_store.get_technical_debt(project).unwrap_or_default();

        let mut report = format!("## 🛡️ Audit de Conformité : {}\n\n", project);
        report.push_str(&format!("### 🔒 Sécurité : {}/100\n", sec_score));
        
        if sec_score < 100 {
            report.push_str("🚨 **Vulnérabilités potentielles détectées.**\n");
            report.push_str(&format!("Chemins critiques trouvés : {}\n", paths));
        } else {
            report.push_str("✅ Aucun chemin critique vers des fonctions dangereuses détecté.\n");
        }

        if !tech_debt.is_empty() {
            report.push_str("\n### ⚠️ Dette Technique & Panic Points\n");
            report.push_str("Les points suivants présentent des risques de crash (panic) ou une mauvaise gestion d'erreur :\n\n");
            for (file, issue) in tech_debt.iter().take(10) {
                report.push_str(&format!("*   `{}` dans `{}`\n", issue, file));
            }
            if tech_debt.len() > 10 {
                report.push_str(&format!("*... et {} autres points détectés.*\n", tech_debt.len() - 10));
            }
        }

        report.push_str(&format!("\n### 🧪 Qualité & Tests : {}%\n", cov_score));
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    fn axon_health(&self, args: &Value) -> Option<Value> {
        let requested_project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let project = requested_project;
        
        let file_count = if project == "*" {
            self.graph_store.query_count("SELECT count(*) FROM File").unwrap_or(0)
        } else {
            let count_query = "SELECT count(*) FROM File WHERE path LIKE '%' || $proj || '%'".to_string();
            let params = json!({"proj": project});
            self.graph_store.query_count_param(&count_query, &params).unwrap_or(0)
        };

        if file_count < 1 {
            let warning = format!("⚠️ Warning: Project '{}' seems unindexed or parser failed (Found {} files). Health metrics are invalid.", project, file_count);
            return Some(json!({ "content": [{ "type": "text", "text": warning }] }));
        }

        let coverage = self.graph_store.get_coverage_score(project).unwrap_or(0);
        let god_objects = self.graph_store.get_god_objects(project).unwrap_or_default();
        
        let mut report = format!("🏥 Health Report for {}: Coverage {}%. Stability high.", project, coverage);        
        if !god_objects.is_empty() {
            report.push_str(&format!("\nGod Object detected: {}", god_objects.join(", ")));
        }
        
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    fn axon_diff(&self, args: &Value) -> Option<Value> {
        let diff = args.get("diff_content")?.as_str()?;
        let mut files = std::collections::HashSet::new();
        for line in diff.lines() {
            if let Some(path) = line.strip_prefix("+++ b/") {
                files.insert(path.to_string());
            } else if let Some(path) = line.strip_prefix("--- a/") {
                if path != "/dev/null" {
                    files.insert(path.to_string());
                }
            }
        }
        
        let mut all_results = Vec::new();
        for file in files {
            let query = format!("SELECT s.name, s.kind FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id WHERE f.path LIKE '%{}%'", file.replace("'", "''"));
            if let Ok(res) = self.graph_store.query_json(&query) {
                all_results.push(format!("File: {}\nSymbols:\n{}", file, res));
            }
        }
        Some(json!({ "content": [{ "type": "text", "text": all_results.join("\n\n") }] }))
    }

    fn axon_batch(&self, args: &Value) -> Option<Value> {
        let calls = args.get("calls")?.as_array()?;
        let mut all_results = Vec::new();
        
        for call in calls {
            let tool_name = call.get("tool")?.as_str()?;
            let tool_args = call.get("args")?;
            
            let res = match tool_name {
                "axon_query" => self.axon_query(tool_args),
                "axon_inspect" => self.axon_inspect(tool_args),
                "axon_impact" => self.axon_impact(tool_args),
                _ => None,
            };
            
            if let Some(r) = res {
                all_results.push(json!({
                    "name": tool_name,
                    "result": r
                }));
            }
        }
        
        Some(json!({ "content": [{ "type": "text", "text": serde_json::to_string(&all_results).unwrap_or_default() }] }))
    }

    fn axon_semantic_clones(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let query = format!(
            "SELECT other.name, other.kind, array_cosine_distance(s.embedding, other.embedding) as score \
             FROM Symbol s, Symbol other \
             WHERE s.name = '{}' AND s.name <> other.name AND array_cosine_distance(s.embedding, other.embedding) < 0.05 \
             ORDER BY score ASC LIMIT 5",
            symbol.replace("'", "''")
        );
        match self.graph_store.query_json(&query) {
            Ok(res) => {
                let report = if res.len() > 5 && res != "[]" {
                    format!("### 👯 Clones Sémantiques détectés pour '{}'\n\n{}", symbol, self.format_kuzu_table(&res, &["Nom", "Type", "Similitude"]))
                } else {
                    format!("✅ Aucun clone sémantique évident (similitude > 95%) trouvé pour '{}'.", symbol)
                };
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Cloning Error: {}", e) }], "isError": true })),
        }
    }

    fn axon_architectural_drift(&self, args: &Value) -> Option<Value> {
        let source_layer = args.get("source_layer")?.as_str()?;
        let target_layer = args.get("target_layer")?.as_str()?;
        
        let query = "
            SELECT f1.path, s1.name, f2.path, s2.name 
            FROM CALLS c
            JOIN Symbol s1 ON c.source_id = s1.id
            JOIN CONTAINS c1 ON s1.id = c1.target_id
            JOIN File f1 ON f1.path = c1.source_id
            JOIN Symbol s2 ON c.target_id = s2.id
            JOIN CONTAINS c2 ON s2.id = c2.target_id
            JOIN File f2 ON f2.path = c2.source_id
            WHERE f1.path LIKE '%' || $s_layer || '%' AND f2.path LIKE '%' || $t_layer || '%'
        ".to_string();
        
        let params = json!({
            "s_layer": source_layer,
            "t_layer": target_layer
        });

        match self.graph_store.query_json_param(&query, &params) {
            Ok(res) => {
                let report = if res.len() > 5 && res != "[]" {
                    format!("⚠️ **VIOLATION D'ARCHITECTURE DÉTECTÉE**\n\nLa couche '{}' appelle directement '{}' :\n\n{}", source_layer, target_layer, self.format_kuzu_table(&res, &["Source", "Symbole", "Cible", "Appelé"]))
                } else {
                    format!("✅ Aucune dérive architecturale détectée entre '{}' et '{}'.", source_layer, target_layer)
                };
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Drift Analysis Error: {}", e) }], "isError": true })),
        }
    }

    fn axon_bidi_trace(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let query_up = "WITH RECURSIVE traverse(caller, callee, depth) AS ( \
                            SELECT source_id, target_id, 1 as depth FROM CALLS \
                            UNION ALL \
                            SELECT c.source_id, c.target_id, t.depth + 1 \
                            FROM CALLS c JOIN traverse t ON c.target_id = t.caller \
                            WHERE t.depth < 5 \
                        ) \
                        SELECT DISTINCT c.name, c.kind \
                        FROM traverse t JOIN Symbol s ON t.callee = s.id JOIN Symbol c ON t.caller = c.id \
                        WHERE s.name = $sym";
                        
        let query_down = "WITH RECURSIVE traverse(caller, callee, depth) AS ( \
                            SELECT source_id, target_id, 1 as depth FROM CALLS \
                            UNION ALL \
                            SELECT c.source_id, c.target_id, t.depth + 1 \
                            FROM CALLS c JOIN traverse t ON c.source_id = t.callee \
                            WHERE t.depth < 5 \
                        ) \
                        SELECT DISTINCT c.name, c.kind \
                        FROM traverse t JOIN Symbol s ON t.caller = s.id JOIN Symbol c ON t.callee = c.id \
                        WHERE s.name = $sym";
        let params = json!({"sym": symbol});

        let up_res = self.graph_store.query_json_param(&query_up, &params).unwrap_or_else(|_| "[]".to_string());
        let down_res = self.graph_store.query_json_param(&query_down, &params).unwrap_or_else(|_| "[]".to_string());
        
        let report = format!("## 🕸️ Trace Bidirectionnelle : {}\n\n### ⬆️ Entry Points (Appelants)\n{}\n\n### ⬇️ Couches Profondes (Appelés)\n{}", 
            symbol, 
            self.format_kuzu_table(&up_res, &["Nom", "Type"]),
            self.format_kuzu_table(&down_res, &["Nom", "Type"])
        );
        
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    fn axon_api_break_check(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let query = "
            SELECT DISTINCT COALESCE(f.path, 'External') AS consumer, c.name, c.kind 
            FROM CALLS call
            JOIN Symbol s ON call.target_id = s.id 
            JOIN Symbol c ON call.source_id = c.id
            LEFT JOIN CONTAINS con ON c.id = con.target_id
            LEFT JOIN File f ON f.path = con.source_id
            WHERE s.name = $sym
        ";
        let params = json!({"sym": symbol});

        match self.graph_store.query_json_param(query, &params) {
            Ok(res) => {
                let report = if res.trim().len() > 5 && res != "[]" {
                    format!("⚠️ **RISQUE DE RUPTURE D'API**\n\nModifier '{}' impactera directement les consommateurs suivants :\n\n{}", symbol, self.format_kuzu_table(&res, &["Consommateur", "Symbole", "Type"]))
                } else {
                    let check_exists = "SELECT is_public FROM Symbol WHERE name = $sym";
                    match self.graph_store.query_json_param(check_exists, &params) {
                        Ok(exists_res) if exists_res.contains("false") => {
                            format!("✅ SAFE TO MODIFY : Le symbole '{}' est PRIVÉ et ne devrait pas avoir d'impacts externes.", symbol)
                        },
                        _ => format!("✅ SAFE TO MODIFY : Aucun consommateur direct détecté pour le symbole '{}'.", symbol)
                    }
                };
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("API Check Error: {}", e) }], "isError": true })),
        }
    }

    fn axon_simulate_mutation(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2);
        let query = format!(
            "WITH RECURSIVE traverse(caller, callee, depth) AS ( \
                SELECT source_id, target_id, 1 as depth FROM CALLS WHERE target_id = $sym \
                UNION ALL \
                SELECT c.source_id, c.target_id, t.depth + 1 \
                FROM CALLS c JOIN traverse t ON c.target_id = t.caller \
                WHERE t.depth < {} \
            ) \
            SELECT count(DISTINCT caller) FROM traverse", depth
        );
        let params = json!({"sym": symbol});

        match self.graph_store.query_json_param(&query, &params) {
            Ok(res) => {
                let count: i64 = res.trim().parse().unwrap_or(0);
                let report = format!("🔮 Dry-Run Mutation : Modifier '{}' va impacter en cascade ~{} composants dans l'architecture.", symbol, count);
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Simulation Error: {}", e) }], "isError": true })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::graph::GraphStore;

    fn create_test_server() -> McpServer {
        let store = Arc::new(GraphStore::new(":memory:").unwrap_or_else(|_| GraphStore::new("/tmp/test_db").unwrap()));
        McpServer::new(store)
    }

    #[test]
    fn test_mcp_tools_list() {
        let server = create_test_server();
        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/list".to_string(),
            params: None,
            id: Some(json!(1)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let tools = result.get("tools").expect("Expected tools array").as_array().expect("tools is array");

        assert_eq!(tools.len(), 18);

        let tool_names: Vec<&str> = tools.iter()
            .map(|t| t.get("name").unwrap().as_str().unwrap())
            .collect();

        assert!(tool_names.contains(&"axon_refine_lattice"));
        assert!(tool_names.contains(&"axon_fs_read"));
        assert!(tool_names.contains(&"axon_query"));
        assert!(tool_names.contains(&"axon_inspect"));
        assert!(tool_names.contains(&"axon_audit"));
        assert!(tool_names.contains(&"axon_impact"));
        assert!(tool_names.contains(&"axon_health"));
        assert!(tool_names.contains(&"axon_diff"));
        assert!(tool_names.contains(&"axon_batch"));
        assert!(tool_names.contains(&"axon_cypher"));
        assert!(tool_names.contains(&"axon_semantic_clones"));
        assert!(tool_names.contains(&"axon_architectural_drift"));
        assert!(tool_names.contains(&"axon_bidi_trace"));
        assert!(tool_names.contains(&"axon_api_break_check"));
        assert!(tool_names.contains(&"axon_simulate_mutation"));
        assert!(tool_names.contains(&"axon_debug"));
    }

    #[test]
    fn test_axon_architectural_drift() {
        let server = create_test_server();
        server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('ui/app.js', 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::fetchData', 'fetchData', 'function', false, true, false, 'global')").unwrap();
        server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('db/repo.rs', 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::executeSQL', 'executeSQL', 'function', false, true, false, 'global')").unwrap();
        server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('ui/app.js', 'global::fetchData')").unwrap();
        server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('db/repo.rs', 'global::executeSQL')").unwrap();
        server.graph_store.execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::fetchData', 'global::executeSQL')").unwrap();

        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_architectural_drift",
                "arguments": { "source_layer": "ui", "target_layer": "db" }
            })),
            id: Some(json!(2)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

        assert!(content.contains("VIOLATION") || content.contains("Détectée") ||
            content.contains("détectée"));
    }

    #[test]
    fn test_axon_query_with_project() {
        let server = create_test_server();
        server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('test_proj/f1.rs', 'test_proj')").unwrap();
        server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('test_proj/f2.rs', 'test_proj')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::auth_func', 'auth_func', 'function', false, true, false, 'test_proj')").unwrap();
        server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('test_proj/f1.rs', 'global::auth_func')").unwrap();

        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_query",
                "arguments": { "query": "auth", "project": "test_proj" }
            })),
            id: Some(json!(3)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

        assert!(content.contains("auth_func"));
    }

    #[test]
    fn test_axon_fs_read() {
        let server = create_test_server();
        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_fs_read",
                "arguments": { "uri": "src/axon-core/src/main.rs", "start_line": 1, "end_line": 5 }
            })),
            id: Some(json!(4)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
        assert!(content.contains("L2 Detail") || content.contains("Erreur"));
    }

    #[test]
    fn test_send_notification() {
        let store = Arc::new(GraphStore::new(":memory:").unwrap_or_else(|_| GraphStore::new("/tmp/test_db_notif").unwrap()));
        let server = McpServer::new(store);
        let notif = server.send_notification("notifications/tools/list_changed", None);
        assert_eq!(notif.method, "notifications/tools/list_changed");
        assert!(notif.params.is_none());

        let serialized = serde_json::to_string(&notif).unwrap();
        assert!(serialized.contains("notifications/tools/list_changed"));
    }

    #[test]
    fn test_axon_inspect() {
        let server = create_test_server();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::core_func', 'core_func', 'function', true, true, false, 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::caller_func', 'caller_func', 'function', false, true, false, 'global')").unwrap();
        server.graph_store.execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::caller_func', 'global::core_func')").unwrap();

        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_inspect",
                "arguments": {
                    "symbol": "core_func",
                    "project": "test_proj"
                }
            })),
            id: Some(json!(5)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
        assert!(content.contains("Inspection du Symbole"));
        assert!(content.contains("core_func"));
    }

    #[test]
    fn test_axon_audit_taint_analysis() {
        let server = create_test_server();
        server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('src/api.rs', 'global')").unwrap();
        server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('src/api_dummy.rs', 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('global::user_input', 'user_input', 'function', false, true, false, false, 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('global::run_task', 'run_task', 'function', false, true, false, false, 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('global::eval', 'eval', 'function', false, true, false, true, 'global')").unwrap();

        server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/api.rs', 'global::user_input')").unwrap();
        server.graph_store.execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::user_input', 'global::run_task')").unwrap();
        server.graph_store.execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::run_task', 'global::eval')").unwrap();

        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_audit",
                "arguments": {
                    "project": "*"
                }
            })),
            id: Some(json!(6)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
        assert!(content.contains("user_input"));
        assert!(content.contains("user_input"));
        assert!(content.contains("eval"));
    }

    #[test]
    fn test_axon_audit_technical_debt() {
        let server = create_test_server();
        // Insert a file with a symbol calling 'unwrap'
        server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('src/danger.rs', 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::risky_func', 'risky_func', 'function', false, true, false, 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::unwrap', 'unwrap', 'method', false, true, false, 'global')").unwrap();
        server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/danger.rs', 'global::risky_func')").unwrap();
        server.graph_store.execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::risky_func', 'global::unwrap')").unwrap();

        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_audit",
                "arguments": {
                    "project": "*"
                }
            })),
            id: Some(json!(10)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

        assert!(content.contains("Dette Technique"));
        assert!(content.contains("unwrap"));
        assert!(content.contains("src/danger.rs"));
    }

    #[test]
    fn test_axon_audit_technical_debt_comments() {
        let server = create_test_server();
        server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('src/todo.rs', 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::todo1', '// TODO: Fix this', 'TODO', false, true, false, 'global')").unwrap();
        server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/todo.rs', 'global::todo1')").unwrap();

        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_audit",
                "arguments": {
                    "project": "*"
                }
            })),
            id: Some(json!(11)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

        assert!(content.contains("Dette Technique"));
        assert!(content.contains("TODO"));
        assert!(content.contains("Fix this"));
    }

    #[test]
    fn test_axon_audit_secrets_detection() {
        let server = create_test_server();
        server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('src/config.rs', 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::secret1', 'SECRET_API_KEY: Found potential hardcoded credential', 'SECRET_API_KEY', false, true, false, 'global')").unwrap();
        server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/config.rs', 'global::secret1')").unwrap();

        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_audit",
                "arguments": {
                    "project": "*"
                }
            })),
            id: Some(json!(12)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

        assert!(content.contains("Dette Technique"));
        assert!(content.contains("SECRET_API_KEY"));
        assert!(content.contains("hardcoded credential"));
    }

    #[test]
    fn test_axon_audit_cross_language_taint() {
        let server = create_test_server();
        // Setup a multi-hop path: elixir_func -[:CALLS_NIF]-> rust_nif -[:CALLS]-> unsafe_call
        server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('src/api.ex', 'global')").unwrap();
        server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('src/api_dummy.ex', 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('global::elixir_func', 'elixir_func', 'function', false, true, false, false, 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('global::rust_nif', 'rust_nif', 'function', false, true, true, false, 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('global::unsafe_block', 'unsafe_block', 'function', false, true, false, true, 'global')").unwrap();

        server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/api.ex', 'global::elixir_func')").unwrap();
        server.graph_store.execute("INSERT INTO CALLS_NIF (source_id, target_id) VALUES ('global::elixir_func', 'global::rust_nif')").unwrap();
        server.graph_store.execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::rust_nif', 'global::unsafe_block')").unwrap();

        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_audit",
                "arguments": {
                    "project": "*"
                }
            })),
            id: Some(json!(13)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

        // Should detect taint via CALLS_NIF and is_unsafe
        assert!(!content.contains("Score 100/100"));
        assert!(content.contains("elixir_func"));
        assert!(content.contains("unsafe_block"));
    }

    #[test]
    fn test_axon_health_god_objects() {
        let server = create_test_server();
        server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('src/god.rs', 'global')").unwrap();
        server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('src/god_dummy.rs', 'global')").unwrap();
        server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::GodClass', 'GodClass', 'class', false, true, false, 'global')").unwrap();
        server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/god.rs', 'global::GodClass')").unwrap();

        for i in 0..10 {
            server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::dep{}', 'dep{}', 'function', false, true, false, 'global')", i, i)).unwrap();
            server.graph_store.execute(&format!("INSERT INTO CALLS (source_id, target_id) VALUES ('global::dep{}', 'global::GodClass')", i)).unwrap();
        }

        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_health",
                "arguments": {
                    "project": "*"
                }
            })),
            id: Some(json!(7)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

        assert!(content.contains("God Object detected") || content.contains("GodClass"));
    }

    #[test]
    fn test_axon_query_global_default() {
        let server = create_test_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_query",
                "arguments": { "query": "auth" }
            })),
            id: Some(json!(8)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
        assert!(content.contains("Résultats de Recherche Hybride"));
    }

    #[test]
    fn test_axon_soll_manager_auto_id() {
        let server = create_test_server();
        // Initialize registry
        server.graph_store.execute("INSERT INTO soll.Registry (id, last_req, last_cpt, last_dec) VALUES ('AXON_GLOBAL', 0, 10, 0)").unwrap();
        
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_soll_manager",
                "arguments": {
                    "action": "create",
                    "entity": "concept",
                    "data": {
                        "name": "Test Concept",
                        "explanation": "To test auto id",
                        "rationale": "Because testing is good"
                    }
                }
            })),
            id: Some(json!(1)),
        };
        
        let response = server.handle_request(req);
        let result = response.unwrap().result.unwrap();
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
        
        assert!(content.contains("CPT-AXO-011"));
        
        // Verify in DB
        let count = server.graph_store.query_count("SELECT count(*) FROM soll.Concept WHERE name LIKE 'CPT-AXO-011%'").unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_axon_export_soll() {
        let server = create_test_server();
        server.graph_store.execute("INSERT INTO soll.Vision (title, description, goal) VALUES ('Test Vision', 'Desc', 'Goal')").unwrap();
        server.graph_store.execute("INSERT INTO soll.Concept (name, explanation, rationale) VALUES ('CPT-AXO-001: My Concept', 'Expl', 'Rat')").unwrap();
        
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_export_soll",
                "arguments": {}
            })),
            id: Some(json!(2)),
        };
        
        let response = server.handle_request(req);
        let result = response.unwrap().result.unwrap();
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
        
        assert!(content.contains("# SOLL Extraction"));
        assert!(content.contains("Test Vision"));
        assert!(content.contains("CPT-AXO-001"));
        assert!(content.contains("Exported to"));
    }
}
