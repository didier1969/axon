//! Per-stage worker pool helper for the streaming pipeline (CPT-AXO-054).
//!
//! Each pipeline stage gets one [`spawn_stage_workers`] call: it spawns `n`
//! tokio tasks that race for items off the upstream `mpsc::Receiver`, invoke
//! the `work` closure, and forward results to the downstream `mpsc::Sender`.
//! All instrumentation (in / out / inflight / errors / backpressure / mean
//! duration) is captured automatically through [`StageMetrics`].

use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::Mutex;
use tracing::warn;

use super::metrics::StageMetrics;

/// Spawn `worker_count` tasks that drain `rx`, run `work` on each item, and
/// push the result to `tx`. Stage lifecycle counters are updated through
/// `metrics`.
///
/// Semantics:
///
/// * Each worker awaits `rx.lock().await` then `recv().await` to claim the
///   next item. The `Mutex` is a fairness lever — `tokio`'s `mpsc::Receiver`
///   is single-consumer by design, so the workers serialise their `recv()`
///   calls but immediately release the lock so the next worker can claim
///   while this one runs the (potentially long) `work` future.
/// * If `tx.send` is full, the worker awaits — the upstream stage's `send`
///   will then backpressure naturally. `backpressure_blocks_total` is bumped
///   each time the worker observed a non-immediate `send`.
/// * If the channel is closed (`recv` → `None` or `send` → `Err`), the worker
///   exits cleanly — the surrounding task will be joined or simply dropped by
///   the runtime when the receiver Arc dies.
/// * `work` returning `Err(_)` is logged and counted but does NOT crash the
///   worker — robustness is preferred over crash-the-pipeline-on-first-error.
pub fn spawn_stage_workers<I, O, F, Fut>(
    worker_count: usize,
    rx: Receiver<I>,
    tx: Sender<O>,
    work: F,
    metrics: Arc<StageMetrics>,
) where
    I: Send + 'static,
    O: Send + 'static,
    F: Fn(I) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = Result<O>> + Send,
{
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..worker_count {
        let rx = rx.clone();
        let tx = tx.clone();
        let work = work.clone();
        let metrics = metrics.clone();
        tokio::spawn(async move {
            loop {
                let next = {
                    let mut guard = rx.lock().await;
                    guard.recv().await
                };
                let Some(item) = next else {
                    break;
                };
                metrics.record_started();
                let started = Instant::now();
                match work(item).await {
                    Ok(out) => {
                        let elapsed_us = started.elapsed().as_micros().min(u128::from(u64::MAX))
                            as u64;
                        metrics.record_finished(elapsed_us);
                        if tx.capacity() == 0 {
                            metrics.record_backpressure_block();
                        }
                        if tx.send(out).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        metrics.record_error();
                        warn!(stage = metrics.name(), error = ?err, "stage worker error");
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn workers_forward_items_through_a_simple_doubling_stage() {
        let (in_tx, in_rx) = mpsc::channel::<u32>(16);
        let (out_tx, mut out_rx) = mpsc::channel::<u32>(16);
        let metrics = StageMetrics::new("test_stage");

        spawn_stage_workers(
            2,
            in_rx,
            out_tx,
            |x: u32| async move { Ok(x.saturating_mul(2)) },
            metrics.clone(),
        );

        for v in 1u32..=10 {
            in_tx.send(v).await.unwrap();
        }
        drop(in_tx);

        let mut collected = Vec::new();
        while let Some(v) = out_rx.recv().await {
            collected.push(v);
        }
        collected.sort_unstable();
        assert_eq!(
            collected,
            (1u32..=10).map(|v| v * 2).collect::<Vec<_>>(),
            "every input must be doubled exactly once"
        );

        let snap = metrics.snapshot();
        assert_eq!(snap.items_in_total, 10);
        assert_eq!(snap.items_out_total, 10);
        assert_eq!(snap.errors_total, 0);
        assert_eq!(snap.inflight, 0);
    }

    #[tokio::test]
    async fn worker_errors_are_counted_but_do_not_crash_pipeline() {
        let (in_tx, in_rx) = mpsc::channel::<u32>(8);
        let (out_tx, mut out_rx) = mpsc::channel::<u32>(8);
        let metrics = StageMetrics::new("flaky_stage");

        spawn_stage_workers(
            1,
            in_rx,
            out_tx,
            |x: u32| async move {
                if x % 2 == 0 {
                    Err(anyhow::anyhow!("even values are rejected for the test"))
                } else {
                    Ok(x)
                }
            },
            metrics.clone(),
        );

        for v in 1u32..=6 {
            in_tx.send(v).await.unwrap();
        }
        drop(in_tx);

        let mut collected = Vec::new();
        while let Some(v) = out_rx.recv().await {
            collected.push(v);
        }
        collected.sort_unstable();
        assert_eq!(collected, vec![1, 3, 5]);

        let snap = metrics.snapshot();
        assert_eq!(snap.items_in_total, 6);
        assert_eq!(snap.items_out_total, 3);
        assert_eq!(snap.errors_total, 3);
        assert_eq!(snap.inflight, 0);
    }
}
