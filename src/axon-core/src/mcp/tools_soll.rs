use anyhow::anyhow;
use serde_json::{json, Value};

use super::McpServer;
use super::soll::{find_latest_soll_export, parse_soll_export, SollRestoreCounts};

impl McpServer {
    pub(crate) fn axon_soll_manager(&self, args: &Value) -> Option<Value> {
        let action = args.get("action")?.as_str()?;
        let entity = args.get("entity")?.as_str()?;
        let data = args.get("data")?;

        match action {
            "create" => {
                let project_slug = data.get("project_slug").and_then(|v| v.as_str()).unwrap_or("AXO");
                let reg_col = match entity {
                    "pillar" | "requirement" => "last_req",
                    "concept" => "last_cpt",
                    "decision" => "last_dec",
                    "milestone" => "last_mil",
                    "validation" => "last_val",
                    "stakeholder" => "id",
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
                    "SELECT 0".to_string()
                } else {
                    format!("INSERT INTO soll.Registry (project_slug, id, last_req, last_cpt, last_dec, last_mil, last_val) \
                             VALUES ('{0}', 'AXON_GLOBAL', 0, 0, 0, 0, 0) ON CONFLICT (project_slug) DO NOTHING; \
                             UPDATE soll.Registry SET {1} = {1} + 1 WHERE project_slug = '{0}' RETURNING {1}",
                             project_slug.replace("'", "''"), reg_col)
                };

                match self.graph_store.query_json(&update_query) {
                    Ok(res) => {
                        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
                        let formatted_id = if entity == "stakeholder" {
                            data.get("name")?.as_str()?.to_string()
                        } else {
                            let next_num: u64 = rows[0][0].parse().unwrap_or(0);
                            format!("{}-{}-{:03}", prefix, project_slug, next_num)
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
                            _ => Err(anyhow!("Unknown entity")),
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
                    _ => Err(anyhow!("Unknown entity")),
                };
                match update_res {
                    Ok(_) => Some(json!({ "content": [{ "type": "text", "text": format!("✅ Mise à jour réussie pour `{}`", id) }] })),
                    Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Erreur update: {}", e) }], "isError": true }))
                }
            },
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
                        _ => return Some(json!({ "content": [{ "type": "text", "text": format!("Erreur: Type de relation inconnu '{}'", r) }], "isError": true })),
                    }
                } else {
                    match (src.split('-').next().unwrap_or(""), tgt.split('-').next().unwrap_or("")) {
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

                let q = format!("INSERT INTO {} (source_id, target_id) VALUES (?, ?)", rel_table);
                match self.graph_store.execute_param(&q, &json!([src, tgt])) {
                    Ok(_) => Some(json!({ "content": [{ "type": "text", "text": format!("✅ Liaison établie : `{}` -> `{}` (via {})", src, tgt, rel_table) }] })),
                    Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Erreur liaison: {}", e) }], "isError": true }))
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
        if let Ok(res) = self.graph_store.query_json("SELECT title, description, goal, metadata FROM soll.Vision") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                let meta = r.get(3).cloned().unwrap_or_default();
                markdown.push_str(&format!("### {}\n**Description:** {}\n**Goal:** {}\n**Meta:** `{}`\n\n", r[0], r[1], r[2], meta));
            }
        }

        markdown.push_str("## 2. Piliers d'Architecture\n");
        if let Ok(res) = self.graph_store.query_json("SELECT id, title, description FROM soll.Pillar") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("* **{}** : {} ({})\n", r[0], r[1], r[2]));
            }
        }

