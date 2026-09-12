// REQ-AXO-902677 / DEC-AXO-901709 — TDD Tests for Automated SAST Taint Gate in axon_pre_flight_check.

use std::sync::Arc;

use serde_json::{json, Value};

use super::*;
use crate::ist_snapshot::cache::IstSnapshotCache;
use crate::ist_snapshot::snapshot::{EdgeTriple, NodeFlags, NodeKind, NodeRecord, RelationType};

fn setup_vulnerable_project_snapshot(cache: &Arc<IstSnapshotCache>, project: &str) {
    let nodes = vec![
        NodeRecord {
            id: format!("{}::controller::user_input_endpoint", project),
            name: "user_input_endpoint".to_string(),
            project_code: project.to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
            complexity: Some(2),
        },
        NodeRecord {
            id: format!("{}::db::execute_sql_query", project),
            name: "execute_sql_query".to_string(),
            project_code: project.to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
            complexity: Some(3),
        },
    ];

    let edges = vec![EdgeTriple {
        source: format!("{}::controller::user_input_endpoint", project),
        target: format!("{}::db::execute_sql_query", project),
        rel: RelationType::Calls,
    }];

    let graph = crate::ist_snapshot::snapshot::IstGraph::build(nodes, edges);
    cache.publish(project.to_string(), Arc::new(graph));
}

fn setup_sanitized_project_snapshot(cache: &Arc<IstSnapshotCache>, project: &str) {
    let nodes = vec![
        NodeRecord {
            id: format!("{}::controller::user_input_endpoint", project),
            name: "user_input_endpoint".to_string(),
            project_code: project.to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
            complexity: Some(2),
        },
        NodeRecord {
            id: format!("{}::security::escape_sql_input", project),
            name: "escape_sql_input".to_string(),
            project_code: project.to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
            complexity: Some(1),
        },
        NodeRecord {
            id: format!("{}::db::execute_sql_query", project),
            name: "execute_sql_query".to_string(),
            project_code: project.to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
            complexity: Some(3),
        },
    ];

    let edges = vec![
        EdgeTriple {
            source: format!("{}::controller::user_input_endpoint", project),
            target: format!("{}::security::escape_sql_input", project),
            rel: RelationType::Calls,
        },
        EdgeTriple {
            source: format!("{}::security::escape_sql_input", project),
            target: format!("{}::db::execute_sql_query", project),
            rel: RelationType::Calls,
        },
    ];

    let graph = crate::ist_snapshot::snapshot::IstGraph::build(nodes, edges);
    cache.publish(project.to_string(), Arc::new(graph));
}

#[test]
fn test_pre_flight_check_blocks_unsanitized_sql_injection() {
    let server = create_test_server();
    let cache = crate::ist_snapshot::shared_cache();
    let project = "TST_SAST_VULN";

    setup_vulnerable_project_snapshot(&cache, project);

    let check_req = json!({
        "project_code": project,
        "diff_paths": ["src/controller/user_input.rs"],
        "message": "test commit with sql injection",
    });

    let res = server
        .axon_pre_flight_check(&check_req)
        .expect("pre_flight_check must answer");

    assert!(
        res.get("isError").and_then(Value::as_bool).unwrap_or(false),
        "pre-flight check MUST fail when unsanitized SQL injection is present"
    );

    let data = res.get("data").expect("data field present");
    let taint_audit = data.get("taint_audit").expect("taint_audit field present");
    let critical_count = taint_audit
        .get("critical_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    assert!(
        critical_count >= 1,
        "critical taint violations count must be >= 1, got {}",
        critical_count
    );

    let critical_violations = taint_audit
        .get("critical_violations")
        .and_then(Value::as_array)
        .expect("critical_violations array");

    let has_sql_inj = critical_violations
        .iter()
        .any(|v| v.get("sink_kind").and_then(Value::as_str) == Some("SqlInjection"));
    assert!(has_sql_inj, "finding must classify as SqlInjection");
}

#[test]
fn test_pre_flight_check_passes_sanitized_sql_flow() {
    let server = create_test_server();
    let cache = crate::ist_snapshot::shared_cache();
    let project = "TST_SAST_SAFE";

    setup_sanitized_project_snapshot(&cache, project);

    let check_req = json!({
        "project_code": project,
        "diff_paths": ["src/controller/user_input.rs"],
        "message": "test commit with sanitized flow",
    });

    let res = server
        .axon_pre_flight_check(&check_req)
        .expect("pre_flight_check must answer");

    let data = res.get("data").expect("data field present");
    let taint_audit = data.get("taint_audit").expect("taint_audit field present");

    let critical_count = taint_audit
        .get("critical_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    assert_eq!(
        critical_count, 0,
        "critical violations must be 0 for sanitized flow"
    );

    let sanitized_count = taint_audit
        .get("sanitized_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    assert!(
        sanitized_count >= 1,
        "sanitized flows count must be >= 1 for sanitized flow"
    );
}
