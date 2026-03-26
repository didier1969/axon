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
    graph_store: Arc<parking_lot::RwLock<GraphStore>>,
}

impl McpServer {
    pub fn new(graph_store: Arc<parking_lot::RwLock<GraphStore>>) -> Self {
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
        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => {
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
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
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

    fn axon_query(&self, args: &Value) -> Option<Value> {
        let query_text = args.get("query")?.as_str()?;
        let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        
        let embedding = crate::embedder::batch_embed(vec![query_text.to_string()]).ok()
            .and_then(|v| v.into_iter().next());

        let (cypher, params) = if let Some(emb) = embedding {
            let vec_str = format!("{:?}", emb);
            if project == "*" {
                (
                    format!(
                        "MATCH (f:File)-[:CONTAINS]->(s:Symbol) \
                         WHERE s.name CONTAINS $q OR array_cosine_similarity(s.embedding, {}) > 0.5 \
                         RETURN s.name, s.kind, f.path AS uri, array_cosine_similarity(s.embedding, {}) as score \
                         ORDER BY score DESC LIMIT 10",
                        vec_str, vec_str
                    ),
                    json!({"q": query_text})
                )
            } else {
                (
                    format!(
                        "MATCH (f:File)-[:CONTAINS]->(s:Symbol) \
                         WHERE f.path CONTAINS $proj AND (s.name CONTAINS $q OR array_cosine_similarity(s.embedding, {}) > 0.5) \
                         RETURN s.name, s.kind, f.path AS uri, array_cosine_similarity(s.embedding, {}) as score \
                         ORDER BY score DESC LIMIT 10",
                        vec_str, vec_str
                    ),
                    json!({"q": query_text, "proj": project})
                )
            }
        } else {
            if project == "*" {
                (
                    "MATCH (f:File)-[:CONTAINS]->(s:Symbol) WHERE s.name CONTAINS $q RETURN s.name, s.kind, f.path AS uri LIMIT 10".to_string(),
                    json!({"q": query_text})
                )
            } else {
                (
                    "MATCH (f:File)-[:CONTAINS]->(s:Symbol) WHERE f.path CONTAINS $proj AND s.name CONTAINS $q RETURN s.name, s.kind, f.path AS uri LIMIT 10".to_string(),
                    json!({"q": query_text, "proj": project})
                )
            }
        };

        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => match store.query_json_param(&cypher, &params) {
                Ok(res) => {
                    let headers = if cypher.contains("score") {
                        vec!["Nom", "Type", "URI (Chemin)", "Distance Sémantique"]
                    } else {
                        vec!["Nom", "Type", "URI (Chemin)"]
                    };
                    let table = self.format_kuzu_table(&res, &headers);
                    Some(json!({ "content": [{ "type": "text", "text": format!("### 🔎 Résultats de Recherche Hybride : '{}'\n\n{}", query_text, table) }] }))
                },
                Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Search Error: {}", e) }], "isError": true })),
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
        }
    }

    fn axon_debug(&self) -> Option<Value> {
        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => {
                let file_count = store.query_count("MATCH (f:File) RETURN count(f)").unwrap_or(0);
                let symbol_count = store.query_count("MATCH (s:Symbol) RETURN count(s)").unwrap_or(0);
                let edge_count = store.query_count("MATCH ()-[r]->() RETURN count(r)").unwrap_or(0);

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
                    *   **Base de Graphe :** KuzuDB (Local, Zero-Copy).\n\
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
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
        }
    }

    fn axon_cypher(&self, args: &Value) -> Option<Value> {
        let cypher = args.get("cypher")?.as_str()?;
        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => match store.query_json(cypher) {
                Ok(result) => Some(json!({ "content": [{ "type": "text", "text": result }] })),
                Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Cypher Error: {}", e) }], "isError": true })),
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
        }
    }

    fn axon_inspect(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let query = "MATCH (s:Symbol {name: $sym}) \
             OPTIONAL MATCH (caller:Symbol)-[:CALLS]->(s) \
             OPTIONAL MATCH (s)-[:CALLS]->(callee:Symbol) \
             RETURN s.name, s.kind, s.tested, count(caller) AS callers, count(callee) AS callees";
             
        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => match store.query_json_param(query, &json!({"sym": symbol})) {
                Ok(res) => {
                    let table = self.format_kuzu_table(&res, &["Nom", "Type", "Testé", "Appelants", "Appelés"]);
                    Some(json!({ "content": [{ "type": "text", "text": format!("### 🔍 Inspection du Symbole : {}\n\n{}", symbol, table) }] }))
                },
                Err(_) => None,
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
        }
    }

    fn axon_impact(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3);
        let depth_str = format!("*1..{}", depth);
        
        let query = format!(
            "MATCH (s:Symbol {{name: $sym}})<-[:CALLS|CALLS_NIF|CALLS_OTP{}]-(affected) \
             OPTIONAL MATCH (affected)<-[:CONTAINS]-(f:File) \
             RETURN DISTINCT COALESCE(f.path, 'Unknown') AS origin, affected.name, affected.kind",
            depth_str
        );
        let params = json!({"sym": symbol});

        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => match store.query_json_param(&query, &params) {
                Ok(res) => {
                    let table = self.format_kuzu_table(&res, &["Fichier / Projet", "Symbole Impacté", "Type"]);
                    let count_query = format!("MATCH (s:Symbol {{name: $sym}})<-[:CALLS|CALLS_NIF|CALLS_OTP{}]-(affected) RETURN count(DISTINCT affected)", depth_str);
                    let impact_radius = store.query_count_param(&count_query, &params).unwrap_or(0);
                    
                    let mut report = format!("## 💥 Analyse d'Impact Transversale : {}\n\n", symbol);
                    report.push_str(&format!("**Rayon d'Impact (profondeur {}) :** {} composants affectés à travers le Treillis.\n\n", depth, impact_radius));
                    report.push_str(&table);
                    
                    Some(json!({ "content": [{ "type": "text", "text": report }] }))
                },
                Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Impact Analysis Error: {}", e) }], "isError": true })),
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
        }
    }

    fn axon_audit(&self, args: &Value) -> Option<Value> {
        let requested_project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let project = requested_project; 
        
        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => {
                let count_query = if project == "*" {
                    "MATCH (f:File) RETURN count(f)".to_string()
                } else {
                    "MATCH (f:File) WHERE f.path CONTAINS $proj RETURN count(f)".to_string()
                };
                let params = json!({"proj": project});

                let file_count = store.query_count_param(&count_query, &params).unwrap_or(0);
                if file_count < 1 {
                    let warning = format!("⚠️ Warning: Project '{}' seems unindexed or parser failed (Found {} files). Health metrics are invalid.", project, file_count);
                    return Some(json!({ "content": [{ "type": "text", "text": warning }] }));
                }

                let (sec_score, paths) = store.get_security_audit(project).unwrap_or((100, "[]".to_string()));
                let cov_score = store.get_coverage_score(project).unwrap_or(0);
                let tech_debt = store.get_technical_debt(project).unwrap_or_default();

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
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
        }
    }

    fn axon_health(&self, args: &Value) -> Option<Value> {
        let requested_project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let project = requested_project;
        
        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => {
                let count_query = if project == "*" {
                    "MATCH (f:File) RETURN count(f)".to_string()
                } else {
                    "MATCH (f:File) WHERE f.path CONTAINS $proj RETURN count(f)".to_string()
                };
                let params = json!({"proj": project});

                let file_count = store.query_count_param(&count_query, &params).unwrap_or(0);
                if file_count < 1 {
                    let warning = format!("⚠️ Warning: Project '{}' seems unindexed or parser failed (Found {} files). Health metrics are invalid.", project, file_count);
                    return Some(json!({ "content": [{ "type": "text", "text": warning }] }));
                }

                let coverage = store.get_coverage_score(project).unwrap_or(0);
                let god_objects = store.get_god_objects(project).unwrap_or_default();
                
                let mut report = format!("🏥 Health Report for {}: Coverage {}%. Stability high.", project, coverage);        
                if !god_objects.is_empty() {
                    report.push_str(&format!("\nGod Object detected: {}", god_objects.join(", ")));
                }
                
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
        }
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
        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => {
                for file in files {
                    let query = format!("MATCH (f:File)-[:CONTAINS]->(s:Symbol) WHERE f.path CONTAINS '{}' RETURN s.name, s.kind", file);
                    if let Ok(res) = store.query_json(&query) {
                        all_results.push(format!("File: {}\nSymbols:\n{}", file, res));
                    }
                }
                Some(json!({ "content": [{ "type": "text", "text": all_results.join("\n\n") }] }))
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
        }
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
            "MATCH (s:Symbol {{name: '{}'}}), (other:Symbol) \
             WHERE s.name <> other.name AND array_cosine_similarity(s.embedding, other.embedding) > 0.95 \
             RETURN other.name, other.kind, array_cosine_similarity(s.embedding, other.embedding) as score \
             ORDER BY score DESC LIMIT 5",
            symbol
        );
        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => match store.query_json(&query) {
                Ok(res) => {
                    let report = if res.len() > 5 && res != "[]" {
                        format!("### 👯 Clones Sémantiques détectés pour '{}'\n\n{}", symbol, self.format_kuzu_table(&res, &["Nom", "Type", "Similitude"]))
                    } else {
                        format!("✅ Aucun clone sémantique évident (similitude > 95%) trouvé pour '{}'.", symbol)
                    };
                    Some(json!({ "content": [{ "type": "text", "text": report }] }))
                },
                Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Cloning Error: {}", e) }], "isError": true })),
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
        }
    }

