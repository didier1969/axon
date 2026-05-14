#![recursion_limit = "512"]

extern crate self as axon_core;

pub mod bench_pipeline_stages;
pub mod benchmark_store;
pub mod bridge;
pub mod code_chunker;
pub mod config;
pub mod embedder;
pub mod embedding_contract;
pub mod embedding_profile;
pub mod file_ingress_guard;
pub mod fs_watcher;
pub mod graph;
pub mod graph_analytics;
pub mod graph_bootstrap;
pub mod graph_ingestion;
pub mod graph_query;
pub mod hot_status_cache;
pub mod indexing_policy;
pub mod ingress_buffer;
mod main_background;
mod main_services;
mod main_telemetry;
pub mod mcp;
pub mod mcp_http;
pub mod optimizer;
pub mod parser;
pub mod pipeline_v2;
pub mod pipeline_v2_runtime;
pub mod postgres;
pub mod project_meta;
pub mod queue;
pub mod runtime_boot;
pub mod runtime_command_proxy;
pub mod runtime_mode;
pub mod runtime_observability;
pub mod runtime_operational_profile;
pub mod runtime_profile;
pub mod runtime_readiness;
pub mod runtime_topology;
pub mod runtime_truth_contract;
pub mod runtime_tuning;
pub mod runtime_watchdog;
pub mod runtime_writer_guard;
#[cfg(test)]
pub(crate) mod test_support;
pub mod scanner;
pub mod service_guard;
pub mod soll_snapshot;
pub mod vector_control;
pub mod watcher_probe;
pub mod worker;

#[cfg(test)]
pub mod tests;
