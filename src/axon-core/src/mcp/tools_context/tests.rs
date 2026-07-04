use super::ChunkCandidate;
use super::McpServer;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[test]
fn rationale_quality_flags_no_direct_traceability_as_fixable_proof_gap() {
    // REQ-AXO-901989 / NEX — a weak verdict from no_direct_traceability must
    // be self-explanatory as a fixable proof_gap (with remediation tools),
    // not read as a tool limitation.
    let states = vec![json!({"state": "no_direct_traceability", "severity": "medium"})];
    let q = McpServer::build_rationale_quality(&states, &[], &[], &[], &[]);
    assert_eq!(q["level"], "weak");
    assert_eq!(q["proof_gap"], true);
    let remediation = q["remediation"].as_str().unwrap_or("");
    assert!(
        remediation.contains("soll_attach_evidence") && remediation.contains("soll_manager"),
        "remediation must name the fix tools; got: {remediation}"
    );
}

#[test]
fn rationale_quality_no_proof_gap_when_governing_intent_present() {
    // Governing intent present, no evidence-gap states → not a proof_gap,
    // remediation null.
    let governing = vec![json!({"id": "REQ-AXO-1", "title": "relation schema"})];
    let terms = vec!["relation".to_string()];
    let q = McpServer::build_rationale_quality(&[], &governing, &[], &[], &terms);
    assert_eq!(q["proof_gap"], false);
    assert!(q["remediation"].is_null());
}

#[test]
fn rationale_quality_gates_strong_when_governing_irrelevant_to_question() {
    // REQ-AXO-901976 critère #3 — a governing requirement that shares NO
    // overlap (term / anchor) with the question must NOT yield `strong`.
    // Repro: a concept-bridge sibling REQ (off-topic, no code anchor, title
    // disjoint from the question terms) was crowning the packet `strong`.
    let terms = vec!["relation".to_string(), "schema".to_string()];

    // (a) Relevant governing req (title overlaps a question term) → strong preserved.
    let relevant = vec![json!({
        "id": "REQ-AXO-2",
        "title": "SOLL relation schema validation",
        "evidence_class": "soll_concept_bridge",
    })];
    let q = McpServer::build_rationale_quality(&[], &relevant, &[], &[], &terms);
    assert_eq!(q["level"], "strong", "term overlap must keep strong");

    // (b) Off-topic governing req (no term overlap, no anchor) → downgraded to mixed.
    let irrelevant = vec![json!({
        "id": "REQ-AXO-901631",
        "title": "embed throughput drain batching",
        "evidence_class": "soll_concept_bridge",
    })];
    let q = McpServer::build_rationale_quality(&[], &irrelevant, &[], &[], &terms);
    assert_eq!(
        q["level"], "mixed",
        "irrelevant governing must not be strong"
    );
    assert_eq!(q["proof_gap"], false);

    // (c) Direct traceability anchor (entrypoint-traced) → relevant by anchor,
    //     even when the title shares no term with the question → strong preserved.
    let anchored = vec![json!({
        "id": "REQ-AXO-3",
        "title": "completeness enforcement",
        "evidence_class": "soll_traceability",
    })];
    let q = McpServer::build_rationale_quality(&[], &anchored, &[], &[], &terms);
    assert_eq!(q["level"], "strong", "anchor overlap must keep strong");
}

fn candidate(
    source_id: &str,
    uri: &str,
    part_index: usize,
    part_count: usize,
    anchored_to_entry: bool,
    same_file_as_entry: bool,
) -> ChunkCandidate {
    ChunkCandidate {
        chunk_id: format!("{source_id}::{part_index}"),
        source_id: source_id.to_string(),
        project_code: "PRJ".to_string(),
        uri: uri.to_string(),
        content: "snippet".to_string(),
        match_reason: "entry_anchor".to_string(),
        lexical_hits: 1,
        semantic_distance: None,
        chunk_part_index: part_index,
        chunk_part_count: part_count,
        chunk_path: format!("{part_index}/{part_count}"),
        anchored_to_entry,
        same_file_as_entry,
        score: 0.0,
        reasons: Vec::new(),
        fts_rank: None,
    }
}

// REQ-AXO-901952 — the CONTAINS file_path enrichment is RAM-only. A
// hand-built snapshot (no PG round-trip) proves both directions resolve:
// forward CONTAINS (file → symbols) for resolve_file_symbol_bindings, and
// reverse CONTAINS (symbol → containing file) for resolve_containing_file_ram.
// The file node is auto-registered from the edge source (snapshot::build),
// so it need not be a declared ist.symbol node.
#[test]
fn contains_file_path_resolves_from_ram_snapshot_both_directions() {
    use crate::ist_snapshot::snapshot::{
        EdgeTriple, IstGraph, NodeFlags, NodeKind, NodeRecord, RelationType,
    };
    use crate::ist_snapshot::{evict_process_snapshot, publish_process_snapshot};

    let code = "TCF"; // test-contains-file ; single-threaded --lib run, evicted below.
    let symbol_id = "TCF::widget.rs::render".to_string();
    let file_path = "src/widget.rs".to_string();

    let nodes = vec![NodeRecord {
        id: symbol_id.clone(),
        name: "render".to_string(),
        project_code: code.to_string(),
        kind: NodeKind::Function,
        flags: NodeFlags::default(),
        complexity: None,
    }];
    let edges = vec![EdgeTriple {
        source: file_path.clone(),
        target: symbol_id.clone(),
        rel: RelationType::Contains,
    }];
    evict_process_snapshot(code);
    publish_process_snapshot(code.to_string(), Arc::new(IstGraph::build(nodes, edges)));

    let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
    let server = McpServer::new(store);

    // Reverse CONTAINS: symbol → containing file.
    assert_eq!(
        server.resolve_containing_file_ram(code, &symbol_id),
        file_path,
        "reverse CONTAINS must resolve the containing file from RAM"
    );
    // Forward CONTAINS: file → contained symbols.
    let bindings = server.resolve_file_symbol_bindings(&[(file_path.clone(), code.to_string())]);
    assert_eq!(
        bindings,
        vec![(symbol_id.clone(), file_path.clone())],
        "forward CONTAINS must bind the file's symbols from RAM"
    );
    // Cold project → empty (loud-by-absence, never a silent PG fallback).
    assert_eq!(
        server.resolve_containing_file_ram("ZZZ", &symbol_id),
        "",
        "unknown project must not resolve via any PG fallback"
    );

    evict_process_snapshot(code);
    evict_process_snapshot("ZZZ");
}

