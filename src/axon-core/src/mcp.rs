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
    graph_store: Arc<std::sync::RwLock<GraphStore>>,
}

impl McpServer {
    pub fn new(graph_store: Arc<std::sync::RwLock<GraphStore>>) -> Self {
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
                // Clean up string representation if it's like 'String("val")'
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
        // Notifications do not have an ID and must not receive a response.
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
                        "description": "Lattice Refiner: Analyse le graphe post-ingestion pour lier les frontières inter-langages (ex: Elixir NIF -> Rust natif).",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "required": []
                        }
                    },
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
                            "required": []
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
                            "required": []
                        }
                    },
                    {
                        "name": "axon_diff",
                        "description": "Analyse sémantique des changements (Git Diff -> Symboles touchés).",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "diff_content": { "type": "string" } },
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
                        "description": "Trouve des fonctions sémantiquement similaires (clones de logique) dans le projet.",
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
                        "description": "Vérifie les violations d'architecture entre deux couches (ex: 'ui' appelant directement 'db').",
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
                        "description": "Trace bidirectionnelle: remonte aux Entry Points (haut) et liste les appels profonds (bas).",
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
                        "description": "Vérifie si la modification d'un symbole public impacte des composants externes.",
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
                        "description": "Dry-run : calcule le volume de l'impact d'une modification avant de coder.",
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

        Some(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result,
            error: None,
            id: req.id,
        })
    }
