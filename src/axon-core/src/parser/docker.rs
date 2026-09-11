use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static FROM_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)^\s*FROM\s+(?:--platform=[^\s]+\s+)?([^\s]+)(?:\s+[aA][sS]\s+([^\s]+))?"#)
        .unwrap()
});

static COPY_FROM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)^\s*COPY\s+--from=([^\s]+)\s+(.+)$"#).unwrap());

static ENTRYPOINT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)^\s*ENTRYPOINT\s+(.+)$"#).unwrap());

static CMD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?i)^\s*CMD\s+(.+)$"#).unwrap());

static EXPOSE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?i)^\s*EXPOSE\s+(.+)$"#).unwrap());

static ENV_ARG_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)^\s*(?:ENV|ARG)\s+([a-zA-Z0-9_]+)(?:=|\s+)"#).unwrap());

pub struct DockerParser;

impl Default for DockerParser {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerParser {
    pub fn new() -> Self {
        Self
    }

    fn clean_command_str(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            // JSON array style: ["/bin/sh", "-c", "echo hi"]
            let inner = &trimmed[1..trimmed.len() - 1];
            let parts: Vec<String> = inner
                .split(',')
                .map(|p| p.trim().trim_matches('"').trim_matches('\'').to_string())
                .collect();
            parts.join(" ")
        } else {
            trimmed.to_string()
        }
    }
}