// REQ-AXO-902039 element 2/5 — RAM-fusion helpers + SOLL↔IST co-residence.
// Pure-RAM unit tests: `SollSnapshot::build` mirrors the SOLL graph without a
// PG round-trip, so these prove the RAM reimplementation of the
// symbol→governing-intent fusion reads is faithful and that the SOLL-RAM and
// IST-RAM mirrors are co-resident + traversable for the same project.
fn fusion_entry(name: &str, kind: &str, uri: &str) -> super::EntryCandidate {
    super::EntryCandidate {
        id: format!("X::{name}"),
        name: name.to_string(),
        kind: kind.to_string(),
        project_code: "TSF".to_string(),
        uri: uri.to_string(),
        lexical_hits: 0,
        exact_match: false,
        score: 0.0,
        reasons: Vec::new(),
        semantic_distance: None,
    }
}

fn fusion_snapshot() -> crate::soll_snapshot::SollSnapshot {
    use crate::soll_snapshot::{SnapshotEdge, SnapshotNode, SnapshotTraceability, SollSnapshot};
    let mk_node = |id: &str, ty: &str| SnapshotNode {
        id: id.to_string(),
        entity_type: ty.to_string(),
        title: format!("title-{id}"),
        status: "current".to_string(),
        metadata_raw: "{}".to_string(),
    };
    let mut nodes = HashMap::new();
    nodes.insert(
        "REQ-TSF-001".to_string(),
        mk_node("REQ-TSF-001", "Requirement"),
    );
    nodes.insert(
        "DEC-TSF-001".to_string(),
        mk_node("DEC-TSF-001", "Decision"),
    );
    nodes.insert("CPT-TSF-001".to_string(), mk_node("CPT-TSF-001", "Concept"));
    let edges = vec![
        SnapshotEdge {
            source_id: "DEC-TSF-001".to_string(),
            target_id: "REQ-TSF-001".to_string(),
            relation_type: "SOLVES".to_string(),
        },
        SnapshotEdge {
            source_id: "CPT-TSF-001".to_string(),
            target_id: "REQ-TSF-001".to_string(),
            relation_type: "BELONGS_TO".to_string(),
        },
    ];
    // Symbol `render` implements DEC-TSF-001 (which SOLVES REQ-TSF-001). The
    // governing node therefore has an outgoing SOLVES edge, exercising the
    // RAM relation_type preference.
    let trace = vec![SnapshotTraceability {
        id: "T-TSF-1".to_string(),
        soll_entity_type: "Decision".to_string(),
        soll_entity_id: "DEC-TSF-001".to_string(),
        artifact_type: "Symbol".to_string(),
        artifact_ref: "render".to_string(),
        artifact_status: "ok".to_string(),
    }];
    SollSnapshot::build("TSF", 1, nodes, edges, trace)
}

#[test]
fn collect_soll_traceability_ram_resolves_governing_req() {
    let snap = fusion_snapshot();
    let cands = vec![fusion_entry("render", "function", "")];
    let rows = McpServer::collect_soll_traceability_ram(&snap, &cands, 5);
    assert_eq!(rows.len(), 1, "one governing node for the `render` symbol");
    assert_eq!(rows[0]["id"], "DEC-TSF-001");
    assert_eq!(rows[0]["type"], "Decision");
    assert_eq!(rows[0]["artifact_type"], "Symbol");
    assert_eq!(rows[0]["ranking_score"], 100);
    // SOLVES preferred among the node's outgoing edges (deterministic).
    assert_eq!(rows[0]["relation_type"], "SOLVES");
    assert_eq!(rows[0]["evidence_class"], "soll_traceability");
    assert_eq!(rows[0]["ranking_reasons"][0], "direct_symbol_traceability");
    // Case-insensitive symbol match (PG used lower(artifact_ref)).
    let upper = vec![fusion_entry("RENDER", "function", "")];
    assert_eq!(
        McpServer::collect_soll_traceability_ram(&snap, &upper, 5).len(),
        1
    );
    // No match → empty (no governing intent).
    let none = vec![fusion_entry("absent_symbol", "function", "")];
    assert!(McpServer::collect_soll_traceability_ram(&snap, &none, 5).is_empty());
}

#[test]
fn snapshot_has_direct_traceability_ram_matches_pg_semantics() {
    let snap = fusion_snapshot();
    assert!(McpServer::snapshot_has_direct_traceability(
        &snap,
        &[fusion_entry("render", "function", "")]
    ));
    assert!(!McpServer::snapshot_has_direct_traceability(
        &snap,
        &[fusion_entry("nope", "function", "")]
    ));
    // Empty candidate set → false (mirrors the PG `predicates.is_empty()` guard).
    assert!(!McpServer::snapshot_has_direct_traceability(&snap, &[]));
}

#[test]
fn expand_concept_governing_entities_ram_bridges_req_and_decision() {
    let snap = fusion_snapshot();
    let mut selected = vec![json!({
        "id": "CPT-TSF-001",
        "type": "Concept",
        "title": "concept",
        "ranking_reasons": ["seed"],
    })];
    McpServer::expand_concept_governing_entities_ram(&snap, &mut selected, 5);
    let ids: Vec<&str> = selected
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_str()))
        .collect();
    assert!(
        ids.contains(&"REQ-TSF-001"),
        "concept→requirement bridge added the governing REQ"
    );
    assert!(
        ids.contains(&"DEC-TSF-001"),
        "concept→requirement→decision bridge added the governing DEC"
    );
    // Decision row carries the bridge reason + score from the RAM traversal.
    let dec = selected
        .iter()
        .find(|r| r.get("id").and_then(|v| v.as_str()) == Some("DEC-TSF-001"))
        .unwrap();
    assert_eq!(dec["ranking_reasons"][0], "concept_decision_bridge");
    assert_eq!(dec["ranking_score"], 84);
}

#[test]
fn soll_and_ist_ram_mirrors_are_coresident_for_one_project() {
    use crate::ist_snapshot::snapshot::{EdgeTriple, IstGraph, NodeFlags, NodeKind, NodeRecord};
    // IST-RAM: the `render` symbol (what the code IS).
    let symbol_id = "TSF::widget.rs::render";
    let ist = IstGraph::build(
        vec![NodeRecord {
            id: symbol_id.to_string(),
            name: "render".to_string(),
            project_code: "TSF".to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
            complexity: None,
        }],
        Vec::<EdgeTriple>::new(),
    );
    // SOLL-RAM: the REQ governing that same symbol (what it is FOR).
    let snap = fusion_snapshot();

    // Co-residence + coherence: the SAME short name `render` bridges the two
    // mirrors — IST resolves the code symbol id, SOLL resolves its governing
    // intent — under one coherent project scope, both purely in RAM.
    assert_eq!(
        ist.ids_with_short_name("render"),
        vec![symbol_id],
        "IST RAM resolves the code symbol"
    );
    let intent = McpServer::collect_soll_traceability_ram(
        &snap,
        &[fusion_entry("render", "function", "")],
        5,
    );
    assert_eq!(
        intent[0]["id"], "DEC-TSF-001",
        "SOLL RAM resolves the governing intent for the same symbol"
    );
    assert_eq!(snap.project_code, "TSF");
}

