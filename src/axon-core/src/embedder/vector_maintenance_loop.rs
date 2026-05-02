//! Vector maintenance worker loop — extracted from embedder.rs (REQ-AXO-080 Phase 2).
//!
//! Pure structural extraction of `vector_maintenance_worker_loop` from the
//! embedder impl block to its own submodule. Behaviour-preserving move; the
//! function body is verbatim. Free helpers from the parent module are visible
//! via `use super::*` because Rust submodules see crate-private items of
//! their parent.

use super::*;

pub(super) fn vector_maintenance_worker_loop(graph_store: Arc<GraphStore>) {
    info!("Semantic Vector Maintenance Worker: stale inflight recovery enabled");
    let claimable_supply_poll_interval =
        Duration::from_millis(vector_claimable_supply_poll_interval_ms());
    let stale_recovery_interval =
        Duration::from_millis(vector_stale_inflight_recovery_interval_ms());
    let mut last_claimable_supply_maintenance = Instant::now()
        .checked_sub(claimable_supply_poll_interval)
        .unwrap_or_else(Instant::now);
    let mut last_stale_recovery = Instant::now()
        .checked_sub(stale_recovery_interval)
        .unwrap_or_else(Instant::now);
    loop {
        let mut woke = false;
        let now = Instant::now();

        if now.duration_since(last_claimable_supply_maintenance)
            >= claimable_supply_poll_interval
        {
            last_claimable_supply_maintenance = now;
            match maintain_vector_claimable_supply(&graph_store) {
                Ok(promoted) if promoted > 0 => {
                    woke = true;
                    info!(
                        "Semantic Vector Maintenance Worker: promoted {} graph-ready files into claimable vector supply",
                        promoted
                    );
                }
                Ok(_) => {}
                Err(err) => error!(
                    "Semantic Vector Maintenance Worker: failed to maintain claimable vector supply: {:?}",
                    err
                ),
            }
        }

        if now.duration_since(last_stale_recovery) >= stale_recovery_interval {
            last_stale_recovery = now;
            let now_ms = chrono::Utc::now().timestamp_millis();
            match recover_stale_vector_inflight_now(&graph_store, now_ms) {
                Ok(recovered) if recovered > 0 => {
                    woke = true;
                    info!(
                        "Semantic Vector Maintenance Worker: recovered {} stale inflight vectorization jobs",
                        recovered
                    )
                }
                Ok(_) => {}
                Err(err) => error!(
                    "Semantic Vector Maintenance Worker: failed to recover stale inflight vectorization jobs: {:?}",
                    err
                ),
            }
            match recover_stale_vector_outbox_now(&graph_store, now_ms) {
                Ok(recovered) if recovered > 0 => {
                    woke = true;
                    info!(
                        "Semantic Vector Maintenance Worker: recovered {} stale inflight outbox jobs",
                        recovered
                    )
                }
                Ok(_) => {}
                Err(err) => error!(
                    "Semantic Vector Maintenance Worker: failed to recover stale inflight outbox jobs: {:?}",
                    err
                ),
            }
        }
        if woke {
            service_guard::record_runtime_wakeup(
                service_guard::RuntimeWakeSource::SemanticVector,
                0,
                0,
            );
        }
        let next_claimable_due = claimable_supply_poll_interval
            .saturating_sub(last_claimable_supply_maintenance.elapsed());
        let next_recovery_due =
            stale_recovery_interval.saturating_sub(last_stale_recovery.elapsed());
        thread::sleep(
            next_claimable_due
                .min(next_recovery_due)
                .max(Duration::from_millis(25)),
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn extracted_function_links_to_runtime() {
        let _: fn(std::sync::Arc<crate::graph::GraphStore>) =
            super::vector_maintenance_worker_loop;
    }
}
