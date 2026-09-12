use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_PACKAGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*package\s+([a-zA-Z0-9_.]+)").expect("valid regex"));

static RE_IMPORT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*import\s+([a-zA-Z0-9_.*]+)").expect("valid regex"));

static RE_GLOBAL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*global\s+([a-zA-Z0-9_.]+)\s+([a-zA-Z0-9_]+)").expect("valid regex")
});

static RE_DECLARE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?ms)^\s*declare\s+([A-Za-z0-9_]+)\s*\n(.*?)\n\s*end\b").expect("valid regex")
});

static RE_RULE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?ms)^\s*rule\s+(?:["']([^"']+)["']|([a-zA-Z0-9_]+))\s*\n(.*?)\bwhen\b(.*?)\bthen\b(.*?)\bend\b"#)
        .expect("valid regex")
});

static RE_QUERY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?ms)^\s*query\s+(?:["']([^"']+)["']|([a-zA-Z0-9_]+))\s*\n(.*?)\bend\b"#)
        .expect("valid regex")
});

pub struct DrlParser;

impl Default for DrlParser {
    fn default() -> Self {
        Self::new()
    }
}

impl DrlParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for DrlParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Package
        let package_name = if let Some(cap) = RE_PACKAGE.captures(content) {
            cap.get(1).map_or("", |m| m.as_str()).to_string()
        } else {
            String::new()
        };

        if !package_name.is_empty() {
            symbols.push(Symbol {
                name: package_name.clone(),
                kind: "drl_package".to_string(),
                start_line: 1,
                end_line: 1,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 2. Imports
        for cap in RE_IMPORT.captures_iter(content) {
            let imp = cap.get(1).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: if package_name.is_empty() {
                    "root".to_string()
                } else {
                    package_name.clone()
                },
                to: imp.to_string(),
                rel_type: "imports_drl".to_string(),
                properties: HashMap::new(),
            });
        }

        // 3. Globals
        for cap in RE_GLOBAL.captures_iter(content) {
            let gtype = cap.get(1).map_or("", |m| m.as_str());
            let gname = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("type".to_string(), gtype.to_string());

            symbols.push(Symbol {
                name: gname.to_string(),
                kind: "drl_global".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: props,
                embedding: None,
            });
        }

        // 4. Declared types with PII detection
        for cap in RE_DECLARE.captures_iter(content) {
            let tname = cap.get(1).map_or("", |m| m.as_str());
            let body = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut type_props = HashMap::new();
            let mut has_sensitive = false;

            for fline in body.lines() {
                let trimmed = fline.trim();
                if trimmed.is_empty()
                    || trimmed.starts_with("//")
                    || trimmed.starts_with('#')
                    || trimmed.starts_with('@')
                {
                    continue;
                }

                if let Some(col) = trimmed.find(':') {
                    let fname = trimmed[..col].trim();
                    let ftype = trimmed[col + 1..].trim();

                    if !fname.is_empty() {
                        let full_field_name = format!("{}.{}", tname, fname);
                        let mut fprops = HashMap::new();
                        fprops.insert("type".to_string(), ftype.to_string());

                        if let Some(kind) = super::is_sensitive_name(fname) {
                            has_sensitive = true;
                            fprops.insert("is_sensitive".to_string(), "true".to_string());
                            fprops.insert("pii_kind".to_string(), kind.to_string());
                        }

                        symbols.push(Symbol {
                            name: full_field_name.clone(),
                            kind: "drl_field".to_string(),
                            start_line: line,
                            end_line: line,
                            docstring: None,
                            is_entry_point: false,
                            is_public: true,
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: fprops,
                            embedding: None,
                        });

                        relations.push(Relation {
                            from: tname.to_string(),
                            to: full_field_name,
                            rel_type: "contains".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            }

            if has_sensitive {
                type_props.insert("has_sensitive_fields".to_string(), "true".to_string());
            }

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "drl_type".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: type_props,
                embedding: None,
            });
        }

        // 5. Rules
        for cap in RE_RULE.captures_iter(content) {
            let rname = cap
                .get(1)
                .or_else(|| cap.get(2))
                .map_or("rule", |m| m.as_str());
            let header = cap.get(3).map_or("", |m| m.as_str());
            let when_clause = cap.get(4).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut rule_props = HashMap::new();
            if header.contains("salience") {
                rule_props.insert("has_salience".to_string(), "true".to_string());
            }
            if header.contains("agenda-group") {
                rule_props.insert("has_agenda_group".to_string(), "true".to_string());
            }

            // Extract pattern classes matched in when clause
            for wline in when_clause.lines() {
                let trimmed = wline.trim();
                if let Some(paren_idx) = trimmed.find('(') {
                    let before_paren = trimmed[..paren_idx].trim();
                    let class_name = if let Some(col) = before_paren.find(':') {
                        before_paren[col + 1..].trim()
                    } else {
                        before_paren
                    };

                    if !class_name.is_empty()
                        && class_name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        relations.push(Relation {
                            from: rname.to_string(),
                            to: class_name.to_string(),
                            rel_type: "matches_fact".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            }

            symbols.push(Symbol {
                name: rname.to_string(),
                kind: "drl_rule".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: true,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: rule_props,
                embedding: None,
            });
        }

        // 6. Queries
        for cap in RE_QUERY.captures_iter(content) {
            let qname = cap
                .get(1)
                .or_else(|| cap.get(2))
                .map_or("query", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: qname.to_string(),
                kind: "drl_query".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
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
    fn test_drl_parser_basic_and_pii() {
        let code = r#"
package org.nexus.rules

import org.nexus.model.Account
import org.nexus.model.Transaction

global org.slf4j.Logger logger;

declare UserCredential
    username : String
    password_hash : String
    session_token : String
    risk_level : int
end

query "FindHighRiskUsers"
    UserCredential( risk_level > 5 )
end

rule "FlagSuspiciousWithdrawal"
    salience 100
    agenda-group "fraud"
    when
        $acc : Account( balance > 10000 )
        $tx : Transaction( amount > 5000 )
    then
        logger.warn("High withdrawal flag on account: " + $acc.getId());
end
"#;

        let parser = DrlParser::new();
        let res = parser.parse(code);

        assert!(res
            .symbols
            .iter()
            .any(|s| s.name == "org.nexus.rules" && s.kind == "drl_package"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "org.nexus.model.Account" && r.rel_type == "imports_drl"));

        let global_sym = res.symbols.iter().find(|s| s.name == "logger").unwrap();
        assert_eq!(global_sym.kind, "drl_global");

        let type_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "UserCredential")
            .unwrap();
        assert_eq!(
            type_sym
                .properties
                .get("has_sensitive_fields")
                .map(|s| s.as_str()),
            Some("true")
        );

        let pass_field = res
            .symbols
            .iter()
            .find(|s| s.name == "UserCredential.password_hash")
            .unwrap();
        assert_eq!(
            pass_field
                .properties
                .get("is_sensitive")
                .map(|s| s.as_str()),
            Some("true")
        );

        let tok_field = res
            .symbols
            .iter()
            .find(|s| s.name == "UserCredential.session_token")
            .unwrap();
        assert_eq!(
            tok_field.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let q_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "FindHighRiskUsers")
            .unwrap();
        assert_eq!(q_sym.kind, "drl_query");

        let rule_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "FlagSuspiciousWithdrawal")
            .unwrap();
        assert_eq!(rule_sym.kind, "drl_rule");
        assert!(rule_sym.is_entry_point);
        assert_eq!(
            rule_sym.properties.get("has_salience").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(
            rule_sym
                .properties
                .get("has_agenda_group")
                .map(|s| s.as_str()),
            Some("true")
        );

        assert!(res
            .relations
            .iter()
            .any(|r| r.from == "FlagSuspiciousWithdrawal"
                && r.to == "Account"
                && r.rel_type == "matches_fact"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.from == "FlagSuspiciousWithdrawal"
                && r.to == "Transaction"
                && r.rel_type == "matches_fact"));
    }
}