        markdown.push_str("\n## 2b. Concepts\n");
        if let Ok(res) = self.graph_store.query_json("SELECT name, explanation, rationale FROM soll.Concept") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("* **{}** : {} ({})\n", r[0], r[1], r[2]));
            }
        }

        markdown.push_str("\n## 3. Jalons & Roadmap (Milestones)\n");
        if let Ok(res) = self.graph_store.query_json("SELECT id, title, status FROM soll.Milestone") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("### {} : {}\n*Statut :* `{}`\n\n", r[0], r[1], r[2]));
            }
        }

        markdown.push_str("## 4. Exigences & Rayon d'Impact (Requirements)\n");
        let req_query = "SELECT id, title, priority, description FROM soll.Requirement";
        if let Ok(res) = self.graph_store.query_json(req_query) {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("### {} - {}\n*Priorité :* `{}`\n*Description :* {}\n\n", r[0], r[1], r[2], r[3]));
            }
        }

        markdown.push_str("## 5. Registre des Décisions (ADR)\n");
        if let Ok(res) = self.graph_store.query_json("SELECT id, title, status, rationale FROM soll.Decision") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("### {}\n**Titre :** {}\n**Statut :** `{}`\n**Rationnel :** {}\n\n", r[0], r[1], r[2], r[3]));
            }
        }

        markdown.push_str("## 6. Preuves de Validation & Witness\n");
        if let Ok(res) = self.graph_store.query_json("SELECT id, method, result, timestamp FROM soll.Validation") {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for r in rows {
                markdown.push_str(&format!("*   `{}` : **{}** via `{}` (Certifié le {})\n", r[0], r[2], r[1], r[3]));
            }
        }

        let file_name = format!("SOLL_EXPORT_{}.md", datetime.format("%Y-%m-%d_%H%M%S"));
        let file_path = format!("docs/vision/{}", file_name);

        let _ = std::fs::create_dir_all("docs/vision");
        match std::fs::write(&file_path, &markdown) {
            Ok(_) => {
                let report = format!("✅ Exported to {}\n\n---\n\n{}", file_path, markdown.chars().take(300).collect::<String>());
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            },
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Erreur d'écriture: {}", e) }], "isError": true }))
        }
    }

    pub(crate) fn axon_restore_soll(&self, args: &Value) -> Option<Value> {
        let path = args.get("path").and_then(|v| v.as_str())
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
            "INSERT INTO soll.Registry (project_slug, id, last_req, last_cpt, last_dec, last_mil, last_val)
             VALUES ('AXO', 'AXON_GLOBAL', 0, 0, 0, 0, 0)
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
                })
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore vision error: {}", e) }], "isError": true }));
            }
            restored.vision += 1;
        }

        for pillar in restore.pillars {
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Pillar (id, title, description, metadata)
                 VALUES ($id, $title, $description, '{}')
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   description = EXCLUDED.description",
                &json!({"id": pillar.id, "title": pillar.title, "description": pillar.description})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore pillar error: {}", e) }], "isError": true }));
            }
            restored.pillars += 1;
        }

        for concept in restore.concepts {
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Concept (name, explanation, rationale, metadata)
                 VALUES ($name, $explanation, $rationale, '{}')
                 ON CONFLICT (name) DO UPDATE SET
                   explanation = EXCLUDED.explanation,
                   rationale = EXCLUDED.rationale",
                &json!({"name": concept.name, "explanation": concept.explanation, "rationale": concept.rationale})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore concept error: {}", e) }], "isError": true }));
            }
            restored.concepts += 1;
        }

        for milestone in restore.milestones {
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Milestone (id, title, status, metadata)
                 VALUES ($id, $title, $status, '{}')
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   status = EXCLUDED.status",
                &json!({"id": milestone.id, "title": milestone.title, "status": milestone.status})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore milestone error: {}", e) }], "isError": true }));
            }
            restored.milestones += 1;
        }

        for requirement in restore.requirements {
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Requirement (id, title, description, status, priority, metadata)
                 VALUES ($id, $title, $description, 'restored', $priority, '{}')
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   description = EXCLUDED.description,
                   priority = EXCLUDED.priority",
                &json!({"id": requirement.id, "title": requirement.title, "description": requirement.description, "priority": requirement.priority})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore requirement error: {}", e) }], "isError": true }));
            }
            restored.requirements += 1;
        }

        for decision in restore.decisions {
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Decision (id, title, description, context, rationale, status, metadata)
                 VALUES ($id, $title, '', '', $rationale, $status, '{}')
                 ON CONFLICT (id) DO UPDATE SET
                   title = EXCLUDED.title,
                   rationale = EXCLUDED.rationale,
                   status = EXCLUDED.status",
                &json!({"id": decision.id, "title": decision.title, "rationale": decision.rationale, "status": decision.status})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore decision error: {}", e) }], "isError": true }));
            }
            restored.decisions += 1;
        }

        for validation in restore.validations {
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Validation (id, method, result, timestamp, metadata)
                 VALUES ($id, $method, $result, $timestamp, '{}')
                 ON CONFLICT (id) DO UPDATE SET
                   method = EXCLUDED.method,
                   result = EXCLUDED.result,
                   timestamp = EXCLUDED.timestamp",
                &json!({"id": validation.id, "method": validation.method, "result": validation.result, "timestamp": validation.timestamp})
            ) {
                return Some(json!({ "content": [{ "type": "text", "text": format!("SOLL restore validation error: {}", e) }], "isError": true }));
            }
            restored.validations += 1;
        }

        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "### Restauration SOLL terminee\n\nSource: `{}`\n\nRestaure en mode merge:\n- Vision: {}\n- Pillars: {}\n- Concepts: {}\n- Milestones: {}\n- Requirements: {}\n- Decisions: {}\n- Validations: {}\n\nNote: ce chemin de restauration reconstruit les entites conceptuelles depuis le format Markdown officiel d'export. Les liaisons hierarchiques et metadonnees absentes de l'export restent hors perimetre.",
                    path,
                    restored.vision,
                    restored.pillars,
                    restored.concepts,
                    restored.milestones,
                    restored.requirements,
                    restored.decisions,
                    restored.validations
                )
            }]
        }))
    }
}
