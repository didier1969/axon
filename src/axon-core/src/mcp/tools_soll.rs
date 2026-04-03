use anyhow::anyhow;
use serde_json::{json, Value};

use super::soll::{
    canonical_soll_export_dir, find_latest_soll_export, parse_soll_export, SollRestoreCounts,
};
use super::McpServer;

const SOLL_RELATION_EXPORTS: [(&str, &str); 12] = [
    ("EPITOMIZES", "soll.EPITOMIZES"),
    ("BELONGS_TO", "soll.BELONGS_TO"),
    ("EXPLAINS", "soll.EXPLAINS"),
    ("SOLVES", "soll.SOLVES"),
    ("TARGETS", "soll.TARGETS"),
    ("VERIFIES", "soll.VERIFIES"),
    ("ORIGINATES", "soll.ORIGINATES"),
    ("SUPERSEDES", "soll.SUPERSEDES"),
    ("CONTRIBUTES_TO", "soll.CONTRIBUTES_TO"),
    ("REFINES", "soll.REFINES"),
    ("IMPACTS", "IMPACTS"),
    ("SUBSTANTIATES", "SUBSTANTIATES"),
];

impl McpServer {
    pub(crate) fn axon_soll_manager(&self, args: &Value) -> Option<Value> {
        let action = args.get("action")?.as_str()?;
        let entity = args.get("entity")?.as_str()?;
        let data = args.get("data")?;

        match action {
            "create" => {
                let project_slug = data
                    .get("project_slug")
                    .and_then(|v| v.as_str())
                    .unwrap_or("AXO");
                let formatted_id = match entity {
                    "stakeholder" => data.get("name")?.as_str()?.to_string(),
                    "vision" => data
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("VIS-AXO-001")
                        .to_string(),
                    _ => match self.next_soll_numeric_id(project_slug, entity) {
                        Ok((prefix, next_num)) => {
                            format!("{}-{}-{:03}", prefix, project_slug, next_num)
                        }
                        Err(e) => {
                            return Some(
                                json!({ "content": [{ "type": "text", "text": format!("Erreur registre: {}", e) }], "isError": true }),
                            )
                        }
                    },
                };

                let insert_res = match entity {
                    "vision" => {
                        let title = data.get("title")?.as_str()?;
                        let description = data.get("description")?.as_str()?;
                        let goal = data.get("goal")?.as_str()?;
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q = "INSERT INTO soll.Vision (id, title, description, goal, metadata) VALUES (?, ?, ?, ?, ?) \
                                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, goal = EXCLUDED.goal, metadata = EXCLUDED.metadata";
                        self.graph_store.execute_param(
                            q,
                            &json!([formatted_id, title, description, goal, meta.to_string()]),
                        )
                    }
                    "pillar" => {
                        let title = data.get("title")?.as_str()?;
                        let desc = data.get("description")?.as_str()?;
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q = "INSERT INTO soll.Pillar (id, title, description, metadata) VALUES (?, ?, ?, ?)";
                        self.graph_store
                            .execute_param(q, &json!([formatted_id, title, desc, meta.to_string()]))
                    }
                    "requirement" => {
                        let title = data.get("title")?.as_str()?;
                        let desc = data.get("description")?.as_str()?;
                        let prio = data
                            .get("priority")
                            .and_then(|v| v.as_str())
                            .unwrap_or("P2");
                        let status = data
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("current");
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q = "INSERT INTO soll.Requirement (id, title, description, status, priority, metadata) VALUES (?, ?, ?, ?, ?, ?)";
                        self.graph_store.execute_param(
                            q,
                            &json!([formatted_id, title, desc, status, prio, meta.to_string()]),
                        )
                    }
                    "concept" => {
                        let name = data.get("name")?.as_str()?;
                        let expl = data.get("explanation")?.as_str()?;
                        let rat = data.get("rationale")?.as_str()?;
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let final_name = if name.starts_with(&formatted_id) {
                            name.to_string()
                        } else {
                            format!("{}: {}", formatted_id, name)
                        };
                        let q = "INSERT INTO soll.Concept (name, explanation, rationale, metadata) VALUES (?, ?, ?, ?)";
                        self.graph_store
                            .execute_param(q, &json!([final_name, expl, rat, meta.to_string()]))
                    }
                    "decision" => {
                        let title = data.get("title")?.as_str()?;
                        let description = data
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let ctx = data.get("context")?.as_str()?;
                        let rat = data.get("rationale")?.as_str()?;
                        let status = data
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("accepted");
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q = "INSERT INTO soll.Decision (id, title, description, context, rationale, status, metadata) VALUES (?, ?, ?, ?, ?, ?, ?)";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                formatted_id,
                                title,
                                description,
                                ctx,
                                rat,
                                status,
                                meta.to_string()
                            ]),
                        )
                    }
                    "milestone" => {
                        let title = data.get("title")?.as_str()?;
                        let status = data
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("planned");
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q = "INSERT INTO soll.Milestone (id, title, status, metadata) VALUES (?, ?, ?, ?)";
                        self.graph_store.execute_param(
                            q,
                            &json!([formatted_id, title, status, meta.to_string()]),
                        )
                    }
                    "stakeholder" => {
                        let name = data.get("name")?.as_str()?;
                        let role = data.get("role")?.as_str()?;
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q =
                            "INSERT INTO soll.Stakeholder (name, role, metadata) VALUES (?, ?, ?)";
                        self.graph_store
                            .execute_param(q, &json!([name, role, meta.to_string()]))
                    }
                    "validation" => {
                        let method = data.get("method")?.as_str()?;
                        let result = data.get("result")?.as_str()?;
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64;
                        let q = "INSERT INTO soll.Validation (id, method, result, timestamp, metadata) VALUES (?, ?, ?, ?, ?)";
                        self.graph_store.execute_param(
                            q,
                            &json!([formatted_id, method, result, ts, meta.to_string()]),
                        )
                    }
                    _ => Err(anyhow!("Unknown entity")),
                };

