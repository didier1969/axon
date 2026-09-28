// REQ-AXO-902677 / DEC-AXO-901709 — Inter-procedural Data-Flow Graph (DFG) & Taint Analysis on CSR.
//
// Ultra-fast in-memory taint flow verification across procedure, module,
// and language boundaries (via FFI/CallsNif). Detects injection risks, PII leaks,
// and sanitizer barriers to eliminate false positives.

use super::snapshot::{IstGraph, RelationType};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaintKind {
    UserInput,
    PiiSecret,
    UntrustedPayload,
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SinkKind {
    SqlInjection,
    CommandInjection,
    CodeEval,
    PathTraversal,
    PiiLeak,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaintFlowFinding {
    pub source: String,
    pub source_kind: TaintKind,
    pub sink: String,
    pub sink_kind: SinkKind,
    pub path: Vec<String>,
    pub edges: Vec<String>,
    pub sanitized: bool,
    pub sanitizer: Option<String>,
    pub crosses_ffi: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TaintTraceOptions {
    pub max_depth: usize,
    pub include_sanitized: bool,
    pub category: Option<String>,
    pub source_filter: Option<String>,
    pub sink_filter: Option<String>,
}

pub fn trace_taint_flows(
    graph: &IstGraph,
    project: &str,
    options: &TaintTraceOptions,
) -> Vec<TaintFlowFinding> {
    let mut findings = Vec::new();
    let max_depth = if options.max_depth == 0 {
        10
    } else {
        options.max_depth
    };

    let node_count = graph.node_count() as u32;

    for start_idx in 0..node_count {
        if !project_matches(graph, start_idx, project) {
            continue;
        }

        let start_id = graph.id_of(start_idx);
        let maybe_source = classify_source(graph, start_idx);
        if maybe_source.is_none() {
            continue;
        }
        let source_kind = maybe_source.unwrap();

        if let Some(ref s_filter) = options.source_filter {
            if !start_id.contains(s_filter.as_str()) {
                continue;
            }
        }

        // BFS traversal tracking path, sanitizers, and FFI crossings
        struct State {
            node: u32,
            path: Vec<u32>,
            edges: Vec<RelationType>,
            sanitized: bool,
            sanitizer: Option<String>,
            crosses_ffi: bool,
        }

        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();

        queue.push_back(State {
            node: start_idx,
            path: vec![start_idx],
            edges: Vec::new(),
            sanitized: false,
            sanitizer: None,
            crosses_ffi: false,
        });
        visited.insert(start_idx);

        while let Some(current) = queue.pop_front() {
            if current.path.len() > max_depth {
                continue;
            }

            // Check if current is a sink (and not the start node itself)
            if current.node != start_idx {
                if let Some(sink_kind) = classify_sink(graph, current.node) {
                    if matches_category(source_kind, sink_kind, options.category.as_deref()) {
                        let sink_id = graph.id_of(current.node);
                        let sink_matches_filter = options
                            .sink_filter
                            .as_ref()
                            .map(|sf| sink_id.contains(sf.as_str()))
                            .unwrap_or(true);

                        if sink_matches_filter {
                            if options.include_sanitized || !current.sanitized {
                                findings.push(TaintFlowFinding {
                                    source: start_id.to_string(),
                                    source_kind,
                                    sink: sink_id.to_string(),
                                    sink_kind,
                                    path: current
                                        .path
                                        .iter()
                                        .map(|&idx| graph.id_of(idx).to_string())
                                        .collect(),
                                    edges: current
                                        .edges
                                        .iter()
                                        .map(|rel| rel.as_db().to_string())
                                        .collect(),
                                    sanitized: current.sanitized,
                                    sanitizer: current.sanitizer.clone(),
                                    crosses_ffi: current.crosses_ffi,
                                });
                            }
                        }
                    }
                }
            }

            // Explore forward neighbors
            for (tgt, rel) in graph.forward_neighbors(current.node) {
                if !is_traversable_relation(rel) {
                    continue;
                }

                let mut next_sanitized = current.sanitized;
                let mut next_sanitizer = current.sanitizer.clone();

                if rel == RelationType::Sanitizes || is_sanitizer_node(graph, tgt) {
                    next_sanitized = true;
                    if next_sanitizer.is_none() {
                        next_sanitizer = Some(graph.id_of(tgt).to_string());
                    }
                }

                let next_crosses_ffi = current.crosses_ffi || rel == RelationType::CallsNif;

                if !current.path.contains(&tgt) {
                    let mut next_path = current.path.clone();
                    next_path.push(tgt);
                    let mut next_edges = current.edges.clone();
                    next_edges.push(rel);

                    queue.push_back(State {
                        node: tgt,
                        path: next_path,
                        edges: next_edges,
                        sanitized: next_sanitized,
                        sanitizer: next_sanitizer,
                        crosses_ffi: next_crosses_ffi,
                    });
                }
            }
        }
    }

    findings
}

fn is_traversable_relation(rel: RelationType) -> bool {
    matches!(
        rel,
        RelationType::FlowsTo
            | RelationType::Taints
            | RelationType::Calls
            | RelationType::CallsNif
            | RelationType::FrameworkInvokes
            | RelationType::Sanitizes
            | RelationType::Reads
    )
}

fn project_matches(graph: &IstGraph, idx: u32, project: &str) -> bool {
    if project == "*" || project.is_empty() {
        return true;
    }
    let (_, proj, _) = graph.node_meta(idx);
    proj == project
}

fn classify_source(graph: &IstGraph, idx: u32) -> Option<TaintKind> {
    let id_lower = graph.id_of(idx).to_ascii_lowercase();
    let name_lower = graph.name_of(idx).to_ascii_lowercase();

    // Explicit outgoing Taints relation makes it a source
    for (_, rel) in graph.forward_neighbors(idx) {
        if rel == RelationType::Taints {
            if is_pii_pattern(&id_lower, &name_lower) {
                return Some(TaintKind::PiiSecret);
            }
            return Some(TaintKind::UserInput);
        }
    }

    if is_pii_pattern(&id_lower, &name_lower) {
        return Some(TaintKind::PiiSecret);
    }

    if is_user_input_pattern(&id_lower, &name_lower) {
        return Some(TaintKind::UserInput);
    }

    None
}

fn is_pii_pattern(id: &str, name: &str) -> bool {
    let patterns = [
        "password",
        "passwd",
        "secret",
        "token",
        "api_key",
        "apikey",
        "credential",
        "credit_card",
        "ssn",
        "private_key",
    ];
    patterns.iter().any(|p| id.contains(p) || name.contains(p))
}

fn is_user_input_pattern(id: &str, name: &str) -> bool {
    let patterns = [
        "user_input",
        "user_param",
        "request",
        "req_param",
        "query_param",
        "params",
        "form_data",
        "raw_payload",
        "http_body",
        "controller",
        "endpoint",
        "handler",
        "action",
        "route",
    ];
    patterns.iter().any(|p| id.contains(p) || name.contains(p))
}

fn classify_sink(graph: &IstGraph, idx: u32) -> Option<SinkKind> {
    let id_lower = graph.id_of(idx).to_ascii_lowercase();
    let name_lower = graph.name_of(idx).to_ascii_lowercase();

    // SQL Injection Sinks
    let sql_sinks = [
        "execute_sql",
        "raw_sql",
        "query_raw",
        "db_execute",
        "sql_query",
        "exec_sql",
    ];
    if sql_sinks
        .iter()
        .any(|s| id_lower.contains(s) || name_lower.contains(s))
    {
        return Some(SinkKind::SqlInjection);
    }

    // Command Injection Sinks — jugés sur le NOM DE L'APPEL, jamais sur l'id complet.
    // L'id porte le chemin du fichier : une recherche de sous-chaîne « system » / « exec » y
    // classait `System.get_env`, `System.system_time`, `:telemetry.execute`, une fonction
    // `extraction_system` ou tout fichier sous un dossier `ecosystem/` comme puits de commande
    // (mesuré 2026-09-28 sur LXL : 155 flux « critiques », 155 faux positifs, 0 vrai).
    // Les deux DERNIERS segments de l'id rattrapent un appel dont le nom est coupé au `::`
    // (Rust : id `…::Command::new`, nom `new`) sans jamais lire les dossiers du chemin.
    let id_tail = {
        let mut segs: Vec<&str> = id_lower.rsplit("::").take(2).collect();
        segs.reverse();
        segs.join("::")
    };
    if is_command_sink(&name_lower) || is_command_sink(&id_tail) {
        return Some(SinkKind::CommandInjection);
    }

    // Code Eval Sinks
    let eval_sinks = ["code_eval", "eval_string", "vm_run", "evaluate_script"];
    if eval_sinks
        .iter()
        .any(|s| id_lower.contains(s) || name_lower.contains(s))
    {
        return Some(SinkKind::CodeEval);
    }

    // PII Leak Sinks
    let pii_sinks = [
        "log_info",
        "log_error",
        "logger",
        "console_log",
        "print_debug",
        "telemetry_emit",
    ];
    if pii_sinks
        .iter()
        .any(|s| id_lower.contains(s) || name_lower.contains(s))
    {
        return Some(SinkKind::PiiLeak);
    }

    None
}

/// Appels qui lancent un processus ou un shell, reconnus par leur nom QUALIFIÉ (comparaison
/// exacte, ou suffixe après `.` / `::`), jamais par sous-chaîne.
const COMMAND_SINKS_QUALIFIED: &[&str] = &[
    "system.cmd",
    "system.shell",
    ":os.cmd",
    "os.cmd",
    "port.open",
    "os.system",
    "os.popen",
    "os.execv",
    "os.execve",
    "os.execvp",
    "os.spawnv",
    "subprocess.run",
    "subprocess.call",
    "subprocess.popen",
    "subprocess.check_call",
    "subprocess.check_output",
    "command::new",
    "command.new",
    "child_process.exec",
    "child_process.execsync",
    "child_process.spawn",
    "child_process.spawnsync",
    "runtime.exec",
];

/// Noms NUS dont la seule présence désigne un lancement de commande (fonction nommée exactement
/// ainsi, p. ex. `libc::system`). Comparés au DERNIER segment du nom, jamais par sous-chaîne :
/// `extraction_system`, `system_time` ou `execute` ne sont pas des puits.
const COMMAND_SINKS_BARE: &[&str] = &[
    "system",
    "exec",
    "execv",
    "execve",
    "execvp",
    "execl",
    "execlp",
    "popen",
    "shell_exec",
    "proc_open",
    "passthru",
    "spawn_process",
    "sh_exec",
    "eval_bash",
];

fn is_command_sink(name_lower: &str) -> bool {
    let qualified = COMMAND_SINKS_QUALIFIED.iter().any(|q| {
        name_lower == *q
            || name_lower.ends_with(&format!(".{q}"))
            || name_lower.ends_with(&format!("::{q}"))
    });
    if qualified {
        return true;
    }
    let last = name_lower
        .rsplit(|c| c == '.' || c == ':')
        .find(|s| !s.is_empty())
        .unwrap_or(name_lower);
    COMMAND_SINKS_BARE.contains(&last)
}

fn is_sanitizer_node(graph: &IstGraph, idx: u32) -> bool {
    let id_lower = graph.id_of(idx).to_ascii_lowercase();
    let name_lower = graph.name_of(idx).to_ascii_lowercase();

    let sanitizer_patterns = [
        "sanitize",
        "escape",
        "clean_input",
        "validate_param",
        "verify_input",
        "hash_password",
        "bcrypt",
        "hmac",
        "quote_sql",
    ];

    sanitizer_patterns
        .iter()
        .any(|p| id_lower.contains(p) || name_lower.contains(p))
}

fn matches_category(source: TaintKind, sink: SinkKind, cat: Option<&str>) -> bool {
    match cat {
        Some("injection") => matches!(
            sink,
            SinkKind::SqlInjection
                | SinkKind::CommandInjection
                | SinkKind::CodeEval
                | SinkKind::PathTraversal
        ),
        Some("pii") => source == TaintKind::PiiSecret && sink == SinkKind::PiiLeak,
        Some("all") | None => true,
        _ => true,
    }
}
