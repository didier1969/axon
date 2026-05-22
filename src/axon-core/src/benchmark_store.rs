use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::embedder::{
    current_embedding_provider_diagnostics, current_gpu_memory_snapshot,
    current_runtime_tuning_state, embedding_lane_config_from_env,
};
use crate::graph_ingestion::VectorBatchRun;
use crate::service_guard;

const BENCHMARK_WRITES_ENV: &str = "AXON_BENCHMARK_ACTIVE";

fn benchmark_db_path_from_ist_db(ist_db_path: &Path) -> PathBuf {
    let graph_root = ist_db_path.parent().unwrap_or(ist_db_path);
    let runtime_root = graph_root.parent().unwrap_or(graph_root);
    runtime_root.join("run").join("benchmark.sqlite3")
}

pub(crate) fn benchmark_db_path_for_graph_store(db_path: Option<&Path>) -> Option<PathBuf> {
    db_path.map(benchmark_db_path_from_ist_db)
}

pub(crate) fn benchmark_writes_enabled() -> bool {
    std::env::var(BENCHMARK_WRITES_ENV)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn open_benchmark_connection(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create benchmark store parent directory {}",
                parent.display()
            )
        })?;
    }
    let connection = Connection::open(path)
        .with_context(|| format!("failed to open benchmark store {}", path.display()))?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .context("failed to enable benchmark store WAL")?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .context("failed to set benchmark store synchronous mode")?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS vector_batch_run (
                run_id TEXT PRIMARY KEY,
                prepare_started_at_ms INTEGER NOT NULL DEFAULT 0,
                prepare_finished_at_ms INTEGER NOT NULL DEFAULT 0,
                ready_enqueued_at_ms INTEGER NOT NULL DEFAULT 0,
                started_at_ms INTEGER NOT NULL,
                finished_at_ms INTEGER NOT NULL,
                gpu_started_at_ms INTEGER NOT NULL DEFAULT 0,
                gpu_finished_at_ms INTEGER NOT NULL DEFAULT 0,
                persist_enqueued_at_ms INTEGER NOT NULL DEFAULT 0,
                persist_started_at_ms INTEGER NOT NULL DEFAULT 0,
                persist_finished_at_ms INTEGER NOT NULL DEFAULT 0,
                finalize_enqueued_at_ms INTEGER NOT NULL DEFAULT 0,
                finalize_finished_at_ms INTEGER NOT NULL DEFAULT 0,
                wall_ms INTEGER NOT NULL,
                instance_kind TEXT NOT NULL,
                runtime_mode TEXT NOT NULL,
                provider TEXT NOT NULL,
                provider_effective TEXT NOT NULL,
                runner_kind TEXT NOT NULL DEFAULT '',
                model_id TEXT NOT NULL,
                vector_workers INTEGER NOT NULL,
                graph_workers INTEGER NOT NULL,
                ready_queue_depth INTEGER NOT NULL,
                prepare_pipeline_depth INTEGER NOT NULL,
                prepare_workers_per_vector INTEGER NOT NULL,
                micro_batch_max_items INTEGER NOT NULL,
                micro_batch_max_total_tokens INTEGER NOT NULL,
                max_embed_batch_bytes INTEGER NOT NULL,
                chunk_count INTEGER NOT NULL,
                file_count INTEGER NOT NULL,
                input_bytes INTEGER NOT NULL,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                max_item_tokens INTEGER NOT NULL DEFAULT 0,
                avg_item_tokens REAL NOT NULL DEFAULT 0,
                micro_batch_count INTEGER NOT NULL DEFAULT 0,
                max_micro_batch_tokens INTEGER NOT NULL DEFAULT 0,
                avg_micro_batch_tokens REAL NOT NULL DEFAULT 0,
                effective_vector_workers_admitted INTEGER NOT NULL DEFAULT 0,
                ready_queue_depth_at_gpu_start INTEGER NOT NULL DEFAULT 0,
                prepare_inflight_at_gpu_start INTEGER NOT NULL DEFAULT 0,
                ready_queue_chunks_at_gpu_start INTEGER NOT NULL DEFAULT 0,
                prepare_inflight_chunks_at_gpu_start INTEGER NOT NULL DEFAULT 0,
                vector_worker_admission_reason TEXT NOT NULL DEFAULT '',
                allowed_gpu_workers INTEGER NOT NULL DEFAULT 0,
                batch_wait_for_ready_ms INTEGER NOT NULL DEFAULT 0,
                persist_queue_wait_ms INTEGER NOT NULL DEFAULT 0,
                finalize_queue_wait_ms INTEGER NOT NULL DEFAULT 0,
                batch_lane TEXT NOT NULL DEFAULT 'mixed',
                batch_shape TEXT NOT NULL DEFAULT 'homogeneous',
                lane_small_max_tokens INTEGER NOT NULL DEFAULT 0,
                lane_medium_max_tokens INTEGER NOT NULL DEFAULT 0,
                fetch_ms INTEGER NOT NULL,
                embed_ms INTEGER NOT NULL,
                db_write_ms INTEGER NOT NULL,
                mark_done_ms INTEGER NOT NULL,
                graph_backlog_depth_current INTEGER NOT NULL,
                prepare_inflight_current INTEGER NOT NULL,
                runtime_ready_queue_depth_current INTEGER NOT NULL,
                gpu_used_mb INTEGER,
                gpu_total_mb INTEGER,
                chunk_embeddings_per_second REAL NOT NULL,
                last_embed_attempt_wall_ms INTEGER NOT NULL,
                avg_embed_attempt_wall_ms REAL NOT NULL,
                last_embed_gap_ms INTEGER NOT NULL,
                avg_embed_gap_ms REAL NOT NULL,
                success INTEGER NOT NULL,
                error_reason TEXT
            );
            CREATE INDEX IF NOT EXISTS vector_batch_run_finished_idx
                ON vector_batch_run(finished_at_ms DESC);
            CREATE INDEX IF NOT EXISTS vector_batch_run_embed_idx
                ON vector_batch_run(embed_ms DESC);",
        )
        .context("failed to initialize benchmark store schema")?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "prepare_started_at_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "prepare_finished_at_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "ready_enqueued_at_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "gpu_started_at_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "gpu_finished_at_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "persist_enqueued_at_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "persist_started_at_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "persist_finished_at_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "finalize_enqueued_at_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "finalize_finished_at_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "runner_kind",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "total_tokens",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "max_item_tokens",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "avg_item_tokens",
        "REAL NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "micro_batch_count",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "max_micro_batch_tokens",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "avg_micro_batch_tokens",
        "REAL NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "effective_vector_workers_admitted",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "ready_queue_depth_at_gpu_start",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "prepare_inflight_at_gpu_start",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "ready_queue_chunks_at_gpu_start",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "prepare_inflight_chunks_at_gpu_start",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "vector_worker_admission_reason",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "allowed_gpu_workers",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "batch_wait_for_ready_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "persist_queue_wait_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "finalize_queue_wait_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "batch_lane",
        "TEXT NOT NULL DEFAULT 'mixed'",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "batch_shape",
        "TEXT NOT NULL DEFAULT 'homogeneous'",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "lane_small_max_tokens",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &connection,
        "vector_batch_run",
        "lane_medium_max_tokens",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    Ok(connection)
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut statement = connection
        .prepare(&pragma)
        .with_context(|| format!("failed to inspect schema for {table}"))?;
    let known_columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .with_context(|| format!("failed to read schema rows for {table}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("failed to collect schema rows for {table}"))?;
    if known_columns.iter().any(|known| known == column) {
        return Ok(());
    }
    connection
        .execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )
        .with_context(|| format!("failed to add column {column} to {table}"))?;
    Ok(())
}

