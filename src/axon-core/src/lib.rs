pub mod bridge;
pub mod config;
pub mod embedder;
pub mod file_ingress_guard;
pub mod fs_watcher;
pub mod graph;
pub mod graph_analytics;
pub mod graph_bootstrap;
pub mod graph_ingestion;
pub mod graph_query;
pub mod mcp;
pub mod mcp_http;
pub mod parser;
pub mod queue;
pub mod runtime_observability;
pub mod runtime_profile;
pub mod scanner;
pub mod service_guard;
pub mod watcher_probe;
pub mod worker;

#[cfg(test)]
pub mod tests;
