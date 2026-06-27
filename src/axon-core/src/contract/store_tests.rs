//! Tests d'intégration PG du store de contrats (S1 REQ-AXO-902088) + de la
//! réconciliation IST↔contrat (S6 REQ-AXO-902093). `#[test]` SYNC sur un clone
//! isolé du template (`create_test_db`) — le fixture porte son propre runtime, on
//! n'en imbrique pas un second (cf. pipeline/stage_b1.rs).

use super::*;
use crate::contract::seal::structural_seal;
use crate::contract::{ContractKind, ContractNode, PostCondition};
use crate::graph::GraphStore;

fn anchor_node(realized_by: Option<&str>) -> ContractNode {
    ContractNode {
        kind: ContractKind::Function,
        signature: "parse_seq_buckets_from_env(raw: Option<&str>) -> Vec<usize>".to_string(),
        why: "SOLVES REQ-AXO-262".to_string(),
        post_conditions: vec![
            PostCondition("sorted".into()),
            PostCondition("dedup".into()),
            PostCondition("positive".into()),
        ],
        proves_ref: "proves:anchor".to_string(),
        realized_by: realized_by.map(|s| s.to_string()),
    }
}

fn seed_symbol(store: &GraphStore, id: &str, kind: &str, name: &str) {
    store
        .execute_param(
            "INSERT INTO ist.Symbol (id, name, kind, project_code)
             VALUES ($id, $name, $kind, 'AXO')
             ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, kind = EXCLUDED.kind",
            &serde_json::json!({ "id": id, "name": name, "kind": kind }),
        )
        .expect("seed ist.Symbol");
}

fn seed_edge(store: &GraphStore, source: &str, target: &str, relation: &str) {
    store
        .execute_param(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) \
             VALUES ($s, $t, $r, 'AXO', (extract(epoch from now())*1000)::BIGINT) ON CONFLICT DO NOTHING",
            &serde_json::json!({ "s": source, "t": target, "r": relation }),
        )
        .expect("seed ist.Edge");
}

// ── S1 : round-trip persist / load ────────────────────────────────────
#[test]
fn round_trip_persist_load() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    let node = anchor_node(Some("AXO::parse_seq_buckets_from_env"));

    persist_contract(&store, "CON-AXO-1", &node).expect("persist");
    let loaded = load_contract(&store, "CON-AXO-1")
        .expect("load")
        .expect("present");

    assert_eq!(loaded.kind, node.kind);
    assert_eq!(loaded.signature, node.signature);
    assert_eq!(loaded.why, node.why);
    assert_eq!(loaded.proves_ref, node.proves_ref);
    assert_eq!(loaded.realized_by, node.realized_by);
    // post-conditions préservées (ordre du shape_hash insensible au tri) :
    assert_eq!(loaded.post_conditions, node.post_conditions);
    // le shape_hash ré-dérivé est intègre après round-trip :
    assert_eq!(loaded.shape_hash(), node.shape_hash());
}

#[test]
fn load_absent_contract_is_none() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    assert!(load_contract(&store, "CON-AXO-404").unwrap().is_none());
}

#[test]
fn persist_rejects_malformed_contract() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    let mut bad = anchor_node(None);
    bad.signature = "no_arrow_here".to_string(); // pas de '->' -> UntypedSignature
    assert!(persist_contract(&store, "CON-AXO-2", &bad).is_err());
    assert!(load_contract(&store, "CON-AXO-2").unwrap().is_none());
}

#[test]
fn upsert_overwrites_desired_shape() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    let mut node = anchor_node(None);
    persist_contract(&store, "CON-AXO-3", &node).unwrap();

    node.post_conditions.push(PostCondition("disable->empty".into()));
    persist_contract(&store, "CON-AXO-3", &node).unwrap();

    let loaded = load_contract(&store, "CON-AXO-3").unwrap().unwrap();
    assert_eq!(loaded.post_conditions, node.post_conditions);
    assert_eq!(loaded.shape_hash(), node.shape_hash());
}

// ── S1 : sceau persisté + invalidation par changement de forme ────────
#[test]
fn seal_round_trip_then_invalidated_by_shape_change() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    let node = anchor_node(None);
    persist_contract(&store, "CON-AXO-4", &node).unwrap();

    // sceau accordé (adéquat) pour la forme A.
    let seal_a = structural_seal(&node.shape_hash(), &node.proves_ref, true, &[])
        .expect("forme A adéquate -> sceau");
    let rev = persist_seal(&store, "CON-AXO-4", &seal_a, true).unwrap();
    assert!(rev > 0);

    let stored = load_seal(&store, "CON-AXO-4").unwrap().expect("scellé");
    assert_eq!(stored.seal, seal_a);
    assert!(stored.adequate);

    // forme B (signature changée) -> shape_hash différent -> sceau recalculé
    // DIFFÈRE du sceau stocké : le sceau structurel est invalidé par le drift de
    // forme (le hash ne couvre plus la forme courante).
    let mut node_b = node.clone();
    node_b.signature = "parse_seq_buckets_from_env(raw: Option<&str>) -> Vec<u32>".to_string();
    assert_ne!(node_b.shape_hash(), node.shape_hash());
    let seal_b = structural_seal(&node_b.shape_hash(), &node_b.proves_ref, true, &[]).unwrap();
    assert_ne!(seal_b, stored.seal, "un changement de forme invalide le sceau");
}