#[test]
fn resolve_scoped_symbol_id_canonical_uses_ist_ram() {
    use crate::ist_snapshot::snapshot::{IstGraph, NodeFlags, NodeKind, NodeRecord};
    use crate::ist_snapshot::{evict_process_snapshot, publish_process_snapshot};
    let code = "TSR"; // test-symbol-resolve ; single-threaded --lib run, evicted below.
    let symbol_id = "TSR::widget.rs::render".to_string();
    evict_process_snapshot(code);
    publish_process_snapshot(
        code.to_string(),
        Arc::new(IstGraph::build(
            vec![NodeRecord {
                id: symbol_id.clone(),
                name: "render".to_string(),
                project_code: code.to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::default(),
                complexity: None,
            }],
            vec![],
        )),
    );
    let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
    let server = McpServer::new(store);
    // By short name → IST RAM resolves the canonical id (no PG).
    assert_eq!(
        server
            .resolve_scoped_symbol_id_canonical("render", Some(code))
            .as_deref(),
        Some(symbol_id.as_str())
    );
    // By canonical id → recognised directly from RAM.
    assert_eq!(
        server
            .resolve_scoped_symbol_id_canonical(&symbol_id, Some(code))
            .as_deref(),
        Some(symbol_id.as_str())
    );
    evict_process_snapshot(code);
}

// REQ-AXO-902043 — `fuse` returns a symbol's governing SOLL intent (WHY) AND
// its IST impact radius (HOW) in one RAM read, WHY-primary. IST published to
// RAM + SOLL traceability seeded in PG (loaded into the snapshot cache).
#[test]
fn fuse_returns_governing_intent_and_impact_from_ram() {
    use crate::ist_snapshot::snapshot::{IstGraph, NodeFlags, NodeKind, NodeRecord};
    use crate::ist_snapshot::{evict_process_snapshot, publish_process_snapshot};
    let code = "TFU";
    let symbol_id = "TFU::widget.rs::render".to_string();
    evict_process_snapshot(code);
    publish_process_snapshot(
        code.to_string(),
        Arc::new(IstGraph::build(
            vec![NodeRecord {
                id: symbol_id.clone(),
                name: "render".to_string(),
                project_code: code.to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::default(),
                complexity: None,
            }],
            vec![],
        )),
    );
    let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
    // SOLL: a REQ governing the `render` symbol via Symbol traceability.
    store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                 VALUES ('REQ-TFU-001','Requirement','TFU','fuse req','body','planned','{}') \
                 ON CONFLICT (id) DO NOTHING",
        )
        .unwrap();
    store
            .execute(
                "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref) \
                 VALUES ('T-TFU-1','Requirement','REQ-TFU-001','Symbol','render') ON CONFLICT (id) DO NOTHING",
            )
            .unwrap();
    let server = McpServer::new(store);

    let out = server
        .axon_fuse(&json!({"symbol": "render", "project": code}))
        .expect("fuse returns a response");
    assert_eq!(out["data"]["status"], "ok");
    let intent = out["data"]["governing_intent"]
        .as_array()
        .expect("governing_intent array");
    assert!(
        intent.iter().any(|n| n["id"] == "REQ-TFU-001"),
        "the governing REQ is fused in: {intent:?}"
    );
    // Impact radius present (render has no callers in this fixture → 0).
    assert!(out["data"]["impact"]["radius"].is_i64());
    assert_eq!(out["data"]["fusion_provenance"]["soll"], "soll_ram");
    evict_process_snapshot(code);
}

#[test]
fn fuse_unresolved_symbol_is_loud_not_pg_fallback() {
    let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
    let server = McpServer::new(store);
    let out = server
        .axon_fuse(&json!({"symbol": "no_such_symbol_xyz", "project": "ZZZ"}))
        .expect("fuse returns a response");
    assert_eq!(out["data"]["status"], "input_not_found");
    assert_eq!(out["isError"], true);
}

// REQ-AXO-901947 slice 2 — the reactive repair form fills a project field's
// valid_values with the registered project codes (real registry), so a
// wrong-project failure surfaces the valid set without a second round-trip.
#[test]
fn enrich_form_dynamic_values_fills_project_codes_from_registry() {
    let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
    let server = McpServer::new(store);
    let mut form = vec![
        json!({"name": "project", "required": false, "type": "string"}),
        json!({"name": "symbol", "required": true, "type": "string"}),
    ];
    server.enrich_form_dynamic_values(&mut form);
    let project = form.iter().find(|f| f["name"] == "project").unwrap();
    let vv: Vec<&str> = project["valid_values"]
        .as_array()
        .expect("project valid_values filled from registry")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(vv.contains(&"AXO"), "registered codes surfaced: {vv:?}");
    // Non-project field is left untouched.
    let symbol = form.iter().find(|f| f["name"] == "symbol").unwrap();
    assert!(symbol.get("valid_values").is_none());
}

#[test]
fn multipart_uri_reuse_allows_one_adjacent_anchor_chunk() {
    let first = candidate("PRJ::sym", "/repo/file.rs", 1, 3, true, true);
    let second = candidate("PRJ::sym", "/repo/file.rs", 2, 3, true, true);
    let third = candidate("PRJ::sym", "/repo/file.rs", 3, 3, true, true);
    let other = candidate("PRJ::other", "/repo/file.rs", 1, 1, false, false);

    let mut seen_uris = HashSet::new();
    seen_uris.insert(first.uri.clone());
    let mut selected_source_parts = HashMap::new();
    selected_source_parts.insert(first.source_id.clone(), vec![1]);

    assert!(super::util::can_reuse_uri_for_multipart(
        &second,
        &seen_uris,
        &selected_source_parts
    ));
    assert!(!super::util::can_reuse_uri_for_multipart(
        &third,
        &seen_uris,
        &selected_source_parts
    ));
    assert!(!super::util::can_reuse_uri_for_multipart(
        &other,
        &seen_uris,
        &selected_source_parts
    ));

    selected_source_parts.insert(first.source_id.clone(), vec![1, 2]);
    assert!(!super::util::can_reuse_uri_for_multipart(
        &third,
        &seen_uris,
        &selected_source_parts
    ));
}