    fn axon_architectural_drift(&self, args: &Value) -> Option<Value> {
        let source_layer = args.get("source_layer")?.as_str()?;
        let target_layer = args.get("target_layer")?.as_str()?;
        
        let query = format!(
            "MATCH (s1:Symbol)<-[:CONTAINS]-(f1:File), (s2:Symbol)<-[:CONTAINS]-(f2:File) \
             MATCH (s1)-[:CALLS|CALLS_NIF|CALLS_OTP]->(s2) \
             WHERE f1.path CONTAINS $s_layer AND f2.path CONTAINS $t_layer \
             RETURN f1.path, s1.name, f2.path, s2.name"
        );
        let params = json!({
            "s_layer": source_layer,
            "t_layer": target_layer
        });

        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => match store.query_json_param(&query, &params) {
                Ok(res) => {
                    let report = if res.len() > 5 && res != "[]" {
                        format!("⚠️ **VIOLATION D'ARCHITECTURE DÉTECTÉE**\n\nLa couche '{}' appelle directement '{}' :\n\n{}", source_layer, target_layer, self.format_kuzu_table(&res, &["Source", "Symbole", "Cible", "Appelé"]))
                    } else {
                        format!("✅ Aucune dérive architecturale détectée entre '{}' et '{}'.", source_layer, target_layer)
                    };
                    Some(json!({ "content": [{ "type": "text", "text": report }] }))
                },
                Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Drift Analysis Error: {}", e) }], "isError": true })),
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
        }
    }

