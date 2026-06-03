//! Sanity tests for [`super`] — the fixtures themselves must round-trip
//! through the isolated PG template clone before any production test relies
//! on them.

use super::{
    assert_ist_count, create_test_server_with_ist_seed, seed_ist, CallFixture, EdgeFixture,
    IstSeed, SollNodeFixture, SymbolFixture,
};

#[test]
fn symbol_fixture_round_trips() {
    let harness = create_test_server_with_ist_seed(IstSeed::new().symbol(
        SymbolFixture::new("prj::core_func", "core_func", "function", "PRJ")
            .tested(true)
            .is_nif(false)
            .is_unsafe(false),
    ))
    .unwrap();

    assert_ist_count(
        &harness.store,
        "SELECT count(*) FROM ist.Symbol WHERE id = 'prj::core_func' AND tested = true AND is_nif = false",
        1,
    );
}

#[test]
fn call_fixture_canonical_and_synthetic_both_persist() {
    let harness = create_test_server_with_ist_seed(
        IstSeed::new()
            .symbol(SymbolFixture::new(
                "axon::wrong_project_scope_response",
                "wrong_project_scope_response",
                "method",
                "AXO",
            ))
            .call(CallFixture::canonical(
                "axon::caller_a",
                "axon::wrong_project_scope_response",
                "AXO",
            ))
            .call(CallFixture::synthetic(
                "axon::caller_b",
                "tools_dx",
                "wrong_project_scope_response",
                "AXO",
            )),
    )
    .unwrap();

    assert_ist_count(
        &harness.store,
        "SELECT count(*) FROM ist.Edge WHERE relation_type = 'CALLS' \
         AND target_id = 'axon::wrong_project_scope_response'",
        1,
    );
    assert_ist_count(
        &harness.store,
        "SELECT count(*) FROM ist.Edge WHERE relation_type = 'CALLS' \
         AND target_id = 'tools_dx::wrong_project_scope_response'",
        1,
    );
}

#[test]
fn soll_node_fixture_round_trips() {
    let harness = create_test_server_with_ist_seed(IstSeed::new().node(
        SollNodeFixture::new("REQ-AXO-9999", "Requirement", "AXO", "fixture sanity")
            .description("test fixture")
            .status("current")
            .metadata_json("{\"priority\":\"P3\"}"),
    ))
    .unwrap();

    assert_ist_count(
        &harness.store,
        "SELECT count(*) FROM soll.Node WHERE id = 'REQ-AXO-9999' \
         AND metadata->>'priority' = 'P3'",
        1,
    );
}

#[test]
fn edge_fixture_round_trips_to_ist_edge() {
    let harness = create_test_server_with_ist_seed(IstSeed::new().edge(EdgeFixture::new(
        "CONTAINS",
        "src/file.rs",
        "axon::sym",
        "AXO",
    )))
    .unwrap();

    assert_ist_count(
        &harness.store,
        "SELECT count(*) FROM ist.Edge WHERE relation_type = 'CONTAINS' \
         AND source_id = 'src/file.rs' AND target_id = 'axon::sym'",
        1,
    );
}

#[test]
fn seed_ist_escapes_single_quotes_in_values() {
    let harness = create_test_server_with_ist_seed(IstSeed::new().symbol(SymbolFixture::new(
        "prj::it's_a_func",
        "it's_a_func",
        "function",
        "PRJ",
    )))
    .unwrap();

    let _ = seed_ist(&harness.store, &IstSeed::new()).unwrap();
    assert_ist_count(
        &harness.store,
        "SELECT count(*) FROM ist.Symbol WHERE name = 'it''s_a_func'",
        1,
    );
}
