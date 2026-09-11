use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_PACKAGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*package\s+([a-zA-Z0-9_.]+)").expect("valid regex"));

static RE_IMPORT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*import\s+([a-zA-Z0-9_.]+)").expect("valid regex"));

static RE_DEFAULT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*default\s+([a-zA-Z0-9_]+)\s*(?::=|=)\s*([a-zA-Z0-9_]+)")
        .expect("valid regex")
});

static RE_RULE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?m)^\s*([a-zA-Z0-9_]+)(?:\[[^\]]*\])?(?:\s*(?::=|=)\s*[a-zA-Z0-9_]+)?(?:\s+if\b|\s*\{)",
    )
    .expect("valid regex")
});

pub struct RegoParser;

impl Default for RegoParser {
    fn default() -> Self {
        Self::new()
    }
}

impl RegoParser {
    pub fn new() -> Self {
        Self
    }

    fn is_casbin_config(content: &str) -> bool {
        content.contains("[request_definition]") && content.contains("[policy_definition]")
    }

    fn parse_casbin(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let relations = Vec::new();
        let mut props = HashMap::new();

        let mut current_section = String::new();
        let mut start_line = 1;
        let mut end_line = 1;

        for (idx, line) in content.lines().enumerate() {
            let line_num = idx + 1;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
                continue;
            }

            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                current_section = trimmed[1..trimmed.len() - 1].to_string();
                start_line = line_num;
                end_line = line_num;
            } else if !current_section.is_empty() {
                end_line = line_num;
                props.insert(format!("sec_{}", current_section), trimmed.to_string());
            }
        }

        props.insert("framework".to_string(), "casbin".to_string());
        symbols.push(Symbol {
            name: "casbin_policy_model".to_string(),
            kind: "casbin_model".to_string(),
            start_line,
            end_line,
            docstring: None,
            is_entry_point: true,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: props,
            embedding: None,
        });

        ExtractionResult {
            project_code: None,
            symbols,
            relations,
        }
    }
}

impl Parser for RegoParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        if Self::is_casbin_config(content) {
            return self.parse_casbin(content);
        }

        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let package_name = if let Some(cap) = RE_PACKAGE.captures(content) {
            cap.get(1).map_or("main", |m| m.as_str()).to_string()
        } else {
            "main".to_string()
        };

        let mut package_props = HashMap::new();
        package_props.insert("policy_engine".to_string(), "opa_rego".to_string());

        let mut is_default_deny = false;
        for cap in RE_DEFAULT.captures_iter(content) {
            let rule_name = cap.get(1).map_or("", |m| m.as_str());
            let default_val = cap.get(2).map_or("", |m| m.as_str());
            package_props.insert(format!("default_{}", rule_name), default_val.to_string());
            if rule_name == "allow" && default_val == "false" {
                is_default_deny = true;
            }
        }

        if is_default_deny {
            package_props.insert("is_default_deny".to_string(), "true".to_string());
        }

        // Add package symbol
        symbols.push(Symbol {
            name: package_name.clone(),
            kind: "policy_package".to_string(),
            start_line: 1,
            end_line: content.lines().count().max(1),
            docstring: None,
            is_entry_point: true,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: package_props,
            embedding: None,
        });

        // Extract imports
        for cap in RE_IMPORT.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                let target = m.as_str();
                relations.push(Relation {
                    from: package_name.clone(),
                    to: target.to_string(),
                    rel_type: "imports_policy".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        // Extract rules
        for cap in RE_RULE.captures_iter(content) {
            let Some(rule_match) = cap.get(1) else {
                continue;
            };
            let rule_name = rule_match.as_str();

            // Ignore keywords like package, import, default
            if rule_name == "package" || rule_name == "import" || rule_name == "default" {
                continue;
            }

            let full_rule_name = format!("{}.{}", package_name, rule_name);

            // Avoid duplicate symbols for multi-body rules
            if symbols.iter().any(|s| s.name == full_rule_name) {
                continue;
            }

            let rule_type = match rule_name {
                "allow" => "allow",
                "deny" => "deny",
                _ => "custom",
            };

            let mut rule_props = HashMap::new();
            rule_props.insert("rule_type".to_string(), rule_type.to_string());
            if is_default_deny && rule_name == "allow" {
                rule_props.insert("default_deny_enforced".to_string(), "true".to_string());
            }

            // Calculate start line
            let offset = rule_match.start();
            let line_num = content[..offset].chars().filter(|&c| c == '\n').count() + 1;

            symbols.push(Symbol {
                name: full_rule_name.clone(),
                kind: "policy_rule".to_string(),
                start_line: line_num,
                end_line: line_num,
                docstring: None,
                is_entry_point: rule_name == "allow" || rule_name == "main",
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: rule_props,
                embedding: None,
            });

            relations.push(Relation {
                from: package_name.clone(),
                to: full_rule_name,
                rel_type: "contains".to_string(),
                properties: HashMap::new(),
            });
        }

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
    fn test_rego_policy_parsing() {
        let rego_content = r#"
package authz.rbac

import data.roles
import future.keywords.in

default allow := false

# Allow admin users
allow if {
    input.user.role == "admin"
}

# Deny suspended users
deny if {
    input.user.is_suspended == true
}

custom_check {
    input.valid == true
}
"#;

        let parser = RegoParser::new();
        let res = parser.parse(rego_content);

        let pkg = res
            .symbols
            .iter()
            .find(|s| s.kind == "policy_package")
            .unwrap();
        assert_eq!(pkg.name, "authz.rbac");
        assert_eq!(
            pkg.properties.get("is_default_deny").map(|s| s.as_str()),
            Some("true")
        );

        let allow_rule = res
            .symbols
            .iter()
            .find(|s| s.name == "authz.rbac.allow")
            .unwrap();
        assert_eq!(allow_rule.kind, "policy_rule");
        assert_eq!(
            allow_rule.properties.get("rule_type").map(|s| s.as_str()),
            Some("allow")
        );
        assert!(allow_rule.is_entry_point);

        let deny_rule = res
            .symbols
            .iter()
            .find(|s| s.name == "authz.rbac.deny")
            .unwrap();
        assert_eq!(deny_rule.kind, "policy_rule");
        assert_eq!(
            deny_rule.properties.get("rule_type").map(|s| s.as_str()),
            Some("deny")
        );

        // Verify imports relation
        assert!(res.relations.iter().any(|r| r.from == "authz.rbac"
            && r.to == "data.roles"
            && r.rel_type == "imports_policy"));
    }

    #[test]
    fn test_casbin_model_parsing() {
        let casbin_content = r#"
[request_definition]
r = sub, obj, act

[policy_definition]
p = sub, obj, act

[role_definition]
g = _, _

[policy_effect]
e = some(where (p.eft == allow))

[matchers]
m = g(r.sub, p.sub) && r.obj == p.obj && r.act == p.act
"#;

        let parser = RegoParser::new();
        let res = parser.parse(casbin_content);

        assert_eq!(res.symbols.len(), 1);
        let model = &res.symbols[0];
        assert_eq!(model.kind, "casbin_model");
        assert_eq!(
            model.properties.get("framework").map(|s| s.as_str()),
            Some("casbin")
        );
        assert_eq!(
            model
                .properties
                .get("sec_request_definition")
                .map(|s| s.as_str()),
            Some("r = sub, obj, act")
        );
    }
}
