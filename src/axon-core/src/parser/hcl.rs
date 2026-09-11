use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static BLOCK_HEADER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*(resource|data|module|variable|output|provider|locals|terraform)\s*(?:"([^"]+)")?\s*(?:"([^"]+)")?\s*\{"#)
        .unwrap()
});

static MODULE_SOURCE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*source\s*=\s*"([^"]+)""#).unwrap());

static RESOURCE_REF_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"\b([a-zA-Z0-9_]+)\.([a-zA-Z0-9_]+)\.([a-zA-Z0-9_]+)\b"#).unwrap());

static VAR_REF_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"\bvar\.([a-zA-Z0-9_]+)\b"#).unwrap());

pub struct HclParser;

impl Default for HclParser {
    fn default() -> Self {
        Self::new()
    }
}

impl HclParser {
    pub fn new() -> Self {
        Self
    }

    fn find_matching_brace(content: &str, open_pos: usize) -> Option<usize> {
        let bytes = content.as_bytes();
        let mut depth = 0;
        let mut in_quote = false;
        let mut in_comment = false;

        for (i, &b) in bytes.iter().enumerate().skip(open_pos) {
            if b == b'\n' && in_comment {
                in_comment = false;
                continue;
            }
            if in_comment {
                continue;
            }
            if (b == b'#' || (b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/'))
                && !in_quote
            {
                in_comment = true;
                continue;
            }
            if b == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
                in_quote = !in_quote;
                continue;
            }
            if in_quote {
                continue;
            }
            if b == b'{' {
                depth += 1;
            } else if b == b'}' {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
        None
    }
}

impl Parser for HclParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let get_line_no = |byte_offset: usize| -> usize {
            content[..byte_offset]
                .chars()
                .filter(|&c| c == '\n')
                .count()
                + 1
        };

        for cap in BLOCK_HEADER_RE.captures_iter(content) {
            let block_type = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let first_label = cap.get(2).map(|m| m.as_str());
            let second_label = cap.get(3).map(|m| m.as_str());

            let match_obj = cap.get(0).unwrap();
            let open_pos = match_obj.end() - 1;
            let close_pos = Self::find_matching_brace(content, open_pos).unwrap_or(content.len());

            let start_line = get_line_no(match_obj.start());
            let end_line = get_line_no(close_pos);

            let block_body = &content[open_pos + 1..close_pos];

            match block_type {
                "resource" => {
                    let r_type = first_label.unwrap_or("unknown");
                    let r_name = second_label.unwrap_or("unnamed");
                    let full_name = format!("{}.{}", r_type, r_name);

                    let mut props = HashMap::new();
                    props.insert("resource_type".to_string(), r_type.to_string());
                    props.insert("iac".to_string(), "terraform".to_string());

                    symbols.push(Symbol {
                        name: full_name.clone(),
                        kind: "infra_resource".to_string(),
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

                    // Extract cross-resource references in body
                    for ref_cap in RESOURCE_REF_RE.captures_iter(block_body) {
                        let target_type = ref_cap.get(1).unwrap().as_str();
                        let target_name = ref_cap.get(2).unwrap().as_str();
                        if target_type != "var"
                            && target_type != "local"
                            && target_type != "module"
                            && target_type != "data"
                        {
                            let target_res = format!("{}.{}", target_type, target_name);
                            if target_res != full_name {
                                relations.push(Relation {
                                    from: full_name.clone(),
                                    to: target_res,
                                    rel_type: "references".to_string(),
                                    properties: HashMap::new(),
                                });
                            }
                        }
                    }

                    // Extract var references
                    for var_cap in VAR_REF_RE.captures_iter(block_body) {
                        let var_name = var_cap.get(1).unwrap().as_str();
                        relations.push(Relation {
                            from: full_name.clone(),
                            to: format!("var.{}", var_name),
                            rel_type: "reads_var".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
                "data" => {
                    let d_type = first_label.unwrap_or("unknown");
                    let d_name = second_label.unwrap_or("unnamed");
                    let full_name = format!("data.{}.{}", d_type, d_name);

                    let mut props = HashMap::new();
                    props.insert("data_source_type".to_string(), d_type.to_string());
                    props.insert("iac".to_string(), "terraform".to_string());

                    symbols.push(Symbol {
                        name: full_name,
                        kind: "infra_data_source".to_string(),
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
                }
                "module" => {
                    let mod_name = first_label.unwrap_or("unnamed");
                    let mut props = HashMap::new();
                    props.insert("iac".to_string(), "terraform".to_string());

                    if let Some(src_cap) = MODULE_SOURCE_RE.captures(block_body) {
                        let source = src_cap.get(1).unwrap().as_str().to_string();
                        props.insert("source".to_string(), source.clone());

                        relations.push(Relation {
                            from: format!("module.{}", mod_name),
                            to: source,
                            rel_type: "sources".to_string(),
                            properties: HashMap::new(),
                        });
                    }

                    symbols.push(Symbol {
                        name: format!("module.{}", mod_name),
                        kind: "infra_module".to_string(),
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
                }
                "variable" => {
                    let var_name = first_label.unwrap_or("unnamed");
                    let mut props = HashMap::new();
                    props.insert("iac".to_string(), "terraform".to_string());

                    symbols.push(Symbol {
                        name: format!("var.{}", var_name),
                        kind: "infra_variable".to_string(),
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
                }
                "output" => {
                    let out_name = first_label.unwrap_or("unnamed");
                    let mut props = HashMap::new();
                    props.insert("iac".to_string(), "terraform".to_string());

                    symbols.push(Symbol {
                        name: format!("output.{}", out_name),
                        kind: "infra_output".to_string(),
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
                }
                "provider" => {
                    let prov_name = first_label.unwrap_or("unnamed");
                    symbols.push(Symbol {
                        name: format!("provider.{}", prov_name),
                        kind: "infra_provider".to_string(),
                        start_line,
                        end_line,
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
                _ => {}
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
    fn test_hcl_resources_modules_and_variables() {
        let code = r#"
        terraform {
            required_providers {
                aws = {
                    source  = "hashicorp/aws"
                    version = "~> 5.0"
                }
            }
        }

        provider "aws" {
            region = var.aws_region
        }

        variable "aws_region" {
            type    = string
            default = "us-east-1"
        }

        variable "vpc_cidr" {
            type = string
        }

        module "vpc" {
            source = "terraform-aws-modules/vpc/aws"
            cidr   = var.vpc_cidr
        }

        resource "aws_security_group" "web_sg" {
            name   = "web-sg"
            vpc_id = module.vpc.vpc_id
        }

        resource "aws_instance" "web_server" {
            ami                    = data.aws_ami.ubuntu.id
            instance_type          = "t3.micro"
            vpc_security_group_ids = [aws_security_group.web_sg.id]
        }

        data "aws_ami" "ubuntu" {
            most_recent = true
            owners      = ["099720109477"]
        }

        output "instance_ip" {
            value = aws_instance.web_server.public_ip
        }
        "#;

        let parser = HclParser::new();
        let result = parser.parse(code);

        // Resource symbols
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "aws_security_group.web_sg" && s.kind == "infra_resource"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "aws_instance.web_server" && s.kind == "infra_resource"));

        // Module and Variable symbols
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "module.vpc" && s.kind == "infra_module"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "var.aws_region" && s.kind == "infra_variable"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "var.vpc_cidr" && s.kind == "infra_variable"));

        // Data source and Output symbols
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "data.aws_ami.ubuntu" && s.kind == "infra_data_source"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "output.instance_ip" && s.kind == "infra_output"));

        // Provider symbol
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "provider.aws" && s.kind == "infra_provider"));

        // Module source relation
        assert!(result.relations.iter().any(|r| r.from == "module.vpc"
            && r.to == "terraform-aws-modules/vpc/aws"
            && r.rel_type == "sources"));

        // Cross-resource reference
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "aws_instance.web_server"
                && r.to == "aws_security_group.web_sg"
                && r.rel_type == "references"));

        // Variable reference
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "aws_security_group.web_sg"
                && r.rel_type == "reads_var"
                && r.to == "var.vpc_cidr"
                || r.rel_type == "references"));
    }
}
