use super::dataflow::*;
use super::snapshot::{EdgeTriple, IstGraph, NodeFlags, NodeKind, NodeRecord, RelationType};

fn node(id: &str, project: &str, kind: NodeKind) -> NodeRecord {
    NodeRecord {
        id: id.to_string(),
        name: id.rsplit("::").next().unwrap_or(id).to_string(),
        project_code: project.to_string(),
        kind,
        flags: NodeFlags::default(),
        complexity: None,
    }
}

fn edge(src: &str, tgt: &str, rel: RelationType) -> EdgeTriple {
    EdgeTriple {
        source: src.to_string(),
        target: tgt.to_string(),
        rel,
    }
}

#[test]
fn test_direct_untrusted_input_to_sql_sink_detected() {
    let nodes = vec![
        node("api::user_input", "AXO", NodeKind::Function),
        node("service::process_query", "AXO", NodeKind::Function),
        node("db::execute_sql", "AXO", NodeKind::Function),
    ];
    let edges = vec![
        edge(
            "api::user_input",
            "service::process_query",
            RelationType::FlowsTo,
        ),
        edge(
            "service::process_query",
            "db::execute_sql",
            RelationType::Calls,
        ),
    ];
    let g = IstGraph::build(nodes, edges);

    let opts = TaintTraceOptions {
        max_depth: 10,
        include_sanitized: true,
        category: Some("injection".to_string()),
        source_filter: None,
        sink_filter: None,
    };

    let findings = trace_taint_flows(&g, "AXO", &opts);
    assert!(!findings.is_empty(), "Should detect SQL injection flow");
    let f = &findings[0];
    assert_eq!(f.source, "api::user_input");
    assert_eq!(f.sink, "db::execute_sql");
    assert_eq!(f.sink_kind, SinkKind::SqlInjection);
    assert!(!f.sanitized);
    assert!(f.sanitizer.is_none());
    assert!(!f.crosses_ffi);
    assert_eq!(
        f.path,
        vec![
            "api::user_input",
            "service::process_query",
            "db::execute_sql"
        ]
    );
}

#[test]
fn test_sanitizer_intercepts_taint_flow_eliminates_false_positive() {
    let nodes = vec![
        node("api::user_input", "AXO", NodeKind::Function),
        node("sec::sanitize_sql_param", "AXO", NodeKind::Function),
        node("db::execute_sql", "AXO", NodeKind::Function),
    ];
    let edges = vec![
        edge(
            "api::user_input",
            "sec::sanitize_sql_param",
            RelationType::FlowsTo,
        ),
        edge(
            "sec::sanitize_sql_param",
            "db::execute_sql",
            RelationType::Sanitizes,
        ),
    ];
    let g = IstGraph::build(nodes, edges);

    let opts = TaintTraceOptions {
        max_depth: 10,
        include_sanitized: true,
        category: Some("injection".to_string()),
        source_filter: None,
        sink_filter: None,
    };

    let findings = trace_taint_flows(&g, "AXO", &opts);
    assert!(!findings.is_empty());
    let f = &findings[0];
    assert_eq!(f.source, "api::user_input");
    assert_eq!(f.sink, "db::execute_sql");
    assert!(
        f.sanitized,
        "Flow must be marked as sanitized by the sanitizer"
    );
    assert_eq!(f.sanitizer.as_deref(), Some("sec::sanitize_sql_param"));
}

#[test]
fn test_cross_language_taint_traversal_via_calls_nif() {
    let nodes = vec![
        node("Elixir.Web.Controller.handle", "AXO", NodeKind::Function),
        node("axon_core::nif_eval", "AXO", NodeKind::Function),
        node("libc::system", "AXO", NodeKind::Function),
    ];
    let edges = vec![
        edge(
            "Elixir.Web.Controller.handle",
            "axon_core::nif_eval",
            RelationType::CallsNif,
        ),
        edge("axon_core::nif_eval", "libc::system", RelationType::Calls),
    ];
    let g = IstGraph::build(nodes, edges);

    let opts = TaintTraceOptions {
        max_depth: 10,
        include_sanitized: true,
        category: Some("injection".to_string()),
        source_filter: None,
        sink_filter: None,
    };

    let findings = trace_taint_flows(&g, "AXO", &opts);
    assert!(
        !findings.is_empty(),
        "Cross-language taint flow must be detected"
    );
    let f = &findings[0];
    assert_eq!(f.source, "Elixir.Web.Controller.handle");
    assert_eq!(f.sink, "libc::system");
    assert_eq!(f.sink_kind, SinkKind::CommandInjection);
    assert!(
        f.crosses_ffi,
        "Must flag that the taint chain crossed FFI / NIF boundaries"
    );
    assert!(!f.sanitized);
}

