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
                        let owner = data
                            .get("owner")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let acceptance_criteria = data
                            .get("acceptance_criteria")
                            .cloned()
                            .unwrap_or(json!([]))
                            .to_string();
                        let evidence_refs = data
                            .get("evidence_refs")
                            .cloned()
                            .unwrap_or(json!([]))
                            .to_string();
                        let updated_at = now_unix_ms();
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q = "INSERT INTO soll.Requirement (id, title, description, status, priority, metadata, owner, acceptance_criteria, evidence_refs, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                formatted_id,
                                title,
                                desc,
                                status,
                                prio,
                                meta.to_string(),
                                owner,
                                acceptance_criteria,
                                evidence_refs,
                                updated_at
                            ]),
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
                        let supersedes_decision_id = data
                            .get("supersedes_decision_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let impact_scope = data
                            .get("impact_scope")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let updated_at = now_unix_ms();
                        let meta = data.get("metadata").cloned().unwrap_or(json!({}));
                        let q = "INSERT INTO soll.Decision (id, title, description, context, rationale, status, metadata, supersedes_decision_id, impact_scope, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
                        self.graph_store.execute_param(
                            q,
                            &json!([
                                formatted_id,
                                title,
                                description,
                                ctx,
                                rat,
                                status,
                                meta.to_string(),
                                supersedes_decision_id,
                                impact_scope,
                                updated_at
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
                                    "SELECT title, description, priority, status, metadata, owner, acceptance_criteria, evidence_refs FROM soll.Requirement WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                8,
                            )?;
                        let q =
                            "UPDATE soll.Requirement SET title = ?, description = ?, priority = ?, status = ?, metadata = ?, owner = ?, acceptance_criteria = ?, evidence_refs = ?, updated_at = ? WHERE id = ?";
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
                                data.get("owner")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[5]),
                                data.get("acceptance_criteria")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[6].clone()),
                                data.get("evidence_refs")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| current[7].clone()),
                                now_unix_ms(),
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
                                    "SELECT title, description, context, rationale, status, metadata, supersedes_decision_id, impact_scope FROM soll.Decision WHERE id = '{}'",
                                    escape_sql(id)
                                ),
                                8,
                            )?;
                        let q = "UPDATE soll.Decision SET title = ?, description = ?, context = ?, rationale = ?, status = ?, metadata = ?, supersedes_decision_id = ?, impact_scope = ?, updated_at = ? WHERE id = ?";
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
                                data.get("supersedes_decision_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[6]),
                                data.get("impact_scope")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&current[7]),
                                now_unix_ms(),
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

