import re

with open("src/axon-core/src/mcp/tools_soll.rs", "r") as f:
    content = f.read()

# We will just replace snapshot_entity entirely.
# Let's find snapshot_entity.
start = content.find("fn snapshot_entity(&self")
end = content.find("fn format_soll_export", start)

new_func = """fn snapshot_entity(&self, entity: &str, entity_id: &str) -> Option<Value> {
        let query = match entity {
            "pillar" => format!("SELECT title, description, metadata FROM soll.Node WHERE type='Pillar' AND id = '{}'", escape_sql(entity_id)),
            "requirement" => format!("SELECT title, description, status, metadata FROM soll.Node WHERE type='Requirement' AND id = '{}'", escape_sql(entity_id)),
            "decision" => format!("SELECT title, description, status, metadata FROM soll.Node WHERE type='Decision' AND id = '{}'", escape_sql(entity_id)),
            "milestone" => format!("SELECT title, status, metadata FROM soll.Node WHERE type='Milestone' AND id = '{}'", escape_sql(entity_id)),
            "guideline" => format!("SELECT title, description, status, metadata FROM soll.Node WHERE type='Guideline' AND id = '{}'", escape_sql(entity_id)),
            "concept" => format!("SELECT title, description, metadata FROM soll.Node WHERE type='Concept' AND id = '{}'", escape_sql(entity_id)),
            "stakeholder" => format!("SELECT title, metadata FROM soll.Node WHERE type='Stakeholder' AND id = '{}'", escape_sql(entity_id)),
            "validation" => format!("SELECT status, metadata FROM soll.Node WHERE type='Validation' AND id = '{}'", escape_sql(entity_id)),
            _ => return None,
        };
        // USE query_json_writer so we can see uncommitted transactions!
        let raw = self.graph_store.query_json_writer(&query).ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).ok()?;
        let first = rows.first()?;
        match entity {
            "pillar" | "concept" => Some(json!({
                "title": first.get(0).cloned().unwrap_or_default(),
                "description": first.get(1).cloned().unwrap_or_default(),
                "metadata": first.get(2).cloned().unwrap_or_else(|| "{}".to_string())
            })),
            "requirement" | "decision" | "guideline" => Some(json!({
                "title": first.get(0).cloned().unwrap_or_default(),
                "description": first.get(1).cloned().unwrap_or_default(),
                "status": first.get(2).cloned().unwrap_or_default(),
                "metadata": first.get(3).cloned().unwrap_or_else(|| "{}".to_string())
            })),
            "milestone" => Some(json!({
                "title": first.get(0).cloned().unwrap_or_default(),
                "status": first.get(1).cloned().unwrap_or_default(),
                "metadata": first.get(2).cloned().unwrap_or_else(|| "{}".to_string())
            })),
            "stakeholder" => Some(json!({
                "role": first.get(0).cloned().unwrap_or_default(),
                "metadata": first.get(1).cloned().unwrap_or_else(|| "{}".to_string())
            })),
            "validation" => Some(json!({
                "result": first.get(0).cloned().unwrap_or_default(),
                "metadata": first.get(1).cloned().unwrap_or_else(|| "{}".to_string())
            })),
            _ => None
        }
    }

    """

content = content[:start] + new_func + content[end:]
with open("src/axon-core/src/mcp/tools_soll.rs", "w") as f:
    f.write(content)

