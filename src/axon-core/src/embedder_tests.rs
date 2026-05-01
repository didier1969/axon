// REQ-AXO-087 — `unavailable_embedding_reason` must distinguish
// profile-design exclusion (brain_only / indexer_graph never start the
// vector worker by design) from transient unavailability
// (indexer_vector / indexer_full own the worker but it is not yet up).
// The wording lets an LLM client decide whether to retry or fall back
// permanently to structural search. Companion path satisfies
// GUI-PRO-001 (REQ-AXO-121).

use crate::embedder::unavailable_embedding_reason;
use crate::runtime_mode::AxonRuntimeMode;

#[test]
fn brain_only_reason_signals_profile_exclusion() {
    let msg = unavailable_embedding_reason(AxonRuntimeMode::BrainOnly);
    assert!(
        msg.contains("brain_only") && msg.contains("profile change"),
        "must name the profile and tell the LLM the absence is structural: {msg}"
    );
    assert!(
        !msg.contains("not ready"),
        "must not use 'not ready' which implies transient: {msg}"
    );
}

#[test]
fn indexer_graph_reason_also_signals_profile_exclusion() {
    let msg = unavailable_embedding_reason(AxonRuntimeMode::IndexerGraph);
    assert!(
        msg.contains("indexer_graph") && msg.contains("profile change"),
        "indexer_graph profile excludes vector workers by design: {msg}"
    );
}

#[test]
fn indexer_full_reason_signals_transient_unavailability() {
    let msg = unavailable_embedding_reason(AxonRuntimeMode::IndexerFull);
    assert!(
        msg.contains("transient"),
        "indexer_full profile expects the worker; absence is transient: {msg}"
    );
    assert!(
        !msg.contains("profile change"),
        "must not suggest a profile change for transient outage: {msg}"
    );
}

#[test]
fn indexer_vector_reason_signals_transient_unavailability() {
    let msg = unavailable_embedding_reason(AxonRuntimeMode::IndexerVector);
    assert!(
        msg.contains("transient"),
        "indexer_vector profile expects the worker; absence is transient: {msg}"
    );
}