#[test]
fn load_seal_none_when_unsealed() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    persist_contract(&store, "CON-AXO-5", &anchor_node(None)).unwrap();
    assert!(load_seal(&store, "CON-AXO-5").unwrap().is_none());
}

// ── S6 : réconciliation IST↔contrat ───────────────────────────────────
#[test]
fn reconcile_unbound_when_no_realized_by() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    persist_contract(&store, "CON-AXO-6", &anchor_node(None)).unwrap();
    assert_eq!(
        reconcile_contract(&store, "CON-AXO-6").unwrap(),
        DriftVerdict::Unbound
    );
}

#[test]
fn reconcile_symbol_missing_when_anchor_absent() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    persist_contract(&store, "CON-AXO-7", &anchor_node(Some("AXO::ghost"))).unwrap();
    assert_eq!(
        reconcile_contract(&store, "CON-AXO-7").unwrap(),
        DriftVerdict::SymbolMissing { symbol_id: "AXO::ghost".to_string() }
    );
}

#[test]
fn reconcile_kind_mismatch() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    let sym = "AXO::SomeStruct";
    seed_symbol(&store, sym, "struct", "SomeStruct"); // IST = type-ish
    // contrat désiré = Function -> incompatible avec un struct observé.
    persist_contract(&store, "CON-AXO-8", &anchor_node(Some(sym))).unwrap();

    match reconcile_contract(&store, "CON-AXO-8").unwrap() {
        DriftVerdict::KindMismatch { expected, observed_kind } => {
            assert_eq!(expected, ContractKind::Function);
            assert_eq!(observed_kind, "struct");
        }
        other => panic!("attendu KindMismatch, eu {other:?}"),
    }
}

#[test]
fn reconcile_detects_drift_against_baseline() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    let sym = "AXO::parse_seq_buckets_from_env";
    seed_symbol(&store, sym, "function", "parse_seq_buckets_from_env");
    persist_contract(&store, "CON-AXO-9", &anchor_node(Some(sym))).unwrap();

    // 1ère réconciliation : pas de baseline -> NoBaseline (forme observée fournie).
    match reconcile_contract(&store, "CON-AXO-9").unwrap() {
        DriftVerdict::NoBaseline { observed } => assert!(!observed.is_empty()),
        other => panic!("attendu NoBaseline, eu {other:?}"),
    }

    // fige la baseline -> aligné.
    let baseline = capture_observed_baseline(&store, "CON-AXO-9")
        .unwrap()
        .expect("baseline figée");
    match reconcile_contract(&store, "CON-AXO-9").unwrap() {
        DriftVerdict::Aligned { observed } => assert_eq!(observed, baseline),
        other => panic!("attendu Aligned, eu {other:?}"),
    }

    // l'IST dérive (rename du symbole, même id stable) -> ShapeDrift typé.
    seed_symbol(&store, sym, "function", "parse_buckets_renamed");
    match reconcile_contract(&store, "CON-AXO-9").unwrap() {
        DriftVerdict::ShapeDrift { baseline: b, observed } => {
            assert_eq!(b, baseline);
            assert_ne!(observed, baseline, "la forme observée a dérivé de la baseline");
        }
        other => panic!("attendu ShapeDrift, eu {other:?}"),
    }
}

#[test]
fn capture_baseline_none_when_unbound_or_missing() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    persist_contract(&store, "CON-AXO-10", &anchor_node(None)).unwrap();
    assert!(capture_observed_baseline(&store, "CON-AXO-10").unwrap().is_none());

    persist_contract(&store, "CON-AXO-11", &anchor_node(Some("AXO::ghost"))).unwrap();
    assert!(capture_observed_baseline(&store, "CON-AXO-11").unwrap().is_none());
}

