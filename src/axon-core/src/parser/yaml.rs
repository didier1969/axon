use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use tree_sitter::Node;

static K8S_DOC_SPLIT: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?m)^---\s*$"#).unwrap());
static K8S_API_VERSION: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*apiVersion:\s*([^\s#]+)"#).unwrap());
static K8S_KIND: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?m)^\s*kind:\s*([^\s#]+)"#).unwrap());
static K8S_NAME: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?m)^\s*name:\s*([^\s#]+)"#).unwrap());
static K8S_NAMESPACE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*namespace:\s*([^\s#]+)"#).unwrap());
static K8S_CONFIG_MAP_REF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)(?:configMapKeyRef|configMapRef)[\s\S]*?name:\s*([^\s#]+)"#).unwrap()
});
static K8S_SECRET_REF: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)(?:secretKeyRef|secretRef)[\s\S]*?name:\s*([^\s#]+)"#).unwrap());

static GHA_WORKFLOW_NAME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*name:\s*['"]?([^'"\n]+)['"]?"#).unwrap());
static GHA_JOB_HEADER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^  ([a-zA-Z0-9_-]+):\s*$"#).unwrap());
static GHA_RUNS_ON: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*runs-on:\s*([^\s#]+)"#).unwrap());
static GHA_USES: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*(?:-\s*)?uses:\s*['"]?([^'"\s#]+)['"]?"#).unwrap());
static GHA_NEEDS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*needs:\s*\[?([a-zA-Z0-9_,\s-]+)\]?"#).unwrap());

pub struct YamlParser {
    wasm_bytes: &'static [u8],
}

impl YamlParser {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-yaml.wasm"),
        }
    }

    fn parse_kubernetes(content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let docs: Vec<&str> = K8S_DOC_SPLIT.split(content).collect();
        let mut line_offset = 1;

        for doc in docs {
            let doc_lines = doc.lines().count();
            if let (Some(api_cap), Some(kind_cap)) =
                (K8S_API_VERSION.captures(doc), K8S_KIND.captures(doc))
            {
                let api_version = api_cap
                    .get(1)
                    .unwrap()
                    .as_str()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string();
                let kind = kind_cap
                    .get(1)
                    .unwrap()
                    .as_str()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string();

                let name = K8S_NAME
                    .captures(doc)
                    .map(|c| {
                        c.get(1)
                            .unwrap()
                            .as_str()
                            .trim_matches('"')
                            .trim_matches('\'')
                            .to_string()
                    })
                    .unwrap_or_else(|| "unnamed".to_string());
                let ns = K8S_NAMESPACE.captures(doc).map(|c| {
                    c.get(1)
                        .unwrap()
                        .as_str()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string()
                });

                let full_name = match &ns {
                    Some(namespace) => format!("{}/{}", namespace, name),
                    None => name.clone(),
                };

                let mut props = HashMap::new();
                props.insert("api_version".to_string(), api_version);
                props.insert("k8s_kind".to_string(), kind.clone());
                if let Some(namespace) = ns {
                    props.insert("namespace".to_string(), namespace);
                }

                let is_entry = matches!(
                    kind.as_str(),
                    "Service"
                        | "Ingress"
                        | "Deployment"
                        | "StatefulSet"
                        | "DaemonSet"
                        | "CronJob"
                        | "Job"
                );

                symbols.push(Symbol {
                    name: full_name.clone(),
                    kind: format!("k8s_{}", kind.to_lowercase()),
                    start_line: line_offset,
                    end_line: line_offset + doc_lines.saturating_sub(1),
                    docstring: None,
                    is_entry_point: is_entry,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: props,
                    embedding: None,
                });

                // Detect ConfigMap references
                for cm_cap in K8S_CONFIG_MAP_REF.captures_iter(doc) {
                    let cm_name = cm_cap
                        .get(1)
                        .unwrap()
                        .as_str()
                        .trim_matches('"')
                        .trim_matches('\'');
                    relations.push(Relation {
                        from: full_name.clone(),
                        to: cm_name.to_string(),
                        rel_type: "references_config_map".to_string(),
                        properties: HashMap::new(),
                    });
                }

                // Detect Secret references
                for sec_cap in K8S_SECRET_REF.captures_iter(doc) {
                    let sec_name = sec_cap
                        .get(1)
                        .unwrap()
                        .as_str()
                        .trim_matches('"')
                        .trim_matches('\'');
                    relations.push(Relation {
                        from: full_name.clone(),
                        to: sec_name.to_string(),
                        rel_type: "references_secret".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
            line_offset += doc_lines + 1;
        }

        ExtractionResult {
            project_code: None,
            symbols,
            relations,
        }
    }

    fn parse_cicd(content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let lines: Vec<&str> = content.lines().collect();

        // GitHub Actions workflow parsing
        if content.contains("jobs:") && (content.contains("runs-on:") || content.contains("steps:"))
        {
            let wf_name = GHA_WORKFLOW_NAME
                .captures(content)
                .map(|c| c.get(1).unwrap().as_str().to_string())
                .unwrap_or_else(|| "CI Workflow".to_string());

            let mut wf_props = HashMap::new();
            wf_props.insert("ci_provider".to_string(), "github_actions".to_string());

            symbols.push(Symbol {
                name: wf_name.clone(),
                kind: "ci_workflow".to_string(),
                start_line: 1,
                end_line: lines.len(),
                docstring: None,
                is_entry_point: true,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: wf_props,
                embedding: None,
            });

            // Parse jobs inside the `jobs:` section
            let mut in_jobs = false;
            let mut current_job: Option<(String, usize, usize)> = None;
            let mut current_job_content = String::new();

            for (idx, line) in lines.iter().enumerate() {
                let line_no = idx + 1;
                if line.starts_with("jobs:") {
                    in_jobs = true;
                    continue;
                }
                if !in_jobs {
                    continue;
                }

                if let Some(cap) = GHA_JOB_HEADER.captures(line) {
                    // Flush previous job
                    if let Some((j_name, j_start, _)) = current_job.take() {
                        Self::flush_gha_job(
                            &wf_name,
                            &j_name,
                            j_start,
                            line_no.saturating_sub(1),
                            &current_job_content,
                            &mut symbols,
                            &mut relations,
                        );
                        current_job_content.clear();
                    }
                    let job_id = cap.get(1).unwrap().as_str().to_string();
                    current_job = Some((job_id, line_no, line_no));
                } else if current_job.is_some() {
                    current_job_content.push_str(line);
                    current_job_content.push('\n');
                }
            }

            if let Some((j_name, j_start, _)) = current_job.take() {
                Self::flush_gha_job(
                    &wf_name,
                    &j_name,
                    j_start,
                    lines.len(),
                    &current_job_content,
                    &mut symbols,
                    &mut relations,
                );
            }

            return ExtractionResult {
                project_code: None,
                symbols,
                relations,
            };
        }

        // GitLab CI parsing
        if content.contains("stages:") || content.contains("script:") {
            let mut current_job: Option<(String, usize)> = None;
            let mut job_content = String::new();

            for (idx, line) in lines.iter().enumerate() {
                let line_no = idx + 1;
                let trimmed = line.trim();
                if !line.starts_with(' ')
                    && !line.starts_with('\t')
                    && trimmed.ends_with(':')
                    && !trimmed.starts_with('#')
                {
                    if let Some((j_name, j_start)) = current_job.take() {
                        if job_content.contains("script:") {
                            Self::flush_gitlab_job(
                                &j_name,
                                j_start,
                                line_no.saturating_sub(1),
                                &job_content,
                                &mut symbols,
                                &mut relations,
                            );
                        }
                        job_content.clear();
                    }
                    let key = trimmed.trim_end_matches(':').trim().to_string();
                    if key != "stages" && key != "variables" && key != "default" && key != "include"
                    {
                        current_job = Some((key, line_no));
                    }
                } else if current_job.is_some() {
                    job_content.push_str(line);
                    job_content.push('\n');
                }
            }

            if let Some((j_name, j_start)) = current_job.take() {
                if job_content.contains("script:") {
                    Self::flush_gitlab_job(
                        &j_name,
                        j_start,
                        lines.len(),
                        &job_content,
                        &mut symbols,
                        &mut relations,
                    );
                }
            }

            return ExtractionResult {
                project_code: None,
                symbols,
                relations,
            };
        }

        ExtractionResult {
            project_code: None,
            symbols,
            relations,
        }
    }

    fn flush_gha_job(
        wf_name: &str,
        job_id: &str,
        start_line: usize,
        end_line: usize,
        body: &str,
        symbols: &mut Vec<Symbol>,
        relations: &mut Vec<Relation>,
    ) {
        let mut props = HashMap::new();
        props.insert("ci_provider".to_string(), "github_actions".to_string());
        if let Some(runs_on_cap) = GHA_RUNS_ON.captures(body) {
            props.insert(
                "runs_on".to_string(),
                runs_on_cap.get(1).unwrap().as_str().to_string(),
            );
        }

        symbols.push(Symbol {
            name: job_id.to_string(),
            kind: "ci_job".to_string(),
            start_line,
            end_line,
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
            from: wf_name.to_string(),
            to: job_id.to_string(),
            rel_type: "contains".to_string(),
            properties: HashMap::new(),
        });

        for uses_cap in GHA_USES.captures_iter(body) {
            let action = uses_cap.get(1).unwrap().as_str().to_string();
            relations.push(Relation {
                from: job_id.to_string(),
                to: action,
                rel_type: "uses_action".to_string(),
                properties: HashMap::new(),
            });
        }

        for needs_cap in GHA_NEEDS.captures_iter(body) {
            let deps = needs_cap.get(1).unwrap().as_str();
            for dep in deps.split(',') {
                let dep_trimmed = dep.trim().trim_matches('\'').trim_matches('"');
                if !dep_trimmed.is_empty() {
                    relations.push(Relation {
                        from: job_id.to_string(),
                        to: dep_trimmed.to_string(),
                        rel_type: "depends_on".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }
    }

    fn flush_gitlab_job(
        job_name: &str,
        start_line: usize,
        end_line: usize,
        body: &str,
        symbols: &mut Vec<Symbol>,
        relations: &mut Vec<Relation>,
    ) {
        let mut props = HashMap::new();
        props.insert("ci_provider".to_string(), "gitlab_ci".to_string());

        symbols.push(Symbol {
            name: job_name.to_string(),
            kind: "ci_job".to_string(),
            start_line,
            end_line,
            docstring: None,
            is_entry_point: false,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: props,
            embedding: None,
        });

        if let Some(needs_cap) = GHA_NEEDS.captures(body) {
            let deps = needs_cap.get(1).unwrap().as_str();
            for dep in deps.split(',') {
                let dep_trimmed = dep.trim().trim_matches('\'').trim_matches('"');
                if !dep_trimmed.is_empty() {
                    relations.push(Relation {
                        from: job_name.to_string(),
                        to: dep_trimmed.to_string(),
                        rel_type: "depends_on".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }
    }
}

impl Parser for YamlParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        if content.contains("openapi:") || content.contains("swagger:") {
            return super::openapi::OpenApiParser::new().parse(content);
        }

        if content.contains("apiVersion:") && content.contains("kind:") {
            return Self::parse_kubernetes(content);
        }

        if (content.contains("jobs:")
            && (content.contains("runs-on:") || content.contains("steps:")))
            || (content.contains("stages:") && content.contains("script:"))
        {
            return Self::parse_cicd(content);
        }

        let mut symbols = Vec::new();
        let relations = Vec::new();

        if let Some(tree) = parse_with_wasm_safe("yaml", self.wasm_bytes, content) {
            let root_node = tree.root_node();
            let source_bytes = content.as_bytes();

            fn traverse(
                node: Node,
                source_bytes: &[u8],
                symbols: &mut Vec<Symbol>,
                current_path: &str,
                depth: usize,
            ) {
                let kind = node.kind();

                if kind == "block_mapping_pair" || kind == "flow_mapping_pair" {
                    if let Some(key_node) = node.child_by_field_name("key") {
                        let mut key_name = String::new();

                        if let Ok(text) = key_node.utf8_text(source_bytes) {
                            key_name = text.trim().to_string();
                        }

                        if !key_name.is_empty() {
                            let full_name = if current_path.is_empty() {
                                key_name.clone()
                            } else {
                                format!("{}.{}", current_path, key_name)
                            };

                            let is_sensitive = ["secret", "password", "token", "key"]
                                .iter()
                                .any(|s| key_name.to_lowercase().contains(s));

                            let mut properties = HashMap::new();
                            if is_sensitive {
                                properties.insert("sensitive".to_string(), "true".to_string());
                            }

                            if depth <= 1 {
                                symbols.push(Symbol {
                                    name: full_name.clone(),
                                    kind: "config_key".to_string(),
                                    start_line: key_node.start_position().row + 1,
                                    end_line: key_node.end_position().row + 1,
                                    docstring: None,
                                    is_entry_point: false,
                                    is_public: true,
                                    tested: false,
                                    is_nif: false,
                                    is_unsafe: false,
                                    properties,
                                    embedding: None,
                                });
                            }

                            if let Some(value_node) = node.child_by_field_name("value") {
                                let mut cursor = value_node.walk();
                                for child in value_node.children(&mut cursor) {
                                    traverse(child, source_bytes, symbols, &full_name, depth + 1);
                                }
                            }
                        }
                    }
                } else {
                    let mut cursor = node.walk();
                    for child in node.children(&mut cursor) {
                        traverse(child, source_bytes, symbols, current_path, depth);
                    }
                }
            }

            traverse(root_node, source_bytes, &mut symbols, "", 0);
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
    fn test_kubernetes_manifests_parsing() {
        let k8s_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: axon-backend
  namespace: production
spec:
  replicas: 3
  template:
    spec:
      containers:
      - name: backend
        image: axon:latest
        env:
        - name: DB_PASSWORD
          valueFrom:
            secretKeyRef:
              name: db-credentials
              key: password
        - name: APP_CONFIG
          valueFrom:
            configMapKeyRef:
              name: app-settings
              key: config.json
---
apiVersion: v1
kind: Service
metadata:
  name: axon-service
  namespace: production
spec:
  type: ClusterIP
  ports:
  - port: 44127
"#;

        let parser = YamlParser::new();
        let result = parser.parse(k8s_yaml);

        let deployment = result
            .symbols
            .iter()
            .find(|s| s.name == "production/axon-backend")
            .expect("deployment symbol");
        assert_eq!(deployment.kind, "k8s_deployment");
        assert!(deployment.is_entry_point);
        assert_eq!(
            deployment.properties.get("api_version").map(String::as_str),
            Some("apps/v1")
        );
        assert_eq!(
            deployment.properties.get("namespace").map(String::as_str),
            Some("production")
        );

        let service = result
            .symbols
            .iter()
            .find(|s| s.name == "production/axon-service")
            .expect("service symbol");
        assert_eq!(service.kind, "k8s_service");
        assert!(service.is_entry_point);

        // Secret and ConfigMap relations
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "production/axon-backend"
                && r.to == "db-credentials"
                && r.rel_type == "references_secret"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "production/axon-backend"
                && r.to == "app-settings"
                && r.rel_type == "references_config_map"));
    }

    #[test]
    fn test_github_actions_workflow_parsing() {
        let gha_yaml = r#"
name: CI Pipeline
on:
  push:
    branches: [main]
  pull_request:

jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo fmt --check

  test:
    needs: [lint]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo test
"#;

        let parser = YamlParser::new();
        let result = parser.parse(gha_yaml);

        let wf = result
            .symbols
            .iter()
            .find(|s| s.name == "CI Pipeline")
            .expect("workflow symbol");
        assert_eq!(wf.kind, "ci_workflow");
        assert!(wf.is_entry_point);

        let lint_job = result
            .symbols
            .iter()
            .find(|s| s.name == "lint")
            .expect("lint job");
        assert_eq!(lint_job.kind, "ci_job");
        assert_eq!(
            lint_job.properties.get("runs_on").map(String::as_str),
            Some("ubuntu-latest")
        );

        let test_job = result
            .symbols
            .iter()
            .find(|s| s.name == "test")
            .expect("test job");
        assert_eq!(test_job.kind, "ci_job");

        // Relations
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "CI Pipeline" && r.to == "lint" && r.rel_type == "contains"));
        assert!(result.relations.iter().any(|r| r.from == "lint"
            && r.to == "actions/checkout@v4"
            && r.rel_type == "uses_action"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "test" && r.to == "lint" && r.rel_type == "depends_on"));
    }

    #[test]
    fn test_gitlab_ci_parsing() {
        let gitlab_yaml = r#"
stages:
  - build
  - test

compile:
  stage: build
  script:
    - cargo build --release

unit_tests:
  stage: test
  needs: [compile]
  script:
    - cargo test
"#;

        let parser = YamlParser::new();
        let result = parser.parse(gitlab_yaml);

        let compile = result
            .symbols
            .iter()
            .find(|s| s.name == "compile")
            .expect("compile job");
        assert_eq!(compile.kind, "ci_job");

        let unit_tests = result
            .symbols
            .iter()
            .find(|s| s.name == "unit_tests")
            .expect("unit_tests job");
        assert_eq!(unit_tests.kind, "ci_job");

        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "unit_tests" && r.to == "compile" && r.rel_type == "depends_on"));
    }
}