                match insert_res {
                    Ok(_) => {
                        let report = format!("✅ Entité SOLL créée : `{}`", formatted_id);
                        Some(json!({ "content": [{ "type": "text", "text": report }] }))
                    }
                    Err(e) => Some(
                        json!({ "content": [{ "type": "text", "text": format!("Erreur d'insertion: {}", e) }], "isError": true }),
                    ),
                }
            }
            "update" => {
                let id = data.get("id")?.as_str()?;
                let update_res: anyhow::Result<()> = (|| match entity {
                    "pillar" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT title, description, metadata FROM soll.Pillar WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                3,
                            )?;
                        let q =
                            "UPDATE soll.Pillar SET title = ?, description = ?, metadata = ? WHERE id = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("description")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[2].clone()),
                                id
                            ]),
                        )
                    }
                    "requirement" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT title, description, priority, status, metadata FROM soll.Requirement WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                5,
                            )?;
                        let q =
                            "UPDATE soll.Requirement SET title = ?, description = ?, priority = ?, status = ?, metadata = ? WHERE id = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("description")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                data.get("priority")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[2]),
                                data.get("status")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[3]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[4].clone()),
                                id
                            ]),
                        )
                    }
                    "concept" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT name, explanation, rationale, metadata FROM soll.Concept WHERE name LIKE '{}:%'",
                                    escape_sql(id)
                                ),
                                4,
                            )?;
                        let concept_name = data
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(|name| {
                                if name.starts_with(id) {
                                    name.to_string()
                                } else {
                                    format!("{}: {}", id, name)
                                }
                            })
                            .unwrap_or_else(|| current[0].clone());
                        let q = "UPDATE soll.Concept SET name = ?, explanation = ?, rationale = ?, metadata = ? WHERE name LIKE ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                concept_name,
                                data.get("explanation")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                data.get("rationale")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[2]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[3].clone()),
                                format!("{}:%", id)
                            ]),
                        )
                    }
                    "decision" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT title, description, context, rationale, status, metadata FROM soll.Decision WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                6,
                            )?;
                        let q = "UPDATE soll.Decision SET title = ?, description = ?, context = ?, rationale = ?, status = ?, metadata = ? WHERE id = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("description")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                data.get("context")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[2]),
                                data.get("rationale")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[3]),
                                data.get("status")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[4]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[5].clone()),
                                id
                            ]),
                        )
                    }
                    "milestone" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT title, status, metadata FROM soll.Milestone WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                3,
                            )?;
                        let q = "UPDATE soll.Milestone SET title = ?, status = ?, metadata = ? WHERE id = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("status")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[2].clone()),
                                id
                            ]),
                        )
                    }
                    "stakeholder" => {
                        let current = self.query_named_row(
                            &format!(
                                "SELECT role, metadata FROM soll.Stakeholder WHERE name = '{}'",
                                escape_sql(id)
                            ),
                            2,
                        )?;
                        let q = "UPDATE soll.Stakeholder SET role = ?, metadata = ? WHERE name = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("role")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[1].clone()),
                                id
                            ]),
                        )
                    }
                    "validation" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT method, result, metadata FROM soll.Validation WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                3,
                            )?;
                        let ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64;
                        let q = "UPDATE soll.Validation SET method = ?, result = ?, timestamp = ?, metadata = ? WHERE id = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("method")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("result")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                ts,
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[2].clone()),
                                id
                            ]),
                        )
                    }
                    "vision" => {
                        let current = self
                            .query_named_row(
                                &format!(
                                    "SELECT title, description, goal, metadata FROM soll.Vision WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                4,
                            )?;
                        let q = "UPDATE soll.Vision SET title = ?, description = ?, goal = ?, metadata = ? WHERE id = ?";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                data.get("title")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[0]),
                                data.get("description")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[1]),
                                data.get("goal")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[2]),
                                data.get("metadata")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[3].clone()),
                                id
                            ]),
                        )
                    }
                    _ => Err(anyhow!("Unknown entity")),
                })();
                match update_res {
                    Ok(_) => Some(
                        json!({ "content": [{ "type": "text", "text": format!("✅ Mise à jour réussie pour `{}`", id) }] }),
                    ),
                    Err(e) => Some(
                        json!({ "content": [{ "type": "text", "text": format!("Erreur update: {}", e) }], "isError": true }),
                    ),
                }
            }
            "link" => {
                let src = data.get("source_id")?.as_str()?;
                let tgt = data.get("target_id")?.as_str()?;
                let explicit_rel = data.get("relation_type").and_then(|v| v.as_str());

                let rel_table = if let Some(r) = explicit_rel {
                    match r.to_uppercase().as_str() {
                        "EPITOMIZES" => "soll.EPITOMIZES",
                        "BELONGS_TO" => "soll.BELONGS_TO",
                        "EXPLAINS" => "soll.EXPLAINS",
                        "SOLVES" => "soll.SOLVES",
                        "TARGETS" => "soll.TARGETS",
                        "VERIFIES" => "soll.VERIFIES",
                        "ORIGINATES" => "soll.ORIGINATES",
                        "SUPERSEDES" => "soll.SUPERSEDES",
                        "CONTRIBUTES_TO" => "soll.CONTRIBUTES_TO",
                        "REFINES" => "soll.REFINES",
                        "IMPACTS" => "IMPACTS",
                        "SUBSTANTIATES" => "SUBSTANTIATES",
                        _ => {
                            return Some(
                                json!({ "content": [{ "type": "text", "text": format!("Erreur: Type de relation inconnu '{}'", r) }], "isError": true }),
                            )
                        }
                    }
                } else {
                    match (
                        src.split('-').next().unwrap_or(""),
                        tgt.split('-').next().unwrap_or(""),
                    ) {
                        ("PIL", "REQ") | ("REQ", "PIL") => "soll.BELONGS_TO",
                        ("CPT", "REQ") | ("REQ", "CPT") => "soll.EXPLAINS",
                        ("PIL", "AXO") | ("AXO", "PIL") => "soll.EPITOMIZES",
                        ("DEC", "REQ") | ("REQ", "DEC") => "soll.SOLVES",
                        ("MIL", "REQ") | ("REQ", "MIL") => "soll.TARGETS",
                        ("VAL", "REQ") | ("REQ", "VAL") => "soll.VERIFIES",
                        ("STK", "REQ") | ("REQ", "STK") => "soll.ORIGINATES",
                        ("DEC", _) => "IMPACTS",
                        _ => "SUBSTANTIATES",
                    }
                };

                let q = format!(
                    "INSERT INTO {} (source_id, target_id) VALUES (?, ?)",
                    rel_table
                );
                match self.graph_store.execute_param(&q, &json!([src, tgt])) {
                    Ok(_) => Some(
                        json!({ "content": [{ "type": "text", "text": format!("✅ Liaison établie : `{}` -> `{}` (via {})", src, tgt, rel_table) }] }),
                    ),
                    Err(e) => Some(
                        json!({ "content": [{ "type": "text", "text": format!("Erreur liaison: {}", e) }], "isError": true }),
                    ),
                }
            }
            _ => None,
        }
    }

    pub(crate) fn axon_export_soll(&self) -> Option<Value> {
        let mut markdown = String::from("# SOLL Extraction\n\n");

        let now = std::time::SystemTime::now();
        let datetime: chrono::DateTime<chrono::Local> = now.into();
        let timestamp_str = datetime.format("%Y-%m-%d %H:%M:%S").to_string();
        markdown.push_str(&format!("*Généré le : {}*\n\n", timestamp_str));

        markdown.push_str("## 1. Vision & Objectifs Stratégiques\n");
        if let Ok(res) = self
            .graph_store
            .query_json("SELECT title, description, goal, metadata FROM soll.Vision")
        {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                let meta = r.get(3).cloned().unwrap_or_default();
                markdown.push_str(&format!(
                    "### {}\n**Description:** {}\n**Goal:** {}\n**Meta:** `{}`\n\n",
                    r[0], r[1], r[2], meta
                ));
            }
        }

        markdown.push_str("## 2. Piliers d'Architecture\n");
        if let Ok(res) = self
            .graph_store
            .query_json("SELECT id, title, description, metadata FROM soll.Pillar")
        {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("* **{}** : {} ({})\n", r[0], r[1], r[2]));
                if let Some(meta) = r.get(3).filter(|m| !m.is_empty() && *m != "{}") {
                    markdown.push_str(&format!("  Meta: `{}`\n", meta));
                }
            }
        }

        markdown.push_str("\n## 2b. Concepts\n");
        if let Ok(res) = self
            .graph_store
            .query_json("SELECT name, explanation, rationale, metadata FROM soll.Concept")
        {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("* **{}** : {} ({})\n", r[0], r[1], r[2]));
                if let Some(meta) = r.get(3).filter(|m| !m.is_empty() && *m != "{}") {
                    markdown.push_str(&format!("  Meta: `{}`\n", meta));
                }
            }
        }

        markdown.push_str("\n## 3. Jalons & Roadmap (Milestones)\n");
        if let Ok(res) = self
            .graph_store
            .query_json("SELECT id, title, status, metadata FROM soll.Milestone")
        {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!(
                    "### {} : {}\n*Statut :* `{}`\n\n",
                    r[0], r[1], r[2]
                ));
                if let Some(meta) = r.get(3).filter(|m| !m.is_empty() && *m != "{}") {
                    markdown.push_str(&format!("*Meta :* `{}`\n", meta));
                }
                markdown.push('\n');
            }
        }

        markdown.push_str("## 4. Exigences & Rayon d'Impact (Requirements)\n");
        let req_query =
            "SELECT id, title, priority, description, status, metadata FROM soll.Requirement";
        if let Ok(res) = self.graph_store.query_json(req_query) {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!(
                    "### {} - {}\n*Priorité :* `{}`\n*Description :* {}\n",
                    r[0], r[1], r[2], r[3]
                ));
                if let Some(status) = r.get(4).filter(|m| !m.is_empty()) {
                    markdown.push_str(&format!("*Statut :* `{}`\n", status));
                }
                if let Some(meta) = r.get(5).filter(|m| !m.is_empty() && *m != "{}") {
                    markdown.push_str(&format!("*Meta :* `{}`\n", meta));
                }
                markdown.push('\n');
            }
        }

        markdown.push_str("## 5. Registre des Décisions (ADR)\n");
        if let Ok(res) = self.graph_store.query_json("SELECT id, title, status, context, description, rationale, metadata FROM soll.Decision") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("### {}\n**Titre :** {}\n**Statut :** `{}`\n", r[0], r[1], r[2]));
                if let Some(context) = r.get(3).filter(|m| !m.is_empty()) {
                    markdown.push_str(&format!("**Contexte :** {}\n", context));
                }
                if let Some(description) = r.get(4).filter(|m| !m.is_empty()) {
                    markdown.push_str(&format!("**Description :** {}\n", description));
                }
                markdown.push_str(&format!("**Rationnel :** {}\n", r[5]));
                if let Some(meta) = r.get(6).filter(|m| !m.is_empty() && *m != "{}") {
                    markdown.push_str(&format!("**Meta :** `{}`\n", meta));
                }
                markdown.push('\n');
            }
        }

        markdown.push_str("## 6. Preuves de Validation & Witness\n");
        if let Ok(res) = self
            .graph_store
            .query_json("SELECT id, method, result, timestamp, metadata FROM soll.Validation")
        {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!(
                    "*   `{}` : **{}** via `{}` (Certifié le {})\n",
                    r[0], r[2], r[1], r[3]
                ));
                if let Some(meta) = r.get(4).filter(|m| !m.is_empty() && *m != "{}") {
                    markdown.push_str(&format!("  Meta: `{}`\n", meta));
                }
            }
        }

        markdown.push_str("\n## 7. Liens de Traçabilité SOLL\n");
        for (relation_type, table_name) in SOLL_RELATION_EXPORTS {
            if let Ok(res) = self.graph_store.query_json(&format!(
                "SELECT source_id, target_id FROM {} ORDER BY source_id, target_id",
                table_name
            )) {
                let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
                for row in rows {
                    if row.len() >= 2 {
                        markdown.push_str(&format!(
                            "* `{}`: `{}` -> `{}`\n",
                            relation_type, row[0], row[1]
                        ));
                    }
                }
            }
        }

        let export_dir = match canonical_soll_export_dir() {
            Some(path) => path,
            None => {
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": "Erreur d'écriture: impossible de résoudre le répertoire canonique docs/vision du dépôt"
                    }],
                    "isError": true
                }))
            }
        };

        let file_name = format!("SOLL_EXPORT_{}.md", datetime.format("%Y-%m-%d_%H%M%S_%3f"));
        let file_path = export_dir.join(file_name);

        let _ = std::fs::create_dir_all(&export_dir);
        match std::fs::write(&file_path, &markdown) {
            Ok(_) => {
                let report = format!(
                    "✅ Exported to {}\n\n---\n\n{}",
                    file_path.display(),
                    markdown.chars().take(300).collect::<String>()
                );
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("Erreur d'écriture: {}", e) }], "isError": true }),
            ),
        }
    }

    pub(crate) fn axon_validate_soll(&self) -> Option<Value> {
        let orphan_requirements = self
            .query_single_column(
                "SELECT id FROM soll.Requirement r
                 WHERE NOT EXISTS (SELECT 1 FROM soll.BELONGS_TO WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM soll.EXPLAINS WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM soll.SOLVES WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM soll.TARGETS WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM soll.VERIFIES WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM soll.ORIGINATES WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM SUBSTANTIATES WHERE source_id = r.id OR target_id = r.id)
                   AND NOT EXISTS (SELECT 1 FROM IMPACTS WHERE source_id = r.id OR target_id = r.id)
                 ORDER BY id",
            )
            .ok()?;

        let validations_without_verifies = self
            .query_single_column(
                "SELECT id FROM soll.Validation v
                 WHERE NOT EXISTS (SELECT 1 FROM soll.VERIFIES WHERE source_id = v.id OR target_id = v.id)
                 ORDER BY id",
            )
            .ok()?;

        let decisions_without_links = self
            .query_single_column(
                "SELECT id FROM soll.Decision d
                 WHERE NOT EXISTS (SELECT 1 FROM soll.SOLVES WHERE source_id = d.id OR target_id = d.id)
                   AND NOT EXISTS (SELECT 1 FROM IMPACTS WHERE source_id = d.id OR target_id = d.id)
                 ORDER BY id",
            )
            .ok()?;

        let violation_count = orphan_requirements.len()
            + validations_without_verifies.len()
            + decisions_without_links.len();

        let mut report = format!(
            "Validation SOLL: {} violation(s) de cohérence minimale détectée(s).\n",
            violation_count
        );
        report.push_str("Mode: lecture seule, sans auto-réparation.\n");

        if violation_count == 0 {
            report.push_str("Etat: cohérence minimale vérifiée, 0 violation détectée.\n");
            return Some(json!({ "content": [{ "type": "text", "text": report }] }));
        }

        if !orphan_requirements.is_empty() {
            report.push_str("\n- Requirements orphelins:\n");
            for id in orphan_requirements {
                report.push_str(&format!("  - {}\n", id));
            }
        }

        if !validations_without_verifies.is_empty() {
            report.push_str("\n- Validations sans lien VERIFIES:\n");
            for id in validations_without_verifies {
                report.push_str(&format!("  - {}\n", id));
            }
        }

        if !decisions_without_links.is_empty() {
            report.push_str("\n- Decisions sans lien SOLVES/IMPACTS:\n");
            for id in decisions_without_links {
                report.push_str(&format!("  - {}\n", id));
            }
        }

        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_restore_soll(&self, args: &Value) -> Option<Value> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(find_latest_soll_export)?;

        let markdown = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("SOLL restore read error: {}", e) }],
                    "isError": true
                }))
            }
        };

        let restore = match parse_soll_export(&markdown) {
            Ok(parsed) => parsed,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("SOLL restore parse error: {}", e) }],
                    "isError": true
                }))
            }
        };

        if let Err(e) = self.graph_store.execute(
            "INSERT INTO soll.Registry (project_slug, id, last_pil, last_req, last_cpt, last_dec, last_mil, last_val)
             VALUES ('AXO', 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0)
             ON CONFLICT (project_slug) DO NOTHING"
        ) {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("SOLL restore registry error: {}", e) }],
                "isError": true
            }));
        }

        let mut restored = SollRestoreCounts::default();

        for vision in restore.vision {
            let metadata = vision.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Vision (id, title, description, goal, metadata)
                 VALUES ('VIS-AXO-001', $title, $description, $goal, $metadata)
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   description = EXCLUDED.description,
                   goal = EXCLUDED.goal,
                   metadata = EXCLUDED.metadata",
                &json!({
                    "title": vision.title,
                    "description": vision.description,
                    "goal": vision.goal,
                    "metadata": metadata
                }),
            ) {
                return Some(
                    json!({ "content": [{ "type": "text", "text": format!("SOLL restore vision error: {}", e) }], "isError": true }),
                );
            }
            restored.vision += 1;
        }

        for pillar in restore.pillars {
            let metadata = pillar.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Pillar (id, title, description, metadata)
                 VALUES ($id, $title, $description, $metadata)
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   description = EXCLUDED.description,
                   metadata = EXCLUDED.metadata",
                &json!({"id": pillar.id, "title": pillar.title, "description": pillar.description, "metadata": metadata})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore pillar error: {}", e) }], "isError": true }));
            }
            restored.pillars += 1;
        }

        for concept in restore.concepts {
            let metadata = concept.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Concept (name, explanation, rationale, metadata)
                 VALUES ($name, $explanation, $rationale, $metadata)
                 ON CONFLICT (name) DO UPDATE SET
                   explanation = EXCLUDED.explanation,
                   rationale = EXCLUDED.rationale,
                   metadata = EXCLUDED.metadata",
                &json!({"name": concept.name, "explanation": concept.explanation, "rationale": concept.rationale, "metadata": metadata})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore concept error: {}", e) }], "isError": true }));
            }
            restored.concepts += 1;
        }

        for milestone in restore.milestones {
            let metadata = milestone.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Milestone (id, title, status, metadata)
                 VALUES ($id, $title, $status, $metadata)
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   status = EXCLUDED.status,
                   metadata = EXCLUDED.metadata",
                &json!({"id": milestone.id, "title": milestone.title, "status": milestone.status, "metadata": metadata})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore milestone error: {}", e) }], "isError": true }));
            }
            restored.milestones += 1;
        }

        for requirement in restore.requirements {
            let metadata = requirement.metadata.unwrap_or_else(|| "{}".to_string());
            let status = requirement.status.unwrap_or_else(|| "restored".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Requirement (id, title, description, status, priority, metadata)
                 VALUES ($id, $title, $description, $status, $priority, $metadata)
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   description = EXCLUDED.description,
                   status = EXCLUDED.status,
                   priority = EXCLUDED.priority,
                   metadata = EXCLUDED.metadata",
                &json!({"id": requirement.id, "title": requirement.title, "description": requirement.description, "priority": requirement.priority, "status": status, "metadata": metadata})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore requirement error: {}", e) }], "isError": true }));
            }
            restored.requirements += 1;
        }

        for decision in restore.decisions {
            let description = decision.description.unwrap_or_default();
            let context = decision.context.unwrap_or_default();
            let metadata = decision.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Decision (id, title, description, context, rationale, status, metadata)
                 VALUES ($id, $title, $description, $context, $rationale, $status, $metadata)
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   description = EXCLUDED.description,
                   context = EXCLUDED.context,
                   rationale = EXCLUDED.rationale,
                   status = EXCLUDED.status,
                   metadata = EXCLUDED.metadata",
                &json!({"id": decision.id, "title": decision.title, "description": description, "context": context, "rationale": decision.rationale, "status": decision.status, "metadata": metadata})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore decision error: {}", e) }], "isError": true }));
            }
            restored.decisions += 1;
        }

        for validation in restore.validations {
            let metadata = validation.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Validation (id, method, result, timestamp, metadata)
                 VALUES ($id, $method, $result, $timestamp, $metadata)
                 ON CONFLICT (id) DO UPDATE SET
                   method = EXCLUDED.method,
                   result = EXCLUDED.result,
                   timestamp = EXCLUDED.timestamp,
                   metadata = EXCLUDED.metadata",
                &json!({"id": validation.id, "method": validation.method, "result": validation.result, "timestamp": validation.timestamp, "metadata": metadata})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore validation error: {}", e) }], "isError": true }));
            }
            restored.validations += 1;
        }

        for relation in restore.relations {
            if let Err(e) = self.restore_soll_relation(
                &relation.relation_type,
                &relation.source_id,
                &relation.target_id,
            ) {
                return Some(
                    json!({ "content": [{ "type": "text", "text": format!("SOLL restore relation error: {}", e) }], "isError": true }),
                );
            }
            restored.relations += 1;
        }

        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "### Restauration SOLL terminee\n\nSource: `{}`\n\nRestaure en mode merge:\n- Vision: {}\n- Pillars: {}\n- Concepts: {}\n- Milestones: {}\n- Requirements: {}\n- Decisions: {}\n- Validations: {}\n- Relations: {}\n\nNote: ce chemin de restauration reconstruit les entites conceptuelles depuis le format Markdown officiel d'export. Les metadonnees et liaisons presentes dans l'export sont rejouees en mode merge; les champs absents conservent le comportement historique tolerant.",
                    path,
                    restored.vision,
                    restored.pillars,
                    restored.concepts,
                    restored.milestones,
                    restored.requirements,
                    restored.decisions,
                    restored.validations,
                    restored.relations
                )
            }]
        }))
    }

    fn query_single_column(&self, query: &str) -> anyhow::Result<Vec<String>> {
        let res = self.graph_store.query_json(query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect())
    }

    fn query_named_row(&self, query: &str, expected_columns: usize) -> anyhow::Result<Vec<String>> {
        let res = self.graph_store.query_json(query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Entité SOLL introuvable"))?;
        if row.len() < expected_columns {
            return Err(anyhow!("Résultat SOLL incomplet pour la mise à jour"));
        }
        Ok(row)
    }

    fn next_soll_numeric_id(
        &self,
        project_slug: &str,
        entity: &str,
    ) -> anyhow::Result<(&'static str, u64)> {
        let (prefix, reg_col, table, id_expr) = match entity {
            "pillar" => ("PIL", "last_pil", "soll.Pillar", "id"),
            "requirement" => ("REQ", "last_req", "soll.Requirement", "id"),
            "concept" => ("CPT", "last_cpt", "soll.Concept", "name"),
            "decision" => ("DEC", "last_dec", "soll.Decision", "id"),
            "milestone" => ("MIL", "last_mil", "soll.Milestone", "id"),
            "validation" => ("VAL", "last_val", "soll.Validation", "id"),
            _ => return Err(anyhow!("Unknown entity")),
        };

        self.graph_store.execute(&format!(
            "INSERT INTO soll.Registry (project_slug, id, last_pil, last_req, last_cpt, last_dec, last_mil, last_val) \
             VALUES ('{}', 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0) ON CONFLICT (project_slug) DO NOTHING",
            escape_sql(project_slug)
        ))?;

        let current_query = format!(
            "SELECT COALESCE({}, 0) FROM soll.Registry WHERE project_slug = '{}'",
            reg_col,
            escape_sql(project_slug)
        );
        let current = self
            .query_single_column(&current_query)?
            .into_iter()
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);

        let ids_query = format!(
            "SELECT {} FROM {} WHERE {} LIKE '{}-{}-%'",
            id_expr,
            table,
            id_expr,
            prefix,
            escape_sql(project_slug)
        );
        let observed_max = self
            .query_single_column(&ids_query)?
            .into_iter()
            .filter_map(|value| parse_numeric_suffix(&value))
            .max()
            .unwrap_or(0);

        let next = current.max(observed_max) + 1;
        self.graph_store.execute(&format!(
            "UPDATE soll.Registry SET {} = {} WHERE project_slug = '{}'",
            reg_col,
            next,
            escape_sql(project_slug)
        ))?;

        Ok((prefix, next))
    }

    fn restore_soll_relation(
        &self,
        relation_type: &str,
        source_id: &str,
        target_id: &str,
    ) -> anyhow::Result<()> {
        let table_name = match relation_type {
            "EPITOMIZES" => "soll.EPITOMIZES",
            "BELONGS_TO" => "soll.BELONGS_TO",
            "EXPLAINS" => "soll.EXPLAINS",
            "SOLVES" => "soll.SOLVES",
            "TARGETS" => "soll.TARGETS",
            "VERIFIES" => "soll.VERIFIES",
            "ORIGINATES" => "soll.ORIGINATES",
            "SUPERSEDES" => "soll.SUPERSEDES",
            "CONTRIBUTES_TO" => "soll.CONTRIBUTES_TO",
            "REFINES" => "soll.REFINES",
            "IMPACTS" => "IMPACTS",
            "SUBSTANTIATES" => "SUBSTANTIATES",
            _ => return Ok(()),
        };

        self.graph_store.execute_param(
            &format!(
                "INSERT INTO {} (source_id, target_id)
                 SELECT ?, ?
                 WHERE NOT EXISTS (
                   SELECT 1 FROM {} WHERE source_id = ? AND target_id = ?
                 )",
                table_name, table_name
            ),
            &json!([source_id, target_id, source_id, target_id]),
        )?;
        Ok(())
    }
}

fn parse_numeric_suffix(value: &str) -> Option<u64> {
    let head = value.split(':').next()?.trim();
    head.rsplit('-').next()?.parse::<u64>().ok()
}

fn escape_sql(value: &str) -> String {
    value.replace('\'', "''")
}
