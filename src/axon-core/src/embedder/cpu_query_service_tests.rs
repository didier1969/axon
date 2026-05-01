// REQ-AXO-128 — sibling tests for the CPU query embedder spawner.
// The model load happens off-thread inside the spawned worker; here we
// assert the spawn contract (no-op for indexer profiles, batch_embed
// either succeeds via the registered worker OR surfaces a structured
// error if the model snapshot is unavailable in the test environment).

use std::time::Duration;

use crate::embedder::batch_embed;
use crate::runtime_mode::AxonRuntimeMode;

use super::spawn_brain_query_worker_if_needed;

#[test]
fn spawn_is_noop_for_indexer_profiles() {
    // Indexer profiles have their own GPU-backed worker pool; the CPU
    // spawner must not interfere. Calling it is a no-op contract — no
    // panic, no sender registration that could conflict with the GPU
    // pipeline.
    spawn_brain_query_worker_if_needed(AxonRuntimeMode::IndexerFull);
    spawn_brain_query_worker_if_needed(AxonRuntimeMode::IndexerVector);
}

#[test]
fn batch_embed_short_circuits_on_empty_input_regardless_of_worker_state() {
    // Empty input must succeed without ever touching the worker — this
    // lets callers no-op safely without paying any thread / channel /
    // model-load cost. This invariant must hold whether or not a CPU
    // worker has been spawned.
    let result = batch_embed(Vec::new()).expect("empty input must succeed");
    assert!(
        result.is_empty(),
        "empty input → empty output (no embeddings to compute)"
    );
}

#[test]
fn spawn_for_brain_only_makes_batch_embed_either_succeed_or_fail_with_structured_error() {
    // Under brain_only the CPU worker should register a sender so
    // batch_embed routes through it. Whether the model loads is
    // environment-dependent (CI fixtures may not have the snapshot),
    // so we accept either:
    //  - success (model loaded → embedding returned, dim > 0); OR
    //  - structured failure (channel closed because worker exited
    //    after model build error).
    // Both are valid contract states; the only forbidden outcome is
    // an indefinite block or a panic.
    spawn_brain_query_worker_if_needed(AxonRuntimeMode::BrainOnly);

    // Give the worker a moment to either load the model or exit on
    // load failure.
    std::thread::sleep(Duration::from_millis(2000));

    let result = batch_embed(vec!["axonctl status".to_string()]);
    match result {
        Ok(embeddings) => {
            assert_eq!(embeddings.len(), 1, "one input → one embedding");
            assert!(
                !embeddings[0].is_empty(),
                "embedding must not be empty when the CPU model loaded successfully"
            );
        }
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("worker unavailable")
                    || msg.contains("CPU query embedder")
                    || msg.contains("paused under")
                    || msg.contains("Use structural search")
                    || msg.contains("timed out"),
                "load failure must surface a structured error message, not a raw panic: {msg}"
            );
        }
    }
}