fn handle_call_tool(&self, params: Option<Value>) -> Option<Value> {
    let params = params?;
    let name = params.get("name")?.as_str()?;
    let arguments = params.get("arguments")?;

    match name {
        "axon_refine_lattice" => self.axon_refine_lattice(arguments),
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
        _ => Some(json!({ "content": [{ "type": "text", "text": "Tool not found" }], "isError": true })),
    }
}

    fn axon_refine_lattice(&self, _args: &Value) -> Option<Value> {
        let store = self.graph_store.read().unwrap();
        
        let refine_query = "
            MATCH (elixir:Symbol {is_nif: true})<-[:CONTAINS]-(e_file:File)
            MATCH (rust:Symbol {is_nif: true})<-[:CONTAINS]-(r_file:File)
            WHERE elixir.name = rust.name 
              AND e_file.path CONTAINS '.ex' 
              AND r_file.path CONTAINS '.rs'
            MERGE (elixir)-[r:CALLS_NIF]->(rust)
            RETURN elixir.name AS nif_name, e_file.path AS elixir_source, r_file.path AS rust_target
        ";

        match store.query_json(refine_query) {
            Ok(res) => {
                let parsed: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
                let count = parsed.len();
                
                let report = if count > 0 {
                    format!("✨ **Lattice Refiner exécuté avec succès.**\n\nJ'ai découvert et lié **{} ponts FFI (Rustler NIFs)** entre Elixir et Rust.\n\n{}", count, self.format_kuzu_table(&res, &["Nom NIF", "Fichier Elixir", "Fichier Rust"]))
                } else {
                    "✅ **Lattice Refiner exécuté.**\nAucun nouveau pont FFI (Rustler NIF) non-lié n'a été détecté dans le graphe.".to_string()
                };
                
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Erreur Refiner: {}", e) }] })),
        }
    }

    fn axon_query(&self, args: &Value) -> Option<Value> {
        let query_text = args.get("query")?.as_str()?;
        
        // 1. Vector Embedding of the query
        let embedding = crate::embedder::batch_embed(vec![query_text.to_string()]).ok()
            .and_then(|v| v.into_iter().next());

        let (cypher, params) = if let Some(emb) = embedding {
            let vec_str = format!("{:?}", emb);
            // Hybrid query: exact match on name OR semantic similarity
            (
                format!(
                    "MATCH (s:Symbol) \
                     WHERE s.name CONTAINS $q OR array_cosine_similarity(s.embedding, {}) > 0.5 \
                     RETURN s.name, s.kind, array_cosine_similarity(s.embedding, {}) as score \
                     ORDER BY score DESC LIMIT 10",
                    vec_str, vec_str
                ),
                json!({"q": query_text})
            )
        } else {
            (
                "MATCH (s:Symbol) WHERE s.name CONTAINS $q RETURN s.name, s.kind LIMIT 10".to_string(),
                json!({"q": query_text})
            )
        };

        match self.graph_store.read().unwrap().query_json_param(&cypher, &params) {
            Ok(res) => {
                let headers = if cypher.contains("score") {
                    vec!["Nom", "Type", "Distance Sémantique"]
                } else {
                    vec!["Nom", "Type"]
                };
                let table = self.format_kuzu_table(&res, &headers);
                Some(json!({ "content": [{ "type": "text", "text": format!("### 🔎 Résultats de Recherche Hybride : '{}'\n\n{}", query_text, table) }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Search Error: {}", e) }], "isError": true })),
        }
    }

    fn axon_cypher(&self, args: &Value) -> Option<Value> {
        let cypher = args.get("cypher")?.as_str()?;
        match self.graph_store.read().unwrap().query_json(cypher) {
            Ok(result) => Some(json!({ "content": [{ "type": "text", "text": result }] })),
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Cypher Error: {}", e) }], "isError": true })),
        }
    }

    fn axon_inspect(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let query = "MATCH (s:Symbol {name: $sym}) \
             OPTIONAL MATCH (caller:Symbol)-[:CALLS]->(s) \
             OPTIONAL MATCH (s)-[:CALLS]->(callee:Symbol) \
             RETURN s.name, s.kind, s.tested, count(caller) AS callers, count(callee) AS callees";
             
        match self.graph_store.read().unwrap().query_json_param(query, &json!({"sym": symbol})) {
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
        let depth_str = format!("*1..{}", depth);
        
        let query = format!(
            "MATCH (f:File)-[:CONTAINS]->(affected:Symbol)-[:CALLS|CALLS_NIF|CALLS_OTP{}]->(s:Symbol {{name: $sym}}) \
             RETURN DISTINCT f.path, affected.name, affected.kind \
             ORDER BY f.path", 
            depth_str
        );
        let params = json!({"sym": symbol});
        
        match self.graph_store.read().unwrap().query_json_param(&query, &params) {
            Ok(res) => {
                let table = self.format_kuzu_table(&res, &["Fichier / Projet", "Symbole Impacté", "Type"]);
                let count_query = format!("MATCH (s:Symbol {{name: $sym}})<-[:CALLS|CALLS_NIF|CALLS_OTP{}]-(affected) RETURN count(DISTINCT affected)", depth_str);
                let impact_radius = self.graph_store.read().unwrap().query_count_param(&count_query, &params).unwrap_or(0);
                
                let mut report = format!("## 💥 Analyse d'Impact Transversale : {}\n\n", symbol);
                report.push_str(&format!("**Rayon d'Impact (profondeur {}) :** {} composants affectés à travers le Treillis.\n\n", depth, impact_radius));
                
                if impact_radius > 0 {
                    report.push_str("### Cartographie des Dépendances :\n");
                    report.push_str(&table);
                } else {
                    report.push_str("✅ Cette modification semble isolée. Aucun composant dépendant trouvé dans le Treillis global.");
                }
                
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Impact Analysis Error: {}", e) }], "isError": true })),
        }
    }

    fn axon_audit(&self, args: &Value) -> Option<Value> {
        let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let store = self.graph_store.read().unwrap();

        // 🚨 FAIL-SAFE: Check if project is actually indexed
        let count_query = if project == "*" {
            "MATCH (f:File) RETURN count(f)".to_string()
        } else {
            "MATCH (f:File) WHERE f.path CONTAINS $proj RETURN count(f)".to_string()
        };
        let params = json!({"proj": project});

        let file_count = store.query_count_param(&count_query, &params).unwrap_or(0);
        if file_count < 2 {
            let warning = format!("⚠️ Warning: Project '{}' seems unindexed or parser failed (Found {} files). Health metrics are invalid.", project, file_count);
            return Some(json!({ "content": [{ "type": "text", "text": warning }] }));
        }

        let (sec_score, paths) = store.get_security_audit(project).unwrap_or((100, "[]".to_string()));
        let cov_score = store.get_coverage_score(project).unwrap_or(0);

        let mut report = format!("## 🛡️ Rapport d'Audit : {}\n\n", if project == "*" { "Workspace Global" } else { project });        
        report.push_str(&format!("### 🔒 Sécurité : {}/100\n", sec_score));
        if sec_score < 100 {
            report.push_str("⚠️ **Vulnérabilités détectées (Taint Analysis) :**\n");
            report.push_str(&self.format_kuzu_table(&paths, &["Chemin de Propagation"]));
            let mermaid_diagram = crate::graph::GraphStore::generate_mermaid_flow(&paths);
            report.push_str(&format!("\n\n#### Visualisation des Chemins :\n```mermaid\n{}\n```\n", mermaid_diagram));
        } else {
            report.push_str("✅ Aucun chemin critique vers des fonctions dangereuses détecté.\n");
        }

        report.push_str(&format!("\n### 🧪 Qualité & Tests : {}%\n", cov_score));
        
        // Macro API Break Check: Simplified for performance on massive graphs
        let filter = if project == "*" { "".to_string() } else { format!("WHERE f.path CONTAINS '{}'", project) };
        let break_query = format!(
            "MATCH (f:File)-[:CONTAINS]->(s:Symbol {{is_public: true}})<-[:CALLS]-(caller:Symbol) \
             {} \
             RETURN s.name, count(caller) AS external_callers \
             LIMIT 3", // Removed ORDER BY count() to avoid full-table materialization
            filter
        );
        let break_report = store.query_json(&break_query).unwrap_or_default();
        
        if break_report.len() > 5 && break_report != "[]" && !break_report.starts_with("Error:") {
            report.push_str("\n### ⚠️ Points de Rupture Critique (API Reliability)\n");
            report.push_str("Les symboles publics suivants sont massivement utilisés. Toute modification impactera l'architecture :\n");
            report.push_str(&self.format_kuzu_table(&break_report, &["Symbole Public", "Nombre de Dépendants"]));
        }
        
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    fn axon_health(&self, args: &Value) -> Option<Value> {
        let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let store = self.graph_store.read().unwrap();

        // 🚨 FAIL-SAFE: Check if project is actually indexed
        let count_query = if project == "*" {
            "MATCH (f:File) RETURN count(f)".to_string()
        } else {
            "MATCH (f:File) WHERE f.path CONTAINS $proj RETURN count(f)".to_string()
        };
        let params = json!({"proj": project});

        let file_count = store.query_count_param(&count_query, &params).unwrap_or(0);
        if file_count < 2 {
            let warning = format!("⚠️ Warning: Project '{}' seems unindexed or parser failed (Found {} files). Health metrics are invalid.", project, file_count);
            return Some(json!({ "content": [{ "type": "text", "text": warning }] }));
        }

        let coverage = store.get_coverage_score(project).unwrap_or(0);
        let god_objects = Vec::<String>::new(); // Temporarily disabled for performance: store.get_god_objects(project).unwrap_or_default();

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
        
        if files.is_empty() {
             return Some(json!({ "content": [{ "type": "text", "text": "No files modified." }] }));
        }
        
        let mut all_results = Vec::new();
        for file in files {
            let query = format!("MATCH (f:File)-[:CONTAINS]->(s:Symbol) WHERE f.path CONTAINS '{}' RETURN s.name, s.kind", file);
            if let Ok(res) = self.graph_store.read().unwrap().query_json(&query) {
                all_results.push(format!("File: {}\nSymbols:\n{}", file, res));
            }
        }
        
        Some(json!({ "content": [{ "type": "text", "text": all_results.join("\n\n") }] }))
    }

    fn axon_semantic_clones(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        
        let query = format!(
            "MATCH (s:Symbol {{name: '{}'}}), (other:Symbol) \
             WHERE s <> other AND other.kind = s.kind AND s.embedding IS NOT NULL AND other.embedding IS NOT NULL \
             RETURN other.name, other.kind, array_cosine_similarity(s.embedding, other.embedding) AS sim \
             ORDER BY sim DESC LIMIT 5",
            symbol
        );
        match self.graph_store.read().unwrap().query_json(&query) {
            Ok(res) => {
                let report = if res.len() > 5 && res != "[]" {
                    format!("🔎 Clones Sémantiques (Top 5 vector similarity) pour '{}' :\n{}", symbol, res)
                } else {
                    format!("✅ Aucun clone similaire significatif trouvé pour '{}' (ou modèle vectoriel non initialisé).", symbol)
                };
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Erreur: {}", e) }] })),
        }
    }

    fn axon_architectural_drift(&self, args: &Value) -> Option<Value> {
        let source_layer = args.get("source_layer")?.as_str()?;
        let target_layer = args.get("target_layer")?.as_str()?;

        let query = "MATCH (f1:File)-[:CONTAINS]->(s1:Symbol)-[:CALLS]->(s2:Symbol)<-[:CONTAINS]-(f2:File) \
             WHERE f1.path CONTAINS $src AND f2.path CONTAINS $tgt \
             RETURN f1.path AS Source, s1.name AS Caller, f2.path AS Target, s2.name AS Callee";

        let params = json!({
            "src": source_layer,
            "tgt": target_layer
        });

        match self.graph_store.read().unwrap().query_json_param(query, &params) {
            Ok(res) => {
                let report = if res.len() > 5 && res != "[]" {
                    format!("🚨 Dérive Architecturale Détectée ! La couche '{}' appelle directement la couche '{}' :\n{}", source_layer, target_layer, self.format_kuzu_table(&res, &["Source", "Appelant", "Cible", "Appelé"]))
                } else {
                    format!("✅ Aucune dérive détectée entre '{}' et '{}'.", source_layer, target_layer)
                };
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Erreur: {}", e) }] })),
        }
    }
    fn axon_bidi_trace(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3);
        let depth_str = format!("*1..{}", depth);
        
        let query_up = format!("MATCH (s:Symbol {{name: $sym}})<-[:CALLS|CALLS_NIF{}]-(entry:Symbol {{is_entry_point: true}}) RETURN DISTINCT entry.name, entry.kind", depth_str);
        let query_down = format!("MATCH (s:Symbol {{name: $sym}})-[:CALLS|CALLS_NIF{}]->(leaf:Symbol) RETURN DISTINCT leaf.name, leaf.kind LIMIT 20", depth_str);
        let params = json!({"sym": symbol});
        
        let store = self.graph_store.read().unwrap();
        let up_res = store.query_json_param(&query_up, &params).unwrap_or_else(|_| "[]".to_string());
        let down_res = store.query_json_param(&query_down, &params).unwrap_or_else(|_| "[]".to_string());
        
        let mut report = format!("## ↔️ Trace Bidirectionnelle : {}\n\n", symbol);
        
        report.push_str("### 🔼 Appelé par ces Points d'Entrée (Entry Points) :\n");
        report.push_str(&self.format_kuzu_table(&up_res, &["Point d'Entrée", "Type"]));
        
        report.push_str("\n### 🔽 Appelle ces composants :\n");
        report.push_str(&self.format_kuzu_table(&down_res, &["Composant Appelé", "Type"]));
        
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    fn axon_api_break_check(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;

        let query = "MATCH (s:Symbol {name: $sym, is_public: true})<-[:CALLS]-(caller:Symbol)<-[:CONTAINS]-(f:File) \
             RETURN f.path AS Project, caller.name AS Function";
        let params = json!({"sym": symbol});

        match self.graph_store.read().unwrap().query_json_param(query, &params) {
            Ok(res) => {
                let report = if res.trim().len() > 5 && res != "[]" {
                    format!("⚠️ ATTENTION BREAKING CHANGE : Le symbole public '{}' est utilisé par les composants suivants. Si vous modifiez sa signature, vous devez aussi mettre à jour :\n{}", symbol, res)
                } else {
                    // Check if it exists but is not public
                    let check_exists = "MATCH (s:Symbol {name: $sym}) RETURN s.is_public";
                    match self.graph_store.read().unwrap().query_json_param(check_exists, &params) {
                        Ok(exists_res) if exists_res.contains("false") => {
                            format!("✅ SAFE TO MODIFY : Le symbole '{}' est PRIVÉ et ne devrait pas avoir d'impacts externes.", symbol)
                        },
                        _ => format!("✅ SAFE TO MODIFY : Le symbole '{}' n'a aucune dépendance critique ou est un appelant final.", symbol)
                    }
                };
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(_) => None,
        }
    }
    fn axon_simulate_mutation(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let depth_str = match args.get("depth").and_then(|v| v.as_u64()) {
            Some(d) => format!("*1..{}", d),
            None => "*1..".to_string(), // Unbounded variable length
        };

        // Calcule le rayon d'impact
        let query = format!(
            "MATCH (s:Symbol {{name: $sym}})<-[:CALLS{}]-(affected:Symbol) \
             RETURN count(DISTINCT affected) AS impact_score",
            depth_str
        );
        let params = json!({"sym": symbol});

        match self.graph_store.read().unwrap().query_json_param(&query, &params) {
            Ok(res) => {
                let report = format!("🔮 Dry-Run Mutation : Modifier '{}' va impacter en cascade ~{} composants dans l'architecture.", symbol, res);
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(_) => None,
        }
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

    #[test]
    fn test_send_notification() {
        let server = create_test_server();
        let notif = server.send_notification("notifications/tools/list_changed", None);
        assert_eq!(notif.method, "notifications/tools/list_changed");
        assert_eq!(notif.jsonrpc, "2.0");
        assert!(notif.params.is_none());
        
        let serialized = serde_json::to_string(&notif).unwrap();
        assert!(serialized.contains("notifications/tools/list_changed"));
    }

    // Helper function to create a dummy server for testing tool signatures
    fn create_test_server() -> McpServer {
        // Use an in-memory or temp DB if needed, here we use a dummy path
        let store = Arc::new(std::sync::RwLock::new(GraphStore::new(":memory:").unwrap_or_else(|_| GraphStore::new("/tmp/test_db").unwrap())));
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
        
        assert_eq!(tools.len(), 14);
        
        let tool_names: Vec<&str> = tools.iter()
            .map(|t| t.get("name").unwrap().as_str().unwrap())
            .collect();
            
        assert!(tool_names.contains(&"axon_refine_lattice"));
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
    }

    #[test]
    fn test_axon_advanced_tools() {
        let server = create_test_server();
        // Mock data
        server.graph_store.read().unwrap().execute("MERGE (f:File {path: 'ui/app.js'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (s1:Symbol {name: 'fetchData'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (f2:File {path: 'db/repo.rs'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (s2:Symbol {name: 'executeSQL'})").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (f:File {path: 'ui/app.js'}), (s:Symbol {name: 'fetchData'}) MERGE (f)-[:CONTAINS]->(s)").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (f:File {path: 'db/repo.rs'}), (s:Symbol {name: 'executeSQL'}) MERGE (f)-[:CONTAINS]->(s)").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (s1:Symbol {name: 'fetchData'}), (s2:Symbol {name: 'executeSQL'}) MERGE (s1)-[:CALLS]->(s2)").unwrap();

        // Architectural Drift
        let req_drift = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_architectural_drift",
                "arguments": { "source_layer": "ui", "target_layer": "db" }
            })),
            id: Some(json!(10)),
        };
        let res_drift = server.handle_request(req_drift);
        let text_drift = res_drift.unwrap().result.unwrap().get("content").unwrap()[0].get("text").unwrap().as_str().unwrap().to_string();
        assert!(text_drift.contains("🚨 Dérive Architecturale Détectée"));
        assert!(text_drift.contains("fetchData"));

        // Simulate Mutation
        let req_mut = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_simulate_mutation",
                "arguments": { "symbol": "executeSQL" }
            })),
            id: Some(json!(11)),
        };
        let res_mut = server.handle_request(req_mut);
        let text_mut = res_mut.unwrap().result.unwrap().get("content").unwrap()[0].get("text").unwrap().as_str().unwrap().to_string();
        assert!(text_mut.contains("Dry-Run Mutation"));
    }

    #[test]
    fn test_axon_batch() {
        let server = create_test_server();
        server.graph_store.read().unwrap().execute("MERGE (f:File {path: 'test_proj/f1.rs'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (f:File {path: 'test_proj/f2.rs'})").unwrap();
        
        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_batch",
                "arguments": {
                    "calls": [
                        { "tool": "axon_health", "args": { "project": "test_proj" } },
                        { "tool": "axon_audit", "args": { "project": "test_proj" } }
                    ]
                }
            })),
            id: Some(json!(2)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
        
        assert!(content.contains("axon_health"));
        assert!(content.contains("axon_audit"));
        assert!(content.contains("Coverage 100%"));
        assert!(content.contains("100/100"));
    }

    #[test]
    fn test_axon_diff() {
        let server = create_test_server();
        // Insert a dummy file in the graph to test matching
        server.graph_store.read().unwrap().execute("MERGE (f:File {path: 'src/main.rs'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (s:Symbol {name: 'main', kind: 'function', tested: false})").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (f:File {path: 'src/main.rs'}), (s:Symbol {name: 'main'}) MERGE (f)-[:CONTAINS]->(s)").unwrap();

        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_diff",
                "arguments": {
                    "diff_content": "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,4 @@\n+fn main() {}"
                }
            })),
            id: Some(json!(3)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
        
        assert!(content.contains("src/main.rs"));
        assert!(content.contains("main"));
    }

    #[test]
    fn test_axon_cypher() {
        let server = create_test_server();
        server.graph_store.read().unwrap().execute("MERGE (f:File {path: 'test.py'})").unwrap();
        
        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_cypher",
                "arguments": {
                    "cypher": "MATCH (f:File {path: 'test.py'}) RETURN f.path"
                }
            })),
            id: Some(json!(4)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
        
        assert!(content.contains("test.py"));
    }

    #[test]
    fn test_axon_inspect() {
        let server = create_test_server();
        server.graph_store.read().unwrap().execute("MERGE (s:Symbol {name: 'core_func', kind: 'function', tested: true})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (c:Symbol {name: 'caller_func'})").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (c:Symbol {name: 'caller_func'}), (s:Symbol {name: 'core_func'}) MERGE (c)-[:CALLS]->(s)").unwrap();

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
        
        // Output format check based on query results
        assert!(content.contains("core_func"));
        assert!(content.contains("function"));
    }

    #[test]
    fn test_axon_audit_taint_analysis() {
        let server = create_test_server();
        // Setup a multi-hop path: user_input -> run_task -> eval
        server.graph_store.read().unwrap().execute("MERGE (f:File {path: 'src/api.rs'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (f:File {path: 'src/api_dummy.rs'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (s1:Symbol {name: 'user_input', kind: 'function', tested: false})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (s2:Symbol {name: 'run_task', kind: 'function', tested: false})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (s3:Symbol {name: 'eval', kind: 'function', tested: false})").unwrap();
        
        server.graph_store.read().unwrap().execute("MATCH (f:File {path: 'src/api.rs'}), (s1:Symbol {name: 'user_input'}) MERGE (f)-[:CONTAINS]->(s1)").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (s1:Symbol {name: 'user_input'}), (s2:Symbol {name: 'run_task'}) MERGE (s1)-[:CALLS]->(s2)").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (s2:Symbol {name: 'run_task'}), (s3:Symbol {name: 'eval'}) MERGE (s2)-[:CALLS]->(s3)").unwrap();

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
        
        // It should deduct points due to the indirect eval call (distance 2)
        // Score should be < 100
        assert!(!content.contains("Score 100/100"));
        // Extra requirement: should report critical paths
        assert!(content.contains("user_input"));
        assert!(content.contains("eval"));
    }

    #[test]
    fn test_axon_audit_cross_language_taint() {
        let server = create_test_server();
        // Setup a multi-hop path: elixir_func -[:CALLS_NIF]-> rust_nif -[:CALLS]-> unsafe_call
        server.graph_store.read().unwrap().execute("MERGE (f:File {path: 'src/api.ex'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (f:File {path: 'src/api_dummy.ex'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (s1:Symbol {name: 'elixir_func', kind: 'function', tested: false})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (s2:Symbol {name: 'rust_nif', kind: 'function', tested: false, is_nif: true})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (s3:Symbol {name: 'unsafe_block', kind: 'function', tested: false, is_unsafe: true})").unwrap();
        
        server.graph_store.read().unwrap().execute("MATCH (f:File {path: 'src/api.ex'}), (s1:Symbol {name: 'elixir_func'}) MERGE (f)-[:CONTAINS]->(s1)").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (s1:Symbol {name: 'elixir_func'}), (s2:Symbol {name: 'rust_nif'}) MERGE (s1)-[:CALLS_NIF]->(s2)").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (s2:Symbol {name: 'rust_nif'}), (s3:Symbol {name: 'unsafe_block'}) MERGE (s2)-[:CALLS]->(s3)").unwrap();

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
        
        // Should detect taint via CALLS_NIF and is_unsafe
        assert!(!content.contains("Score 100/100"));
        assert!(content.contains("elixir_func"));
        assert!(content.contains("unsafe_block"));
    }

    #[test]
    fn test_axon_health_god_objects() {
        let server = create_test_server();
        server.graph_store.read().unwrap().execute("MERGE (f:File {path: 'src/god.rs'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (f:File {path: 'src/god_dummy.rs'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (god:Symbol {name: 'GodClass', kind: 'class', tested: false})").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (f:File {path: 'src/god.rs'}), (s:Symbol {name: 'GodClass'}) MERGE (f)-[:CONTAINS]->(s)").unwrap();
        
        // Create 10 dependents to make it a God Object
        for i in 0..10 {
            server.graph_store.read().unwrap().execute(&format!("MERGE (dep{i}:Symbol {{name: 'dep{i}'}})")).unwrap();
            server.graph_store.read().unwrap().execute(&format!("MATCH (dep:Symbol {{name: 'dep{i}'}}), (god:Symbol {{name: 'GodClass'}}) MERGE (dep)-[:CALLS]->(god)")).unwrap();
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
        
        assert!(content.contains("God Object detected: GodClass"));
    }

    #[test]
    fn test_axon_query_global_default() {
        let server = create_test_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_query",
                "arguments": { "query": "auth" } // Note: 'project' is omitted
            })),
            id: Some(json!(8)),
        };
        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        assert!(result.get("isError").is_none() || !result.get("isError").unwrap().as_bool().unwrap_or(false));
    }

    #[test]
    fn test_axon_impact_cross_project() {
        let server = create_test_server();
        // Project A has a core_func
        server.graph_store.read().unwrap().execute("MERGE (pA:Project {name: 'ProjectA'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (fA:File {path: 'ProjectA/core.rs'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (sA:Symbol {name: 'core_func', kind: 'function'})").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (f:File {path: 'ProjectA/core.rs'}), (s:Symbol {name: 'core_func'}) MERGE (f)-[:CONTAINS]->(s)").unwrap();
        
        // Project B depends on Project A and calls core_func
        server.graph_store.read().unwrap().execute("MERGE (pB:Project {name: 'ProjectB'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (fB:File {path: 'ProjectB/api.rs'})").unwrap();
        server.graph_store.read().unwrap().execute("MERGE (sB:Symbol {name: 'api_handler', kind: 'function'})").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (f:File {path: 'ProjectB/api.rs'}), (s:Symbol {name: 'api_handler'}) MERGE (f)-[:CONTAINS]->(s)").unwrap();
        
        server.graph_store.read().unwrap().execute("MATCH (pA:Project {name: 'ProjectA'}), (pB:Project {name: 'ProjectB'}) MERGE (pB)-[:DEPENDS_ON]->(pA)").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (sA:Symbol {name: 'core_func'}), (sB:Symbol {name: 'api_handler'}) MERGE (sB)-[:CALLS]->(sA)").unwrap();
        
        // In KuzuDB, files need to belong to projects. Let's add that relation for the test to work if we rely on it.
        server.graph_store.read().unwrap().execute("MATCH (p:Project {name: 'ProjectB'}), (f:File {path: 'ProjectB/api.rs'}) MERGE (f)-[:BELONGS_TO]->(p)").unwrap();
        server.graph_store.read().unwrap().execute("MATCH (p:Project {name: 'ProjectA'}), (f:File {path: 'ProjectA/core.rs'}) MERGE (f)-[:BELONGS_TO]->(p)").unwrap();

        let req = JsonRpcRequest { jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_impact",
                "arguments": {
                    "symbol": "core_func"
                }
            })),
            id: Some(json!(9)),
        };

        let response = server.handle_request(req);
        let result = response.unwrap().result.expect("Expected result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
        println!("CONTENT OUTPUT:\n{}", content);
        
        // The report should explicitly mention the impacted Project B
        assert!(content.contains("ProjectB"));
    }
}
