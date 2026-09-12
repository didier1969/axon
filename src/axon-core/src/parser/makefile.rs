use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};

static RE_INCLUDE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*-?include\s+([^\n#]+)").expect("valid regex"));

static RE_VAR_ASSIGN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*([a-zA-Z0-9_]+)\s*(?::\?|\?|::|:|)\s*=\s*(.*)").expect("valid regex")
});

static RE_TARGET_RULE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^([a-zA-Z0-9_./%-]+(?:\s+[a-zA-Z0-9_./%-]+)*)\s*:\s*([^#=\n]*)")
        .expect("valid regex")
});

pub struct MakefileParser;

impl Default for MakefileParser {
    fn default() -> Self {
        Self::new()
    }
}

impl MakefileParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for MakefileParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Includes
        for cap in RE_INCLUDE.captures_iter(content) {
            let paths_str = cap.get(1).map_or("", |m| m.as_str());
            for p in paths_str.split_whitespace() {
                if !p.is_empty() {
                    relations.push(Relation {
                        from: "Makefile".to_string(),
                        to: p.to_string(),
                        rel_type: "includes_makefile".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }

        // 2. Phony target collection
        let mut phony_targets = HashSet::new();
        for cap in RE_TARGET_RULE.captures_iter(content) {
            let targets_raw = cap.get(1).map_or("", |m| m.as_str());
            let prereqs_raw = cap.get(2).map_or("", |m| m.as_str());
            if targets_raw.trim() == ".PHONY" {
                for p in prereqs_raw.split_whitespace() {
                    phony_targets.insert(p.to_string());
                }
            }
        }

        // 3. Variables with PII detection
        for cap in RE_VAR_ASSIGN.captures_iter(content) {
            let vname = cap.get(1).map_or("", |m| m.as_str()).trim();
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            if let Some(kind) = super::is_sensitive_name(vname) {
                props.insert("is_sensitive".to_string(), "true".to_string());
                props.insert("pii_kind".to_string(), kind.to_string());
            }

            symbols.push(Symbol {
                name: vname.to_string(),
                kind: "make_variable".to_string(),
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

        // 4. Targets and dependencies
        let mut first_target = true;
        for cap in RE_TARGET_RULE.captures_iter(content) {
            let targets_raw = cap.get(1).map_or("", |m| m.as_str());
            let prereqs_raw = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            if targets_raw.starts_with('.') && !targets_raw.starts_with("./") {
                continue; // Skip special directives like .PHONY, .SUFFIXES
            }

            for tname in targets_raw.split_whitespace() {
                let clean_target = tname.trim();
                if clean_target.is_empty() {
                    continue;
                }

                let is_phony = phony_targets.contains(clean_target);
                let is_entry = first_target
                    || clean_target == "all"
                    || clean_target == "build"
                    || clean_target == "install"
                    || clean_target == "default";
                let is_test = clean_target == "test"
                    || clean_target.starts_with("test-")
                    || clean_target.starts_with("check");

                first_target = false;

                let mut target_props = HashMap::new();
                if is_phony {
                    target_props.insert("is_phony".to_string(), "true".to_string());
                }

                symbols.push(Symbol {
                    name: clean_target.to_string(),
                    kind: if is_test {
                        "make_test".to_string()
                    } else {
                        "make_target".to_string()
                    },
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: is_entry,
                    is_public: true,
                    tested: is_test,
                    is_nif: false,
                    is_unsafe: false,
                    properties: target_props,
                    embedding: None,
                });

                // Prereq relations
                for prereq in prereqs_raw.split_whitespace() {
                    let clean_prereq = prereq.trim();
                    if !clean_prereq.is_empty() && !clean_prereq.starts_with(';') {
                        relations.push(Relation {
                            from: clean_target.to_string(),
                            to: clean_prereq.to_string(),
                            rel_type: "depends_on".to_string(),
                            properties: HashMap::new(),
                        });
                    }
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
    fn test_makefile_parser_basic_and_pii() {
        let code = r#"
include common.mk
-include local.mk

CC = clang
CFLAGS ?= -O2 -Wall
DATABASE_PASSWORD ?= secret_pass
DEPLOY_TOKEN := live_token_xyz

.PHONY: all clean test

all: build test

build: main.o utils.o
	$(CC) $(CFLAGS) -o app main.o utils.o

test:
	./run_tests.sh

clean:
	rm -f *.o app
"#;

        let parser = MakefileParser::new();
        let res = parser.parse(code);

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "common.mk" && r.rel_type == "includes_makefile"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "local.mk" && r.rel_type == "includes_makefile"));

        let pass_var = res
            .symbols
            .iter()
            .find(|s| s.name == "DATABASE_PASSWORD")
            .unwrap();
        assert_eq!(
            pass_var.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let tok_var = res
            .symbols
            .iter()
            .find(|s| s.name == "DEPLOY_TOKEN")
            .unwrap();
        assert_eq!(
            tok_var.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let all_target = res.symbols.iter().find(|s| s.name == "all").unwrap();
        assert_eq!(all_target.kind, "make_target");
        assert!(all_target.is_entry_point);
        assert_eq!(
            all_target.properties.get("is_phony").map(|s| s.as_str()),
            Some("true")
        );

        let build_target = res.symbols.iter().find(|s| s.name == "build").unwrap();
        assert_eq!(build_target.kind, "make_target");

        assert!(res
            .relations
            .iter()
            .any(|r| r.from == "all" && r.to == "build" && r.rel_type == "depends_on"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.from == "all" && r.to == "test" && r.rel_type == "depends_on"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.from == "build" && r.to == "main.o" && r.rel_type == "depends_on"));

        let test_target = res.symbols.iter().find(|s| s.name == "test").unwrap();
        assert_eq!(test_target.kind, "make_test");
        assert!(test_target.tested);
    }
}
