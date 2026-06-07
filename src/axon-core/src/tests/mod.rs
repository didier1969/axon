pub mod bench_embedding_runtime;
pub mod bench_extraction;
pub mod embedder_gpu_backend_tests;
pub mod embedder_gpu_policy_tests;
pub mod embedder_gpu_telemetry_tests;
pub mod embedder_provider_runtime_tests;
// REQ-AXO-901653 slice-5c — graph_ingestion_split_tests + maillon_tests +
// pipeline_test deleted ; exercised legacy v1 worker.rs + File state-machine,
// replaced by pipeline_v2 (REQ-AXO-289 / CPT-AXO-054). REQ-AXO-901634 absorbed.
pub mod queue_decoupling_tests;
pub mod test_helpers;
// REQ-AXO-901663 / 901669 — restored coverage for LIVE vector_runtime methods.
pub mod vector_runtime_tests;
// REQ-AXO-284 Slice 2 — PG health helpers.
pub mod pg_health_tests;
// REQ-AXO-901675 — registry NOTIFY end-to-end integration (DB trigger only;
// the Rust listener + ingress_buffer feed were ripped in the LEGACY FEED PURGE
// — REQ-AXO-901893 RIP / REQ-AXO-901899).
pub mod registry_notify_integration_tests;
// REQ-AXO-901676 — public MCP tool `rescan_project(project_code, full=false)`.
pub mod rescan_project_tests;
