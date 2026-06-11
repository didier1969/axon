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

// REQ-AXO-257 — sustained bench helper + input validation.

#[test]
fn rolling_window_min_returns_none_on_empty_observations() {
    let observations: Vec<(std::time::Instant, usize, u64)> = vec![];
    let got = crate::embedder::rolling_window_min_ch_per_s(
        &observations,
        std::time::Duration::from_secs(10),
    );
    assert!(got.is_none(), "empty observations must return None");
}

#[test]
fn rolling_window_min_handles_single_observation() {
    // 100 chunks in 1000ms => 100 ch/s
    let now = std::time::Instant::now();
    let observations = vec![(now, 100usize, 1000u64)];
    let got = crate::embedder::rolling_window_min_ch_per_s(
        &observations,
        std::time::Duration::from_secs(10),
    );
    assert!(got.is_some());
    let value = got.unwrap();
    assert!(
        (value - 100.0).abs() < 0.001,
        "single observation must report its own ch/s; got {value}"
    );
}

#[test]
fn rolling_window_min_finds_dip_in_sustained_run() {
    // Build 12 observations spaced ~1s apart. First 11 observations
    // run at 100 ch/s. Observation 11 spikes downward to 10 ch/s.
    // The window ending at observation 11 should reflect the dip.
    let mut t = std::time::Instant::now();
    let mut obs: Vec<(std::time::Instant, usize, u64)> = Vec::new();
    for i in 0..12 {
        // 100 chunks per 1000ms = 100 ch/s by default
        let (chunks, ms) = if i == 11 {
            (10usize, 1000u64) // dip
        } else {
            (100usize, 1000u64)
        };
        obs.push((t, chunks, ms));
        t += std::time::Duration::from_millis(1000);
    }
    let got =
        crate::embedder::rolling_window_min_ch_per_s(&obs, std::time::Duration::from_secs(10))
            .unwrap();
    // The 10s window ending at obs 11 covers obs 2..11 (10 obs):
    // 9 obs at 100 ch/s + 1 obs at 10 ch/s in same total wall time
    // = (9*100 + 10) chunks / 10s ≈ 91 ch/s
    assert!(
        got < 95.0,
        "dip must drag rolling-min below 95 ch/s; got {got:.2}"
    );
    assert!(
        got > 80.0,
        "dip diluted by 9 healthy observations should not crash the rolling-min below 80 ch/s; got {got:.2}"
    );
}

#[test]
fn rolling_window_min_skips_zero_window_observations() {
    // Observations with ms=0 are degenerate: skip rather than divide
    // by zero.
    let now = std::time::Instant::now();
    let obs = vec![
        (now, 0usize, 0u64), // zero-ms observation, must be skipped
        (
            now + std::time::Duration::from_millis(500),
            100usize,
            1000u64,
        ),
    ];
    let got =
        crate::embedder::rolling_window_min_ch_per_s(&obs, std::time::Duration::from_secs(10))
            .unwrap();
    // Window containing both observations: 100 chunks / 1000ms = 100 ch/s
    // (the 0/0 observation contributes 0 chunks + 0ms, so it doesn't pollute)
    assert!(
        (got - 100.0).abs() < 0.001,
        "zero-ms observations must not corrupt rolling-min; got {got:.2}"
    );
}

#[test]
fn run_embedder_sustained_bench_rejects_empty_pool() {
    let res = crate::embedder::run_embedder_sustained_bench("test", vec![], 1, 0, 1, false);
    assert!(res.is_err(), "empty pool must return error");
}

#[test]
fn run_embedder_sustained_bench_rejects_zero_batch() {
    let res = crate::embedder::run_embedder_sustained_bench(
        "test",
        vec!["text".to_string()],
        0,
        0,
        1,
        false,
    );
    assert!(res.is_err(), "batch_size=0 must return error");
}

#[test]
fn run_embedder_sustained_bench_rejects_zero_sustained_secs() {
    let res = crate::embedder::run_embedder_sustained_bench(
        "test",
        vec!["text".to_string()],
        1,
        0,
        0,
        false,
    );
    assert!(res.is_err(), "sustained_secs=0 must return error");
}

#[test]
fn run_embedder_pipeline_bench_rejects_zero_channel() {
    let res = crate::embedder::run_embedder_pipeline_bench(
        "test",
        vec!["text".to_string()],
        4,
        0,
        128,
        0,
        1,
        false,
    );
    assert!(res.is_err(), "channel_capacity=0 must return error");
}
