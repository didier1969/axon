pub mod bench_embedding_runtime;
pub mod bench_extraction;
pub mod embedder_batch_lanes_tests;
pub mod embedder_gpu_backend_tests;
pub mod embedder_gpu_policy_tests;
pub mod embedder_gpu_telemetry_tests;
pub mod embedder_provider_runtime_tests;
// REQ-AXO-901653 slice-5c — graph_ingestion_split_tests + maillon_tests +
// pipeline_test deleted ; exercised legacy v1 worker.rs + File state-machine,
// replaced by pipeline_v2 (REQ-AXO-289 / CPT-AXO-054). REQ-AXO-901634 absorbed.
pub mod queue_decoupling_tests;
pub mod test_helpers;
