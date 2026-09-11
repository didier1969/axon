use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_ENV_LINE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$"#).expect("valid regex")
});

pub struct DotenvParser;

impl Default for DotenvParser {
    fn default() -> Self {
        Self::new()
    }
}

impl DotenvParser {
    pub fn new() -> Self {
        Self
    }

    fn is_secret_var_name(name: &str) -> bool {
        let upper = name.to_ascii_uppercase();
        upper.contains("SECRET")
            || upper.contains("KEY")
            || upper.contains("TOKEN")
            || upper.contains("PASSWORD")
            || upper.contains("PASSWD")
            || upper.contains("AUTH")
            || upper.contains("PRIVATE")
            || upper.contains("DATABASE_URL")
            || upper.contains("CREDENTIAL")
            || upper.contains("SALT")
    }

    fn is_placeholder_value(val: &str) -> bool {
        let clean = val.trim().trim_matches('"').trim_matches('\'').trim();
        if clean.is_empty() {
            return true;
        }
        let lower = clean.to_ascii_lowercase();
        lower.starts_with('<') && lower.ends_with('>')
            || lower.contains("changeme")
            || lower.contains("your_")
            || lower.contains("example")
            || lower.contains("todo")
            || lower.contains("replace_me")
    }
}

impl Parser for DotenvParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let mut var_count = 0usize;
        let mut secret_count = 0usize;

        for (idx, line) in content.lines().enumerate() {
            let line_num = idx + 1;
            let trimmed = line.trim();

            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            if let Some(cap) = RE_ENV_LINE.captures(trimmed) {
                let var_name = cap.get(1).map_or("", |m| m.as_str());
                let raw_val = cap.get(2).map_or("", |m| m.as_str());

                // Strip inline comment if not quoted
                let val = if raw_val.starts_with('"') || raw_val.starts_with('\'') {
                    raw_val
                } else {
                    raw_val.split('#').next().unwrap_or(raw_val).trim()
                };

                let is_secret = Self::is_secret_var_name(var_name);
                let has_default = !Self::is_placeholder_value(val);

                var_count += 1;
                if is_secret {
                    secret_count += 1;
                }

                let mut var_props = HashMap::new();
                var_props.insert(
                    "has_default".to_string(),
                    if has_default { "true" } else { "false" }.to_string(),
                );
                var_props.insert(
                    "is_secret".to_string(),
                    if is_secret { "true" } else { "false" }.to_string(),
                );
                var_props.insert("line".to_string(), line_num.to_string());

                symbols.push(Symbol {
                    name: var_name.to_string(),
                    kind: "env_var_decl".to_string(),
                    start_line: line_num,
                    end_line: line_num,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: is_secret,
                    properties: var_props,
                    embedding: None,
                });

                relations.push(Relation {
                    from: "env_contract".to_string(),
                    to: var_name.to_string(),
                    rel_type: "contains".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        let mut contract_props = HashMap::new();
        contract_props.insert("total_variables".to_string(), var_count.to_string());
        contract_props.insert("secret_variables".to_string(), secret_count.to_string());

        symbols.insert(
            0,
            Symbol {
                name: "env_contract".to_string(),
                kind: "env_contract".to_string(),
                start_line: 1,
                end_line: content.lines().count().max(1),
                docstring: None,
                is_entry_point: true,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: contract_props,
                embedding: None,
            },
        );

        ExtractionResult {
            project_code: None,
            symbols,
            relations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dotenv_contract_parsing() {
        let env_example = r#"
# Database Configuration
DATABASE_URL=postgres://user:password@localhost:5432/app_dev
PORT=4000

# Security Secrets
JWT_SECRET=<changeme_in_production>
API_KEY=your_api_key_here
SESSION_SALT=

# Flags
ENABLE_METRICS=true
"#;

        let parser = DotenvParser::new();
        let res = parser.parse(env_example);

        let contract = res
            .symbols
            .iter()
            .find(|s| s.kind == "env_contract")
            .unwrap();
        assert_eq!(
            contract
                .properties
                .get("total_variables")
                .map(|s| s.as_str()),
            Some("6")
        );
        assert_eq!(
            contract
                .properties
                .get("secret_variables")
                .map(|s| s.as_str()),
            Some("4")
        );

        let jwt_secret = res.symbols.iter().find(|s| s.name == "JWT_SECRET").unwrap();
        assert_eq!(jwt_secret.kind, "env_var_decl");
        assert_eq!(
            jwt_secret.properties.get("is_secret").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(
            jwt_secret.properties.get("has_default").map(|s| s.as_str()),
            Some("false")
        );

        let port_var = res.symbols.iter().find(|s| s.name == "PORT").unwrap();
        assert_eq!(
            port_var.properties.get("is_secret").map(|s| s.as_str()),
            Some("false")
        );
        assert_eq!(
            port_var.properties.get("has_default").map(|s| s.as_str()),
            Some("true")
        );

        assert!(res
            .relations
            .iter()
            .any(|r| r.from == "env_contract" && r.to == "JWT_SECRET" && r.rel_type == "contains"));
    }
}