// REQ-AXO-901757 slice B3b — the semantic SOLL arm returns the node whose
// description embedding is nearest the question vector, project-scoped, with
// the distinct `soll_semantic_ann` evidence_class. Synthetic axis vectors
// (no embedder, GUI-PRO-004): a query on axis-k is closest to the node
// embedded on axis-k.
#[test]
fn collect_soll_entities_via_ann_returns_nearest_project_scoped() {
    let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
    fn axis_vec(axis: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; crate::embedding_contract::DIMENSION];
        v[axis] = 1.0;
        v
    }
    for (id, axis) in [
        ("REQ-TST-001", 0usize),
        ("REQ-TST-002", 1),
        ("REQ-TST-003", 2),
    ] {
        store
                .execute(&format!(
                    "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                     VALUES ('{id}', 'Requirement', 'TST', 'node {id}', 'semantic body', 'planned', '{{}}') \
                     ON CONFLICT (id) DO NOTHING"
                ))
                .unwrap();
        store
            .upsert_soll_node_embedding(id, "TST", &format!("h-{axis}"), &axis_vec(axis), 0)
            .unwrap();
    }
    let server = McpServer::new(store);

    let hits = server.collect_soll_entities_via_ann(&axis_vec(1), Some("TST"), 3);
    assert!(!hits.is_empty(), "semantic SOLL arm returns results");
    assert_eq!(
        hits[0].get("id").and_then(|v| v.as_str()),
        Some("REQ-TST-002"),
        "nearest SOLL node by embedding ranks first: {hits:?}"
    );
    assert_eq!(
        hits[0].get("evidence_class").and_then(|v| v.as_str()),
        Some("soll_semantic_ann"),
        "semantic hits carry the distinct evidence_class"
    );
    // Project scoping: an unrelated project yields nothing.
    assert!(
        server
            .collect_soll_entities_via_ann(&axis_vec(1), Some("ZZZ"), 3)
            .is_empty(),
        "ANN results are project-scoped on the soll.Node join"
    );
}

// REQ-AXO-901757 slice B3b — fusion is a union by id: a node present from
// traceability keeps its tier but gains the semantic reason; a genuinely new
// semantic node is appended.
#[test]
fn merge_soll_entities_unions_by_id_and_annotates_reasons() {
    let mut base = vec![json!({
        "id": "REQ-AXO-1",
        "type": "Requirement",
        "ranking_reasons": ["direct_symbol_traceability"],
        "ranking_score": 100,
        "evidence_class": "soll_traceability",
    })];
    let additions = vec![
        json!({
            "id": "REQ-AXO-1",
            "type": "Requirement",
            "ranking_reasons": ["semantic_ann (cosine_distance=0.012)"],
            "ranking_score": 88,
            "evidence_class": "soll_semantic_ann",
        }),
        json!({
            "id": "DEC-AXO-2",
            "type": "Decision",
            "ranking_reasons": ["semantic_ann (cosine_distance=0.100)"],
            "ranking_score": 81,
            "evidence_class": "soll_semantic_ann",
        }),
    ];
    McpServer::merge_soll_entities(&mut base, additions);
    assert_eq!(base.len(), 2, "duplicate id merged, new id appended");
    let first = &base[0];
    assert_eq!(
        first["evidence_class"], "soll_traceability",
        "traceability tier preserved"
    );
    let reasons = first["ranking_reasons"].as_array().unwrap();
    assert_eq!(
        reasons.len(),
        2,
        "semantic reason appended to the traceability node"
    );
    assert!(reasons
        .iter()
        .any(|r| r.as_str().unwrap_or("").starts_with("semantic_ann")));
    assert_eq!(base[1]["id"], "DEC-AXO-2", "new semantic node appended");
}

// REQ-AXO-902018 tier A — the degradation notice is emitted (fail-loud) only
// for pressure / backlog semantic skips, classified TRANSIENT_UNAVAILABILITY,
// and never for a non-degradation reason or no reason at all.
#[test]
fn build_degradation_notice_fails_loud_for_pressure_skip_only() {
    use super::ServicePressure;

    let crit = McpServer::build_degradation_notice(
        Some("semantic_chunk_search_skipped_due_to_pressure_critical"),
        ServicePressure::Critical,
        false,
    )
    .expect("pressure skip emits a notice");
    assert_eq!(crit["class"], "TRANSIENT_UNAVAILABILITY");
    assert_eq!(crit["degraded"], true);
    assert_eq!(crit["semantic_rerank_applied"], false);
    assert!(crit["remediation"].as_str().unwrap().contains("retry"));

    let backlog = McpServer::build_degradation_notice(
        Some("semantic_chunk_search_skipped_due_to_vector_backlog"),
        ServicePressure::Healthy,
        false,
    )
    .expect("backlog skip emits a notice");
    assert!(backlog["remediation"].as_str().unwrap().contains("backlog"));

    // REQ-AXO-902018 tier B — when the lite re-rank ran, the notice says so
    // and the impact is framed as better-than-lexical-only.
    let reranked = McpServer::build_degradation_notice(
        Some("semantic_chunk_search_skipped_due_to_pressure_critical"),
        ServicePressure::Critical,
        true,
    )
    .expect("notice still emitted when re-rank applied");
    assert_eq!(reranked["semantic_rerank_applied"], true);
    assert!(reranked["impact"].as_str().unwrap().contains("re-ranked"));

    assert!(
        McpServer::build_degradation_notice(None, ServicePressure::Healthy, false).is_none(),
        "no reason → no notice"
    );
    assert!(
        McpServer::build_degradation_notice(
            Some("graph_expansion_disabled"),
            ServicePressure::Critical,
            false,
        )
        .is_none(),
        "a non-semantic reason is not a retrieval-degradation notice"
    );
}

// REQ-AXO-902023 tier C.1 — bounded `wait_for_semantic` recovery loop.
#[test]
fn resolve_pressure_with_wait_no_budget_samples_once() {
    use super::ServicePressure;
    use std::cell::Cell;
    let samples = Cell::new(0u32);
    let (pressure, waited) = McpServer::resolve_pressure_with_wait(
        None,
        10,
        || {
            samples.set(samples.get() + 1);
            ServicePressure::Critical
        },
        |_| panic!("must not sleep without a budget"),
    );
    assert_eq!(pressure, ServicePressure::Critical);
    assert_eq!(waited, 0);
    assert_eq!(
        samples.get(),
        1,
        "exactly one sample when no wait requested"
    );
}

#[test]
fn resolve_pressure_with_wait_short_circuits_when_already_ok() {
    use super::ServicePressure;
    let (pressure, waited) = McpServer::resolve_pressure_with_wait(
        Some(1000),
        50,
        || ServicePressure::Healthy,
        |_| panic!("must not sleep when pressure already permits the corpus ANN"),
    );
    assert_eq!(pressure, ServicePressure::Healthy);
    assert_eq!(waited, 0);
}

