// Copyright (c) Didier Stadelmann. All rights reserved.

//! Qualification TDD de REQ-AXO-902562:
//! 1. La reachability d'orphan_clusters reconnaît les arêtes de dispatch par trait,
//!    ou les symboles atteints par un tel dispatch sont exemptés comme le sont les entrées déclarées en SOLL.
//!    Régression : mcp.rs::handle_request ne doit JAMAIS figurer dans un cluster mort.
//! 2. debt_digest.unlinked_code publie ce qu'il compte et ce qu'il exclut.
//! 3. indexer_lifecycle porte un état distinct pour « non activé par le mode de runtime »,
//!    et promote_status cesse de rendre crashed_or_abandoned dans ce cas.

use std::collections::HashSet;
use crate::ist_snapshot::code_smells::{is_inferred_entry, orphan_clusters};
use crate::ist_snapshot::snapshot::{EdgeTriple, IstGraph, NodeFlags, NodeKind, NodeRecord, RelationType};
use crate::mcp::runtime_topology_support::{
    resolve_indexer_liveness, IndexerSupervisorObservation,
    INDEXER_LIFECYCLE_DISABLED_FOR_MODE, LIFECYCLE_CERTAINTY_OBSERVED,
    EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
};
use crate::release_reconciler::{
    evaluate_liveness_gates, liveness_next_action, liveness_phase, LivenessFacts,
};

fn file(id: &str) -> NodeRecord {
    NodeRecord {
        id: id.to_string(),
        name: id.rsplit("::").next().unwrap_or(id).to_string(),
        project_code: "AXO".to_string(),
        kind: NodeKind::File,
        flags: NodeFlags::default(),
        complexity: None,
    }
}

fn func(id: &str, public: bool) -> NodeRecord {
    let mut flags = NodeFlags::default();
    if public {
        flags.0 |= NodeFlags::PUBLIC;
    }
    NodeRecord {
        id: id.to_string(),
        name: id.rsplit("::").next().unwrap_or(id).to_string(),
        project_code: "AXO".to_string(),
        kind: NodeKind::Function,
        flags,
        complexity: None,
    }
}

fn edge(s: &str, t: &str, rel: RelationType) -> EdgeTriple {
    EdgeTriple {
        source: s.to_string(),
        target: t.to_string(),
        rel,
    }
}

#[test]
fn c1_is_inferred_entry_recognizes_handle_request_and_notification() {
    assert!(
        is_inferred_entry("handle_request", "src/axon-core/src/mcp.rs"),
        "handle_request must be recognized as an inferred entry point"
    );
    assert!(
        is_inferred_entry("handle_notification", "src/axon-core/src/mcp.rs"),
        "handle_notification must be recognized as an inferred entry point"
    );
    assert!(
        is_inferred_entry("AxonMcpServer::handle_request", "src/axon-core/src/mcp.rs"),
        "qualified handle_request must be recognized as an inferred entry point"
    );
}

#[test]
fn c1_orphan_clusters_recognizes_qualified_soll_declared_entry() {
    // Un graphe avec mcp.rs qui contient handle_request et une fonction interne install.
    let nodes = vec![
        file("/proj/src/mcp.rs"),
        func("AXO::axon::src::axon-core::src::mcp.rs::handle_request", true),
        func("AXO::axon::src::axon-core::src::mcp.rs::install", false),
    ];
    let edges = vec![
        edge(
            "/proj/src/mcp.rs",
            "AXO::axon::src::axon-core::src::mcp.rs::handle_request",
            RelationType::Contains,
        ),
        edge(
            "/proj/src/mcp.rs",
            "AXO::axon::src::axon-core::src::mcp.rs::install",
            RelationType::Contains,
        ),
        edge(
            "AXO::axon::src::axon-core::src::mcp.rs::handle_request",
            "AXO::axon::src::axon-core::src::mcp.rs::install",
            RelationType::Calls,
        ),
    ];
    let g = IstGraph::build(nodes, edges);

    // Déclaration SOLL qualifiée par suffixe de fichier/symbole (comme dans la SOLL réelle)
    let mut declared = HashSet::new();
    declared.insert("mcp.rs::handle_request".to_string());

    let report = orphan_clusters(&g, "AXO", &declared);

    // handle_request et install ne doivent PAS figurer dans un cluster mort
    assert_eq!(
        report.clusters.len(),
        0,
        "declared entry and its callees must not form a dead cluster"
    );
    assert_eq!(report.unreached_count, 0);
}

#[test]
fn c3_indexer_liveness_disabled_for_runtime_mode_when_supervisor_disabled() {
    let now = 1_000_000;
    let obs = IndexerSupervisorObservation {
        status: "Disabled".to_string(),
        exit_code: 0,
        is_running: false,
    };

    // Cas 1: avec battement périmé
    let live_stale = resolve_indexer_liveness(
        now,
        Some(now - 60_000),
        EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
        Some(&obs),
    );
    assert_eq!(
        live_stale.lifecycle, INDEXER_LIFECYCLE_DISABLED_FOR_MODE,
        "supervisor Disabled must yield disabled_for_runtime_mode instead of crashed_or_abandoned"
    );
    assert_eq!(live_stale.certainty, LIFECYCLE_CERTAINTY_OBSERVED);

    // Cas 2: sans aucun battement
    let live_absent = resolve_indexer_liveness(
        now,
        None,
        EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
        Some(&obs),
    );
    assert_eq!(
        live_absent.lifecycle, INDEXER_LIFECYCLE_DISABLED_FOR_MODE,
        "supervisor Disabled without heartbeat must yield disabled_for_runtime_mode"
    );
}

#[test]
fn c3_promote_status_indexer_disabled_does_not_fail_gates_or_declare_indexer_down() {
    let mut l = LivenessFacts::default();
    l.brain_serving = true;
    l.indexer_expected = true;
    l.indexer_ready = false;
    l.indexer_lifecycle = INDEXER_LIFECYCLE_DISABLED_FOR_MODE.to_string();
    l.indexer_source = "supervisor_disabled".to_string();

    // 1. La phase globale ne doit PAS être indexer_down
    assert_eq!(
        liveness_phase(&l),
        None,
        "indexer disabled for runtime mode must not trigger indexer_down phase"
    );

    // 2. Les gates de liveness doivent toutes passer
    let gates = evaluate_liveness_gates(&l);
    assert!(
        gates.iter().all(|g| g.passes()),
        "indexer_alive gate must pass when disabled for runtime mode"
    );

    // 3. L'action suivante doit indiquer la configuration normale et non un crash
    let action = liveness_next_action(&l);
    if let Some(msg) = action {
        assert!(
            !msg.contains("crashed") && !msg.contains("stale"),
            "next action must not diagnose a crash: {msg}"
        );
        assert!(
            msg.contains("mode de runtime") || msg.contains("Disabled") || msg.contains("configuration"),
            "next action must explain the runtime mode configuration: {msg}"
        );
    }
}
