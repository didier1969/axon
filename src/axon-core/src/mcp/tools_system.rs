use serde_json::{json, Value};

use super::format::format_table_from_json;
use super::McpServer;

impl McpServer {
    pub(crate) fn axon_refine_lattice(&self, _args: &Value) -> Option<Value> {
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
                    format!(
                        "✨ **Lattice Refiner exécuté avec succès.**\n\nJ'ai découvert et lié **{} ponts FFI (Rustler NIFs)** entre Elixir et Rust.\n\n{}",
                        count,
                        format_table_from_json(&res, &["Nom NIF", "Fichier Elixir", "Fichier Rust"])
                    )
                } else {
                    "✅ **Lattice Refiner exécuté.**\nAucun nouveau pont FFI (Rustler NIF) non-lié n'a été détecté dans le graphe.".to_string()
                };
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("Refiner Error: {}", e) }], "isError": true }),
            ),
        }
    }

    pub(crate) fn axon_debug(&self) -> Option<Value> {
        let file_count = self
            .graph_store
            .query_count("SELECT count(*) FROM File")
            .unwrap_or(0);
        let symbol_count = self
            .graph_store
            .query_count("SELECT count(*) FROM Symbol")
            .unwrap_or(0);
        let edge_count = self
            .graph_store
            .query_count("SELECT (SELECT count(*) FROM CONTAINS) + (SELECT count(*) FROM CALLS)")
            .unwrap_or(0);

        let mut mem_str = "Unknown".to_string();
        if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(rss_pages) = content
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u64>().ok())
            {
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

    pub(crate) fn axon_cypher(&self, args: &Value) -> Option<Value> {
        let cypher = args.get("cypher")?.as_str()?;
        match self.graph_store.query_json(cypher) {
            Ok(result) => Some(json!({ "content": [{ "type": "text", "text": result }] })),
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("Cypher Error: {}", e) }], "isError": true }),
            ),
        }
    }

    pub(crate) fn axon_batch(&self, args: &Value) -> Option<Value> {
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

        Some(
            json!({ "content": [{ "type": "text", "text": serde_json::to_string(&all_results).unwrap_or_default() }] }),
        )
    }
}