#[test]
fn test_pii_leak_to_logger_detected() {
    let nodes = vec![
        node("auth::user_password", "AXO", NodeKind::Field),
        node("auth::login_service", "AXO", NodeKind::Function),
        node("logger::log_info", "AXO", NodeKind::Function),
    ];
    let edges = vec![
        edge(
            "auth::user_password",
            "auth::login_service",
            RelationType::FlowsTo,
        ),
        edge(
            "auth::login_service",
            "logger::log_info",
            RelationType::Calls,
        ),
    ];
    let g = IstGraph::build(nodes, edges);

    let opts = TaintTraceOptions {
        max_depth: 10,
        include_sanitized: true,
        category: Some("pii".to_string()),
        source_filter: None,
        sink_filter: None,
    };

    let findings = trace_taint_flows(&g, "AXO", &opts);
    assert!(!findings.is_empty(), "PII leak must be detected");
    let f = &findings[0];
    assert_eq!(f.source, "auth::user_password");
    assert_eq!(f.sink, "logger::log_info");
    assert_eq!(f.source_kind, TaintKind::PiiSecret);
    assert_eq!(f.sink_kind, SinkKind::PiiLeak);
    assert!(!f.sanitized);
}

// Puits de commande jugés sur le NOM DE L'APPEL : les faux positifs mesurés sur LXL le
// 2026-09-28 (ids réels, chemin compris) ne doivent PLUS être des puits.
fn injection_sinks_from(sink_ids: &[&str]) -> Vec<TaintFlowFinding> {
    let mut nodes = vec![node("api::user_input", "TST", NodeKind::Function)];
    let mut edges = Vec::new();
    for id in sink_ids {
        nodes.push(node(id, "TST", NodeKind::Function));
        edges.push(edge("api::user_input", id, RelationType::Calls));
    }
    let g = IstGraph::build(nodes, edges);
    let opts = TaintTraceOptions {
        max_depth: 10,
        include_sanitized: true,
        category: Some("injection".to_string()),
        source_filter: None,
        sink_filter: None,
    };
    trace_taint_flows(&g, "TST", &opts)
}

#[test]
fn test_command_sink_ignores_path_and_substring_false_positives() {
    let faux = [
        "LXL::Lexoria::elixir::apps::fiscaly_ai::lib::fiscaly_ai::cache.ex::System.system_time",
        "LXL::Lexoria::elixir::apps::fiscaly_ai::lib::fiscaly_ai::claude.ex::System.get_env",
        "LXL::Lexoria::elixir::apps::lexgraph::lib::lexgraph::bridge.ex::System.put_env",
        "LXL::Lexoria::elixir::apps::lexgraph::cozo::nif_server.ex::System.unique_integer",
        "LXL::Lexoria::elixir::apps::fiscaly_ai::lib::fiscaly_ai::prompts.ex::FiscalyAi.Prompts.extraction_system",
        "LXL::Lexoria::elixir::apps::lexgraph::lib::lexgraph::bridge.ex:::telemetry.execute",
        "LXL::Lexoria::elixir::apps::fiscaly_core::lib::fiscaly_core::ecosystem::hydra_bridge.ex::Node.ping",
        "LXL::Lexoria::elixir::apps::lexoria_inference::test::conditional_test.exs::LexoriaInference.ConditionalTest.execute",
        "TST::src::system::exec_helpers.rs::format_report",
    ];
    let findings = injection_sinks_from(&faux);
    let cmd: Vec<&str> = findings
        .iter()
        .filter(|f| f.sink_kind == SinkKind::CommandInjection)
        .map(|f| f.sink.as_str())
        .collect();
    assert!(cmd.is_empty(), "faux puits de commande : {cmd:?}");
}

#[test]
fn test_command_sink_detects_real_process_launch_calls() {
    let vrais = [
        "TST::lib::runner.ex::System.cmd",
        "TST::lib::runner.ex::System.shell",
        "TST::lib::runner.ex:::os.cmd",
        "TST::lib::runner.ex::Port.open",
        "TST::app::tools.py::os.system",
        "TST::app::tools.py::subprocess.run",
        "TST::src::main.rs::std::process::Command::new",
        "libc::system",
    ];
    let findings = injection_sinks_from(&vrais);
    for id in vrais {
        assert!(
            findings
                .iter()
                .any(|f| f.sink == id && f.sink_kind == SinkKind::CommandInjection),
            "vrai puits de commande non détecté : {id}"
        );
    }
}
