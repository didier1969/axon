use super::{ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use toml::Value;

pub struct TomlParser;

impl Default for TomlParser {
    fn default() -> Self {
        Self::new()
    }
}

impl TomlParser {
    pub fn new() -> Self {
        Self
    }

    fn extract_cargo_manifest(
        table: &toml::map::Map<String, Value>,
        content: &str,
    ) -> Option<ExtractionResult> {
        let is_cargo = table.contains_key("package")
            || table.contains_key("dependencies")
            || table.contains_key("dev-dependencies")
            || table.contains_key("build-dependencies")
            || table.contains_key("workspace");

        if !is_cargo {
            return None;
        }

        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let package_name = if let Some(pkg) = table.get("package").and_then(|v| v.as_table()) {
            let name = pkg
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unnamed");
            let version = pkg.get("version").and_then(|v| v.as_str()).unwrap_or("");
            let edition = pkg.get("edition").and_then(|v| v.as_str()).unwrap_or("");

            let mut props = HashMap::new();
            props.insert("ecosystem".to_string(), "cargo".to_string());
            if !version.is_empty() {
                props.insert("version".to_string(), version.to_string());
            }
            if !edition.is_empty() {
                props.insert("edition".to_string(), edition.to_string());
            }

            symbols.push(Symbol {
                name: name.to_string(),
                kind: "package".to_string(),
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

            name.to_string()
        } else {
            "workspace".to_string()
        };

        // Parse dependency sections
        let dep_sections = [
            ("dependencies", "dependencies"),
            ("dev-dependencies", "dev-dependencies"),
            ("build-dependencies", "build-dependencies"),
        ];

        for (section_key, section_label) in dep_sections {
            if let Some(deps) = table.get(section_key).and_then(|v| v.as_table()) {
                Self::extract_cargo_deps_table(
                    deps,
                    section_label,
                    &package_name,
                    &mut symbols,
                    &mut relations,
                );
            }
        }

        // Workspace dependencies & members
        if let Some(ws) = table.get("workspace").and_then(|v| v.as_table()) {
            if let Some(ws_deps) = ws.get("dependencies").and_then(|v| v.as_table()) {
                Self::extract_cargo_deps_table(
                    ws_deps,
                    "workspace-dependencies",
                    &package_name,
                    &mut symbols,
                    &mut relations,
                );
            }

            if let Some(members) = ws.get("members").and_then(|v| v.as_array()) {
                for m in members {
                    if let Some(member_str) = m.as_str() {
                        symbols.push(Symbol {
                            name: member_str.to_string(),
                            kind: "workspace_member".to_string(),
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

                        relations.push(Relation {
                            from: package_name.clone(),
                            to: member_str.to_string(),
                            rel_type: "contains".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            }
        }

        let _ = content;
        Some(ExtractionResult {
            project_code: None,
            symbols,
            relations,
        })
    }

    fn extract_cargo_deps_table(
        deps: &toml::map::Map<String, Value>,
        section_label: &str,
        package_name: &str,
        symbols: &mut Vec<Symbol>,
        relations: &mut Vec<Relation>,
    ) {
        for (dep_name, val) in deps {
            let mut props = HashMap::new();
            props.insert("ecosystem".to_string(), "cargo".to_string());
            props.insert("section".to_string(), section_label.to_string());

            match val {
                Value::String(ver) => {
                    props.insert("version".to_string(), ver.clone());
                }
                Value::Table(tbl) => {
                    if let Some(ver) = tbl.get("version").and_then(|v| v.as_str()) {
                        props.insert("version".to_string(), ver.to_string());
                    }
                    if let Some(path) = tbl.get("path").and_then(|v| v.as_str()) {
                        props.insert("path".to_string(), path.to_string());
                    }
                    if let Some(git) = tbl.get("git").and_then(|v| v.as_str()) {
                        props.insert("git".to_string(), git.to_string());
                    }
                }
                _ => {}
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
                from: package_name.to_string(),
                to: dep_name.clone(),
                rel_type: "references".to_string(),
                properties: HashMap::new(),
            });
        }
    }

    fn extract_pyproject_manifest(
        table: &toml::map::Map<String, Value>,
    ) -> Option<ExtractionResult> {
        let is_pyproject = table.contains_key("project") || table.contains_key("tool");
        if !is_pyproject {
            return None;
        }

        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let mut pkg_name = "python-project".to_string();

        if let Some(project) = table.get("project").and_then(|v| v.as_table()) {
            if let Some(name) = project.get("name").and_then(|v| v.as_str()) {
                pkg_name = name.to_string();
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
                    properties: [("ecosystem".to_string(), "pypi".to_string())].into(),
                    embedding: None,
                });
            }

            if let Some(deps) = project.get("dependencies").and_then(|v| v.as_array()) {
                for dep in deps {
                    if let Some(dep_str) = dep.as_str() {
                        let clean_name = dep_str
                            .split(&['>', '<', '=', '~', ';', ' ', '['][..])
                            .next()
                            .unwrap_or(dep_str)
                            .trim();
                        symbols.push(Symbol {
                            name: clean_name.to_string(),
                            kind: "dependency".to_string(),
                            start_line: 1,
                            end_line: 1,
                            docstring: None,
                            is_entry_point: false,
                            is_public: true,
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: [
                                ("ecosystem".to_string(), "pypi".to_string()),
                                ("spec".to_string(), dep_str.to_string()),
                            ]
                            .into(),
                            embedding: None,
                        });
                        relations.push(Relation {
                            from: pkg_name.clone(),
                            to: clean_name.to_string(),
                            rel_type: "references".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            }
        }

        if symbols.is_empty() {
            None
        } else {
            Some(ExtractionResult {
                project_code: None,
                symbols,
                relations,
            })
        }
    }
}

impl Parser for TomlParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let value: Value = match toml::from_str(content) {
            Ok(v) => v,
            Err(_) => {
                return ExtractionResult::default();
            }
        };

        if let Some(table) = value.as_table() {
            if let Some(cargo_res) = Self::extract_cargo_manifest(table, content) {
                return cargo_res;
            }
            if let Some(py_res) = Self::extract_pyproject_manifest(table) {
                return py_res;
            }

            // Generic TOML tables and keys
            let mut symbols = Vec::new();
            for (key, val) in table {
                let kind = match val {
                    Value::Table(_) => "table",
                    Value::Array(_) => "array",
                    _ => "variable",
                };
                symbols.push(Symbol {
                    name: key.clone(),
                    kind: kind.to_string(),
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

            ExtractionResult {
                project_code: None,
                symbols,
                relations: Vec::new(),
            }
        } else {
            ExtractionResult::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cargo_toml_parsing() {
        let toml_str = r#"
        [package]
        name = "axon-core"
        version = "0.8.0"
        edition = "2021"

        [dependencies]
        tokio = { version = "1.36", features = ["full"] }
        serde = "1.0"
        rusqlite = { version = "0.39.0", features = ["bundled"] }

        [dev-dependencies]
        tempfile = "3.8"

        [workspace]
        members = ["src/axon-plugin-postgres"]
        "#;

        let parser = TomlParser::new();
        let res = parser.parse(toml_str);

        let pkg = res
            .symbols
            .iter()
            .find(|s| s.kind == "package")
            .expect("package symbol");
        assert_eq!(pkg.name, "axon-core");
        assert_eq!(
            pkg.properties.get("version").map(|s| s.as_str()),
            Some("0.8.0")
        );

        let deps: Vec<&str> = res
            .symbols
            .iter()
            .filter(|s| s.kind == "dependency")
            .map(|s| s.name.as_str())
            .collect();
        assert!(deps.contains(&"tokio"));
        assert!(deps.contains(&"serde"));
        assert!(deps.contains(&"rusqlite"));
        assert!(deps.contains(&"tempfile"));

        let refs: Vec<&str> = res
            .relations
            .iter()
            .filter(|r| r.rel_type == "references")
            .map(|r| r.to.as_str())
            .collect();
        assert!(refs.contains(&"tokio"));
        assert!(refs.contains(&"serde"));

        let ws_rel = res
            .relations
            .iter()
            .find(|r| r.rel_type == "contains")
            .expect("workspace contains member");
        assert_eq!(ws_rel.to, "src/axon-plugin-postgres");
    }

    #[test]
    fn test_pyproject_toml_parsing() {
        let toml_str = r#"
        [project]
        name = "my-fastapi-service"
        version = "0.1.0"
        dependencies = [
            "fastapi>=0.100.0",
            "uvicorn[standard]==0.24.0",
            "pydantic"
        ]
        "#;

        let parser = TomlParser::new();
        let res = parser.parse(toml_str);

        let pkg = res
            .symbols
            .iter()
            .find(|s| s.kind == "package")
            .expect("package symbol");
        assert_eq!(pkg.name, "my-fastapi-service");

        let deps: Vec<&str> = res
            .symbols
            .iter()
            .filter(|s| s.kind == "dependency")
            .map(|s| s.name.as_str())
            .collect();
        assert!(deps.contains(&"fastapi"));
        assert!(deps.contains(&"uvicorn"));
        assert!(deps.contains(&"pydantic"));
    }
}
