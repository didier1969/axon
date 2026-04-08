use crate::embedder::{
    calibrated_embedding_profile_for_backend, default_embedding_profile,
    file_vectorization_runtime_budget, semantic_worker_fetch_limits_for_profile,
    EmbeddingExecutionBackend,
};
use crate::graph::GraphStore;
use crate::parser::{ExtractionResult, Symbol};
use crate::queue::ProcessingMode;
use crate::service_guard::ServicePressure;
use crate::worker::DbWriteTask;

fn file_extraction_task(path: &str, symbol_name: &str) -> DbWriteTask {
    DbWriteTask::FileExtraction {
        reservation_id: format!("res-{symbol_name}"),
        path: path.to_string(),
        content: Some(format!("fn {symbol_name}() {{}}\n")),
        extraction: ExtractionResult {
            project_slug: Some("proj".to_string()),
            symbols: vec![Symbol {
                name: symbol_name.to_string(),
                kind: "function".to_string(),
                start_line: 1,
                end_line: 1,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: Default::default(),
                embedding: None,
            }],
            relations: Vec::new(),
        },
        processing_mode: ProcessingMode::Full,
        trace_id: format!("trace-{symbol_name}"),
        observed_cost_bytes: 128,
        t0: 0,
        t1: 0,
        t2: 0,
        t3: 0,
    }
}

#[test]
fn test_gpu_backend_calibrates_embedding_profile_for_larger_batches() {
    let baseline = default_embedding_profile();

    let calibrated = calibrated_embedding_profile_for_backend(
        &baseline,
        EmbeddingExecutionBackend::GpuCuda,
    );

    assert!(
        calibrated.chunk.batch_size > baseline.chunk.batch_size,
        "GPU calibration must raise the chunk batch size"
    );
    assert!(
        calibrated.symbol.batch_size > baseline.symbol.batch_size,
        "GPU calibration must raise the symbol batch size"
    );
    assert!(
        calibrated.file_vectorization_batch_size > baseline.file_vectorization_batch_size,
        "GPU calibration must raise the file vectorization batch size"
    );
    assert!(
        calibrated.graph.batch_size >= baseline.graph.batch_size,
        "GPU calibration must not shrink graph batching"
    );
}

#[test]
fn test_file_vectorization_runtime_budget_throttles_under_pressure() {
    let profile = calibrated_embedding_profile_for_backend(
        &default_embedding_profile(),
        EmbeddingExecutionBackend::GpuCuda,
    );

    let healthy = file_vectorization_runtime_budget(
        &profile,
        ServicePressure::Healthy,
        120,
    );
    let degraded = file_vectorization_runtime_budget(
        &profile,
        ServicePressure::Degraded,
        2_500,
    );
    let critical = file_vectorization_runtime_budget(
        &profile,
        ServicePressure::Critical,
        4_000,
    );

    assert!(!healthy.pause, "healthy runtime should keep vectorization enabled");
    assert!(
        healthy.total_chunk_budget >= profile.chunk.batch_size,
        "healthy runtime must expose a meaningful chunk budget"
    );
    assert!(
        degraded.total_chunk_budget < healthy.total_chunk_budget,
        "degraded runtime must reduce chunk budget"
    );
    assert!(
        degraded.file_fetch_limit < healthy.file_fetch_limit,
        "degraded runtime must reduce file fetch pressure"
    );
    assert!(critical.pause, "critical runtime must pause file vectorization");
    assert_eq!(critical.file_fetch_limit, 0);
    assert_eq!(critical.total_chunk_budget, 0);
}

#[test]
fn test_semantic_worker_fetch_limits_follow_gpu_calibrated_profile() {
    let profile = calibrated_embedding_profile_for_backend(
        &default_embedding_profile(),
        EmbeddingExecutionBackend::GpuCuda,
    );

    let limits = semantic_worker_fetch_limits_for_profile(&profile);

    assert_eq!(
        limits.symbol_fetch_batch_size,
        profile.symbol.batch_size,
        "symbol fetch size must follow the calibrated runtime profile"
    );
    assert_eq!(
        limits.graph_projection_batch_size,
        profile.graph.batch_size,
        "graph projection fetch size must follow the calibrated runtime profile"
    );
}

#[test]
fn test_fetch_unembedded_chunks_for_files_batches_across_multiple_paths() {
    let store = GraphStore::new(":memory:").unwrap();
    let tasks = vec![
        file_extraction_task("/tmp/file_a.rs", "alpha"),
        file_extraction_task("/tmp/file_b.rs", "beta"),
    ];
    store.insert_file_data_batch(&tasks).unwrap();

    let rows = store
        .fetch_unembedded_chunks_for_files(
            &["/tmp/file_a.rs".to_string(), "/tmp/file_b.rs".to_string()],
            &default_embedding_profile().chunk.model_id,
            8,
        )
        .unwrap();

    assert_eq!(rows.len(), 2, "the batch should return both files in one wave");
    assert!(
        rows.iter().any(|(path, _, _, _)| path == "/tmp/file_a.rs"),
        "the batch must include file_a"
    );
    assert!(
        rows.iter().any(|(path, _, _, _)| path == "/tmp/file_b.rs"),
        "the batch must include file_b"
    );
}
