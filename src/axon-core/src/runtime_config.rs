//! REQ-AXO-901806 F2 — Runtime config snapshot writer.
//!
//! At indexer boot, read the env vars that drive worker counts, batch
//! sizes and NOTIFY channel, then UPSERT a single row into
//! `axon_runtime.runtime_config_snapshot` so the dashboard state
//! composition (1 Hz) reads them via PG instead of receiving 15+ args
//! from `main_telemetry`. Aligns with PIL-AXO-009 (PG canonical) without
//! write amplification — the row is written once per process boot.

use std::sync::Arc;

use anyhow::Result;
use serde_json::json;

use crate::env_alias::read_with_alias;
use crate::graph::GraphStore;
use crate::pipeline_v2::channels::{
    A3_TO_B1_BUFFER_CAP_DEFAULT, B2_BATCH_SIZE_DEFAULT, B2_BATCH_TIMEOUT_MS_DEFAULT,
    B3_BATCH_SIZE_DEFAULT, B3_BATCH_TIMEOUT_MS_DEFAULT, INGRESS_DRAIN_BATCH_DEFAULT,
};
use crate::pipeline_v2::notify_listener::LISTEN_CHANNEL;

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

/// Compose the indexer config snapshot as a JSON value. Mirrors the
/// shape consumed by the `embedding_status` MCP tool so the dashboard
/// downstream can read either source identically. Defaults match
/// `mcp/tools_system.rs` so an unset env still reports canonical
/// values.
pub fn compose_indexer_config() -> serde_json::Value {
    let a1 = env_usize("AXON_A1_WORKERS", 4);
    let a2 = env_usize("AXON_A2_WORKERS", 8);
    let a3 = env_usize("AXON_A3_WORKERS", 2);
    let a3_batch = env_usize("AXON_A3_BATCH_SIZE", 32);
    let a3_timeout = env_u64("AXON_A3_BATCH_TIMEOUT_MS", 10);

    let b1 = env_usize("AXON_B1_WORKERS", 4);
    let b2 = env_usize("AXON_B2_WORKERS", 1);
    let b3 = env_usize("AXON_B3_WORKERS", 2);
    let b2_batch = env_usize("AXON_B2_BATCH_SIZE", B2_BATCH_SIZE_DEFAULT);
    let b2_timeout = env_u64("AXON_B2_BATCH_TIMEOUT_MS", B2_BATCH_TIMEOUT_MS_DEFAULT);
    let b3_batch = env_usize("AXON_B3_BATCH_SIZE", B3_BATCH_SIZE_DEFAULT);
    let b3_timeout = env_u64("AXON_B3_BATCH_TIMEOUT_MS", B3_BATCH_TIMEOUT_MS_DEFAULT);
    let ingress_drain = env_usize("AXON_INGRESS_DRAIN_BATCH", INGRESS_DRAIN_BATCH_DEFAULT);

    // A3→B1 buffer cap : canonical env (with one-shot alias deprecation
    // for `AXON_A3_TO_B1_BUFFER`) mirrors `mcp::tools_system` resolution
    // — single source of truth = `pipeline_v2::channels` default.
    let a3_to_b1_cap = read_with_alias("AXON_PIPELINE_A3_TO_B1_BUFFER_CAP", "AXON_A3_TO_B1_BUFFER")
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(A3_TO_B1_BUFFER_CAP_DEFAULT);

    json!({
        "pipeline_a": {
            "a1_workers": a1,
            "a2_workers": a2,
            "a3_workers": a3,
            "a3_batch_size": a3_batch,
            "a3_batch_timeout_ms": a3_timeout,
        },
        "pipeline_b": {
            "b1_workers": b1,
            "b2_workers": b2,
            "b3_workers": b3,
            "b2_batch_size": b2_batch,
            "b2_batch_timeout_ms": b2_timeout,
            "b3_batch_size": b3_batch,
            "b3_batch_timeout_ms": b3_timeout,
            "a3_to_b1_buffer_cap": a3_to_b1_cap,
        },
        // Canonical PG NOTIFY channel for the brain demand-pull
        // listener — exposed as `pub const` from notify_listener so this
        // value isn't hardcoded in 3 separate places.
        "notify_channel": LISTEN_CHANNEL,
        "ingress_drain_batch": ingress_drain,
    })
}

/// Persist the indexer config snapshot to PG. Called once at indexer
/// boot after schema bootstrap. Idempotent (UPSERT on `runtime_role`).
pub fn write_indexer_config_snapshot(store: &Arc<GraphStore>) -> Result<()> {
    let config = compose_indexer_config();
    let config_json = serde_json::to_string(&config)?.replace('\'', "''");
    let sql = format!(
        "INSERT INTO axon_runtime.runtime_config_snapshot (runtime_role, config) \
         VALUES ('indexer', '{}'::jsonb) \
         ON CONFLICT (runtime_role) DO UPDATE \
            SET config = EXCLUDED.config, written_at = clock_timestamp()",
        config_json
    );
    store.execute_raw_sql_gateway(&sql)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_indexer_config_emits_canonical_shape() {
        let v = compose_indexer_config();
        assert!(v["pipeline_a"]["a1_workers"].is_number());
        assert!(v["pipeline_a"]["a2_workers"].is_number());
        assert!(v["pipeline_a"]["a3_workers"].is_number());
        assert!(v["pipeline_a"]["a3_batch_size"].is_number());
        assert!(v["pipeline_a"]["a3_batch_timeout_ms"].is_number());
        assert!(v["pipeline_b"]["b1_workers"].is_number());
        assert!(v["pipeline_b"]["b2_workers"].is_number());
        assert!(v["pipeline_b"]["b3_workers"].is_number());
        assert!(v["pipeline_b"]["b2_batch_size"].is_number());
        assert!(v["pipeline_b"]["a3_to_b1_buffer_cap"].is_number());
        assert_eq!(
            v["notify_channel"],
            crate::pipeline_v2::notify_listener::LISTEN_CHANNEL
        );
        assert!(v["ingress_drain_batch"].is_number());
    }
}
