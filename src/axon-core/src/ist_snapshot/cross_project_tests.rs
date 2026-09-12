// REQ-AXO-902678 / DEC-AXO-901711 — Cross-Project CSR Federation & Traversal Tests.
//
// Validates multi-repo CSR federation where each project occupies independent ShardId partitions,
// linked via CrossShardEdge bridges, allowing cross-repo blast radius analysis (HYC -> NEX -> FSF).

use std::sync::Arc;

use crate::ist_snapshot::shard::{CrossShardEdge, ShardedIstGraph, ShardedIstView};
use crate::ist_snapshot::snapshot::{
    EdgeTriple, IstGraph, NodeFlags, NodeKind, NodeRecord, RelationType,
};

fn make_node(id: &str, project: &str) -> NodeRecord {
    NodeRecord {
        id: id.to_string(),
        name: id.rsplit("::").next().unwrap_or(id).to_string(),
        project_code: project.to_string(),
        kind: NodeKind::Function,
        flags: NodeFlags::default(),
        complexity: Some(2),
    }
}

fn make_project_graph(
    project_code: &str,
    node_names: &[&str],
    internal_calls: &[(&str, &str)],
) -> Arc<IstGraph> {
    let nodes: Vec<NodeRecord> = node_names
        .iter()
        .map(|name| make_node(&format!("{project_code}::{name}"), project_code))
        .collect();

    let edges: Vec<EdgeTriple> = internal_calls
        .iter()
        .map(|(from, to)| EdgeTriple {
            source: format!("{project_code}::{from}"),
            target: format!("{project_code}::{to}"),
            rel: RelationType::Calls,
        })
        .collect();

    Arc::new(IstGraph::build(nodes, edges))
}

#[test]
fn test_cross_project_federation_and_traversal() {
    // Project 1: HYC (Hydra Collector) - Shard 1
    // Symbols: fetch_estv_data -> publish_stream
    let hyc_graph = make_project_graph(
        "HYC",
        &["fetch_estv_data", "publish_stream"],
        &[("fetch_estv_data", "publish_stream")],
    );

    // Project 2: NEX (Nexus Quantitative Engine) - Shard 2
    // Symbols: ingest_hyc_stream -> compile_law_to_logic
    let nex_graph = make_project_graph(
        "NEX",
        &["ingest_hyc_stream", "compile_law_to_logic"],
        &[("ingest_hyc_stream", "compile_law_to_logic")],
    );

    // Project 3: FSF (Fiscaly Rules Engine) - Shard 3
    // Symbols: evaluate_cozo_rules -> certify_conformance
    let fsf_graph = make_project_graph(
        "FSF",
        &["evaluate_cozo_rules", "certify_conformance"],
        &[("evaluate_cozo_rules", "certify_conformance")],
    );

    // Construct Cross-Project Edges:
    // HYC::publish_stream --[CALLS]--> NEX::ingest_hyc_stream
    // NEX::compile_law_to_logic --[CALLS]--> FSF::evaluate_cozo_rules
    let cross_edges = vec![
        CrossShardEdge {
            source_symbol: "HYC::publish_stream".to_string(),
            target_symbol: "NEX::ingest_hyc_stream".to_string(),
            source_shard: 1,
            target_shard: 2,
            rel: RelationType::Calls,
        },
        CrossShardEdge {
            source_symbol: "NEX::compile_law_to_logic".to_string(),
            target_symbol: "FSF::evaluate_cozo_rules".to_string(),
            source_shard: 2,
            target_shard: 3,
            rel: RelationType::Calls,
        },
    ];

    // Build federated multi-project graph
    let federated_graph = ShardedIstGraph::federate_subgraphs(
        "MULTI_PROJECT_FEDERATION",
        vec![
            (1, "HYC", hyc_graph),
            (2, "NEX", nex_graph),
            (3, "FSF", fsf_graph),
        ],
        cross_edges,
    );

    assert_eq!(federated_graph.shard_count(), 3);
    assert_eq!(federated_graph.total_node_count(), 6);
    assert_eq!(federated_graph.cross_shard_edge_count(), 2);

    let view = ShardedIstView::new(&federated_graph);

    // 1. Cross-Project Path Finding: HYC -> NEX -> FSF
    let path = view.find_path("HYC::fetch_estv_data", "FSF::certify_conformance");
    assert!(
        path.is_some(),
        "BFS must find complete path across 3 distinct repositories"
    );
    let p = path.unwrap();
    assert_eq!(p.len(), 6);
    assert_eq!(p[0], "HYC::fetch_estv_data");
    assert_eq!(p[1], "HYC::publish_stream");
    assert_eq!(p[2], "NEX::ingest_hyc_stream");
    assert_eq!(p[3], "NEX::compile_law_to_logic");
    assert_eq!(p[4], "FSF::evaluate_cozo_rules");
    assert_eq!(p[5], "FSF::certify_conformance");

    // 2. Cross-Project Transitive Impact: Blast radius of modifying HYC::publish_stream
    let impact = view.transitive_callers_of("FSF::certify_conformance", 10);
    assert!(impact.contains(&"FSF::evaluate_cozo_rules".to_string()));
    assert!(impact.contains(&"NEX::compile_law_to_logic".to_string()));
    assert!(impact.contains(&"NEX::ingest_hyc_stream".to_string()));
    assert!(impact.contains(&"HYC::publish_stream".to_string()));
    assert!(impact.contains(&"HYC::fetch_estv_data".to_string()));

    // 3. Isolated Invalidation: Invalidate NEX shard
    let partial_graph = federated_graph.invalidate_shard(2);
    assert_eq!(partial_graph.shard_count(), 2);
    let partial_view = ShardedIstView::new(&partial_graph);
    // Path across invalidated shard must no longer exist
    assert!(partial_view
        .find_path("HYC::fetch_estv_data", "FSF::certify_conformance")
        .is_none());
}
