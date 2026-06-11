// REQ-AXO-259/260/261 TDD — pure-function tests for the 4-bench
// helpers. ORT/GPU paths gated by run_*_bench() proper are exercised
// in their respective bin smoke tests.

use super::{
    collect_supported_files, run_graph_projection_bench, run_writer_bench, summarize_end_to_end,
    SyntheticChunkRow,
};
use std::io::Write;
use std::path::PathBuf;

fn make_test_corpus() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("axon-bench-pipeline-stages-test-")
        .tempdir()
        .expect("tempdir");

    // Two supported files (.rs, .py)
    let rs = dir.path().join("hello.rs");
    let mut f = std::fs::File::create(&rs).unwrap();
    writeln!(
        f,
        "pub fn add(a: i32, b: i32) -> i32 {{ a + b }}\npub fn sub(a: i32, b: i32) -> i32 {{ a - b }}"
    )
    .unwrap();

    let py = dir.path().join("hello.py");
    let mut f = std::fs::File::create(&py).unwrap();
    writeln!(
        f,
        "def add(a, b):\n    return a + b\ndef sub(a, b):\n    return a - b"
    )
    .unwrap();

    // One unsupported file (.bin) — must be skipped
    let bin = dir.path().join("ignored.bin");
    let mut f = std::fs::File::create(&bin).unwrap();
    writeln!(f, "binary content").unwrap();

    // One file in target/ — must be excluded by walker
    let target = dir.path().join("target");
    std::fs::create_dir_all(&target).unwrap();
    let target_rs = target.join("excluded.rs");
    let mut f = std::fs::File::create(&target_rs).unwrap();
    writeln!(f, "pub fn excluded() {{}}").unwrap();

    dir
}