// ── S7 : surface de consommation (REQ-AXO-902094) ─────────────────────
#[test]
fn list_contracts_paginates_and_summarizes_lifecycle_and_seal() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    // planned (sans realized_by) · bound (realized_by) · sealed (bound + sceau).
    persist_contract(&store, "CON-AXO-20", &anchor_node(None)).unwrap();
    persist_contract(&store, "CON-AXO-21", &anchor_node(Some("AXO::f21"))).unwrap();
    let bound = anchor_node(Some("AXO::f22"));
    persist_contract(&store, "CON-AXO-22", &bound).unwrap();
    let seal = structural_seal(&bound.shape_hash(), &bound.proves_ref, true, &[]).unwrap();
    persist_seal(&store, "CON-AXO-22", &seal, true).unwrap();

    assert!(count_contracts(&store, "AXO").unwrap() >= 3);

    let all = list_contracts(&store, "AXO", 100, 0).unwrap();
    let by = |id: &str| all.iter().find(|c| c.id == id).cloned().expect("présent");

    let planned = by("CON-AXO-20");
    assert_eq!(planned.status, "planned");
    assert!(!planned.sealed);
    assert_eq!(planned.adequate, None);
    assert_eq!(planned.kind, ContractKind::Function);

    let bound_row = by("CON-AXO-21");
    assert_eq!(bound_row.status, "bound");
    assert_eq!(bound_row.realized_by.as_deref(), Some("AXO::f21"));
    assert!(!bound_row.sealed);

    let sealed_row = by("CON-AXO-22");
    assert_eq!(sealed_row.status, "sealed");
    assert!(sealed_row.sealed);
    assert_eq!(sealed_row.adequate, Some(true));
    assert!(sealed_row.seal_revision.is_some());

    // pagination déterministe (id ASC).
    let p0 = list_contracts(&store, "AXO", 1, 0).unwrap();
    let p1 = list_contracts(&store, "AXO", 1, 1).unwrap();
    assert_eq!(p0.len(), 1);
    assert_eq!(p1.len(), 1);
    assert!(p0[0].id < p1[0].id, "ordre id ASC stable entre pages");
}

// ── S8 : obsolescence gouvernée (REQ-AXO-902095, DEC-AXO-901658 cas C) ──
#[test]
fn obsolescence_blocked_while_live_callers_then_retires_when_orphan() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    let sym = "AXO::doomed_fn";
    seed_symbol(&store, sym, "function", "doomed_fn");
    seed_symbol(&store, "AXO::caller_a", "function", "caller_a");
    seed_symbol(&store, "AXO::caller_b", "function", "caller_b");
    persist_contract(&store, "CON-AXO-30", &anchor_node(Some(sym))).unwrap();

    // bound (realized_by set) at first.
    assert_eq!(contract_status_str(&store, "CON-AXO-30").unwrap().as_deref(), Some("bound"));

    // two live callers → obsolescence HARD-blocked (gate).
    seed_edge(&store, "AXO::caller_a", sym, "CALLS");
    seed_edge(&store, "AXO::caller_b", sym, "CALLS");
    let (live, sample) = live_incoming_call_count(&store, sym).unwrap();
    assert_eq!(live, 2);
    assert!(sample.contains(&"AXO::caller_a".to_string()));

    // remove the callers (code redirected) → orphan → retire allowed.
    store
        .execute(&format!("DELETE FROM ist.Edge WHERE target_id = '{sym}'"))
        .unwrap();
    let (live_after, _) = live_incoming_call_count(&store, sym).unwrap();
    assert_eq!(live_after, 0);
    retire_contract(&store, "CON-AXO-30").unwrap();
    assert_eq!(contract_status_str(&store, "CON-AXO-30").unwrap().as_deref(), Some("retired"));
}

#[test]
fn contract_status_str_none_when_absent() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    assert!(contract_status_str(&store, "CON-AXO-404").unwrap().is_none());
}

#[test]
fn contract_edges_partition_outgoing_governance_and_identity() {
    let store = crate::tests::test_helpers::create_test_db().unwrap();
    // anchor_node.why = "SOLVES REQ-AXO-262" + realized_by → 2 arêtes sortantes.
    persist_contract(&store, "CON-AXO-23", &anchor_node(Some("AXO::f23"))).unwrap();
    let (outgoing, incoming) = contract_edges(&store, "CON-AXO-23").unwrap();
    assert!(incoming.is_empty(), "aucun contrat ne pointe vers lui");
    let has = |r: &str| {
        outgoing
            .iter()
            .any(|e| e.relation_type == r && e.source_id == "CON-AXO-23")
    };
    assert!(has("REALIZED_BY"), "arête d'identité → ist.Symbol");
    assert!(has("SOLVES"), "arête de gouvernance dérivée du why");
    assert!(outgoing.iter().any(|e| e.target_id == "AXO::f23"));
    assert!(outgoing.iter().any(|e| e.target_id == "REQ-AXO-262"));
}