#[test]
fn resolve_pressure_with_wait_recovers_within_budget() {
    use super::ServicePressure;
    use std::cell::Cell;
    let n = Cell::new(0u32);
    let slept = Cell::new(0u64);
    let (pressure, waited) = McpServer::resolve_pressure_with_wait(
        Some(1000),
        50,
        || {
            let c = n.get();
            n.set(c + 1);
            // Critical for the first 3 samples, then recovers.
            if c < 3 {
                ServicePressure::Critical
            } else {
                ServicePressure::Recovering
            }
        },
        |ms| slept.set(slept.get() + ms),
    );
    assert_eq!(
        pressure,
        ServicePressure::Recovering,
        "stops polling the instant pressure recovers"
    );
    assert_eq!(waited, 150, "three 50ms steps before recovery");
    assert_eq!(slept.get(), 150, "slept exactly the waited budget");
}

#[test]
fn resolve_pressure_with_wait_exhausts_budget_and_clamps_last_step() {
    use super::ServicePressure;
    use std::cell::Cell;
    let slept = Cell::new(0u64);
    let (pressure, waited) = McpServer::resolve_pressure_with_wait(
        Some(120),
        50,
        || ServicePressure::Critical,
        |ms| slept.set(slept.get() + ms),
    );
    assert_eq!(pressure, ServicePressure::Critical, "never recovered");
    assert_eq!(
        waited, 120,
        "50 + 50 + 20 (final step clamped to remaining)"
    );
    assert_eq!(slept.get(), 120);
}

#[test]
fn parse_wait_for_semantic_accepts_ms_and_bool_shorthand() {
    assert_eq!(McpServer::parse_wait_for_semantic(&json!(750)), Some(750));
    assert_eq!(
        McpServer::parse_wait_for_semantic(&json!(true)),
        Some(super::DEFAULT_WAIT_FOR_SEMANTIC_MS)
    );
    assert_eq!(McpServer::parse_wait_for_semantic(&json!(false)), None);
    assert_eq!(McpServer::parse_wait_for_semantic(&json!("soon")), None);
}

// REQ-AXO-902023 tier C.2 — composed-question split, high precision.
#[test]
fn split_composed_question_keeps_single_question_intact() {
    assert_eq!(
        McpServer::split_composed_question("How does admission control decide rejection?"),
        None
    );
    // Noun list joined by "and" — right side not interrogative → no split.
    assert_eq!(
        McpServer::split_composed_question("List the symbols in catalog and the ones in dispatch"),
        None
    );
    // Single comparison question with an embedded "and" → no split.
    assert_eq!(
        McpServer::split_composed_question(
            "What is the difference between the graph and vector pipelines?"
        ),
        None
    );
    // Too short to be composed.
    assert_eq!(McpServer::split_composed_question("why slow?"), None);
}

#[test]
fn split_composed_question_splits_on_multiple_terminators() {
    let parts =
        McpServer::split_composed_question("How does admission work? Why was the queue chosen?")
            .expect("two '?' → composed");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "How does admission work?");
    assert_eq!(parts[1], "Why was the queue chosen?");
}

#[test]
fn split_composed_question_splits_on_interrogative_coordinator() {
    let parts =
        McpServer::split_composed_question("How does admission work and why was the queue chosen?")
            .expect("coordinator + interrogative right → composed");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "How does admission work");
    assert_eq!(parts[1], "why was the queue chosen?");
}

#[test]
fn split_composed_question_handles_french_cues() {
    let parts = McpServer::split_composed_question(
        "comment fonctionne l'admission et pourquoi la file est choisie ?",
    )
    .expect("FR comment/pourquoi → composed");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "comment fonctionne l'admission");
    assert!(parts[1].starts_with("pourquoi la file"));
}

// REQ-AXO-901653 slice-5c — `retrieve_context_retains_adjacent_chunks_for_split_symbol`
// deleted ; relied on v1 worker::DbWriteTask + insert_file_data_batch path.
// Pipeline_v2 ingestion harness rewrite tracked by REQ-AXO-901663.

#[test]
fn rerank_prefers_head_and_adjacent_multipart_chunks() {
    let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
    let server = McpServer::new(store);
    let entry_candidates = vec![super::EntryCandidate {
        id: "PRJ::file.rs::multipart_lookup_probe".to_string(),
        name: "multipart_lookup_probe".to_string(),
        kind: "function".to_string(),
        project_code: "PRJ".to_string(),
        uri: "/repo/file.rs".to_string(),
        lexical_hits: 1,
        exact_match: true,
        score: 1.0,
        reasons: vec!["exact".to_string()],
        semantic_distance: None,
    }];
    let mut candidates = vec![
        candidate(
            "PRJ::file.rs::multipart_lookup_probe",
            "/repo/file.rs",
            4,
            4,
            true,
            true,
        ),
        candidate(
            "PRJ::file.rs::multipart_lookup_probe",
            "/repo/file.rs",
            2,
            4,
            true,
            true,
        ),
        candidate(
            "PRJ::file.rs::multipart_lookup_probe",
            "/repo/file.rs",
            1,
            4,
            true,
            true,
        ),
    ];

    server.rerank_chunk_candidates(
        &mut candidates,
        super::RetrievalRoute::ExactLookup,
        &["multipart_lookup_probe".to_string()],
        &entry_candidates,
        &["PRJ".to_string()],
        false,
        false,
    );

    assert_eq!(candidates[0].chunk_part_index, 1);
    assert_eq!(candidates[1].chunk_part_index, 2);
    assert_eq!(candidates[2].chunk_part_index, 4);
    assert!(candidates[0]
        .reasons
        .iter()
        .any(|reason| reason == "multipart_lead_chunk"));
    assert!(candidates[1]
        .reasons
        .iter()
        .any(|reason| reason == "multipart_adjacent_continuation_bonus"));
    assert!(candidates[2]
        .reasons
        .iter()
        .any(|reason| reason == "multipart_late_chunk_penalty"));
}

