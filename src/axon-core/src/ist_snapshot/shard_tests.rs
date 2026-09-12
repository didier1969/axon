use crate::ist_snapshot::shard::{ShardedIstGraph, ShardingStrategy};
use crate::ist_snapshot::snapshot::{EdgeTriple, NodeFlags, NodeKind, NodeRecord, RelationType};

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

#[test]
fn test_csr_shard_creation_and_routing() {
    let mut builder = ShardedIstGraph::builder(
        "PRJ",
        ShardingStrategy::PrefixRule(vec![
            ("PRJ::auth::".to_string(), 0),
            ("PRJ::billing::".to_string(), 1),
        ]),
    );

    let nodes = vec![
        make_node("PRJ::auth::login", "PRJ"),
        make_node("PRJ::auth::verify_token", "PRJ"),
        make_node("PRJ::billing::charge", "PRJ"),
        make_node("PRJ::billing::invoice", "PRJ"),
    ];

    let edges = vec![
        // Intra-shard 0
        EdgeTriple {
            source: "PRJ::auth::login".to_string(),
            target: "PRJ::auth::verify_token".to_string(),
            rel: RelationType::Calls,
        },
        // Cross-shard 0 -> 1
        EdgeTriple {
            source: "PRJ::auth::login".to_string(),
            target: "PRJ::billing::charge".to_string(),
            rel: RelationType::Calls,
        },
        // Intra-shard 1
        EdgeTriple {
            source: "PRJ::billing::charge".to_string(),
            target: "PRJ::billing::invoice".to_string(),
            rel: RelationType::Calls,
        },
    ];

    let sharded = builder.build(nodes, edges).expect("build sharded graph");

    assert_eq!(sharded.shard_count(), 2);
    assert_eq!(sharded.shard_for_symbol("PRJ::auth::login"), Some(0));
    assert_eq!(sharded.shard_for_symbol("PRJ::billing::charge"), Some(1));
    assert_eq!(sharded.shard_for_symbol("PRJ::unknown::sym"), None);

    assert_eq!(sharded.cross_shard_edge_count(), 1);

    let shard0 = sharded.get_shard(0).expect("shard 0 exists");
    assert_eq!(shard0.id(), 0);
    assert_eq!(shard0.local_node_count(), 2);

    let shard1 = sharded.get_shard(1).expect("shard 1 exists");
    assert_eq!(shard1.id(), 1);
    assert_eq!(shard1.local_node_count(), 2);
}

#[test]
fn test_hash_sharding_strategy_deterministic() {
    let strategy = ShardingStrategy::HashModulo(4);
    let sym1 = "PRJ::crypto::sha256";
    let sym2 = "PRJ::crypto::sha512";

    let shard_a = strategy.assign_shard(sym1);
    let shard_b = strategy.assign_shard(sym1);
    assert_eq!(shard_a, shard_b, "hash routing must be deterministic");
    assert!(shard_a < 4);

    let shard_c = strategy.assign_shard(sym2);
    assert!(shard_c < 4);
}

#[test]
fn test_cross_shard_bfs_path_resolution() {
    use crate::ist_snapshot::shard::ShardedIstView;

    let mut builder = ShardedIstGraph::builder(
        "PRJ",
        ShardingStrategy::PrefixRule(vec![
            ("PRJ::shard0::".to_string(), 0),
            ("PRJ::shard1::".to_string(), 1),
            ("PRJ::shard2::".to_string(), 2),
        ]),
    );

    let nodes = vec![
        make_node("PRJ::shard0::entrypoint", "PRJ"),
        make_node("PRJ::shard1::service", "PRJ"),
        make_node("PRJ::shard2::database", "PRJ"),
        make_node("PRJ::shard2::sink", "PRJ"),
    ];

    let edges = vec![
        // Shard 0 -> Shard 1
        EdgeTriple {
            source: "PRJ::shard0::entrypoint".to_string(),
            target: "PRJ::shard1::service".to_string(),
            rel: RelationType::Calls,
        },
        // Shard 1 -> Shard 2
        EdgeTriple {
            source: "PRJ::shard1::service".to_string(),
            target: "PRJ::shard2::database".to_string(),
            rel: RelationType::Calls,
        },
        // Shard 2 -> Shard 2 (local)
        EdgeTriple {
            source: "PRJ::shard2::database".to_string(),
            target: "PRJ::shard2::sink".to_string(),
            rel: RelationType::Calls,
        },
    ];

    let sharded = builder.build(nodes, edges).expect("build sharded graph");
    let view = ShardedIstView::new(&sharded);

    // Test BFS shortest path spanning shard 0 -> 1 -> 2
    let path = view
        .find_path("PRJ::shard0::entrypoint", "PRJ::shard2::sink")
        .expect("path should be found across shards");

    assert_eq!(
        path,
        vec![
            "PRJ::shard0::entrypoint",
            "PRJ::shard1::service",
            "PRJ::shard2::database",
            "PRJ::shard2::sink"
        ]
    );

    // Reverse callers / impact
    let callers = view.callers_of("PRJ::shard2::sink");
    assert!(callers.contains(&"PRJ::shard2::database".to_string()));

    let impact = view.transitive_callers_of("PRJ::shard2::sink", 5);
    assert!(impact.contains(&"PRJ::shard2::database".to_string()));
    assert!(impact.contains(&"PRJ::shard1::service".to_string()));
    assert!(impact.contains(&"PRJ::shard0::entrypoint".to_string()));
}