pub(crate) fn mirror_vector_batch_run(
    db_path: Option<&Path>,
    run: &VectorBatchRun,
) -> Result<Option<PathBuf>> {
    if !benchmark_writes_enabled() {
        return Ok(None);
    }
    let Some(target_path) = benchmark_db_path_for_graph_store(db_path) else {
        return Ok(None);
    };
    let wall_ms = run
        .finished_at_ms
        .saturating_sub(run.started_at_ms)
        .max(0_i64);
    let lane_config = embedding_lane_config_from_env();
    let tuning = current_runtime_tuning_state();
    let provider = current_embedding_provider_diagnostics();
    let vector_runtime = service_guard::vector_runtime_metrics();
    let gpu_snapshot = current_gpu_memory_snapshot();
    // REQ-AXO-901657 slice 4 cluster A : canonical = AXON_INSTANCE.
    let instance_kind =
        crate::env_alias::read_with_alias_or("AXON_INSTANCE", "AXON_INSTANCE_KIND", "dev");
    let runtime_mode = std::env::var("AXON_RUNTIME_MODE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let prepare_pipeline_depth = std::env::var("AXON_VECTOR_PREPARE_PIPELINE_DEPTH")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(3_i64);
    let prepare_workers_per_vector = std::env::var("AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(2_i64);
    let connection = open_benchmark_connection(&target_path)?;
    connection
        .execute(
            "INSERT OR REPLACE INTO vector_batch_run (
                run_id,
                prepare_started_at_ms,
                prepare_finished_at_ms,
                ready_enqueued_at_ms,
                started_at_ms,
                finished_at_ms,
                gpu_started_at_ms,
                gpu_finished_at_ms,
                persist_enqueued_at_ms,
                persist_started_at_ms,
                persist_finished_at_ms,
                finalize_enqueued_at_ms,
                finalize_finished_at_ms,
                wall_ms,
                instance_kind,
                runtime_mode,
                provider,
                provider_effective,
                runner_kind,
                model_id,
                vector_workers,
                graph_workers,
                ready_queue_depth,
                prepare_pipeline_depth,
                prepare_workers_per_vector,
                micro_batch_max_items,
                micro_batch_max_total_tokens,
                max_embed_batch_bytes,
                chunk_count,
                file_count,
                input_bytes,
                total_tokens,
                max_item_tokens,
                avg_item_tokens,
                micro_batch_count,
                max_micro_batch_tokens,
                avg_micro_batch_tokens,
                effective_vector_workers_admitted,
                ready_queue_depth_at_gpu_start,
                prepare_inflight_at_gpu_start,
                ready_queue_chunks_at_gpu_start,
                prepare_inflight_chunks_at_gpu_start,
                vector_worker_admission_reason,
                allowed_gpu_workers,
                batch_wait_for_ready_ms,
                persist_queue_wait_ms,
                finalize_queue_wait_ms,
                batch_lane,
                batch_shape,
                lane_small_max_tokens,
                lane_medium_max_tokens,
                fetch_ms,
                embed_ms,
                db_write_ms,
                mark_done_ms,
                graph_backlog_depth_current,
                prepare_inflight_current,
                runtime_ready_queue_depth_current,
                gpu_used_mb,
                gpu_total_mb,
                chunk_embeddings_per_second,
                last_embed_attempt_wall_ms,
                avg_embed_attempt_wall_ms,
                last_embed_gap_ms,
                avg_embed_gap_ms,
                success,
                error_reason
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                run.run_id,
                run.prepare_started_at_ms,
                run.prepare_finished_at_ms,
                run.ready_enqueued_at_ms,
                run.started_at_ms,
                run.finished_at_ms,
                run.gpu_started_at_ms,
                run.gpu_finished_at_ms,
                run.persist_enqueued_at_ms,
                run.persist_started_at_ms,
                run.persist_finished_at_ms,
                run.finalize_enqueued_at_ms,
                run.finalize_finished_at_ms,
                wall_ms,
                instance_kind,
                runtime_mode,
                run.provider,
                provider.provider_effective,
                run.runner_kind,
                run.model_id,
                tuning.vector_workers as i64,
                tuning.graph_workers as i64,
                tuning.vector_ready_queue_depth as i64,
                prepare_pipeline_depth,
                prepare_workers_per_vector,
                tuning.embed_micro_batch_max_items as i64,
                tuning.embed_micro_batch_max_total_tokens as i64,
                lane_config.max_embed_batch_bytes as i64,
                run.chunk_count as i64,
                run.file_count as i64,
                run.input_bytes as i64,
                run.total_tokens as i64,
                run.max_item_tokens as i64,
                run.avg_item_tokens,
                run.micro_batch_count as i64,
                run.max_micro_batch_tokens as i64,
                run.avg_micro_batch_tokens,
                run.effective_vector_workers_admitted as i64,
                run.ready_queue_depth_at_gpu_start as i64,
                run.prepare_inflight_at_gpu_start as i64,
                run.ready_queue_chunks_at_gpu_start as i64,
                run.prepare_inflight_chunks_at_gpu_start as i64,
                run.vector_worker_admission_reason,
                run.allowed_gpu_workers as i64,
                run.batch_wait_for_ready_ms as i64,
                run.persist_queue_wait_ms as i64,
                run.finalize_queue_wait_ms as i64,
                run.batch_lane,
                run.batch_shape,
                run.lane_small_max_tokens as i64,
                run.lane_medium_max_tokens as i64,
                run.fetch_ms as i64,
                run.embed_ms as i64,
                run.db_write_ms as i64,
                run.mark_done_ms as i64,
                vector_runtime.canonical_backlog_depth_current as i64,
                vector_runtime.prepare_inflight_current as i64,
                vector_runtime.ready_queue_depth_current as i64,
                gpu_snapshot.map(|snapshot| snapshot.used_mb as i64),
                gpu_snapshot.map(|snapshot| snapshot.total_mb as i64),
                vector_runtime.chunk_embeddings_per_second,
                vector_runtime.last_embed_attempt_wall_ms as i64,
                vector_runtime.avg_embed_attempt_wall_ms,
                vector_runtime.last_embed_gap_ms as i64,
                vector_runtime.avg_embed_gap_ms,
                if run.success { 1_i64 } else { 0_i64 },
                run.error_reason,
            ],
        )
        .with_context(|| {
            format!(
                "failed to mirror vector batch run into {}",
                target_path.display()
            )
        })?;
    Ok(Some(target_path))
}

#[cfg(test)]
mod tests {
    use super::{
        benchmark_db_path_for_graph_store, benchmark_writes_enabled, mirror_vector_batch_run,
        BENCHMARK_WRITES_ENV,
    };
    use crate::graph_ingestion::VectorBatchRun;
    use rusqlite::Connection;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};

    fn benchmark_env_guard() -> std::sync::MutexGuard<'static, ()> {
        static ENV_GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_GUARD
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("benchmark env guard lock")
    }

    #[test]
    fn benchmark_store_path_is_separate_from_ist_db() {
        let path =
            benchmark_db_path_for_graph_store(Some(Path::new("/tmp/axon-case/graph_v2/ist.db")))
                .expect("derived path");
        assert_eq!(path, Path::new("/tmp/axon-case/run/benchmark.sqlite3"));
    }

    #[test]
    fn benchmark_writes_are_disabled_by_default() {
        let _guard = benchmark_env_guard();
        std::env::remove_var(BENCHMARK_WRITES_ENV);
        assert!(!benchmark_writes_enabled());
    }

    #[test]
    fn mirror_vector_batch_run_requires_benchmark_activation() {
        let _guard = benchmark_env_guard();
        std::env::remove_var(BENCHMARK_WRITES_ENV);
        let temp = tempfile::tempdir().unwrap();
        let graph_root = temp.path().join("graph_v2");
        std::fs::create_dir_all(&graph_root).unwrap();
        let ist_db = graph_root.join("ist.db");
        std::fs::write(&ist_db, b"").unwrap();
        let sqlite_path = benchmark_db_path_for_graph_store(Some(&ist_db)).unwrap();
        let run = VectorBatchRun {
            run_id: "run-disabled".to_string(),
            prepare_started_at_ms: 91,
            prepare_finished_at_ms: 109,
            ready_enqueued_at_ms: 109,
            started_at_ms: 100,
            finished_at_ms: 160,
            gpu_started_at_ms: 110,
            gpu_finished_at_ms: 141,
            persist_enqueued_at_ms: 142,
            persist_started_at_ms: 143,
            persist_finished_at_ms: 150,
            finalize_enqueued_at_ms: 151,
            finalize_finished_at_ms: 160,
            provider: "test".to_string(),
            runner_kind: "test".to_string(),
            model_id: "model".to_string(),
            chunk_count: 8,
            file_count: 2,
            input_bytes: 1024,
            total_tokens: 512,
            max_item_tokens: 128,
            avg_item_tokens: 64.0,
            micro_batch_count: 4,
            max_micro_batch_tokens: 160,
            avg_micro_batch_tokens: 128.0,
            effective_vector_workers_admitted: 2,
            ready_queue_depth_at_gpu_start: 5,
            prepare_inflight_at_gpu_start: 3,
            ready_queue_chunks_at_gpu_start: 160,
            prepare_inflight_chunks_at_gpu_start: 64,
            vector_worker_admission_reason: "admitted".to_string(),
            allowed_gpu_workers: 2,
            batch_wait_for_ready_ms: 17,
            persist_queue_wait_ms: 1,
            finalize_queue_wait_ms: 1,
            batch_lane: "large".to_string(),
            batch_shape: "homogeneous".to_string(),
            lane_small_max_tokens: 64,
            lane_medium_max_tokens: 128,
            fetch_ms: 7,
            embed_ms: 31,
            db_write_ms: 11,
            mark_done_ms: 5,
            success: true,
            error_reason: None,
        };

        let mirrored = mirror_vector_batch_run(Some(&ist_db), &run).unwrap();
        assert!(mirrored.is_none());
        assert!(!sqlite_path.exists());
    }

    #[test]
    fn mirror_vector_batch_run_writes_sqlite_row_when_benchmark_is_active() {
        let _guard = benchmark_env_guard();
        std::env::set_var(BENCHMARK_WRITES_ENV, "1");
        let temp = tempfile::tempdir().unwrap();
        let graph_root = temp.path().join("graph_v2");
        std::fs::create_dir_all(&graph_root).unwrap();
        let ist_db = graph_root.join("ist.db");
        std::fs::write(&ist_db, b"").unwrap();
        let run = VectorBatchRun {
            run_id: "run-1".to_string(),
            prepare_started_at_ms: 91,
            prepare_finished_at_ms: 109,
            ready_enqueued_at_ms: 109,
            started_at_ms: 100,
            finished_at_ms: 160,
            gpu_started_at_ms: 110,
            gpu_finished_at_ms: 141,
            persist_enqueued_at_ms: 142,
            persist_started_at_ms: 143,
            persist_finished_at_ms: 150,
            finalize_enqueued_at_ms: 151,
            finalize_finished_at_ms: 160,
            provider: "test".to_string(),
            runner_kind: "test".to_string(),
            model_id: "model".to_string(),
            chunk_count: 8,
            file_count: 2,
            input_bytes: 1024,
            total_tokens: 512,
            max_item_tokens: 128,
            avg_item_tokens: 64.0,
            micro_batch_count: 4,
            max_micro_batch_tokens: 160,
            avg_micro_batch_tokens: 128.0,
            effective_vector_workers_admitted: 2,
            ready_queue_depth_at_gpu_start: 5,
            prepare_inflight_at_gpu_start: 3,
            ready_queue_chunks_at_gpu_start: 160,
            prepare_inflight_chunks_at_gpu_start: 64,
            vector_worker_admission_reason: "admitted".to_string(),
            allowed_gpu_workers: 2,
            batch_wait_for_ready_ms: 17,
            persist_queue_wait_ms: 1,
            finalize_queue_wait_ms: 1,
            batch_lane: "large".to_string(),
            batch_shape: "homogeneous".to_string(),
            lane_small_max_tokens: 64,
            lane_medium_max_tokens: 128,
            fetch_ms: 7,
            embed_ms: 31,
            db_write_ms: 11,
            mark_done_ms: 5,
            success: true,
            error_reason: None,
        };
        let sqlite_path = mirror_vector_batch_run(Some(&ist_db), &run)
            .unwrap()
            .expect("sqlite path");
        let connection = Connection::open(sqlite_path).unwrap();
        let row = connection
            .query_row(
                "SELECT chunk_count, total_tokens, max_item_tokens, micro_batch_count, effective_vector_workers_admitted, ready_queue_depth_at_gpu_start, prepare_inflight_at_gpu_start, ready_queue_chunks_at_gpu_start, prepare_inflight_chunks_at_gpu_start, vector_worker_admission_reason, allowed_gpu_workers, batch_wait_for_ready_ms, prepare_started_at_ms, prepare_finished_at_ms, ready_enqueued_at_ms, gpu_started_at_ms, gpu_finished_at_ms, embed_ms, wall_ms, success, vector_workers, ready_queue_depth FROM vector_batch_run WHERE run_id = 'run-1'",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, i64>(10)?,
                        row.get::<_, i64>(11)?,
                        row.get::<_, i64>(12)?,
                        row.get::<_, i64>(13)?,
                        row.get::<_, i64>(14)?,
                        row.get::<_, i64>(15)?,
                        row.get::<_, i64>(16)?,
                        row.get::<_, i64>(17)?,
                        row.get::<_, i64>(18)?,
                        row.get::<_, i64>(19)?,
                        row.get::<_, i64>(20)?,
                        row.get::<_, i64>(21)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0, 8);
        assert_eq!(row.1, 512);
        assert_eq!(row.2, 128);
        assert_eq!(row.3, 4);
        assert_eq!(row.4, 2);
        assert_eq!(row.5, 5);
        assert_eq!(row.6, 3);
        assert_eq!(row.7, 160);
        assert_eq!(row.8, 64);
        assert_eq!(row.9, "admitted");
        assert_eq!(row.10, 2);
        assert_eq!(row.11, 17);
        assert_eq!(row.12, 91);
        assert_eq!(row.13, 109);
        assert_eq!(row.14, 109);
        assert_eq!(row.15, 110);
        assert_eq!(row.16, 141);
        assert_eq!(row.17, 31);
        assert_eq!(row.18, 60);
        assert_eq!(row.19, 1);
        assert!(row.20 >= 1);
        assert!(row.21 >= 1);
        std::env::remove_var(BENCHMARK_WRITES_ENV);
    }
}