/// REQ-AXO-901937 / DEC-AXO-901632 — NL→entrypoint precision eval.
///
/// Deterministic guard for the semantic-primary entry ordering. Semantic
/// distances are injected (rather than embedded live) so the reranker's
/// ordering contract is asserted in isolation, free of HNSW approximation.
/// The repro case mirrors the AXO RCA: a bare `soll` table (exact lexical
/// anchor, weak semantics) must NOT outrank the real target method
/// (`insert_validated_relation`, no lexical hit because of the FR↔EN gap,
/// but strong semantics) on an open-question route.
#[test]
fn entry_rerank_semantic_primary_on_open_questions() {
    fn entry(
        name: &str,
        kind: &str,
        uri: &str,
        lexical_hits: usize,
        exact_match: bool,
        semantic_distance: Option<f64>,
    ) -> super::EntryCandidate {
        super::EntryCandidate {
            id: format!("AXO::{uri}::{name}"),
            name: name.to_string(),
            kind: kind.to_string(),
            project_code: "AXO".to_string(),
            uri: uri.to_string(),
            lexical_hits,
            exact_match,
            score: 0.0,
            reasons: Vec::new(),
            semantic_distance,
        }
    }

    let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
    let server = McpServer::new(store);

    let scope = vec!["AXO".to_string()];
    let terms = vec![
        "soll".to_string(),
        "relations".to_string(),
        "validés".to_string(),
    ];
    let no_hints: Vec<String> = Vec::new();

    let soll = entry(
        "soll",
        "table",
        "/repo/db/ddl/01_soll_schema.sql",
        1,
        true,
        Some(0.46),
    );
    let target = entry(
        "insert_validated_relation",
        "method",
        "/repo/src/axon-core/src/mcp/tools_soll/completeness_relations.rs",
        0,
        false,
        Some(0.18),
    );

    // Case 1 (repro) — open question: semantic relevance wins over bare lexical.
    let mut hybrid = vec![soll.clone(), target.clone()];
    server.rerank_entry_candidates(
        &mut hybrid,
        super::RetrievalRoute::Hybrid,
        &terms,
        &no_hints,
        &scope,
        false,
    );
    assert_eq!(
            hybrid[0].name, "insert_validated_relation",
            "open-question route must pick the semantically-relevant entrypoint, not the bare lexical match"
        );
    assert!(hybrid[0]
        .reasons
        .iter()
        .any(|reason| reason == "semantic_primary_order"));

    // Case 2 (non-regression) — precise route keeps exact lexical primacy.
    let mut exact = vec![soll.clone(), target.clone()];
    server.rerank_entry_candidates(
        &mut exact,
        super::RetrievalRoute::ExactLookup,
        &terms,
        &no_hints,
        &scope,
        false,
    );
    assert_eq!(
        exact[0].name, "soll",
        "precise routes must keep exact lexical anchor primacy"
    );

    // Case 3 — open question, nothing clears the relevance threshold:
    // no semantic reshuffle on noise, lexical order preserved.
    let mut noisy = vec![
        entry(
            "soll",
            "table",
            "/repo/db/ddl/01_soll_schema.sql",
            1,
            true,
            Some(0.55),
        ),
        entry("unrelated_fn", "method", "/repo/x.rs", 0, false, Some(0.6)),
    ];
    server.rerank_entry_candidates(
        &mut noisy,
        super::RetrievalRoute::Hybrid,
        &terms,
        &no_hints,
        &scope,
        false,
    );
    assert_eq!(
        noisy[0].name, "soll",
        "no semantic-primary reshuffle when nothing clears the relevance threshold"
    );

    // Case 4 — a relevant semantic hit outranks a lexically-stronger candidate
    // that has no embedding (`None` sorts last under semantic-primary).
    let mut mixed = vec![
        entry("soll_registry", "table", "/repo/reg.sql", 3, true, None),
        entry(
            "relation_policy",
            "method",
            "/repo/policy.rs",
            0,
            false,
            Some(0.2),
        ),
    ];
    server.rerank_entry_candidates(
        &mut mixed,
        super::RetrievalRoute::SollHybrid,
        &terms,
        &no_hints,
        &scope,
        false,
    );
    assert_eq!(
        mixed[0].name, "relation_policy",
        "relevant semantic hit outranks a lexically-strong candidate with no embedding"
    );

    // Case 5 — a doc/prose section embeds closest to the NL question but must
    // NOT outrank a relevant code candidate (code-retrieval tool intent).
    let mut doc_vs_code = vec![
        entry(
            "4.1 Règles minimales",
            "section",
            "/repo/docs/notes.md",
            0,
            false,
            Some(0.10),
        ),
        entry(
            "insert_validated_relation",
            "method",
            "/repo/x.rs",
            0,
            false,
            Some(0.30),
        ),
    ];
    server.rerank_entry_candidates(
        &mut doc_vs_code,
        super::RetrievalRoute::Hybrid,
        &terms,
        &no_hints,
        &scope,
        false,
    );
    assert_eq!(
        doc_vs_code[0].name, "insert_validated_relation",
        "a relevant code candidate must outrank a semantically-closer doc section"
    );

    // Case 6 — but with NO relevant code candidate, the best doc still anchors.
    let mut doc_only = vec![
        entry(
            "4.1 Règles minimales",
            "section",
            "/repo/docs/notes.md",
            0,
            false,
            Some(0.10),
        ),
        entry("far_method", "method", "/repo/x.rs", 0, false, Some(0.62)),
    ];
    server.rerank_entry_candidates(
        &mut doc_only,
        super::RetrievalRoute::Hybrid,
        &terms,
        &no_hints,
        &scope,
        false,
    );
    assert_eq!(
        doc_only[0].name, "4.1 Règles minimales",
        "with no relevant code candidate, the best doc may anchor the packet"
    );

    // Case 7 (REQ-AXO-901937 live regression) — a TEST embeds closest to the
    // NL question (`test_*` mirrors its vocabulary) but must NOT outrank the
    // relevant production definition site on an open-question route. Mirrors
    // the live repro where `test_soll_relation_schema_resolves_pair_by_ids`
    // crowned the entrypoint over `insert_validated_relation`.
    let mut test_vs_code = vec![
        entry(
            "test_soll_relation_schema_resolves_pair_by_ids",
            "function",
            "/repo/src/axon-core/src/mcp/tests/soll_and_guidelines.rs",
            2,
            false,
            Some(0.12),
        ),
        entry(
            "insert_validated_relation",
            "method",
            "/repo/src/axon-core/src/mcp/tools_soll/completeness_relations.rs",
            0,
            false,
            Some(0.20),
        ),
    ];
    server.rerank_entry_candidates(
        &mut test_vs_code,
        super::RetrievalRoute::Hybrid,
        &terms,
        &no_hints,
        &scope,
        false,
    );
    assert_eq!(
        test_vs_code[0].name, "insert_validated_relation",
        "a test must not outrank the relevant production definition site (provenance demotion)"
    );

    // Case 8 — the whole non-production-code provenance class is secondary,
    // not just tests: a benchmark is likewise demoted below relevant
    // production code, reusing the canonical `evidence_provenance_for_uri`.
    let mut bench_vs_code = vec![
        entry(
            "bench_relation_throughput",
            "function",
            "/repo/benchmark/rel_bench.rs",
            1,
            false,
            Some(0.11),
        ),
        entry(
            "insert_validated_relation",
            "method",
            "/repo/src/x.rs",
            0,
            false,
            Some(0.22),
        ),
    ];
    server.rerank_entry_candidates(
        &mut bench_vs_code,
        super::RetrievalRoute::Hybrid,
        &terms,
        &no_hints,
        &scope,
        false,
    );
    assert_eq!(
        bench_vs_code[0].name, "insert_validated_relation",
        "a benchmark must not outrank the relevant production definition site"
    );

    // Case 9 (symmetry) — with NO relevant production code, the best test
    // still anchors the packet (graceful, mirrors the doc case 6).
    let mut test_only = vec![
        entry(
            "test_relation_schema",
            "function",
            "/repo/src/axon-core/src/mcp/tests/soll_and_guidelines.rs",
            1,
            false,
            Some(0.10),
        ),
        entry(
            "far_method",
            "method",
            "/repo/src/x.rs",
            0,
            false,
            Some(0.61),
        ),
    ];
    server.rerank_entry_candidates(
        &mut test_only,
        super::RetrievalRoute::Hybrid,
        &terms,
        &no_hints,
        &scope,
        false,
    );
    assert_eq!(
        test_only[0].name, "test_relation_schema",
        "with no relevant production code, the best test may anchor the packet"
    );
}

