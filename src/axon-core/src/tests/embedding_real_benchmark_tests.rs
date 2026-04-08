use crate::embedding_benchmark::{
    benchmark_target_for_symbol_kind, collect_repo_benchmark_corpus, expand_benchmark_samples,
    BenchmarkSample, BenchmarkTargetKind, CorpusCollectionLimits,
    BENCHMARK_TARGET_EMBEDDINGS_PER_HOUR,
};
use tempfile::tempdir;

#[test]
fn test_benchmark_symbol_kind_maps_types_and_procedures() {
    assert_eq!(
        benchmark_target_for_symbol_kind("struct"),
        Some(BenchmarkTargetKind::Type)
    );
    assert_eq!(
        benchmark_target_for_symbol_kind("class"),
        Some(BenchmarkTargetKind::Type)
    );
    assert_eq!(
        benchmark_target_for_symbol_kind("function"),
        Some(BenchmarkTargetKind::Procedure)
    );
    assert_eq!(
        benchmark_target_for_symbol_kind("method"),
        Some(BenchmarkTargetKind::Procedure)
    );
    assert_eq!(benchmark_target_for_symbol_kind("TODO"), None);
}

#[test]
fn test_expand_benchmark_samples_repeats_until_target_count() {
    let base = vec![
        BenchmarkSample::new(
            BenchmarkTargetKind::Procedure,
            "alpha".to_string(),
            "/tmp/a.rs".into(),
            "fn alpha() {}".to_string(),
        ),
        BenchmarkSample::new(
            BenchmarkTargetKind::Procedure,
            "beta".to_string(),
            "/tmp/b.rs".into(),
            "fn beta() {}".to_string(),
        ),
    ];

    let expanded = expand_benchmark_samples(&base, 5);

    assert_eq!(expanded.len(), 5);
    assert_eq!(expanded[0].label, "alpha");
    assert_eq!(expanded[1].label, "beta");
    assert_eq!(expanded[2].label, "alpha");
    assert_eq!(expanded[3].label, "beta");
    assert_eq!(expanded[4].label, "alpha");
}

#[test]
fn test_collect_repo_benchmark_corpus_extracts_files_types_and_procedures() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let sample = root.join("sample.rs");
    std::fs::write(
        &sample,
        r#"
pub struct Greeter {
    message: String,
}

impl Greeter {
    pub fn hello(&self) -> String {
        self.message.clone()
    }
}
"#,
    )
    .unwrap();

    let corpus = collect_repo_benchmark_corpus(
        root,
        &CorpusCollectionLimits {
            max_files: 8,
            max_file_chars: 2_000,
            max_symbol_chars: 500,
            max_samples_per_target: 32,
        },
    )
    .unwrap();

    assert_eq!(BENCHMARK_TARGET_EMBEDDINGS_PER_HOUR, 300_000);
    assert_eq!(corpus.files.len(), 1);
    assert!(
        corpus.types.iter().any(|sample| sample.label == "Greeter"),
        "type snippets should include the parsed struct"
    );
    assert!(
        corpus.procedures.iter().any(|sample| sample.label == "hello"),
        "procedure snippets should include the parsed method"
    );
}
