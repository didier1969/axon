use super::{ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;

pub struct SystemdParser;

impl Default for SystemdParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemdParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for SystemdParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let mut description = None;
        let mut unit_kind = "systemd_unit".to_string();
        let mut properties = HashMap::new();
        let mut exec_commands = Vec::new();
        let mut dependencies = Vec::new();

        let lines: Vec<&str> = content.lines().collect();

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
                continue;
            }

            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                let section = trimmed[1..trimmed.len() - 1].trim();
                if section == "Service" {
                    unit_kind = "systemd_service".to_string();
                } else if section == "Timer" {
                    unit_kind = "systemd_timer".to_string();
                } else if section == "Socket" {
                    unit_kind = "systemd_socket".to_string();
                }
                continue;
            }

            if let Some((key, val)) = trimmed.split_once('=') {
                let k = key.trim();
                let v = val.trim();

                match k {
                    "Description" => {
                        description = Some(v.to_string());
                    }
                    "ExecStart" | "ExecStop" | "ExecReload" => {
                        let cmd_bin = v.split_whitespace().next().unwrap_or("").to_string();
                        if !cmd_bin.is_empty() {
                            exec_commands.push((k.to_string(), cmd_bin));
                        }
                    }
                    "Requires" | "Wants" | "After" | "Before" | "BindsTo" => {
                        for dep in v.split_whitespace() {
                            dependencies.push((k.to_string(), dep.to_string()));
                        }
                    }
                    "OnCalendar" => {
                        properties.insert("schedule".to_string(), v.to_string());
                    }
                    "ListenStream" | "ListenDatagram" => {
                        properties.insert("listen".to_string(), v.to_string());
                    }
                    "User" => {
                        properties.insert("user".to_string(), v.to_string());
                    }
                    "Restart" => {
                        properties.insert("restart".to_string(), v.to_string());
                    }
                    _ => {}
                }
            }
        }

        let unit_name = description
            .clone()
            .unwrap_or_else(|| "unnamed.service".to_string());

        symbols.push(Symbol {
            name: unit_name.clone(),
            kind: unit_kind,
            start_line: 1,
            end_line: lines.len().max(1),
            docstring: description,
            is_entry_point: true,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties,
            embedding: None,
        });

        for (_exec_type, cmd) in exec_commands {
            relations.push(Relation {
                from: unit_name.clone(),
                to: cmd,
                rel_type: "executes".to_string(),
                properties: HashMap::new(),
            });
        }

        for (dep_type, dep) in dependencies {
            let mut props = HashMap::new();
            props.insert("dependency_type".to_string(), dep_type);
            relations.push(Relation {
                from: unit_name.clone(),
                to: dep,
                rel_type: "depends_on".to_string(),
                properties: props,
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
    fn test_systemd_service_and_timer_parsing() {
        let service_code = r#"
        [Unit]
        Description=Axon Brain Gateway
        After=network.target postgresql.service
        Wants=postgresql.service

        [Service]
        Type=simple
        User=axon
        ExecStart=/usr/local/bin/axon-brain --port 44127
        Restart=always

        [Install]
        WantedBy=multi-user.target
        "#;

        let parser = SystemdParser::new();
        let result = parser.parse(service_code);

        let service = result
            .symbols
            .iter()
            .find(|s| s.name == "Axon Brain Gateway")
            .expect("service symbol");
        assert_eq!(service.kind, "systemd_service");
        assert!(service.is_entry_point);
        assert_eq!(
            service.properties.get("user").map(String::as_str),
            Some("axon")
        );
        assert_eq!(
            service.properties.get("restart").map(String::as_str),
            Some("always")
        );

        // Executes relation
        assert!(result
            .relations
            .iter()
            .any(|r| r.rel_type == "executes" && r.to == "/usr/local/bin/axon-brain"));

        // Depends on relation
        assert!(result
            .relations
            .iter()
            .any(|r| r.rel_type == "depends_on" && r.to == "postgresql.service"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.rel_type == "depends_on" && r.to == "network.target"));

        let timer_code = r#"
        [Unit]
        Description=Periodic Database Vacuum Timer

        [Timer]
        OnCalendar=*-*-* 03:00:00
        Persistent=true

        [Install]
        WantedBy=timers.target
        "#;

        let timer_result = parser.parse(timer_code);
        let timer = timer_result
            .symbols
            .iter()
            .find(|s| s.name == "Periodic Database Vacuum Timer")
            .expect("timer symbol");
        assert_eq!(timer.kind, "systemd_timer");
        assert_eq!(
            timer.properties.get("schedule").map(String::as_str),
            Some("*-*-* 03:00:00")
        );
    }
}