impl Parser for DockerParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let mut current_stage: Option<String> = None;
        let mut stages_defined: Vec<String> = Vec::new();
        let mut stage_index: usize = 0;

        for (line_idx, line) in content.lines().enumerate() {
            let line_num = line_idx + 1;
            let trimmed = line.trim();

            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            // 1. FROM <image> [AS <stage>]
            if let Some(cap) = FROM_RE.captures(trimmed) {
                let image = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                let stage_alias = cap.get(2).map(|m| m.as_str());

                let stage_name = if let Some(alias) = stage_alias {
                    alias.to_string()
                } else if !image.is_empty() {
                    format!("stage_{stage_index}")
                } else {
                    format!("stage_{stage_index}")
                };

                let mut props = HashMap::new();
                props.insert("base_image".to_string(), image.to_string());
                props.insert("stage_index".to_string(), stage_index.to_string());
                if let Some(alias) = stage_alias {
                    props.insert("stage_alias".to_string(), alias.to_string());
                }

                // If image matches a previously defined stage name, record builds_upon relation
                if stages_defined.contains(&image.to_string()) {
                    relations.push(Relation {
                        from: stage_name.clone(),
                        to: image.to_string(),
                        rel_type: "builds_upon".to_string(),
                        properties: HashMap::new(),
                    });
                }

                symbols.push(Symbol {
                    name: stage_name.clone(),
                    kind: "stage".to_string(),
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

                stages_defined.push(stage_name.clone());
                current_stage = Some(stage_name);
                stage_index += 1;
                continue;
            }

            let active_stage = current_stage
                .clone()
                .unwrap_or_else(|| "default".to_string());

            // 2. COPY --from=<stage> <src> <dest>
            if let Some(cap) = COPY_FROM_RE.captures(trimmed) {
                let from_stage = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                let rest = cap.get(2).map(|m| m.as_str()).unwrap_or("");
                let parts: Vec<&str> = rest.split_whitespace().collect();

                let mut props = HashMap::new();
                props.insert("kind".to_string(), "copy".to_string());
                if parts.len() >= 2 {
                    props.insert("src".to_string(), parts[0].to_string());
                    props.insert("dest".to_string(), parts[parts.len() - 1].to_string());
                }

                let target = if let Ok(idx) = from_stage.parse::<usize>() {
                    if idx < stages_defined.len() {
                        stages_defined[idx].clone()
                    } else {
                        from_stage.to_string()
                    }
                } else {
                    from_stage.to_string()
                };

                relations.push(Relation {
                    from: active_stage.clone(),
                    to: target,
                    rel_type: "references".to_string(),
                    properties: props,
                });
            }

            // 3. ENTRYPOINT & CMD
            if let Some(cap) = ENTRYPOINT_RE.captures(trimmed) {
                let raw_cmd = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                let cmd = Self::clean_command_str(raw_cmd);

                symbols.push(Symbol {
                    name: cmd.clone(),
                    kind: "entrypoint".to_string(),
                    start_line: line_num,
                    end_line: line_num,
                    docstring: None,
                    is_entry_point: true,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: HashMap::new(),
                    embedding: None,
                });

                relations.push(Relation {
                    from: active_stage.clone(),
                    to: cmd,
                    rel_type: "calls".to_string(),
                    properties: HashMap::new(),
                });
            } else if let Some(cap) = CMD_RE.captures(trimmed) {
                let raw_cmd = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                let cmd = Self::clean_command_str(raw_cmd);

                symbols.push(Symbol {
                    name: cmd.clone(),
                    kind: "entrypoint".to_string(),
                    start_line: line_num,
                    end_line: line_num,
                    is_entry_point: true,
                    docstring: None,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: HashMap::new(),
                    embedding: None,
                });

                relations.push(Relation {
                    from: active_stage.clone(),
                    to: cmd,
                    rel_type: "calls".to_string(),
                    properties: HashMap::new(),
                });
            }

            // 4. EXPOSE
            if let Some(cap) = EXPOSE_RE.captures(trimmed) {
                let raw_ports = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                for port_tok in raw_ports.split_whitespace() {
                    let port_name = format!("port_{port_tok}");
                    symbols.push(Symbol {
                        name: port_name.clone(),
                        kind: "port".to_string(),
                        start_line: line_num,
                        end_line: line_num,
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
                        from: active_stage.clone(),
                        to: port_name,
                        rel_type: "exposes".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }

            // 5. ENV / ARG
            if let Some(cap) = ENV_ARG_RE.captures(trimmed) {
                let var_name = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                if !var_name.is_empty() {
                    symbols.push(Symbol {
                        name: var_name.to_string(),
                        kind: "variable".to_string(),
                        start_line: line_num,
                        end_line: line_num,
                        docstring: None,
                        is_entry_point: false,
                        is_public: false,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: HashMap::new(),
                        embedding: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docker_multi_stage_build() {
        let dockerfile = r#"
        # Multi-stage Dockerfile
        FROM rust:1.75-alpine AS builder
        WORKDIR /app
        COPY . .
        RUN cargo build --release

        FROM builder AS tester
        RUN cargo test --release

        FROM alpine:3.19 AS runtime
        EXPOSE 8080 443
        COPY --from=builder /app/target/release/axon /usr/local/bin/axon
        ENTRYPOINT ["/usr/local/bin/axon", "start"]
        CMD ["--foreground"]
        "#;

        let parser = DockerParser::new();
        let res = parser.parse(dockerfile);

        let stages: Vec<&str> = res
            .symbols
            .iter()
            .filter(|s| s.kind == "stage")
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(stages, vec!["builder", "tester", "runtime"]);

        // Tester builds upon builder
        let builds_upon = res
            .relations
            .iter()
            .find(|r| r.rel_type == "builds_upon")
            .expect("builds_upon relation");
        assert_eq!(builds_upon.from, "tester");
        assert_eq!(builds_upon.to, "builder");

        // Runtime copies from builder
        let copy_ref = res
            .relations
            .iter()
            .find(|r| r.rel_type == "references" && r.from == "runtime")
            .expect("copy reference");
        assert_eq!(copy_ref.to, "builder");

        // Expose ports
        let ports: Vec<&str> = res
            .symbols
            .iter()
            .filter(|s| s.kind == "port")
            .map(|s| s.name.as_str())
            .collect();
        assert!(ports.contains(&"port_8080"));
        assert!(ports.contains(&"port_443"));

        // Entrypoint
        let ep = res
            .symbols
            .iter()
            .find(|s| s.kind == "entrypoint" && s.is_entry_point)
            .expect("entrypoint");
        assert!(ep.name.contains("/usr/local/bin/axon start"));
    }

    #[test]
    fn test_docker_single_stage_with_cmd() {
        let dockerfile = r#"
        FROM python:3.11-slim
        ENV PYTHONUNBUFFERED=1
        ARG APP_ENV=production
        WORKDIR /code
        COPY requirements.txt .
        RUN pip install -r requirements.txt
        COPY . .
        EXPOSE 5000
        CMD ["python", "app.py"]
        "#;

        let parser = DockerParser::new();
        let res = parser.parse(dockerfile);

        let stages: Vec<&str> = res
            .symbols
            .iter()
            .filter(|s| s.kind == "stage")
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(stages.len(), 1);

        let vars: Vec<&str> = res
            .symbols
            .iter()
            .filter(|s| s.kind == "variable")
            .map(|s| s.name.as_str())
            .collect();
        assert!(vars.contains(&"PYTHONUNBUFFERED"));
        assert!(vars.contains(&"APP_ENV"));

        let ep = res
            .symbols
            .iter()
            .find(|s| s.kind == "entrypoint")
            .expect("cmd entrypoint");
        assert_eq!(ep.name, "python app.py");
        assert!(ep.is_entry_point);
    }
}