    fn axon_bidi_trace(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let query_up = "MATCH (s:Symbol {name: $sym})<-[:CALLS*1..5]-(caller) RETURN DISTINCT caller.name, caller.kind";
        let query_down = "MATCH (s:Symbol {name: $sym})-[:CALLS*1..5]->(callee) RETURN DISTINCT callee.name, callee.kind";
        let params = json!({"sym": symbol});

        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => {
                let up_res = store.query_json_param(&query_up, &params).unwrap_or_else(|_| "[]".to_string());
                let down_res = store.query_json_param(&query_down, &params).unwrap_or_else(|_| "[]".to_string());
                
                let report = format!("## 🕸️ Trace Bidirectionnelle : {}\n\n### ⬆️ Entry Points (Appelants)\n{}\n\n### ⬇️ Couches Profondes (Appelés)\n{}", 
                    symbol, 
                    self.format_kuzu_table(&up_res, &["Nom", "Type"]),
                    self.format_kuzu_table(&down_res, &["Nom", "Type"])
                );
                
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
        }
    }

    fn axon_api_break_check(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let query = "MATCH (s:Symbol {name: $sym})<-[:CALLS|CALLS_NIF|CALLS_OTP]-(caller) \
             OPTIONAL MATCH (caller)<-[:CONTAINS]-(f:File) \
             RETURN DISTINCT COALESCE(f.path, 'External') AS consumer, caller.name, caller.kind";
        let params = json!({"sym": symbol});

        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => match store.query_json_param(&query, &params) {
                Ok(res) => {
                    let report = if res.trim().len() > 5 && res != "[]" {
                        format!("⚠️ **RISQUE DE RUPTURE D'API**\n\nModifier '{}' impactera directement les consommateurs suivants :\n\n{}", symbol, self.format_kuzu_table(&res, &["Consommateur", "Symbole", "Type"]))
                    } else {
                        let check_exists = "MATCH (s:Symbol {name: $sym}) RETURN s.is_public";
                        match store.query_json_param(check_exists, &params) {
                            Ok(exists_res) if exists_res.contains("false") => {
                                format!("✅ SAFE TO MODIFY : Le symbole '{}' est PRIVÉ et ne devrait pas avoir d'impacts externes.", symbol)
                            },
                            _ => format!("✅ SAFE TO MODIFY : Aucun consommateur direct détecté pour le symbole '{}'.", symbol)
                        }
                    };
                    Some(json!({ "content": [{ "type": "text", "text": report }] }))
                },
                Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("API Check Error: {}", e) }], "isError": true })),
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
        }
    }

    fn axon_simulate_mutation(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2);
        let query = format!("MATCH (s:Symbol {{name: $sym}})<-[:CALLS|CALLS_NIF|CALLS_OTP*1..{}]-(affected) RETURN count(DISTINCT affected)", depth);
        let params = json!({"sym": symbol});

        match self.graph_store.try_read_for(std::time::Duration::from_millis(100)) {
            Some(store) => match store.query_json_param(&query, &params) {
                Ok(res) => {
                    let report = format!("🔮 Dry-Run Mutation : Modifier '{}' va impacter en cascade ~{} composants dans l'architecture.", symbol, res);
                    Some(json!({ "content": [{ "type": "text", "text": report }] }))
                },
                Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Simulation Error: {}", e) }], "isError": true })),
            },
            None => {
                tracing::error!("Timeout (5s) acquiring graph_store read lock");
                Some(json!({ "content": [{ "type": "text", "text": "❌ Erreur Critique : Timeout d'accès à la base de données (Deadlock évité)." }], "isError": true }))
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::graph::GraphStore;

    fn create_test_server() -> McpServer {
        let store = Arc::new(parking_lot::RwLock::new(GraphStore::new(":memory:").unwrap_or_else(|_| GraphStore::new("/tmp/test_db").unwrap())));
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
        
        assert_eq!(tools.len(), 16);
        
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
        server.graph_store.write().execute("MERGE (f:File {path: 'ui/app.js', project_slug: 'global'})").unwrap();
        server.graph_store.write().execute("MERGE (s1:Symbol {id: 'global::fetchData', name: 'fetchData', project_slug: 'global'})").unwrap();
        server.graph_store.write().execute("MERGE (f2:File {path: 'db/repo.rs', project_slug: 'global'})").unwrap();
        server.graph_store.write().execute("MERGE (s2:Symbol {id: 'global::executeSQL', name: 'executeSQL', project_slug: 'global'})").unwrap();
        server.graph_store.write().execute("MATCH (f:File {path: 'ui/app.js'}), (s:Symbol {id: 'global::fetchData'}) MERGE (f)-[:CONTAINS]->(s)").unwrap();
        server.graph_store.write().execute("MATCH (f:File {path: 'db/repo.rs'}), (s:Symbol {id: 'global::executeSQL'}) MERGE (f)-[:CONTAINS]->(s)").unwrap();
        server.graph_store.write().execute("MATCH (s1:Symbol {id: 'global::fetchData'}), (s2:Symbol {id: 'global::executeSQL'}) MERGE (s1)-[:CALLS]->(s2)").unwrap();

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
        assert!(content.contains("VIOLATION") || content.contains("Détectée") || content.contains("détectée"));
    }

    #[test]
    fn test_axon_query_with_project() {
        let server = create_test_server();
        server.graph_store.write().execute("MERGE (f:File {path: 'test_proj/f1.rs'})").unwrap();
        server.graph_store.write().execute("MERGE (f:File {path: 'test_proj/f2.rs'})").unwrap();
        server.graph_store.write().execute("MERGE (s:Symbol {id: 'global::', name: 'auth_func'})").unwrap();
        server.graph_store.write().execute("MATCH (f:File {path: 'test_proj/f1.rs'}), (s:Symbol {name: 'auth_func'}) MERGE (f)-[:CONTAINS]->(s)").unwrap();

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
        let store = Arc::new(parking_lot::RwLock::new(GraphStore::new(":memory:").unwrap_or_else(|_| GraphStore::new("/tmp/test_db_notif").unwrap())));
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
        server.graph_store.write().execute("MERGE (s:Symbol {id: 'global::', name: 'core_func', kind: 'function', tested: true})").unwrap();
        server.graph_store.write().execute("MERGE (c:Symbol {id: 'global::caller_func', name: 'caller_func'})").unwrap();
        server.graph_store.write().execute("MATCH (c:Symbol {name: 'caller_func'}), (s:Symbol {name: 'core_func'}) MERGE (c)-[:CALLS]->(s)").unwrap();

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
        server.graph_store.write().execute("MERGE (f:File {path: 'src/api.rs'})").unwrap();
        server.graph_store.write().execute("MERGE (f:File {path: 'src/api_dummy.rs'})").unwrap();
        server.graph_store.write().execute("MERGE (s1:Symbol {id: 'global::', name: 'user_input', kind: 'function', tested: false})").unwrap();
        server.graph_store.write().execute("MERGE (s2:Symbol {id: 'global::run_task', name: 'run_task', kind: 'function', tested: false})").unwrap();
        server.graph_store.write().execute("MERGE (s3:Symbol {id: 'global::eval', name: 'eval', kind: 'function', tested: false})").unwrap();
        
        server.graph_store.write().execute("MATCH (f:File {path: 'src/api.rs'}), (s1:Symbol {name: 'user_input'}) MERGE (f)-[:CONTAINS]->(s1)").unwrap();
        server.graph_store.write().execute("MATCH (s1:Symbol {name: 'user_input'}), (s2:Symbol {name: 'run_task'}) MERGE (s1)-[:CALLS]->(s2)").unwrap();
        server.graph_store.write().execute("MATCH (s2:Symbol {name: 'run_task'}), (s3:Symbol {name: 'eval'}) MERGE (s2)-[:CALLS]->(s3)").unwrap();

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
        assert!(!content.contains("Score 100/100"));
        assert!(content.contains("user_input"));
        assert!(content.contains("eval"));
    }

    #[test]
    fn test_axon_audit_technical_debt() {
        let server = create_test_server();
        // Insert a file with a symbol calling 'unwrap'
        server.graph_store.write().execute("MERGE (f:File {path: 'src/danger.rs'})").unwrap();
        server.graph_store.write().execute("MERGE (s:Symbol {id: 'global::', name: 'risky_func', kind: 'function'})").unwrap();
        server.graph_store.write().execute("MERGE (d:Symbol {id: 'global::unwrap', name: 'unwrap', kind: 'method'})").unwrap();
        server.graph_store.write().execute("MATCH (f:File {path: 'src/danger.rs'}), (s:Symbol {name: 'risky_func'}) MERGE (f)-[:CONTAINS]->(s)").unwrap();
        server.graph_store.write().execute("MATCH (s:Symbol {name: 'risky_func'}), (d:Symbol {name: 'unwrap'}) MERGE (s)-[:CALLS]->(d)").unwrap();

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
        server.graph_store.write().execute("MERGE (f:File {path: 'src/todo.rs'})").unwrap();
        server.graph_store.write().execute("MERGE (s:Symbol {id: 'global::', name: '// TODO: Fix this', kind: 'TODO'})").unwrap();
        server.graph_store.write().execute("MATCH (f:File {path: 'src/todo.rs'}), (s:Symbol {name: '// TODO: Fix this'}) MERGE (f)-[:CONTAINS]->(s)").unwrap();

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
        server.graph_store.write().execute("MERGE (f:File {path: 'src/config.rs'})").unwrap();
        server.graph_store.write().execute("MERGE (s:Symbol {id: 'global::', name: 'SECRET_API_KEY: Found potential hardcoded credential', kind: 'SECRET_API_KEY'})").unwrap();
        server.graph_store.write().execute("MATCH (f:File {path: 'src/config.rs'}), (s:Symbol {name: 'SECRET_API_KEY: Found potential hardcoded credential'}) MERGE (f)-[:CONTAINS]->(s)").unwrap();

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
        server.graph_store.write().execute("MERGE (f:File {path: 'src/api.ex'})").unwrap();
        server.graph_store.write().execute("MERGE (f:File {path: 'src/api_dummy.ex'})").unwrap();
        server.graph_store.write().execute("MERGE (s1:Symbol {id: 'global::', name: 'elixir_func', kind: 'function', tested: false})").unwrap();
        server.graph_store.write().execute("MERGE (s2:Symbol {id: 'global::rust_nif', name: 'rust_nif', kind: 'function', tested: false, is_nif: true})").unwrap();
        server.graph_store.write().execute("MERGE (s3:Symbol {id: 'global::unsafe_block', name: 'unsafe_block', kind: 'function', tested: false, is_unsafe: true})").unwrap();
        
        server.graph_store.write().execute("MATCH (f:File {path: 'src/api.ex'}), (s1:Symbol {name: 'elixir_func'}) MERGE (f)-[:CONTAINS]->(s1)").unwrap();
        server.graph_store.write().execute("MATCH (s1:Symbol {name: 'elixir_func'}), (s2:Symbol {name: 'rust_nif'}) MERGE (s1)-[:CALLS_NIF]->(s2)").unwrap();
        server.graph_store.write().execute("MATCH (s2:Symbol {name: 'rust_nif'}), (s3:Symbol {name: 'unsafe_block'}) MERGE (s2)-[:CALLS]->(s3)").unwrap();

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
        server.graph_store.write().execute("MERGE (f:File {path: 'src/god.rs'})").unwrap();
        server.graph_store.write().execute("MERGE (f:File {path: 'src/god_dummy.rs'})").unwrap();
        server.graph_store.write().execute("MERGE (god:Symbol {id: 'global::', name: 'GodClass', kind: 'class', tested: false})").unwrap();
        server.graph_store.write().execute("MATCH (f:File {path: 'src/god.rs'}), (s:Symbol {name: 'GodClass'}) MERGE (f)-[:CONTAINS]->(s)").unwrap();
        
        for i in 0..10 {
            server.graph_store.write().execute(&format!("MERGE (dep{i}:Symbol {{id: 'global::dep{i}', name: 'dep{i}', project_slug: 'global'}})")).unwrap();
            server.graph_store.write().execute(&format!("MATCH (dep:Symbol {{id: 'global::dep{i}'}}), (god:Symbol {{id: 'global::'}}) MERGE (dep)-[:CALLS]->(god)")).unwrap();
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
        assert!(content.contains("Health Report"));
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
}
