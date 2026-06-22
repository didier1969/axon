#![recursion_limit = "512"]

extern crate self as axon_core;

pub mod bench_pipeline_stages;
pub mod bridge;
pub mod code_chunker;
pub mod config;
pub mod dashboard_state;
pub mod embedder;
pub mod embedding_contract;
pub mod embedding_profile;
pub mod env_alias;
pub mod graph;
pub mod graph_analytics;
pub mod graph_bootstrap;
pub mod graph_ingestion;
pub mod graph_query;
// REQ-AXO-901653 slice-5d — `hot_status_cache` deleted (env-gated FileVectorizationQueue
// flush path ; pipeline owns chunk-state directly).
pub mod indexer_health_http;
pub mod indexing_policy;
pub mod ist_snapshot;
mod main_background;
mod main_services;
mod main_telemetry;
pub mod mcp;
pub mod mcp_http;
pub mod observed_gpu;
pub mod optimizer;
pub mod parser;
pub mod pipeline;
pub mod pipeline_runtime;
pub mod postgres;
pub mod project_meta;
pub mod queue;
pub mod runtime_boot;
pub mod runtime_command_proxy;
pub mod runtime_config;
pub mod runtime_mode;
pub mod runtime_observability;
pub mod runtime_operational_profile;
pub mod runtime_capacity_profile;
pub mod runtime_readiness;
pub mod runtime_topology;
pub mod runtime_truth_contract;
pub mod runtime_tuning;
pub mod runtime_watchdog;
pub mod runtime_writer_guard;
pub mod scanner;
pub mod service_guard;
pub mod soll_snapshot;
#[cfg(test)]
pub(crate) mod test_support;
pub mod vector_control;
pub mod viz_freshness;
// REQ-AXO-901893 — Watchman-backed file source (clock/cursor reconciliation).
// The legacy notify/inotify watcher + ingress_buffer FIFO + reconciliation/
// periodic sweeps it replaced were RIPPED in the LEGACY FEED PURGE (REQ-AXO-901893
// deferred RIP): fs_watcher, ingress_buffer, file_ingress_guard, watcher_probe,
// registry_notify_listener are gone. Watchman + the DBQ-A claim feeder are the feed.
pub mod watchman_source;
// REQ-AXO-901653 slice-5c — legacy `pub mod worker;` removed (v1 WorkerPool +
// writer-actor + DbWriteTask retired). Pipeline_v2 (REQ-AXO-289 / CPT-AXO-054)
// owns the ingestion path via `pipeline/worker_pool.rs`.

#[cfg(test)]
pub mod tests;
