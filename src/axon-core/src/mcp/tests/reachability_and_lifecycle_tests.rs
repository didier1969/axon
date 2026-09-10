// Copyright (c) Didier Stadelmann. All rights reserved.

//! Qualification TDD de REQ-AXO-902562:
//! 1. La reachability d'orphan_clusters reconnaît les arêtes de dispatch par trait,
//!    ou les symboles atteints par un tel dispatch sont exemptés comme le sont les entrées déclarées en SOLL.
//!    Régression : mcp.rs::handle_request ne doit JAMAIS figurer dans un cluster mort.
//! 2. debt_digest.unlinked_code publie ce qu'il compte et ce qu'il exclut.
//! 3. indexer_lifecycle porte un état distinct pour « non activé par le mode de runtime »,
//!    et promote_status cesse de rendre crashed_or_abandoned dans ce cas.

use crate::ist_snapshot::code_smells::{is_inferred_entry, orphan_clusters};
use crate::ist_snapshot::snapshot::{
    EdgeTriple, IstGraph, NodeFlags, NodeKind, NodeRecord, RelationType,
};
use crate::mcp::runtime_topology_support::{
    resolve_indexer_liveness, IndexerSupervisorObservation,
    EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS, INDEXER_LIFECYCLE_DISABLED_FOR_MODE,
    LIFECYCLE_CERTAINTY_OBSERVED,
};
use crate::release_reconciler::{
    evaluate_liveness_gates, liveness_next_action, liveness_phase, LivenessFacts,
};
use std::collections::HashSet;

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
        func(
            "AXO::axon::src::axon-core::src::mcp.rs::handle_request",
            true,
        ),
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
        ..Default::default()
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
            msg.contains("mode de runtime")
                || msg.contains("Disabled")
                || msg.contains("configuration"),
            "next action must explain the runtime mode configuration: {msg}"
        );
    }
}

/// REQ-AXO-902656 (Feedback #427) — l'indexeur en boucle de relance sous superviseur
/// DOIT échouer bruyamment (ready=false, lifecycle=restart_loop) même si un battement
/// PG résiduel d'une instance précédente/orpheline est encore frais.
#[test]
fn c4_indexer_supervisor_restart_loop_detection_fails_loudly() {
    let now = 1_000_000;
    // Simulation exacte de l'incident du 04.09.2026 (Feedback #427) :
    // 2591 redémarrages, processus âgé de 30 ms.
    let loop_obs = IndexerSupervisorObservation {
        status: "Restarting".to_string(),
        exit_code: 1,
        is_running: false,
        restarts: 2591,
        age_ms: 30,
    };

    // Même avec un battement PG récent (ex: 2s) laissé par une instance orpheline
    let live = resolve_indexer_liveness(
        now,
        Some(now - 2_000),
        EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
        Some(&loop_obs),
    );

    assert!(
        !live.ready,
        "un indexeur en boucle de restart sous superviseur NE DOIT JAMAIS être déclaré ready"
    );
    assert_eq!(
        live.lifecycle,
        crate::mcp::runtime_topology_support::INDEXER_LIFECYCLE_RESTART_LOOP,
        "le lifecycle doit être explicitement restart_loop"
    );
    assert_eq!(
        live.source, "supervisor_restart_loop",
        "la source doit identifier la boucle de redémarrage du superviseur"
    );
    assert_eq!(
        live.certainty, LIFECYCLE_CERTAINTY_OBSERVED,
        "l'état est observé directement auprès du superviseur"
    );
}

/// REQ-AXO-902656 (Feedback #427) — les manifestes process-compose doivent propager
/// AXON_LIVE_DATABASE_URL / AXON_DEV_DATABASE_URL à axon-indexer et capturer les logs.
#[test]
fn c4_process_compose_manifests_declare_database_urls_and_log_locations() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root");
    let live_yaml = std::fs::read_to_string(root.join("process-compose.live.yaml"))
        .expect("process-compose.live.yaml must exist");
    let dev_yaml = std::fs::read_to_string(root.join("process-compose.dev.yaml"))
        .expect("process-compose.dev.yaml must exist");

    // 1. AXON_LIVE_DATABASE_URL dans process-compose.live.yaml pour axon-indexer
    let live_indexer_sec = live_yaml
        .split("axon-indexer:")
        .nth(1)
        .expect("axon-indexer section in live yaml");
    assert!(
        live_indexer_sec.contains("AXON_LIVE_DATABASE_URL"),
        "axon-indexer in process-compose.live.yaml must declare AXON_LIVE_DATABASE_URL"
    );

    // 2. AXON_DEV_DATABASE_URL dans process-compose.dev.yaml pour axon-indexer
    let dev_indexer_sec = dev_yaml
        .split("axon-indexer:")
        .nth(1)
        .expect("axon-indexer section in dev yaml");
    assert!(
        dev_indexer_sec.contains("AXON_DEV_DATABASE_URL"),
        "axon-indexer in process-compose.dev.yaml must declare AXON_DEV_DATABASE_URL"
    );

    // 3. log_location déclarés pour tous les services principaux
    for (role, log_name) in &[
        ("postgres-check", "postgres-check"),
        ("axon-brain", "brain"),
        ("axon-indexer", "indexer"),
        ("dashboard", "dashboard"),
    ] {
        assert!(
            live_yaml.contains(&format!("{role}:"))
                && live_yaml.contains(&format!("/tmp/axon-live-{log_name}.log")),
            "live yaml must declare log_location for {role}"
        );
        assert!(
            dev_yaml.contains(&format!("{role}:"))
                && dev_yaml.contains(&format!("/tmp/axon-dev-{log_name}.log")),
            "dev yaml must declare log_location for {role}"
        );
    }
}

/// REQ-AXO-902656 (Feedback #427) — system_indexer_topology_note doit alerter bruyamment
/// si le superviseur rapporte une boucle de redémarrage sur axon-indexer.
#[test]
fn c4_system_indexer_topology_note_warns_on_restart_loop() {
    use crate::mcp::tools_framework_runtime_status::system_indexer_topology_note;

    let loop_obs = IndexerSupervisorObservation {
        status: "Restarting".to_string(),
        exit_code: 1,
        is_running: false,
        restarts: 2591,
        age_ms: 30,
    };

    // 1. En cas de boucle de restart, l'alerte prime même si un battement zombie traînait
    let note_loop = system_indexer_topology_note("brain", true, false, Some(&loop_obs));
    assert!(note_loop.is_some());
    let txt = note_loop.unwrap();
    assert!(
        txt.contains("⚠️ INSTABLE") && txt.contains("boucle de redémarrage"),
        "la note doit alerter sur l'instabilité du superviseur: {txt}"
    );

    // 2. En fonctionnement normal
    let healthy_obs = IndexerSupervisorObservation {
        status: "Running".to_string(),
        exit_code: 0,
        is_running: true,
        restarts: 0,
        age_ms: 100_000,
    };
    let note_healthy = system_indexer_topology_note("brain", true, true, Some(&healthy_obs));
    assert!(note_healthy.is_some());
    let txt_h = note_healthy.unwrap();
    assert!(
        txt_h.contains("vivant") && txt_h.contains("heartbeat PG frais"),
        "la note saine doit confirmer l'indexeur vivant: {txt_h}"
    );
}


