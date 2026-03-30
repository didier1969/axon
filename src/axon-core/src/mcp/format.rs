pub(crate) fn format_table_from_json(json_res: &str, headers: &[&str]) -> String {
    let rows: Vec<Vec<String>> = match serde_json::from_str(json_res) {
        Ok(r) => r,
        Err(_) => return format!("Erreur de formatage : {}", json_res),
    };

    if rows.is_empty() {
        return "Aucun résultat trouvé.".to_string();
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
            let clean_val = val
                .trim_start_matches("String(\"")
                .trim_end_matches("\")")
                .trim_start_matches("Int64(")
                .trim_end_matches(")")
                .trim_start_matches("Boolean(")
                .trim_end_matches(")")
                .replace("\\\"", "\"");
            output.push_str(&format!(" {} |", clean_val));
        }
        output.push('\n');
    }

    output
}
