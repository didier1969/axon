use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_PREFIX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?im)^\s*(?:@prefix\s+|PREFIX\s+)([a-zA-Z0-9_-]*:)\s*<([^>]+)>\s*\.?\s*$"#)
        .expect("valid regex")
});

static RE_BASE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?im)^\s*(?:@base\s+|BASE\s+)<([^>]+)>\s*\.?\s*$"#).expect("valid regex")
});

static RE_QUERY_TYPE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?im)\b(SELECT|CONSTRUCT|DESCRIBE|ASK)\b").expect("valid regex"));

static RE_CLASS_DECL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*([a-zA-Z0-9_:-]+|<[^>]+>)\s+(?:a|rdf:type)\s+(?:owl:Class|rdfs:Class|<http://www.w3.org/2002/07/owl#Class>|<http://www.w3.org/2000/01/rdf-schema#Class>)").expect("valid regex")
});

static RE_PROPERTY_DECL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*([a-zA-Z0-9_:-]+|<[^>]+>)\s+(?:a|rdf:type)\s+(?:owl:ObjectProperty|owl:DatatypeProperty|rdf:Property|<http://www.w3.org/2002/07/owl#ObjectProperty>|<http://www.w3.org/2002/07/owl#DatatypeProperty>)").expect("valid regex")
});

static RE_TRIPLE_PATTERNS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"([a-zA-Z0-9_:-]+|<[^>]+>|\?[a-zA-Z0-9_]+)\s+([a-zA-Z0-9_:-]+|<[^>]+>)\s+([a-zA-Z0-9_:-]+|<[^>]+>|\?[a-zA-Z0-9_]+)\s*\."#).expect("valid regex")
});

static RE_PII: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(?:password|passwd|secret|token|api_key|apikey|private_key|auth)\s*(?:[:=]|\s)\s*['"]([^'"]+)['"]"#).expect("valid regex")
});

pub struct SparqlTurtleParser;