#[test]
fn collect_supported_files_walks_tree_excluding_noise_dirs() {
    let dir = make_test_corpus();
    let collected = collect_supported_files(dir.path(), 100).unwrap();
    let names: Vec<String> = collected
        .iter()
        .filter_map(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    assert!(
        names.contains(&"hello.rs".to_string()),
        "rs file collected: {names:?}"
    );
    assert!(
        names.contains(&"hello.py".to_string()),
        "py file collected: {names:?}"
    );
    assert!(
        !names.contains(&"ignored.bin".to_string()),
        ".bin must not be collected: {names:?}"
    );
    assert!(
        !names.contains(&"excluded.rs".to_string()),
        "files under target/ must be excluded: {names:?}"
    );
}

#[test]
fn collect_supported_files_respects_max_files_cap() {
    let dir = make_test_corpus();
    let collected = collect_supported_files(dir.path(), 1).unwrap();
    assert_eq!(collected.len(), 1, "max_files=1 must cap output to 1");
}

#[test]
fn run_graph_projection_bench_computes_metrics_on_real_corpus() {
    let dir = make_test_corpus();
    let bench = run_graph_projection_bench("test", dir.path(), 100).unwrap();
    assert!(
        bench.files_processed >= 1,
        "expected >=1 processed file, got {}",
        bench.files_processed
    );
    assert!(
        bench.symbols_extracted > 0,
        "expected >0 symbols extracted from 2 toy files; got {}",
        bench.symbols_extracted
    );
    assert!(
        bench.elapsed_ms < 30_000,
        "bench should complete in <30s on toy corpus; got {}ms",
        bench.elapsed_ms
    );
    assert!(bench.files_per_s > 0.0, "files_per_s must be positive");
}

#[test]
fn run_graph_projection_bench_rejects_missing_dir() {
    let res =
        run_graph_projection_bench("test", &PathBuf::from("/nonexistent/axon/path/12345"), 10);
    assert!(res.is_err(), "missing source_dir must error");
}

#[test]
fn run_graph_projection_bench_rejects_zero_max_files() {
    let dir = make_test_corpus();
    let res = run_graph_projection_bench("test", dir.path(), 0);
    assert!(res.is_err(), "max_files=0 must error");
}

#[test]
fn run_writer_bench_drives_persist_closure_correct_count() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let calls = Arc::new(AtomicUsize::new(0));
    let chunks_seen = Arc::new(AtomicUsize::new(0));
    let calls_p = calls.clone();
    let chunks_p = chunks_seen.clone();

    let bench = run_writer_bench(
        "test-writer",
        100, // total_chunks
        25,  // batch_size
        16,  // small embedding_dim for speed
        "noop-control",
        |rows| {
            calls_p.fetch_add(1, Ordering::Relaxed);
            chunks_p.fetch_add(rows.len(), Ordering::Relaxed);
            assert_eq!(rows[0].embedding.len(), 16, "embedding dim respected");
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(bench.chunks_written, 100);
    assert_eq!(bench.batches_written, 4); // 100 / 25
    assert_eq!(calls.load(Ordering::Relaxed), 4);
    assert_eq!(chunks_seen.load(Ordering::Relaxed), 100);
    assert_eq!(bench.backend, "noop-control");
    assert!(bench.chunks_per_s > 0.0);
}

#[test]
fn run_writer_bench_handles_partial_last_batch() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let last_batch = Arc::new(AtomicUsize::new(0));
    let last_batch_p = last_batch.clone();

    let bench = run_writer_bench("test-partial", 103, 25, 4, "noop", move |rows| {
        last_batch_p.store(rows.len(), Ordering::Relaxed);
        Ok(())
    })
    .unwrap();

    assert_eq!(bench.chunks_written, 103);
    assert_eq!(bench.batches_written, 5); // 25*4 + 3
    assert_eq!(
        last_batch.load(Ordering::Relaxed),
        3,
        "last batch had 3 rows"
    );
}

#[test]
fn run_writer_bench_propagates_persist_failure() {
    let res = run_writer_bench("fail", 50, 25, 4, "noop", |_rows| {
        anyhow::bail!("synthetic write failure")
    });
    assert!(res.is_err(), "persist closure failure must propagate");
}

#[test]
fn run_writer_bench_rejects_zero_inputs() {
    let res = run_writer_bench("z", 0, 25, 4, "noop", |_| Ok(()));
    assert!(res.is_err(), "total_chunks=0 must error");
    let res = run_writer_bench("z", 100, 0, 4, "noop", |_| Ok(()));
    assert!(res.is_err(), "batch_size=0 must error");
    let res = run_writer_bench("z", 100, 25, 0, "noop", |_| Ok(()));
    assert!(res.is_err(), "embedding_dim=0 must error");
}

#[test]
fn synthetic_chunk_row_embedding_is_deterministic() {
    // Same seed -> same embedding (cross-run reproducibility for A/B)
    let mut a: Option<Vec<f32>> = None;
    let _ = run_writer_bench("d", 1, 1, 8, "noop", |rows| {
        a = Some(rows[0].embedding.clone());
        Ok(())
    });
    let mut b: Option<Vec<f32>> = None;
    let _ = run_writer_bench("d", 1, 1, 8, "noop", |rows| {
        b = Some(rows[0].embedding.clone());
        Ok(())
    });
    assert_eq!(
        a.unwrap(),
        b.unwrap(),
        "embedding seed=0 must be deterministic"
    );
}

#[test]
fn summarize_end_to_end_handles_empty_samples() {
    let s = summarize_end_to_end("e", &[]);
    assert_eq!(s.total_chunks, 0);
    assert_eq!(s.sample_count, 0);
    assert_eq!(s.mean_ch_per_s, 0.0);
    assert_eq!(s.rolling_10s_min, 0.0);
}

#[test]
fn summarize_end_to_end_computes_mean_and_percentiles() {
    // 10 samples each: 100 chunks in 1000ms = 100 ch/s steady
    let samples: Vec<(u64, usize, u64)> = (0..10)
        .map(|i| ((i as u64) * 1000, 100usize, 1000u64))
        .collect();
    let s = summarize_end_to_end("steady", &samples);
    assert_eq!(s.total_chunks, 1000);
    assert_eq!(s.sample_count, 10);
    assert!(
        (s.mean_ch_per_s - 100.0).abs() < 0.001,
        "mean: {}",
        s.mean_ch_per_s
    );
    assert!((s.p50_ch_per_s - 100.0).abs() < 0.001);
    assert!((s.p95_ch_per_s - 100.0).abs() < 0.001);
    assert!((s.rolling_10s_min - 100.0).abs() < 0.001);
}

#[test]
fn summarize_end_to_end_detects_rolling_dip() {
    // 11 samples at 100 ch/s, then a dip at sample 11
    let mut samples: Vec<(u64, usize, u64)> = (0..11)
        .map(|i| ((i as u64) * 1000, 100usize, 1000u64))
        .collect();
    samples.push((11_000, 10, 1000)); // 10 ch/s dip in 12th window
    let s = summarize_end_to_end("dip", &samples);
    // 10s window ending at sample 11 (t=11000): covers samples whose t >= 1000
    // which is samples idx 1..11 (10 samples at 100) plus sample 11 (10).
    // = (10*100 + 10) chunks / 11 * 1000 ms = 100 ch/s ish... but rolling_min
    // is computed for ALL anchor positions, so the min over the whole series
    // must be lower than 100.
    assert!(
        s.rolling_10s_min < 100.0,
        "rolling-min must reflect the dip; got {:.2}",
        s.rolling_10s_min
    );
    assert!(
        s.rolling_10s_min > 0.0,
        "rolling-min must not collapse to zero; got {:.2}",
        s.rolling_10s_min
    );
}

#[test]
fn synthetic_chunk_row_struct_carries_required_fields() {
    let row = SyntheticChunkRow {
        symbol_id: "x".to_string(),
        content_hash: "y".to_string(),
        embedding: vec![0.1, 0.2, 0.3],
    };
    assert_eq!(row.embedding.len(), 3);
    assert_eq!(row.symbol_id, "x");
}
