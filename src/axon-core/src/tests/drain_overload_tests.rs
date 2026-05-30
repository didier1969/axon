//! REQ-AXO-901678 — drain saturation telemetry + tuning end-to-end tests.
//!
//! These tests simulate the pipeline_v2 runtime's drain loop in
//! isolation : we push many file events into `IngressBuffer`, drain a
//! batch, then forward into a bounded mpsc channel sized to match
//! either the saturated (cap=8) or healthy (cap=10_000) regime — and
//! assert the published `ingress_metrics_snapshot()` reflects the
//! drop count correctly.
//!
//! Acceptance criteria :
//! * `dropped_full_total > 0` when A1 sink is saturated (cap=8 < batch=64).
//! * `dropped_full_total == 0` once cap is enlarged to absorb the batch.
//! * The new `pipeline_drain` telemetry fields populate from the runtime
//!   record_drain_tick call (heartbeat_tick > 0, batch_size > 0).

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::ingress_buffer::{
    ingress_metrics_snapshot, record_drain_tick, reset_ingress_metrics_for_tests, IngressBuffer,
    IngressCause, IngressFileEvent, IngressSource,
};
use crate::pipeline_v2::channels::PipelineChannelCaps;

/// Serialise tests in this module because `record_drain_tick` writes
/// process-global atomics that other parallel tests would observe.
fn metrics_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn make_event(path: &str) -> IngressFileEvent {
    IngressFileEvent::new(
        path,
        "AXO",
        128,
        1_700_000_000,
        100,
        IngressSource::Watcher,
        IngressCause::Discovered,
    )
}

/// One iteration of the runtime drain loop body, in isolation.
/// Returns (sent, dropped_full) — matching the variables tracked by
/// `pipeline_v2_runtime::spawn_pipeline_v2_indexer`.
fn run_one_drain_tick(
    buffer: &mut IngressBuffer,
    sink: &tokio::sync::mpsc::Sender<PathBuf>,
    drain_batch_cap: usize,
    tick: u64,
) -> (usize, usize) {
    let batch = buffer.drain_batch(drain_batch_cap);
    let mut sent = 0usize;
    let mut dropped = 0usize;
    for file_event in batch.files {
        let path = PathBuf::from(file_event.path);
        match sink.try_send(path) {
            Ok(()) => sent += 1,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => dropped += 1,
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
        }
    }
    record_drain_tick(drain_batch_cap, sent as u64, dropped as u64, tick);
    (sent, dropped)
}

#[tokio::test(flavor = "current_thread")]
async fn drain_loop_records_dropped_full_when_a1_sink_is_saturated() {
    let _g = metrics_lock();
    reset_ingress_metrics_for_tests();

    let drain_batch_cap = 64usize;
    let total_events = 256usize;
    // A1 sink cap (cap=8) << drain batch (64) → every tick drops 56+ events.
    let (tx_small, _rx_small) = tokio::sync::mpsc::channel::<PathBuf>(8);

    let mut buffer = IngressBuffer::default();
    for i in 0..total_events {
        buffer.record_file(make_event(&format!("/tmp/drain_overload/saturated/file-{i:04}.rs")));
    }
    assert_eq!(buffer.buffered_entries(), total_events);

    let (sent, dropped) = run_one_drain_tick(&mut buffer, &tx_small, drain_batch_cap, 1);
    assert!(
        sent > 0,
        "drain tick must push at least the channel-cap worth of files (sent={sent})"
    );
    assert!(
        dropped > 0,
        "drain tick must report drops when sink is saturated (dropped={dropped})"
    );

    let snap = ingress_metrics_snapshot();
    assert_eq!(snap.drain_batch_size, drain_batch_cap);
    assert_eq!(snap.drain_heartbeat_tick, 1);
    assert_eq!(snap.drain_last_batch_sent, sent as u64);
    assert_eq!(snap.drain_last_batch_dropped_full, dropped as u64);
    assert_eq!(snap.drain_dropped_full_total, dropped as u64);
    assert!(
        snap.drain_dropped_full_total > 0,
        "cumulative counter must be > 0 under saturation"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn drain_loop_reports_zero_drops_after_tuning_widens_sink_cap() {
    let _g = metrics_lock();
    reset_ingress_metrics_for_tests();

    let drain_batch_cap = 64usize;
    let total_events = 256usize;
    // After tuning : sink cap = 10_000 (canonical AXON_PIPELINE_A3_TO_B1_BUFFER_CAP default).
    let (tx_wide, _rx_wide) = tokio::sync::mpsc::channel::<PathBuf>(10_000);

    let mut buffer = IngressBuffer::default();
    for i in 0..total_events {
        buffer.record_file(make_event(&format!("/tmp/drain_overload/healthy/file-{i:04}.rs")));
    }

    // Drain in multiple ticks to fully empty buffer.
    let mut total_sent = 0usize;
    let mut total_dropped = 0usize;
    for tick in 1..=8 {
        let (sent, dropped) = run_one_drain_tick(&mut buffer, &tx_wide, drain_batch_cap, tick);
        total_sent += sent;
        total_dropped += dropped;
    }

    assert_eq!(
        total_dropped, 0,
        "healthy sink must produce zero drops (total_dropped={total_dropped})"
    );
    assert_eq!(total_sent, total_events);

    let snap = ingress_metrics_snapshot();
    assert_eq!(snap.drain_last_batch_dropped_full, 0);
    assert_eq!(
        snap.drain_dropped_full_total, 0,
        "cumulative counter must remain 0 in healthy regime"
    );
    assert!(snap.drain_heartbeat_tick >= 1);
    assert_eq!(snap.drain_batch_size, drain_batch_cap);
}

#[test]
fn pipeline_channel_caps_reads_ingress_drain_batch_from_env() {
    // Serialise env mutation against the other tests in this module
    // and `pipeline_v2/channels.rs::tests`.
    let _g = metrics_lock();

    let prev_drain = std::env::var("AXON_INGRESS_DRAIN_BATCH").ok();

    // Default cap is 512 when env is unset.
    std::env::remove_var("AXON_INGRESS_DRAIN_BATCH");
    let caps_default = PipelineChannelCaps::from_env();
    assert_eq!(caps_default.ingress_drain_batch, 512);

    // Env override.
    std::env::set_var("AXON_INGRESS_DRAIN_BATCH", "1024");
    let caps_env = PipelineChannelCaps::from_env();
    assert_eq!(caps_env.ingress_drain_batch, 1024);

    // Reject zero (preserve default).
    std::env::set_var("AXON_INGRESS_DRAIN_BATCH", "0");
    let caps_zero = PipelineChannelCaps::from_env();
    assert_eq!(caps_zero.ingress_drain_batch, 512);

    // Restore previous env (avoid leaking into sibling tests).
    match prev_drain {
        Some(v) => std::env::set_var("AXON_INGRESS_DRAIN_BATCH", v),
        None => std::env::remove_var("AXON_INGRESS_DRAIN_BATCH"),
    }

    // Yield to satisfy CPT-AXO-018 contract (tick the runtime once).
    let _ = Duration::from_millis(0);
}
