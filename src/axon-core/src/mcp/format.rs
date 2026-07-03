pub(crate) fn format_table_from_json(json_res: &str, headers: &[&str]) -> String {
    let rows: Vec<Vec<serde_json::Value>> = match serde_json::from_str(json_res) {
        Ok(r) => r,
        Err(_) => return format!("Formatting error: {}", json_res),
    };

    if rows.is_empty() {
        return "No results found.".to_string();
    }

    let mut output = String::new();

    output.push('|');
    for h in headers {
        output.push_str(&format!(" {} |", h));
    }
    output.push('\n');

    output.push('|');
    for _ in headers {
        output.push_str(" --- |");
    }
    output.push('\n');

    for row in rows {
        output.push('|');
        for val in row {
            let clean_val = match val {
                serde_json::Value::Null => "null".to_string(),
                serde_json::Value::Bool(v) => v.to_string(),
                serde_json::Value::Number(v) => v.to_string(),
                serde_json::Value::String(v) => v,
                serde_json::Value::Array(v) => {
                    serde_json::to_string(&v).unwrap_or_else(|_| "[]".to_string())
                }
                serde_json::Value::Object(v) => {
                    serde_json::to_string(&v).unwrap_or_else(|_| "{}".to_string())
                }
            };
            output.push_str(&format!(" {} |", clean_val));
        }
        output.push('\n');
    }

    output
}

pub(crate) fn format_standard_contract(
    status: &str,
    summary: &str,
    scope: &str,
    evidence: &str,
    next_actions: &[&str],
    confidence: &str,
) -> String {
    let actions = if next_actions.is_empty() {
        "- none".to_string()
    } else {
        next_actions
            .iter()
            .map(|item| format!("- {}", item))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "**Status:** {}\n\
         **Summary:** {}\n\
         **Scope:** {}\n\
         **Confidence:** {}\n\n\
         ### Evidence\n{}\n\n\
         ### Next actions\n{}\n",
        status, summary, scope, confidence, evidence, actions
    )
}

pub(crate) fn evidence_by_mode(evidence: &str, mode: Option<&str>) -> String {
    let normalized = mode.unwrap_or("brief").to_ascii_lowercase();
    if normalized == "verbose" {
        return evidence.to_string();
    }
    let max_chars = 4000usize;
    if evidence.chars().count() <= max_chars {
        return evidence.to_string();
    }
    let mut end = evidence.len();
    for (count, (idx, _)) in evidence.char_indices().enumerate() {
        if count == max_chars {
            end = idx;
            break;
        }
    }
    let mut clipped = evidence[..end].to_string();
    clipped.push_str("\n\n[truncated=true, mode=brief, max_chars=4000]");
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ-AXO-902190 — format_standard_contract is the shared MCP output contract (24 callers,
    // a top uncovered hub). Cover its two branches: full render + the empty-actions "- none".
    #[test]
    fn format_standard_contract_renders_all_sections() {
        let out = format_standard_contract(
            "ok",
            "did the thing",
            "project:AXO",
            "commit abc123",
            &["run tests", "ship it"],
            "high",
        );
        assert!(out.contains("**Status:** ok"));
        assert!(out.contains("**Summary:** did the thing"));
        assert!(out.contains("**Scope:** project:AXO"));
        assert!(out.contains("**Confidence:** high"));
        assert!(out.contains("### Evidence\ncommit abc123"));
        assert!(out.contains("- run tests\n- ship it"));
    }

    #[test]
    fn format_standard_contract_empty_actions_renders_none() {
        let out = format_standard_contract("ok", "s", "sc", "e", &[], "low");
        assert!(out.contains("### Next actions\n- none"));
    }
}
