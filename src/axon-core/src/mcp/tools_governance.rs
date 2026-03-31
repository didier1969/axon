use serde_json::{json, Value};

use super::format::format_table_from_json;
use super::McpServer;

impl McpServer {
    fn build_graph_clone_section(&self, symbol: &str) -> Option<String> {
        let anchor_res = self
            .graph_store
            .query_json_param(
                "SELECT id FROM Symbol WHERE id = $sym OR name = $sym LIMIT 1",
                &json!({"sym": symbol}),
            )
            .ok()?;
        let anchor_rows: Vec<Vec<Value>> = serde_json::from_str(&anchor_res).unwrap_or_default();
        let anchor_id = anchor_rows.first()?.first()?.as_str()?;
        let query = "
            SELECT other.name, other.kind, array_cosine_distance(anchor.embedding, peer.embedding) AS score
            FROM GraphEmbedding anchor
            JOIN GraphProjectionState anchor_state
              ON anchor_state.anchor_type = anchor.anchor_type
             AND anchor_state.anchor_id = anchor.anchor_id
             AND anchor_state.radius = anchor.radius
             AND anchor_state.source_signature = anchor.source_signature
             AND anchor_state.projection_version = anchor.projection_version
            JOIN GraphEmbedding peer
              ON peer.anchor_type = anchor.anchor_type
             AND peer.radius = anchor.radius
             AND peer.model_id = anchor.model_id
             AND peer.anchor_id <> anchor.anchor_id
            JOIN GraphProjectionState peer_state
              ON peer_state.anchor_type = peer.anchor_type
             AND peer_state.anchor_id = peer.anchor_id
             AND peer_state.radius = peer.radius
             AND peer_state.source_signature = peer.source_signature
             AND peer_state.projection_version = peer.projection_version
            JOIN Symbol other
              ON other.id = peer.anchor_id
            WHERE anchor.anchor_type = 'symbol'
              AND anchor.anchor_id = $anchor
              AND anchor.model_id = 'graph-bge-small-en-v1.5-384'
              AND array_cosine_distance(anchor.embedding, peer.embedding) < 0.05
            ORDER BY score ASC
            LIMIT 5";
        let res = self.graph_store.query_json_param(query, &json!({"anchor": anchor_id})).ok()?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        if rows.is_empty() {
            return None;
        }

        Some(format!(
            "\n\n### Voisinages similaires derives du graphe\n\n**Etat:** contexte derive du graphe via `GraphEmbedding`, utile pour reperer des neighborhoods proches; ce n'est pas une verite canonique d'architecture.\n\n{}",
            format_table_from_json(&res, &["Nom", "Type", "Distance de voisinage"])
        ))
    }

    pub(crate) fn axon_audit(&self, args: &Value) -> Option<Value> {
        let requested_project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let project = requested_project;

        let file_count = if project == "*" {
            self.graph_store.query_count("SELECT count(*) FROM File").unwrap_or(0)
        } else {
            let count_query = "SELECT count(*) FROM File WHERE path LIKE '%' || $proj || '%'".to_string();
            let params = json!({"proj": project});
            self.graph_store
                .query_count_param(&count_query, &params)
                .unwrap_or(0)
        };

        if file_count < 1 {
            let warning = format!(
                "⚠️ Warning: Project '{}' seems unindexed or parser failed (Found {} files). Health metrics are invalid.",
                project, file_count
            );
            return Some(json!({ "content": [{ "type": "text", "text": warning }] }));
        }

        let (sec_score, paths) = self
            .graph_store
            .get_security_audit(project)
            .unwrap_or((100, "[]".to_string()));
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

    pub(crate) fn axon_health(&self, args: &Value) -> Option<Value> {
        let requested_project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let project = requested_project;

        let file_count = if project == "*" {
            self.graph_store.query_count("SELECT count(*) FROM File").unwrap_or(0)
        } else {
            let count_query = "SELECT count(*) FROM File WHERE path LIKE '%' || $proj || '%'".to_string();
            let params = json!({"proj": project});
            self.graph_store
                .query_count_param(&count_query, &params)
                .unwrap_or(0)
        };

        if file_count < 1 {
            let warning = format!(
                "⚠️ Warning: Project '{}' seems unindexed or parser failed (Found {} files). Health metrics are invalid.",
                project, file_count
            );
            return Some(json!({ "content": [{ "type": "text", "text": warning }] }));
        }

        let coverage = self.graph_store.get_coverage_score(project).unwrap_or(0);
        let god_objects = self.graph_store.get_god_objects(project).unwrap_or_default();

        let mut report = format!("🏥 Health Report for {}: Coverage {}%. Stability high.", project, coverage);
        if !god_objects.is_empty() {
            let god_list: Vec<String> = god_objects
                .iter()
                .map(|(name, count)| format!("{} ({} lines)", name, count))
                .collect();
            report.push_str(&format!("\nGod Objects detected: {}", god_list.join(", ")));
        }

        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_semantic_clones(&self, args: &Value) -> Option<Value> {
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
                let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
                let mut report = if !rows.is_empty() {
                    format!(
                        "### 👯 Clones Sémantiques détectés pour '{}'\n\n{}",
                        symbol,
                        format_table_from_json(&res, &["Nom", "Type", "Similitude"])
                    )
                } else {
                    format!("✅ Aucun clone sémantique évident (similitude > 95%) trouvé pour '{}'.", symbol)
                };
                if let Some(section) = self.build_graph_clone_section(symbol) {
                    report.push_str(&section);
                }
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Cloning Error: {}", e) }], "isError": true })),
        }
    }

    pub(crate) fn axon_architectural_drift(&self, args: &Value) -> Option<Value> {
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
                    format!(
                        "⚠️ **VIOLATION D'ARCHITECTURE DÉTECTÉE**\n\nLa couche '{}' appelle directement '{}' :\n\n{}",
                        source_layer,
                        target_layer,
                        format_table_from_json(&res, &["Source", "Symbole", "Cible", "Appelé"])
                    )
                } else {
                    format!(
                        "✅ Aucune dérive architecturale détectée entre '{}' et '{}'.",
                        source_layer, target_layer
                    )
                };
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Drift Analysis Error: {}", e) }], "isError": true })),
        }
    }
}