/// REQ-AXO-901883 — the semantic-lane SQL hits the HNSW index (criterion 1).
///
/// This is the *structural* half of the regression guard, split out from the
/// *composition* half (`semantic_chunk_query_preserves_hybrid_composition`)
/// deliberately: HNSW is an APPROXIMATE index, so asserting that a
/// freshly-seeded outlier vector is reachable by the greedy graph traversal
/// over a shared dev table holding thousands of unrelated vectors is
/// structurally flaky (the first cut of this test failed exactly there). The
/// two acceptance concerns are therefore decoupled:
///   - index USE (here) is proven via `EXPLAIN` on the production-built query
///     run through the production `query_ann_json` executor — fully
///     deterministic, independent of recall;
///   - composition/parse correctness is proven separately with a
///     deterministic EXACT-search variant where the seeded near chunk is
///     guaranteed to surface (see the sibling test).
///
/// We build the EXACT production query via `build_semantic_chunk_query`,
/// prefix it with `EXPLAIN (FORMAT TEXT)`, and run it through the same
/// `query_ann_json` path the lane uses (the `SET LOCAL enable_seqscan=off +
/// hnsw.ef_search` wrapper is load-bearing — without it pgvector's cost model
/// picks a Seq Scan + Sort on the few-thousand-row table). Each EXPLAIN line
/// comes back as a one-cell JSON row; we assert the ANN stage plan contains
/// `chunk_embedding_hnsw_idx` and NOT a `Seq Scan` over the embedding ORDER BY.
#[test]
fn semantic_chunk_query_uses_hnsw_index() {
    fn unit_vector_literal(axis: usize) -> String {
        let mut values = vec![0.0_f32; crate::embedding_contract::DIMENSION];
        values[axis] = 1.0;
        crate::postgres::vector::vector_literal(&values).unwrap()
    }

    let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
    let server = McpServer::new(store);

    let code = "TST".to_string();
    let qvec_literal = unit_vector_literal(0);
    let cosine_expr = format!("(ce.embedding <=> {qvec_literal})");
    let project_filter = McpServer::sql_project_filter_for_fields(Some(&code), &["c.project_code"]);

    // The exact production-built ANN-CTE query (same builder, same args shape
    // as the lane). EXPLAIN never touches rows, so this is index-state-only —
    // no recall dependency, no seeding required.
    let query = McpServer::build_semantic_chunk_query(
        &cosine_expr,
        &qvec_literal,
        40,
        &project_filter,
        "1=0",
        "1=0",
        "1=0",
        "1=0",
        "1=0",
        10,
    );
    let explain_sql = format!("EXPLAIN (FORMAT TEXT) {query}");

    // Production executor: the SET LOCAL enable_seqscan=off / hnsw.ef_search
    // wrapper. Without that wrapper pgvector picks Seq Scan + Sort at this
    // table size — so running EXPLAIN through THIS path proves the wrapper is
    // what flips the plan to the HNSW index (acceptance criterion 1).
    let raw = server
        .graph_store
        .query_ann_json(&explain_sql, 64)
        .expect("EXPLAIN on the ANN query must execute");
    let rows: Vec<Vec<serde_json::Value>> =
        serde_json::from_str(&raw).expect("EXPLAIN result must be a JSON row array");
    let plan = rows
        .iter()
        .filter_map(|row| row.first().and_then(|v| v.as_str()))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        plan.contains("chunk_embedding_hnsw_idx"),
        "ANN stage must use the HNSW index, not Seq Scan + Sort. Plan:\n{plan}"
    );
    // The ANN distance ORDER BY must not fall back to a Seq Scan over the
    // embedding column. (A Seq Scan elsewhere — e.g. on a tiny constant
    // subquery — is fine; what must not appear is a seq-scan-then-sort on
    // ist.ChunkEmbedding for the <=> ordering, which the HNSW line replaces.)
    assert!(
            !plan.contains("Sort Key: (ce.embedding <=>"),
            "ANN distance ordering must be served by the HNSW index, not an explicit Sort. Plan:\n{plan}"
        );
}

