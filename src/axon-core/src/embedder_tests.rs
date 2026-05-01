// REQ-AXO-087 (revised by REQ-AXO-128 / DEC-AXO-061) — the historical
// "profile change required" wording for brain_only and indexer_graph
// is retired: those modes now route through the in-process CPU query
// embedder so semantic search works under brain_only without a profile
// change. `unavailable_embedding_reason` is now only reached when (a)
// an indexer profile's GPU subprocess is starting up (transient), or
// (b) the CPU embedder itself failed to load the model (recoverable
// config issue, not a permanent profile boundary). The wording reflects
// each case so the LLM client can decide whether to retry or report
// the missing model snapshot. Companion path satisfies GUI-PRO-001
// (REQ-AXO-121).

use crate::embedder::unavailable_embedding_reason;
use crate::runtime_mode::AxonRuntimeMode;

#[test]
fn brain_only_reason_describes_cpu_embedder_load_failure_not_profile_exclusion() {
    let msg = unavailable_embedding_reason(AxonRuntimeMode::BrainOnly);
    assert!(
        msg.contains("brain_only"),
        "must name the active profile so the LLM knows which runtime hit the issue: {msg}"
    );
    assert!(
        msg.contains("CPU query embedder unavailable")
            || msg.contains("model snapshot")
            || msg.contains("model.onnx"),
        "must point to the CPU embedder load failure, not a permanent profile boundary: {msg}"
    );
    assert!(
        !msg.contains("profile change"),
        "REQ-AXO-128 retired the 'profile change required' wording — semantic search now works under brain_only via in-process CPU embedding: {msg}"
    );
}

#[test]
fn indexer_graph_reason_describes_cpu_embedder_load_failure() {
    let msg = unavailable_embedding_reason(AxonRuntimeMode::IndexerGraph);
    assert!(
        msg.contains("indexer_graph"),
        "must name the active profile: {msg}"
    );
    assert!(
        !msg.contains("profile change"),
        "indexer_graph also routes through the CPU embedder per REQ-AXO-128: {msg}"
    );
}

#[test]
fn indexer_full_reason_signals_transient_gpu_subprocess_unavailability() {
    let msg = unavailable_embedding_reason(AxonRuntimeMode::IndexerFull);
    assert!(
        msg.contains("transient"),
        "indexer_full profile expects the GPU subprocess; absence is transient: {msg}"
    );
    assert!(
        !msg.contains("profile change"),
        "must not suggest a profile change for transient outage: {msg}"
    );
}

#[test]
fn indexer_vector_reason_signals_transient_gpu_subprocess_unavailability() {
    let msg = unavailable_embedding_reason(AxonRuntimeMode::IndexerVector);
    assert!(
        msg.contains("transient"),
        "indexer_vector profile expects the GPU subprocess; absence is transient: {msg}"
    );
}