impl McpServer {
    pub(crate) fn axon_soll_apply_plan(&self, args: &Value) -> Option<Value> {
        let project_slug = args
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let plan = args.get("plan")?;

        let mut created: Vec<Value> = Vec::new();
        let mut updated: Vec<Value> = Vec::new();
        let mut skipped: Vec<Value> = Vec::new();
        let mut errors: Vec<Value> = Vec::new();
        let mut id_map = serde_json::Map::new();

        self.apply_plan_entity_group(
            project_slug,
            "pillar",
            plan.get("pillars"),
            dry_run,
            &mut created,
            &mut updated,
            &mut skipped,
            &mut errors,
            &mut id_map,
        );
        self.apply_plan_entity_group(
            project_slug,
            "requirement",
            plan.get("requirements"),
            dry_run,
            &mut created,
            &mut updated,
            &mut skipped,
            &mut errors,
            &mut id_map,
        );
        self.apply_plan_entity_group(
            project_slug,
            "decision",
            plan.get("decisions"),
            dry_run,
            &mut created,
            &mut updated,
            &mut skipped,
            &mut errors,
            &mut id_map,
        );
        self.apply_plan_entity_group(
            project_slug,
            "milestone",
            plan.get("milestones"),
            dry_run,
            &mut created,
            &mut updated,
            &mut skipped,
            &mut errors,
            &mut id_map,
        );

        let summary = format!(
            "SOLL apply_plan {}: created={}, updated={}, skipped={}, errors={}",
            if dry_run { "DRY-RUN" } else { "APPLIED" },
            created.len(),
            updated.len(),
            skipped.len(),
            errors.len()
        );

        Some(json!({
            "content": [{ "type": "text", "text": summary }],
            "data": {
                "project_slug": project_slug,
                "dry_run": dry_run,
                "created": created,
                "updated": updated,
                "skipped": skipped,
                "errors": errors,
                "id_map": id_map
            },
            "isError": !errors.is_empty()
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_plan_entity_group(
        &self,
        project_slug: &str,
        entity: &str,
        items: Option<&Value>,
        dry_run: bool,
        created: &mut Vec<Value>,
        updated: &mut Vec<Value>,
        skipped: &mut Vec<Value>,
        errors: &mut Vec<Value>,
        id_map: &mut serde_json::Map<String, Value>,
    ) {
        let Some(items) = items.and_then(|v| v.as_array()) else {
            return;
        };

        for item in items {
            let Some(obj) = item.as_object() else {
                errors.push(json!({"entity": entity, "error": "item must be an object"}));
                continue;
            };

            let title = obj.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let logical_key = obj
                .get("logical_key")
                .and_then(|v| v.as_str())
                .unwrap_or(title);

            if logical_key.trim().is_empty() {
                errors.push(json!({"entity": entity, "error": "missing logical_key or title"}));
                continue;
            }

            let existing_id = self.resolve_soll_id(entity, title, logical_key);
            if let Some(existing_id) = existing_id {
                if dry_run {
                    skipped.push(json!({"entity": entity, "logical_key": logical_key, "id": existing_id, "action": "would_update"}));
                    id_map.insert(logical_key.to_string(), json!(existing_id));
                    continue;
                }

                let mut data = serde_json::Map::new();
                data.insert("id".to_string(), json!(existing_id));
                for (k, v) in obj {
                    if k != "logical_key" {
                        data.insert(k.clone(), v.clone());
                    }
                }

                let resp = self.axon_soll_manager(&json!({
                    "action": "update",
                    "entity": entity,
                    "data": Value::Object(data)
                }));

                if soll_tool_is_error(resp.as_ref()) {
                    errors.push(json!({"entity": entity, "logical_key": logical_key, "id": existing_id, "error": soll_tool_text(resp.as_ref())}));
                } else {
                    updated.push(json!({"entity": entity, "logical_key": logical_key, "id": existing_id}));
                    id_map.insert(logical_key.to_string(), json!(existing_id));
                }
                continue;
            }

            if dry_run {
                skipped.push(json!({"entity": entity, "logical_key": logical_key, "action": "would_create"}));
                continue;
            }

            let mut data = serde_json::Map::new();
            data.insert("project_slug".to_string(), json!(project_slug));
            for (k, v) in obj {
                if k != "logical_key" {
                    data.insert(k.clone(), v.clone());
                }
            }

            let mut metadata = data
                .get("metadata")
                .and_then(|m| m.as_object())
                .cloned()
                .unwrap_or_default();
            metadata.insert("logical_key".to_string(), json!(logical_key));
            data.insert("metadata".to_string(), Value::Object(metadata));

            let resp = self.axon_soll_manager(&json!({
                "action": "create",
                "entity": entity,
                "data": Value::Object(data)
            }));

            if soll_tool_is_error(resp.as_ref()) {
                errors.push(json!({"entity": entity, "logical_key": logical_key, "error": soll_tool_text(resp.as_ref())}));
            } else {
                let created_id = soll_tool_text(resp.as_ref())
                    .and_then(extract_soll_id_from_message)
                    .unwrap_or_else(|| "unknown".to_string());
                created.push(json!({"entity": entity, "logical_key": logical_key, "id": created_id}));
                id_map.insert(logical_key.to_string(), json!(created_id));
            }
        }
    }

    fn resolve_soll_id(&self, entity: &str, title: &str, logical_key: &str) -> Option<String> {
        let table = match entity {
            "pillar" => "soll.Pillar",
            "requirement" => "soll.Requirement",
            "decision" => "soll.Decision",
            "milestone" => "soll.Milestone",
            _ => return None,
        };

        let by_metadata = format!(
            "SELECT id FROM {} WHERE metadata LIKE '%\"logical_key\":\"{}\"%' ORDER BY id DESC LIMIT 1",
            table,
            escape_sql(logical_key)
        );
        if let Some(found) = query_first_sql_cell(self, &by_metadata) {
            return Some(found);
        }

        if !title.trim().is_empty() {
            let by_title = format!(
                "SELECT id FROM {} WHERE title = '{}' ORDER BY id DESC LIMIT 1",
                table,
                escape_sql(title)
            );
            if let Some(found) = query_first_sql_cell(self, &by_title) {
                return Some(found);
            }
        }

        None
    }
}

fn query_first_sql_cell(server: &McpServer, query: &str) -> Option<String> {
    let raw = server.execute_raw_sql(query).ok()?;
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    let rows = parsed.get("rows")?.as_array()?;
    let first = rows.first()?.as_array()?;
    first.first()?.as_str().map(|s| s.to_string())
}

fn soll_tool_text(resp: Option<&Value>) -> Option<String> {
    resp.and_then(|v| {
        v.get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|entry| entry.get("text"))
            .and_then(|text| text.as_str())
            .map(|s| s.to_string())
    })
}

fn soll_tool_is_error(resp: Option<&Value>) -> bool {
    resp.and_then(|v| v.get("isError"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn extract_soll_id_from_message(text: String) -> Option<String> {
    let start = text.find('`')?;
    let end = text[start + 1..].find('`')?;
    Some(text[start + 1..start + 1 + end].to_string())
}

impl McpServer {
    pub(crate) fn axon_soll_apply_plan_v2(&self, args: &Value) -> Option<Value> {
        let project_slug = args
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let author = args
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let plan = args.get("plan")?;

        let operations = self.build_plan_operations(project_slug, plan);
        let preview_id = format!("PRV-{}-{}", project_slug, now_unix_ms());
        let payload = json!({
            "project_slug": project_slug,
            "author": author,
            "dry_run": dry_run,
            "operations": operations
        });

        if let Err(e) = self.graph_store.execute_param(
            "INSERT INTO soll.RevisionPreview (preview_id, author, project_slug, payload, created_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (preview_id) DO UPDATE SET author = EXCLUDED.author, project_slug = EXCLUDED.project_slug, payload = EXCLUDED.payload, created_at = EXCLUDED.created_at",
            &json!([preview_id, author, project_slug, payload.to_string(), now_unix_ms()]),
        ) {
            return Some(json!({
                "content": [{"type":"text","text": format!("SOLL apply_plan_v2 error: {}", e)}],
                "isError": true
            }));
        }

        let counts = summarize_ops(&operations);
        if dry_run {
            return Some(json!({
                "content": [{"type":"text","text": format!("SOLL apply_plan_v2 DRY-RUN ready. preview_id={} (create={}, update={})", preview_id, counts.0, counts.1)}],
                "data": { "preview_id": preview_id, "counts": {"create": counts.0, "update": counts.1}, "operations": operations }
            }));
        }

        self.axon_soll_commit_revision(&json!({ "preview_id": preview_id, "author": author }))
    }

    pub(crate) fn axon_soll_commit_revision(&self, args: &Value) -> Option<Value> {
        let preview_id = match args.get("preview_id").and_then(|v| v.as_str()) {
            Some(v) if !v.trim().is_empty() => v,
            _ => {
                return Some(json!({
                    "content": [{"type":"text","text":"Missing required argument: preview_id"}],
                    "isError": true
                }));
            }
        };
        let author = args
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let preview_raw = match query_first_sql_cell(
            self,
            &format!(
                "SELECT payload FROM soll.RevisionPreview WHERE preview_id = '{}'",
                escape_sql(preview_id)
            ),
        ) {
            Some(v) => v,
            None => {
                return Some(json!({
                    "content": [{"type":"text","text": format!("Preview not found: {}", preview_id)}],
                    "isError": true
                }));
            }
        };
        let payload: Value = match serde_json::from_str(&preview_raw) {
            Ok(v) => v,
            Err(e) => {
                return Some(json!({
                    "content": [{"type":"text","text": format!("Invalid preview payload JSON: {}", e)}],
                    "isError": true
                }));
            }
        };
        let operations = payload
            .get("operations")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let project_slug = payload
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");

        let revision_id = format!("REV-{}-{}", project_slug, now_unix_ms());
        let now = now_unix_ms();
        let _ = self.graph_store.execute("BEGIN TRANSACTION");

        if let Err(e) = self.graph_store.execute_param(
            "INSERT INTO soll.Revision (revision_id, author, source, summary, status, created_at, committed_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            &json!([revision_id, author, "mcp", "SOLL plan commit", "committed", now, now]),
        ) {
            let _ = self.graph_store.execute("ROLLBACK");
            return Some(json!({"content":[{"type":"text","text": format!("SOLL commit error (revision row): {}", e)}],"isError": true}));
        }

        for op in &operations {
            if let Err(e) = self.apply_operation_with_audit(&revision_id, op) {
                let _ = self.graph_store.execute("ROLLBACK");
                return Some(json!({"content":[{"type":"text","text": format!("SOLL commit error (operation): {}", e)}],"isError": true}));
            }
        }

        let _ = self.graph_store.execute("COMMIT");
        let _ = self.graph_store.execute(&format!(
            "DELETE FROM soll.RevisionPreview WHERE preview_id = '{}'",
            escape_sql(preview_id)
        ));

        Some(json!({
            "content": [{"type":"text","text": format!("SOLL revision committed: {} ({} operations)", revision_id, operations.len())}],
            "data": {"revision_id": revision_id, "operations": operations.len()}
        }))
    }

    pub(crate) fn axon_soll_query_context(&self, args: &Value) -> Option<Value> {
        let project_slug = args
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25).max(1);

        let reqs = self.query_single_column(&format!(
            "SELECT id || '|' || title || '|' || COALESCE(status,'') || '|' || COALESCE(priority,'') FROM soll.Requirement WHERE id LIKE 'REQ-{}-%' ORDER BY id DESC LIMIT {}",
            escape_sql(project_slug),
            limit
        )).unwrap_or_default();
        let decisions = self.query_single_column(&format!(
            "SELECT id || '|' || title || '|' || COALESCE(status,'') FROM soll.Decision WHERE id LIKE 'DEC-{}-%' ORDER BY id DESC LIMIT {}",
            escape_sql(project_slug),
            limit
        )).unwrap_or_default();
        let revisions = self.query_single_column(&format!(
            "SELECT revision_id || '|' || COALESCE(summary,'') || '|' || COALESCE(author,'') FROM soll.Revision ORDER BY committed_at DESC LIMIT {}",
            limit
        )).unwrap_or_default();

        Some(json!({
            "content": [{"type":"text","text": format!("SOLL context for {} loaded.", project_slug)}],
            "data": {
                "project_slug": project_slug,
                "requirements": reqs,
                "decisions": decisions,
                "revisions": revisions
            }
        }))
    }

    pub(crate) fn axon_soll_attach_evidence(&self, args: &Value) -> Option<Value> {
        let entity_type = args.get("entity_type")?.as_str()?;
        let entity_id = args.get("entity_id")?.as_str()?;
        let artifacts = args.get("artifacts")?.as_array()?;
        let mut attached = 0usize;
        let now = now_unix_ms();

        for (idx, art) in artifacts.iter().enumerate() {
            let artifact_type = art
                .get("artifact_type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let artifact_ref = art
                .get("artifact_ref")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if artifact_ref.is_empty() {
                continue;
            }
            let confidence = art
                .get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.8);
            let metadata = art.get("metadata").cloned().unwrap_or(json!({})).to_string();
            let trace_id = format!("TRC-{}-{}-{}", entity_id, now, idx);

            if self.graph_store.execute_param(
                "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                &json!([trace_id, entity_type, entity_id, artifact_type, artifact_ref, confidence, metadata, now]),
            ).is_ok() {
                attached += 1;
            }
        }

        Some(json!({
            "content": [{"type":"text","text": format!("Attached {} evidence item(s) to {}:{}", attached, entity_type, entity_id)}],
            "data": {"attached": attached}
        }))
    }

    pub(crate) fn axon_soll_verify_requirements(&self, args: &Value) -> Option<Value> {
        let project_slug = args
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let query = format!(
            "SELECT r.id, COALESCE(r.status,''), COALESCE(r.acceptance_criteria,''), COUNT(t.id)
             FROM soll.Requirement r
             LEFT JOIN soll.Traceability t ON t.soll_entity_type = 'requirement' AND t.soll_entity_id = r.id
             WHERE r.id LIKE 'REQ-{}-%'
             GROUP BY 1,2,3
             ORDER BY r.id",
            escape_sql(project_slug)
        );
        let rows_raw = self.graph_store.query_json(&query).ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();
        let mut done = 0usize;
        let mut partial = 0usize;
        let mut missing = 0usize;
        let mut details: Vec<Value> = Vec::new();

        for row in rows {
            if row.len() < 4 {
                continue;
            }
            let id = row[0].clone();
            let status = row[1].clone();
            let criteria = row[2].clone();
            let evidence_count = row[3].parse::<usize>().unwrap_or(0);
            let has_criteria = !criteria.trim().is_empty() && criteria.trim() != "[]";

            let state = if evidence_count > 0 && has_criteria && (status == "current" || status == "accepted") {
                done += 1;
                "done"
            } else if evidence_count > 0 || has_criteria {
                partial += 1;
                "partial"
            } else {
                missing += 1;
                "missing"
            };

            details.push(json!({"id": id, "state": state, "status": status, "evidence_count": evidence_count}));
        }

        Some(json!({
            "content": [{"type":"text","text": format!("Requirement verification: done={}, partial={}, missing={}", done, partial, missing)}],
            "data": {"project_slug": project_slug, "done": done, "partial": partial, "missing": missing, "details": details}
        }))
    }

    pub(crate) fn axon_soll_rollback_revision(&self, args: &Value) -> Option<Value> {
        let revision_id = args.get("revision_id")?.as_str()?;
        let query = format!(
            "SELECT entity_type, entity_id, action, before_json, after_json
             FROM soll.RevisionChange
             WHERE revision_id = '{}'
             ORDER BY created_at DESC",
            escape_sql(revision_id)
        );
        let rows_raw = self.graph_store.query_json(&query).ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();

        let _ = self.graph_store.execute("BEGIN TRANSACTION");
        for row in rows {
            if row.len() < 5 {
                continue;
            }
            let entity_type = &row[0];
            let entity_id = &row[1];
            let action = &row[2];
            let before_json = &row[3];

            let op = if action == "create" {
                json!({"kind":"delete", "entity": entity_type, "entity_id": entity_id})
            } else {
                let before_val: Value = serde_json::from_str(before_json).unwrap_or(json!({}));
                json!({"kind":"restore", "entity": entity_type, "entity_id": entity_id, "before": before_val})
            };

            if let Err(e) = self.apply_rollback_operation(&op) {
                let _ = self.graph_store.execute("ROLLBACK");
                return Some(json!({"content":[{"type":"text","text": format!("Rollback failed: {}", e)}],"isError": true}));
            }
        }

        let _ = self.graph_store.execute("COMMIT");
        let _ = self.graph_store.execute(&format!(
            "UPDATE soll.Revision SET status = 'rolled_back' WHERE revision_id = '{}'",
            escape_sql(revision_id)
        ));
        Some(json!({"content":[{"type":"text","text": format!("Revision rolled back: {}", revision_id)}]}))
    }

    fn build_plan_operations(&self, project_slug: &str, plan: &Value) -> Vec<Value> {
        let mut operations = Vec::new();
        for entity in ["pillar", "requirement", "decision", "milestone"] {
            if let Some(items) = plan.get(format!("{}s", entity)).and_then(|v| v.as_array()) {
                for item in items {
                    if let Some(obj) = item.as_object() {
                        let title = obj.get("title").and_then(|v| v.as_str()).unwrap_or("");
                        let logical_key = obj
                            .get("logical_key")
                            .and_then(|v| v.as_str())
                            .unwrap_or(title);
                        if logical_key.is_empty() {
                            continue;
                        }
                        let existing_id = self.resolve_soll_id(entity, title, logical_key);
                        let kind = if existing_id.is_some() { "update" } else { "create" };
                        operations.push(json!({
                            "kind": kind,
                            "entity": entity,
                            "project_slug": project_slug,
                            "logical_key": logical_key,
                            "entity_id": existing_id,
                            "payload": Value::Object(obj.clone())
                        }));
                    }
                }
            }
        }
        operations
    }

    fn apply_operation_with_audit(&self, revision_id: &str, op: &Value) -> anyhow::Result<()> {
        let kind = op.get("kind").and_then(|v| v.as_str()).unwrap_or("create");
        let entity = op.get("entity").and_then(|v| v.as_str()).unwrap_or("requirement");
        let payload = op.get("payload").cloned().unwrap_or(json!({}));
        let project_slug = op
            .get("project_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let entity_id_hint = op
            .get("entity_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let before = if let Some(id) = entity_id_hint.clone() {
            self.snapshot_entity(entity, &id).unwrap_or(json!({}))
        } else {
            json!({})
        };

        let result = if kind == "update" && entity_id_hint.is_some() {
            let mut data = payload.clone();
            data["id"] = json!(entity_id_hint.clone().unwrap_or_default());
            self.axon_soll_manager(&json!({"action":"update","entity":entity,"data":data}))
        } else {
            let mut data = payload.clone();
            data["project_slug"] = json!(project_slug);
            self.axon_soll_manager(&json!({"action":"create","entity":entity,"data":data}))
        };

        if soll_tool_is_error(result.as_ref()) {
            return Err(anyhow!(
                "{}",
                soll_tool_text(result.as_ref()).unwrap_or_else(|| "unknown error".to_string())
            ));
        }

        let entity_id = if let Some(id) = entity_id_hint {
            id
        } else {
            soll_tool_text(result.as_ref())
                .and_then(extract_soll_id_from_message)
                .unwrap_or_else(|| "unknown".to_string())
        };

        let after = self.snapshot_entity(entity, &entity_id).unwrap_or(json!({}));
        self.graph_store.execute_param(
            "INSERT INTO soll.RevisionChange (revision_id, entity_type, entity_id, action, before_json, after_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            &json!([
                revision_id,
                entity,
                entity_id,
                kind,
                before.to_string(),
                after.to_string(),
                now_unix_ms()
            ]),
        )?;
        Ok(())
    }

    fn apply_rollback_operation(&self, op: &Value) -> anyhow::Result<()> {
        let kind = op.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let entity = op.get("entity").and_then(|v| v.as_str()).unwrap_or("");
        let entity_id = op.get("entity_id").and_then(|v| v.as_str()).unwrap_or("");

        match (kind, entity) {
            ("delete", "pillar") => self.graph_store.execute(&format!("DELETE FROM soll.Pillar WHERE id = '{}'", escape_sql(entity_id)))?,
            ("delete", "requirement") => self.graph_store.execute(&format!("DELETE FROM soll.Requirement WHERE id = '{}'", escape_sql(entity_id)))?,
            ("delete", "decision") => self.graph_store.execute(&format!("DELETE FROM soll.Decision WHERE id = '{}'", escape_sql(entity_id)))?,
            ("delete", "milestone") => self.graph_store.execute(&format!("DELETE FROM soll.Milestone WHERE id = '{}'", escape_sql(entity_id)))?,
            ("restore", _) => {
                let before = op.get("before").cloned().unwrap_or(json!({}));
                let mut data = before;
                data["id"] = json!(entity_id);
                let resp = self.axon_soll_manager(&json!({"action":"update","entity":entity,"data":data}));
                if soll_tool_is_error(resp.as_ref()) {
                    return Err(anyhow!("{}", soll_tool_text(resp.as_ref()).unwrap_or_default()));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn snapshot_entity(&self, entity: &str, entity_id: &str) -> Option<Value> {
        let query = match entity {
            "pillar" => format!("SELECT title, description, metadata FROM soll.Pillar WHERE id = '{}'", escape_sql(entity_id)),
            "requirement" => format!("SELECT title, description, status, priority, metadata, owner, acceptance_criteria, evidence_refs FROM soll.Requirement WHERE id = '{}'", escape_sql(entity_id)),
            "decision" => format!("SELECT title, description, context, rationale, status, metadata, supersedes_decision_id, impact_scope FROM soll.Decision WHERE id = '{}'", escape_sql(entity_id)),
            "milestone" => format!("SELECT title, status, metadata FROM soll.Milestone WHERE id = '{}'", escape_sql(entity_id)),
            _ => return None,
        };
        let raw = self.graph_store.query_json(&query).ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).ok()?;
        let first = rows.first()?;
        match entity {
            "pillar" => Some(json!({
                "title": first.first().cloned().unwrap_or_default(),
                "description": first.get(1).cloned().unwrap_or_default(),
                "metadata": first.get(2).cloned().unwrap_or_else(|| "{}".to_string())
            })),
            "requirement" => Some(json!({
                "title": first.first().cloned().unwrap_or_default(),
                "description": first.get(1).cloned().unwrap_or_default(),
                "status": first.get(2).cloned().unwrap_or_default(),
                "priority": first.get(3).cloned().unwrap_or_default(),
                "metadata": first.get(4).cloned().unwrap_or_else(|| "{}".to_string()),
                "owner": first.get(5).cloned().unwrap_or_default(),
                "acceptance_criteria": first.get(6).cloned().unwrap_or_else(|| "[]".to_string()),
                "evidence_refs": first.get(7).cloned().unwrap_or_else(|| "[]".to_string())
            })),
            "decision" => Some(json!({
                "title": first.first().cloned().unwrap_or_default(),
                "description": first.get(1).cloned().unwrap_or_default(),
                "context": first.get(2).cloned().unwrap_or_default(),
                "rationale": first.get(3).cloned().unwrap_or_default(),
                "status": first.get(4).cloned().unwrap_or_default(),
                "metadata": first.get(5).cloned().unwrap_or_else(|| "{}".to_string()),
                "supersedes_decision_id": first.get(6).cloned().unwrap_or_default(),
                "impact_scope": first.get(7).cloned().unwrap_or_default()
            })),
            "milestone" => Some(json!({
                "title": first.first().cloned().unwrap_or_default(),
                "status": first.get(1).cloned().unwrap_or_default(),
                "metadata": first.get(2).cloned().unwrap_or_else(|| "{}".to_string())
            })),
            _ => None,
        }
    }
}

fn summarize_ops(ops: &[Value]) -> (usize, usize) {
    let mut creates = 0usize;
    let mut updates = 0usize;
    for op in ops {
        match op.get("kind").and_then(|v| v.as_str()).unwrap_or("") {
            "create" => creates += 1,
            "update" => updates += 1,
            _ => {}
        }
    }
    (creates, updates)
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn parse_numeric_suffix(value: &str) -> Option<u64> {
    let head = value.split(':').next()?.trim();
    head.rsplit('-').next()?.parse::<u64>().ok()
}

fn escape_sql(value: &str) -> String {
    value.replace('\'', "''")
}