/// REQ-AXO-901883 — the semantic lane preserves hybrid composition + parses
/// the distance correctly (criterion 2), proven DETERMINISTICALLY.
///
/// Companion to `semantic_chunk_query_uses_hnsw_index`. Because HNSW is
/// approximate (see that test's docs), this assertion CANNOT depend on the
/// freshly-seeded outlier being recalled from the production global ANN pool
/// over the polluted shared dev table. So it exercises the IDENTICAL CTE
/// shape but with the ANN sub-pool scoped to this test's project and run
/// through the plain reader (exact distance scan, no HNSW): the two seeded
/// rows ARE the entire candidate pool, so the near chunk is guaranteed to
/// surface. This still exercises the same merge / dedup / source-tag CASE /
/// `parse_f64_value` rendering that the production lane relies on, so it
/// catches the regressions the first cut targeted:
///   - the alias bug (`missing FROM-clause entry for table "a"`) → Err;
///   - the FLOAT8-as-string distance drop → near chunk would have None dist;
///   - a broken UNION / source-tag CASE → wrong tag or missing arm.
/// It asserts:
///   1. the SQL executes (no REQ-AXO-129 error envelope);
///   2. the near chunk surfaces tagged `semantic` with a real sub-0.55 dist;
///   3. the far chunk surfaces tagged `lexical` with NULL dist (hybrid arm).
#[test]
fn semantic_chunk_query_preserves_hybrid_composition() {
    // 1024-d unit vector on `axis`; everything else 0.0. Cosine distance
    // between two distinct axes is 1.0 (> 0.55 threshold) and an axis with
    // itself is 0.0 (< threshold) → deterministic near/far without an embedder.
    fn unit_vector_literal(axis: usize) -> String {
        let mut values = vec![0.0_f32; crate::embedding_contract::DIMENSION];
        values[axis] = 1.0;
        crate::postgres::vector::vector_literal(&values).unwrap()
    }

    let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
    let server = McpServer::new(store);
    let model_id = crate::embedding_contract::CHUNK_MODEL_ID;

    // Each test runs against its own ephemeral clone (DEC-AXO-901634), so a
    // fixed literal scope isolates the fixtures; derive every fixture id from
    // it and scope the query's project_filter to it.
    let code = "TST".to_string();
    let near_id = format!("chunk-near-{code}");
    let far_id = format!("chunk-far-{code}");
    let near_path = format!("src/near-{code}.rs");
    let far_path = format!("src/far-{code}.rs");
    let near_hash = format!("hash-near-{code}");
    let far_hash = format!("hash-far-{code}");

    // Seed the IST FK targets (Project, then IndexedFile) the Chunk rows
    // reference — same shape as `seed_ist_path` in the soll_and_guidelines
    // module. The Chunk/Embedding FKs point at `axon.Project`.
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO axon.Project (code) VALUES ('{code}') ON CONFLICT (code) DO NOTHING"
        ))
        .unwrap();
    for path in [&near_path, &far_path] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO ist.IndexedFile (path, project_code, last_seen_ms) \
                     VALUES ('{path}', '{code}', 0) ON CONFLICT (path) DO NOTHING"
            ))
            .unwrap();
    }

    // NEAR chunk: embedding aligned with the query vector → distance 0 →
    // passes the cosine threshold → must surface as `semantic`.
    // (Column shape mirrors the proven content-bearing insert in the
    // soll_and_guidelines test module: kind=function + start/end lines.)
    server
            .graph_store
            .execute(&format!(
                "INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                 VALUES ('{near_id}', 'symbol', 'sym-{near_id}', '{code}', '{near_path}', 'function', 'totally unrelated body text', '{near_hash}', 1, 5)"
            ))
            .unwrap();
    // FAR chunk: orthogonal embedding → distance 1.0 → fails the cosine
    // threshold; but its content carries the lexical term → surfaces as `lexical`.
    server
            .graph_store
            .execute(&format!(
                "INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                 VALUES ('{far_id}', 'symbol', 'sym-{far_id}', '{code}', '{far_path}', 'function', 'this body mentions widgetlexeme-{code} explicitly', '{far_hash}', 1, 5)"
            ))
            .unwrap();

    server
            .graph_store
            .execute(&format!(
                "INSERT INTO ist.ChunkEmbedding (chunk_id, model_id, project_code, source_hash, embedding, embedded_at_ms) \
                 VALUES ('{near_id}', '{model_id}', '{code}', '{near_hash}', {emb}, 0)",
                emb = unit_vector_literal(0)
            ))
            .unwrap();
    server
            .graph_store
            .execute(&format!(
                "INSERT INTO ist.ChunkEmbedding (chunk_id, model_id, project_code, source_hash, embedding, embedded_at_ms) \
                 VALUES ('{far_id}', '{model_id}', '{code}', '{far_hash}', {emb}, 0)",
                emb = unit_vector_literal(1)
            ))
            .unwrap();

    let qvec_literal = unit_vector_literal(0);
    let cosine_expr = format!("(ce.embedding <=> {qvec_literal})");
    let project_filter = McpServer::sql_project_filter_for_fields(Some(&code), &["c.project_code"]);
    // Lexical arm: a per-invocation term matches only THIS test's FAR chunk.
    // `c.content` is lowercased by the predicate, so the pattern must be too
    // (the project code is uppercase).
    let lexical_predicate = format!(
        "lower(c.content) LIKE '%widgetlexeme-{lc}%'",
        lc = code.to_ascii_lowercase()
    );

    let production_query = McpServer::build_semantic_chunk_query(
        &cosine_expr,
        &qvec_literal,
        40,
        &project_filter,
        "1=0",              // entry_id_match (none)
        "1=0",              // entry_uri_match (none)
        &lexical_predicate, // lexical_predicate
        "1=0",              // lexical_uri_match (none)
        "1=0",              // path_match (none)
        10,
    );

    // Deterministic composition variant of the IDENTICAL CTE shape: scope the
    // ANN sub-pool to THIS test's project so the two seeded rows are the whole
    // candidate set (HNSW recall of the freshly-seeded outlier over the shared
    // dev table is NOT guaranteed — that concern is covered structurally by
    // `semantic_chunk_query_uses_hnsw_index` via EXPLAIN). The only edit is the
    // ANN CTE's FROM clause; everything downstream (sem/lex UNION, dedup,
    // source-tag CASE, distance projection/parse) is byte-identical to prod.
    let ann_from = "FROM ist.ChunkEmbedding ce ";
    let ann_from_scoped = format!("FROM ist.ChunkEmbedding ce WHERE ce.project_code = '{code}' ");
    assert!(
        production_query.contains(ann_from),
        "ANN CTE FROM clause shape changed; update the composition test scoping"
    );
    let query = production_query.replacen(ann_from, &ann_from_scoped, 1);

    // Plain reader = exact distance scan (no HNSW), fully deterministic. A
    // parse/bind error (the alias bug) returns the REQ-AXO-129 error envelope
    // → `Err`, which still fails here.
    let raw = server
        .graph_store
        .query_json(&query)
        .expect("semantic composition query must execute (no SQL parse/bind error)");
    let rows: Vec<Vec<serde_json::Value>> =
        serde_json::from_str(&raw).expect("query result must be a JSON row array");

    // Composition: both arms surfaced.
    let semantic_row = rows
        .iter()
        .find(|row| row.first().and_then(|v| v.as_str()) == Some(near_id.as_str()))
        .expect("near chunk must surface via the semantic arm");
    assert_eq!(
        semantic_row.get(8).and_then(|v| v.as_str()),
        Some("semantic"),
        "near chunk must be tagged `semantic`: {semantic_row:?}"
    );
    // The native reader renders FLOAT8 as a string; `parse_f64_value` is the
    // exact production parse, so the test asserts the value the lane delivers.
    let near_dist = semantic_row
        .get(9)
        .and_then(super::util::parse_f64_value)
        .expect("near chunk must carry a parseable cosine distance");
    assert!(
        (0.0..0.55).contains(&near_dist),
        "near chunk distance must be under the 0.55 threshold: {near_dist} in {semantic_row:?}"
    );

    let lexical_row = rows
        .iter()
        .find(|row| row.first().and_then(|v| v.as_str()) == Some(far_id.as_str()))
        .expect("far chunk must surface via the lexical arm (hybrid composition)");
    assert_eq!(
            lexical_row.get(8).and_then(|v| v.as_str()),
            Some("lexical"),
            "far chunk must be tagged `lexical` (excluded from semantic arm by threshold): {lexical_row:?}"
        );
    // NULL renders as the literal string `"null"` → `parse_f64_value` is None.
    assert!(
        lexical_row
            .get(9)
            .and_then(super::util::parse_f64_value)
            .is_none(),
        "lexical-arm row must have an absent (NULL) distance: {lexical_row:?}"
    );
}
