use super::{ExtractionResult, Parser, Relation, Symbol};
use serde_json::Value;
use std::collections::HashMap;

pub struct PackageJsonParser;

impl Default for PackageJsonParser {
    fn default() -> Self {
        Self::new()
    }
}

impl PackageJsonParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for PackageJsonParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let value: Value = match serde_json::from_str(content) {
            Ok(v) => v,
            Err(_) => return ExtractionResult::default(),
        };

        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let pkg_name = value
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("package")
            .to_string();

        let version = value.get("version").and_then(|v| v.as_str()).unwrap_or("");

        let mut pkg_props = HashMap::new();
        pkg_props.insert("ecosystem".to_string(), "npm".to_string());
        if !version.is_empty() {
            pkg_props.insert("version".to_string(), version.to_string());
        }

        symbols.push(Symbol {
            name: pkg_name.clone(),
            kind: "package".to_string(),
            start_line: 1,
            end_line: 1,
            docstring: None,
            is_entry_point: true,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: pkg_props,
            embedding: None,
        });

        // Parse scripts as entrypoints
        if let Some(scripts) = value.get("scripts").and_then(|v| v.as_object()) {
            for (script_name, script_cmd) in scripts {
                let cmd_str = script_cmd.as_str().unwrap_or("").to_string();
                let mut props = HashMap::new();
                props.insert("command".to_string(), cmd_str);

                symbols.push(Symbol {
                    name: script_name.clone(),
                    kind: "script".to_string(),
                    start_line: 1,
                    end_line: 1,
                    docstring: None,
                    is_entry_point: true,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: props,
                    embedding: None,
                });

                relations.push(Relation {
                    from: pkg_name.clone(),
                    to: script_name.clone(),
                    rel_type: "calls".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        // Parse dependencies
        let dep_sections = [
            ("dependencies", "dependencies"),
            ("devDependencies", "devDependencies"),
            ("peerDependencies", "peerDependencies"),
        ];

        for (sec_key, sec_label) in dep_sections {
            if let Some(deps) = value.get(sec_key).and_then(|v| v.as_object()) {
                for (dep_name, dep_ver) in deps {
                    let ver_str = dep_ver.as_str().unwrap_or("").to_string();
                    let mut props = HashMap::new();
                    props.insert("ecosystem".to_string(), "npm".to_string());
                    props.insert("section".to_string(), sec_label.to_string());
                    if !ver_str.is_empty() {
                        props.insert("version".to_string(), ver_str);
                    }

                    symbols.push(Symbol {
                        name: dep_name.clone(),
                        kind: "dependency".to_string(),
                        start_line: 1,
                        end_line: 1,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: props,
                        embedding: None,
                    });

                    relations.push(Relation {
                        from: pkg_name.clone(),
                        to: dep_name.clone(),
                        rel_type: "references".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }

        ExtractionResult {
            project_code: None,
            symbols,
            relations,
        }
    }
}

pub struct RequirementsTxtParser;

impl Default for RequirementsTxtParser {
    fn default() -> Self {
        Self::new()
    }
}

impl RequirementsTxtParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for RequirementsTxtParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        for (idx, line) in content.lines().enumerate() {
            let line_num = idx + 1;
            let trimmed = line.trim();

            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            // -r other.txt or -c constraints.txt
            if trimmed.starts_with("-r ") || trimmed.starts_with("--requirement ") {
                let target = trimmed
                    .trim_start_matches("-r ")
                    .trim_start_matches("--requirement ")
                    .trim();
                relations.push(Relation {
                    from: "requirements.txt".to_string(),
                    to: target.to_string(),
                    rel_type: "imports".to_string(),
                    properties: HashMap::new(),
                });
                continue;
            }

            // Pip options like --extra-index-url
            if trimmed.starts_with('-') {
                continue;
            }

            // Parse package name and version specifier
            // e.g. "flask>=2.0.1", "requests[security]==2.28.1", "torch"
            let clean_line = trimmed.split('#').next().unwrap_or(trimmed).trim();
            let name_end = clean_line
                .find(|c: char| {
                    c == '=' || c == '<' || c == '>' || c == '~' || c == '!' || c == ';' || c == ' '
                })
                .unwrap_or(clean_line.len());

            let mut pkg_name = clean_line[..name_end].trim();
            // Remove extras like [security]
            if let Some(extra_pos) = pkg_name.find('[') {
                pkg_name = pkg_name[..extra_pos].trim();
            }

            if pkg_name.is_empty() {
                continue;
            }

            let spec = clean_line[name_end..].trim();
            let mut props = HashMap::new();
            props.insert("ecosystem".to_string(), "pypi".to_string());
            if !spec.is_empty() {
                props.insert("spec".to_string(), spec.to_string());
            }

            symbols.push(Symbol {
                name: pkg_name.to_string(),
                kind: "dependency".to_string(),
                start_line: line_num,
                end_line: line_num,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: props,
                embedding: None,
            });

            relations.push(Relation {
                from: "requirements.txt".to_string(),
                to: pkg_name.to_string(),
                rel_type: "references".to_string(),
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
    fn test_package_json_parsing() {
        let json_str = r#"{
            "name": "my-frontend",
            "version": "1.0.0",
            "scripts": {
                "dev": "vite",
                "build": "vite build",
                "test": "vitest"
            },
            "dependencies": {
                "react": "^18.2.0",
                "react-dom": "^18.2.0"
            },
            "devDependencies": {
                "typescript": "^5.0.0",
                "vitest": "^0.34.0"
            }
        }"#;

        let parser = PackageJsonParser::new();
        let res = parser.parse(json_str);

        let pkg = res
            .symbols
            .iter()
            .find(|s| s.kind == "package")
            .expect("package symbol");
        assert_eq!(pkg.name, "my-frontend");
        assert_eq!(
            pkg.properties.get("version").map(|s| s.as_str()),
            Some("1.0.0")
        );

        let scripts: Vec<&str> = res
            .symbols
            .iter()
            .filter(|s| s.kind == "script")
            .map(|s| s.name.as_str())
            .collect();
        assert!(scripts.contains(&"dev"));
        assert!(scripts.contains(&"build"));
        assert!(scripts.contains(&"test"));
        for s in res.symbols.iter().filter(|s| s.kind == "script") {
            assert!(s.is_entry_point);
        }

        let deps: Vec<&str> = res
            .symbols
            .iter()
            .filter(|s| s.kind == "dependency")
            .map(|s| s.name.as_str())
            .collect();
        assert!(deps.contains(&"react"));
        assert!(deps.contains(&"react-dom"));
        assert!(deps.contains(&"typescript"));
        assert!(deps.contains(&"vitest"));

        let refs: Vec<&str> = res
            .relations
            .iter()
            .filter(|r| r.rel_type == "references")
            .map(|r| r.to.as_str())
            .collect();
        assert!(refs.contains(&"react"));
        assert!(refs.contains(&"typescript"));
    }

    #[test]
    fn test_requirements_txt_parsing() {
        let req_str = r#"
        # Core requirements
        -r base.txt
        flask>=2.0.1
        requests[security]==2.28.1
        pytest~=7.2.0
        gunicorn
        "#;

        let parser = RequirementsTxtParser::new();
        let res = parser.parse(req_str);

        let import_rel = res
            .relations
            .iter()
            .find(|r| r.rel_type == "imports")
            .expect("import relation");
        assert_eq!(import_rel.to, "base.txt");

        let deps: Vec<&str> = res
            .symbols
            .iter()
            .filter(|s| s.kind == "dependency")
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(deps, vec!["flask", "requests", "pytest", "gunicorn"]);

        let flask_sym = res.symbols.iter().find(|s| s.name == "flask").unwrap();
        assert_eq!(
            flask_sym.properties.get("spec").map(|s| s.as_str()),
            Some(">=2.0.1")
        );
    }
}