impl Default for SparqlTurtleParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SparqlTurtleParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for SparqlTurtleParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Base IRI
        for cap in RE_BASE.captures_iter(content) {
            let iri = cap.get(1).unwrap().as_str().to_string();
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("iri".to_string(), iri.clone());

            symbols.push(Symbol {
                name: iri,
                kind: "base".to_string(),
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

        // 2. Prefixes
        for cap in RE_PREFIX.captures_iter(content) {
            let prefix = cap
                .get(1)
                .unwrap()
                .as_str()
                .trim_end_matches(':')
                .to_string();
            let iri = cap.get(2).unwrap().as_str().to_string();

            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("iri".to_string(), iri);

            symbols.push(Symbol {
                name: if prefix.is_empty() {
                    ":".to_string()
                } else {
                    prefix
                },
                kind: "prefix".to_string(),
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

        // 3. Classes
        for cap in RE_CLASS_DECL.captures_iter(content) {
            let class_name = cap
                .get(1)
                .unwrap()
                .as_str()
                .trim_matches(&['<', '>'][..])
                .to_string();
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: class_name,
                kind: "class".to_string(),
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

        // 4. Properties
        for cap in RE_PROPERTY_DECL.captures_iter(content) {
            let prop_name = cap
                .get(1)
                .unwrap()
                .as_str()
                .trim_matches(&['<', '>'][..])
                .to_string();
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: prop_name,
                kind: "property".to_string(),
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

        // 5. Query Types (SPARQL)
        for cap in RE_QUERY_TYPE.captures_iter(content) {
            let qtype = cap.get(1).unwrap().as_str().to_uppercase();
            let line = start_offset_line(cap.get(0).unwrap().start());

            if !symbols
                .iter()
                .any(|s| s.name == qtype && s.start_line == line)
            {
                symbols.push(Symbol {
                    name: qtype,
                    kind: "query".to_string(),
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
        }

        // 6. SubClass & SubProperty Relations (line-by-line Turtle subject-aware)
        let mut current_subject: Option<String> = None;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed.starts_with("@prefix")
                || trimmed.starts_with("PREFIX")
                || trimmed.starts_with("@base")
                || trimmed.starts_with("BASE")
            {
                continue;
            }

            let is_unindented = !line.starts_with("  ") && !line.starts_with('\t');
            if is_unindented || current_subject.is_none() {
                if let Some(s) = trimmed.split_whitespace().next() {
                    if s != ";"
                        && !s.starts_with("rdfs:")
                        && !s.starts_with("owl:")
                        && !s.starts_with("rdf:")
                        && s != "a"
                    {
                        current_subject = Some(s.trim_matches(&['<', '>'][..]).to_string());
                    }
                }
            }

            if let Some(ref subj) = current_subject {
                if trimmed.contains("rdfs:subClassOf") {
                    if let Some(tail) = trimmed.split("rdfs:subClassOf").nth(1) {
                        let target = tail
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .trim_matches(&['<', '>', ';', '.'][..]);
                        if !target.is_empty() {
                            relations.push(Relation {
                                from: subj.clone(),
                                to: target.to_string(),
                                rel_type: "SUBCLASS_OF".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }
                } else if trimmed.contains("rdfs:subPropertyOf") {
                    if let Some(tail) = trimmed.split("rdfs:subPropertyOf").nth(1) {
                        let target = tail
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .trim_matches(&['<', '>', ';', '.'][..]);
                        if !target.is_empty() {
                            relations.push(Relation {
                                from: subj.clone(),
                                to: target.to_string(),
                                rel_type: "SUBPROPERTY_OF".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }
                }
            }

            if trimmed.ends_with('.') {
                current_subject = None;
            }
        }

        // 8. Triples
        for cap in RE_TRIPLE_PATTERNS.captures_iter(content) {
            let s = cap
                .get(1)
                .unwrap()
                .as_str()
                .trim_matches(&['<', '>'][..])
                .to_string();
            let p = cap
                .get(2)
                .unwrap()
                .as_str()
                .trim_matches(&['<', '>'][..])
                .to_string();
            let o = cap
                .get(3)
                .unwrap()
                .as_str()
                .trim_matches(&['<', '>'][..])
                .to_string();

            if !s.starts_with('?')
                && !o.starts_with('?')
                && p != "a"
                && p != "rdf:type"
                && p != "rdfs:subClassOf"
                && p != "rdfs:subPropertyOf"
            {
                if !relations
                    .iter()
                    .any(|r| r.from == s && r.to == o && r.rel_type == p)
                {
                    relations.push(Relation {
                        from: s,
                        to: o,
                        rel_type: p,
                        properties: HashMap::new(),
                    });
                }
            }
        }

        // 9. PII detection
        for cap in RE_PII.captures_iter(content) {
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("is_sensitive".to_string(), "true".to_string());
            props.insert("hardcoded_secret".to_string(), "true".to_string());

            symbols.push(Symbol {
                name: "credential".to_string(),
                kind: "secret".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: false,
                tested: false,
                is_nif: false,
                is_unsafe: true,
                properties: props,
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
    fn test_sparql_turtle_parser_basic_and_pii() {
        let code = r#"
        @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix owl: <http://www.w3.org/2002/07/owl#> .
        @prefix ex: <http://example.org/> .

        ex:Person a owl:Class .
        ex:Employee a owl:Class ;
            rdfs:subClassOf ex:Person .

        ex:worksFor a owl:ObjectProperty .

        ex:alice ex:worksFor ex:AcmeCorp .

        # Secret credential
        ex:alice ex:password "sparql_secret_token_456" .
        "#;

        let parser = SparqlTurtleParser::new();
        let result = parser.parse(code);

        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "prefix" && s.name == "ex"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "class" && s.name == "ex:Person"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "class" && s.name == "ex:Employee"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "property" && s.name == "ex:worksFor"));

        assert!(result.relations.iter().any(|r| r.from == "ex:Employee"
            && r.to == "ex:Person"
            && r.rel_type == "SUBCLASS_OF"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "ex:alice" && r.to == "ex:AcmeCorp" && r.rel_type == "ex:worksFor"));

        let sec = result.symbols.iter().find(|s| s.kind == "secret").unwrap();
        assert_eq!(
            sec.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );
    }

    #[test]
    fn test_sparql_query() {
        let query = r#"
        PREFIX foaf: <http://xmlns.com/foaf/0.1/>
        SELECT ?name ?email
        WHERE {
            ?person a foaf:Person .
            ?person foaf:name ?name .
            ?person foaf:mbox ?email .
        }
        "#;

        let parser = SparqlTurtleParser::new();
        let result = parser.parse(query);

        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "prefix" && s.name == "foaf"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "query" && s.name == "SELECT"));
    }
}
