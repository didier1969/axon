// Copyright (c) Didier Stadelmann. All rights reserved.

use serde_json::{json, Value};

use super::format::format_table_from_json;
use super::McpServer;

impl McpServer {
    fn json_to_i64(value: &Value) -> Option<i64> {
        match value {
            Value::Number(n) => n
                .as_i64()
                .or_else(|| n.as_u64().and_then(|v| i64::try_from(v).ok()))
                .or_else(|| n.as_f64().map(|v| v.round() as i64)),
            Value::String(s) => s
                .parse::<i64>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().map(|v| v.round() as i64)),
            _ => None,
        }
    }

    fn sql_scalar(&self, query: &str) -> i64 {
        let raw = match self.graph_store.execute_raw_sql_gateway(query) {
            Ok(raw) => raw,
            Err(_) => return 0,
        };
        let rows: Vec<Vec<Value>> = match serde_json::from_str(&raw) {
            Ok(rows) => rows,
            Err(_) => return 0,
        };
        rows.first()
            .and_then(|row| row.first())
            .and_then(Self::json_to_i64)
            .unwrap_or(0)
    }

    fn sql_rows(&self, query: &str) -> Vec<Vec<Value>> {
        self.graph_store
            .execute_raw_sql_gateway(query)
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
            .unwrap_or_default()
    }

    fn project_filter(project: &str, column: &str) -> String {
        if project == "*" {
            "1=1".to_string()
        } else {
            format!("{} = '{}'", column, project.replace('\'', "''"))
        }
    }

    pub(crate) fn indexing_diagnosis_markdown(&self, project: &str) -> String {
        let file_filter = Self::project_filter(project, "project_slug");
        let symbol_filter = Self::project_filter(project, "project_slug");
        let known = self.sql_scalar(&format!("SELECT count(*) FROM File WHERE {}", file_filter));
        let global_known = self.sql_scalar("SELECT count(*) FROM File");
        let pending = self.sql_scalar(&format!(
            "SELECT count(*) FROM File WHERE {} AND status = 'pending'",
            file_filter
        ));
        let indexing = self.sql_scalar(&format!(
            "SELECT count(*) FROM File WHERE {} AND status = 'indexing'",
            file_filter
        ));
        let completed = self.sql_scalar(&format!(
            "SELECT count(*) FROM File WHERE {} AND status IN ('indexed','indexed_degraded','skipped','deleted')",
            file_filter
        ));
        let symbols = self.sql_scalar(&format!(
            "SELECT count(*) FROM Symbol WHERE {}",
            symbol_filter
        ));
        let calls_direct = self.sql_scalar(&format!(
            "SELECT count(*) FROM CALLS c JOIN Symbol s ON c.source_id = s.id WHERE {}",
            Self::project_filter(project, "s.project_slug")
        ));
        let calls_nif = self.sql_scalar(&format!(
            "SELECT count(*) FROM CALLS_NIF c JOIN Symbol s ON c.source_id = s.id WHERE {}",
            Self::project_filter(project, "s.project_slug")
        ));
        let top_reasons = self.sql_rows(&format!(
            "SELECT COALESCE(status_reason, 'unknown'), count(*) \
             FROM File \
             WHERE {} AND status IN ('pending','indexing','indexed_degraded','oversized_for_current_budget') \
             GROUP BY 1 ORDER BY 2 DESC, 1 ASC LIMIT 5",
            file_filter
        ));
        let top_errors = self.sql_rows(&format!(
            "SELECT COALESCE(last_error_reason, 'unknown'), count(*) \
             FROM File \
             WHERE {} AND last_error_reason IS NOT NULL \
             GROUP BY 1 ORDER BY 2 DESC, 1 ASC LIMIT 5",
            file_filter
        ));

        let mut causes = Vec::new();
        if known == 0 {
            if project != "*" && global_known > 0 {
                causes.push(
                    "scope_mismatch_or_wrong_project_slug: le workspace contient des fichiers, mais pas ce projet"
                        .to_string(),
                );
            } else {
                causes.push(
                    "discovery_absent_or_filtered: aucun fichier découvert (watch root, ignore rules, permissions)"
                        .to_string(),
                );
            }
        }
        if known > 0 && completed == 0 && (pending + indexing) > 0 {
            causes.push(
                "ingestion_not_completed: fichiers en pending/indexing, pipeline possiblement bloqué ou encore en cours"
                    .to_string(),
            );
        }
        if known > 0 && symbols == 0 {
            causes.push(
                "parser_extraction_gap: fichiers connus mais 0 symbole extrait (langage non supporté ou échec parse)"
                    .to_string(),
            );
        }
        if symbols > 0 && (calls_direct + calls_nif) == 0 {
            causes.push(
                "call_graph_gap: symboles présents mais graphe d'appels vide pour ce scope".to_string(),
            );
        }
        if causes.is_empty() {
            causes.push("no_blocker_detected: aucun blocage majeur détecté par ce diagnostic".to_string());
        }

        let reason_lines = if top_reasons.is_empty() {
            "* aucune raison dominante".to_string()
        } else {
            top_reasons
                .iter()
                .filter_map(|row| {
                    let reason = row.first()?.as_str()?;
                    let count = row.get(1)?.as_i64().or_else(|| row.get(1)?.as_u64().map(|v| v as i64))?;
                    Some(format!("* `{}`: {}", reason, count))
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let error_lines = if top_errors.is_empty() {
            "* aucune erreur parser/commit remontée dans `last_error_reason`".to_string()
        } else {
            top_errors
                .iter()
                .filter_map(|row| {
                    let reason = row.first()?.as_str()?;
                    let count = row.get(1)?.as_i64().or_else(|| row.get(1)?.as_u64().map(|v| v as i64))?;
                    Some(format!("* `{}`: {}", reason, count))
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let cause_lines = causes
            .iter()
            .map(|c| format!("* {}", c))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "### 🔎 Day-1 Indexing Diagnosis ({})\n\n\
             **Scope facts**\n\
             * known files: {}\n\
             * completed files: {}\n\
             * pending: {}\n\
             * indexing: {}\n\
             * symbols: {}\n\
             * calls (direct): {}\n\
             * calls (nif): {}\n\n\
             **Likely root causes**\n{}\n\n\
             **Top status reasons**\n{}\n\n\
             **Top parser/runtime errors**\n{}\n\n\
             **Remediation hints**\n\
             * validate project slug and scope (`project_slug`) used in calls\n\
             * check watch root and ignored paths\n\
             * inspect parser support and `last_error_reason`\n\
             * if symbols > 0 but calls = 0, run bridge refinement and inspect FFI boundaries",
            project, known, completed, pending, indexing, symbols, calls_direct, calls_nif, cause_lines, reason_lines, error_lines
        )
    }

    pub(crate) fn axon_diagnose_indexing(&self, args: &Value) -> Option<Value> {
        let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let report = self.indexing_diagnosis_markdown(project);
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    fn file_count_for_project(&self, project: &str) -> i64 {
        let query = if project == "*" {
            "SELECT count(*) FROM File".to_string()
        } else {
            format!(
                "SELECT count(*) FROM File WHERE project_slug = '{}'",
                project.replace('\'', "''")
            )
        };

        self.graph_store
            .execute_raw_sql_gateway(&query)
            .ok()
            .and_then(|raw| {
                let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).ok()?;
                Self::json_to_i64(rows.first()?.first()?)
            })
            .unwrap_or(0)
    }

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
        let res = self
            .graph_store
            .query_json_param(query, &json!({"anchor": anchor_id}))
            .ok()?;
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

        let file_count = self.file_count_for_project(project);

        if file_count < 1 {
            let warning = format!(
                "⚠️ Warning: Project '{}' seems unindexed or parser failed (Found {} files). Health metrics are invalid.",
                project, file_count
            );
            let diagnostic = self.indexing_diagnosis_markdown(project);
            let report = format!("{}\n\n{}", warning, diagnostic);
            return Some(json!({ "content": [{ "type": "text", "text": report }] }));
        }

        let (sec_score, paths) = self
            .graph_store
            .get_security_audit(project)
            .unwrap_or((100, "[]".to_string()));
        let cov_score = self.graph_store.get_coverage_score(project).unwrap_or(0);
        let tech_debt = self
            .graph_store
            .get_technical_debt(project)
            .unwrap_or_default();

        let mut report = format!("## 🛡️ Audit de Conformité : {}\n\n", project);
        if let Some(note) = self.project_scope_truth_note((project != "*").then_some(project)) {
            report.push_str(&note);
            report.push('\n');
        }
        if let Some(note) =
            self.degraded_truth_note(self.degraded_file_count((project != "*").then_some(project)))
        {
            report.push_str(&note);
            report.push('\n');
        }
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
                report.push_str(&format!(
                    "*... et {} autres points détectés.*\n",
                    tech_debt.len() - 10
                ));
            }
        }

        report.push_str(&format!("\n### 🧪 Qualité & Tests : {}%\n", cov_score));
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_health(&self, args: &Value) -> Option<Value> {
        let requested_project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let project = requested_project;

        let file_count = self.file_count_for_project(project);

        if file_count < 1 {
            let warning = format!(
                "⚠️ Warning: Project '{}' seems unindexed or parser failed (Found {} files). Health metrics are invalid.",
                project, file_count
            );
            let diagnostic = self.indexing_diagnosis_markdown(project);
            let report = format!("{}\n\n{}", warning, diagnostic);
            return Some(json!({ "content": [{ "type": "text", "text": report }] }));
        }

        let coverage = self.graph_store.get_coverage_score(project).unwrap_or(0);
        let god_objects = self
            .graph_store
            .get_god_objects(project)
            .unwrap_or_default();

        let mut report = format!(
            "🏥 Health Report for {}: Coverage {}%. Stability high.",
            project, coverage
        );
        if let Some(note) = self.project_scope_truth_note((project != "*").then_some(project)) {
            report.push_str(&format!("\n{}", note));
        }
        if let Some(note) =
            self.degraded_truth_note(self.degraded_file_count((project != "*").then_some(project)))
        {
            report.push_str(&format!("\n{}", note));
        }
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
                    format!(
                        "✅ Aucun clone sémantique évident (similitude > 95%) trouvé pour '{}'.",
                        symbol
                    )
                };
                if let Some(section) = self.build_graph_clone_section(symbol) {
                    report.push_str(&section);
                }
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("Cloning Error: {}", e) }], "isError": true }),
            ),
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
        "
        .to_string();

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
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("Drift Analysis Error: {}", e) }], "isError": true }),
            ),
        }
    }
}
