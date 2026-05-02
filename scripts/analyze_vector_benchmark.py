#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

try:
    import polars as pl
except ModuleNotFoundError as exc:
    raise SystemExit(
        "Polars is required for vector benchmark analysis. Run through `devenv shell` "
        "or install the project Python environment; this analyzer is intentionally "
        "not allowed to silently degrade."
    ) from exc


PROJECT_ROOT = Path(__file__).resolve().parents[1]
QUALIFICATION_ROOT = PROJECT_ROOT / ".axon" / "qualification-runs"
DEFAULT_BENCHMARK_DB = PROJECT_ROOT / ".axon-dev" / "run" / "benchmark.sqlite3"


@dataclass(frozen=True)
class Thresholds:
    high_vram_mb: int
    plateau_range_mb: int
    low_gpu_util_pct: int
    underfeed_ready_chunks: int
    target_chunks_per_s: float


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Analyze Axon vector benchmark time series with Polars."
    )
    parser.add_argument(
        "run_dir",
        nargs="?",
        default="latest",
        help="Qualification run directory, or 'latest' (default).",
    )
    parser.add_argument(
        "--benchmark-db",
        default=str(DEFAULT_BENCHMARK_DB),
        help="Optional benchmark.sqlite3 path with vector_batch_run rows.",
    )
    parser.add_argument(
        "--format",
        choices=("markdown", "json"),
        default="markdown",
        help="Output format (default: markdown).",
    )
    parser.add_argument(
        "--write-report",
        action="store_true",
        help="Write vector-benchmark-analysis.{md,json} inside the run directory.",
    )
    parser.add_argument("--high-vram-mb", type=int, default=7900)
    parser.add_argument("--plateau-range-mb", type=int, default=128)
    parser.add_argument("--low-gpu-util-pct", type=int, default=10)
    parser.add_argument("--underfeed-ready-chunks", type=int, default=128)
    parser.add_argument("--target-chunks-per-s", type=float, default=30.0)
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="Run a small in-process validation of the analyzer.",
    )
    return parser.parse_args()


def latest_run_dir() -> Path:
    candidates = [path for path in QUALIFICATION_ROOT.glob("*") if path.is_dir()]
    if not candidates:
        raise SystemExit(f"No qualification run found under {QUALIFICATION_ROOT}")
    return max(candidates, key=lambda path: path.stat().st_mtime)


def resolve_run_dir(raw: str) -> Path:
    if raw == "latest":
        return latest_run_dir()
    path = Path(raw)
    if not path.is_absolute():
        path = PROJECT_ROOT / path
    if not path.is_dir():
        raise SystemExit(f"Run directory not found: {path}")
    return path


def read_json(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    return json.loads(path.read_text())


def nested(payload: dict[str, Any], *keys: str, default: Any = None) -> Any:
    current: Any = payload
    for key in keys:
        if not isinstance(current, dict):
            return default
        current = current.get(key)
    return default if current is None else current


def number(value: Any, default: float = 0.0) -> float:
    if value is None:
        return default
    try:
        return float(value)
    except (TypeError, ValueError):
        return default


def load_samples(samples_path: Path) -> pl.DataFrame:
    rows: list[dict[str, Any]] = []
    if not samples_path.exists():
        return pl.DataFrame()
    for line in samples_path.read_text().splitlines():
        if not line.strip():
            continue
        try:
            sample = json.loads(line)
        except json.JSONDecodeError:
            continue
        cockpit = nested(sample, "cockpit", default={})
        graph_queue = nested(cockpit, "graph_projection_queue", default={})
        gpu = nested(sample, "gpu", default={})
        proc = nested(sample, "proc", default={})
        rows.append(
            {
                "timestamp": sample.get("timestamp", ""),
                "elapsed_s": number(sample.get("elapsed_seconds")),
                "pid": int(number(sample.get("pid"))),
                "rss_anon_mb": number(proc.get("rss_anon_bytes")) / (1024 * 1024),
                "cpu_percent": number(proc.get("cpu_percent")),
                "gpu_available": bool(gpu.get("available", False)),
                "gpu_used_mb": number(gpu.get("memory_used_mb")),
                "gpu_free_mb": number(gpu.get("memory_free_mb")),
                "gpu_util_pct": number(gpu.get("utilization_gpu_percent")),
                "gpu_memory_util_pct": number(gpu.get("utilization_memory_percent")),
                "known": number(cockpit.get("known")),
                "pending": number(cockpit.get("pending")),
                "graph_ready": number(cockpit.get("graph_ready")),
                "vector_ready": number(cockpit.get("vector_ready")),
                "vector_chunks_total": number(cockpit.get("vector_chunks_embedded_total")),
                "vector_chunks_inferred_total": number(
                    cockpit.get("vector_chunks_inferred_total")
                ),
                "reported_chunks_per_s": number(cockpit.get("chunk_embeddings_per_second")),
                "ready_chunks": number(cockpit.get("ready_queue_chunks_current")),
                "ready_batches_mixed": number(cockpit.get("ready_batches_mixed")),
                "oldest_ready_batch_age_ms": number(
                    cockpit.get("oldest_ready_batch_age_ms_current")
                ),
                "prepare_chunks": number(cockpit.get("prepare_inflight_chunks_current")),
                "ready_deficit": number(cockpit.get("ready_replenishment_deficit_current")),
                "has_inferred_counter": "vector_chunks_inferred_total" in cockpit,
                "has_embed_telemetry": "embed_attempts_total" in cockpit,
                "has_vector_worker_telemetry": "vector_workers_active_current" in cockpit,
                "embed_attempts_total": number(cockpit.get("embed_attempts_total")),
                "embed_inflight_started_at_ms": number(
                    cockpit.get("embed_inflight_started_at_ms")
                ),
                "embed_inflight_texts": number(cockpit.get("embed_inflight_texts_current")),
                "embed_inflight_bytes": number(
                    cockpit.get("embed_inflight_text_bytes_current")
                ),
                "last_embed_attempt_wall_ms": number(
                    cockpit.get("last_embed_attempt_wall_ms")
                ),
                "vector_workers_active": number(
                    cockpit.get("vector_workers_active_current")
                ),
                "vector_workers_started": number(
                    cockpit.get("vector_workers_started_total")
                ),
                "vector_worker_restarts": number(
                    cockpit.get("vector_worker_restarts_total")
                ),
                "graph_queue_total": number(graph_queue.get("total")),
                "graph_queue_queued": number(graph_queue.get("queued")),
                "graph_queue_inflight": number(graph_queue.get("inflight")),
                "graph_workers_active": number(cockpit.get("graph_workers_active_current")),
                "admission_wip": number(cockpit.get("admission_wip_current")),
                "admission_blocking_authority": cockpit.get("admission_blocking_authority", ""),
                "provider_effective": cockpit.get("vector_provider_effective_strategy", ""),
                "provider_label": cockpit.get("vector_provider_effective_label", ""),
            }
        )
    if not rows:
        return pl.DataFrame()
    return (
        pl.DataFrame(rows)
        .sort("elapsed_s")
        .with_columns(
            pl.col("elapsed_s").diff().fill_null(0).alias("delta_s"),
            pl.col("vector_chunks_total").diff().fill_null(0).clip(0).alias("chunk_delta"),
            pl.col("vector_chunks_inferred_total")
            .diff()
            .fill_null(0)
            .clip(0)
            .alias("inferred_chunk_delta"),
            pl.col("gpu_used_mb").diff().fill_null(0).alias("gpu_delta_mb"),
            pl.col("rss_anon_mb").diff().fill_null(0).alias("rss_delta_mb"),
            pl.col("graph_queue_total").diff().fill_null(0).alias("graph_queue_delta"),
            pl.col("embed_attempts_total").diff().fill_null(0).clip(0).alias(
                "embed_attempt_delta"
            ),
        )
        .with_columns(
            pl.when(pl.col("delta_s") > 0)
            .then(pl.col("chunk_delta") / pl.col("delta_s"))
            .otherwise(0.0)
            .alias("window_chunks_per_s"),
            (pl.col("embed_inflight_started_at_ms") > 0).alias("embed_inflight_active"),
        )
    )


def linear_slope_per_min(df: pl.DataFrame, value_col: str) -> float:
    if df.height < 2 or value_col not in df.columns:
        return 0.0
    stats = df.select(
        pl.col("elapsed_s").mean().alias("x_mean"),
        pl.col(value_col).mean().alias("y_mean"),
    ).row(0, named=True)
    x_mean = float(stats["x_mean"] or 0.0)
    y_mean = float(stats["y_mean"] or 0.0)
    slope_stats = df.select(
        ((pl.col("elapsed_s") - x_mean) * (pl.col(value_col) - y_mean))
        .sum()
        .alias("num"),
        ((pl.col("elapsed_s") - x_mean) ** 2).sum().alias("den"),
    ).row(0, named=True)
    den = float(slope_stats["den"] or 0.0)
    if den <= 0:
        return 0.0
    return float(slope_stats["num"] or 0.0) / den * 60.0


def summarize_samples(df: pl.DataFrame, thresholds: Thresholds) -> dict[str, Any]:
    if df.is_empty():
        return {
            "available": False,
            "diagnosis": "no_samples",
            "recommendations": ["Run a qualification with samples.ndjson enabled."],
        }

    first = df.head(1).row(0, named=True)
    last = df.tail(1).row(0, named=True)
    duration_s = max(0.0, float(last["elapsed_s"]) - float(first["elapsed_s"]))
    vector_delta = max(
        0.0, float(last["vector_chunks_total"]) - float(first["vector_chunks_total"])
    )
    inferred_delta = max(
        0.0,
        float(last["vector_chunks_inferred_total"])
        - float(first["vector_chunks_inferred_total"]),
    )
    persist_lag = max(0.0, inferred_delta - vector_delta)
    aggregate_rate = vector_delta / duration_s if duration_s > 0 else 0.0
    aggregate_inferred_rate = inferred_delta / duration_s if duration_s > 0 else 0.0
    basic = df.select(
        pl.col("window_chunks_per_s").max().alias("peak_window_chunks_per_s"),
        pl.col("inferred_chunk_delta").sum().alias("inferred_chunk_delta"),
        pl.col("reported_chunks_per_s").max().alias("peak_reported_chunks_per_s"),
        pl.col("gpu_used_mb").max().alias("max_gpu_used_mb"),
        pl.col("gpu_used_mb").mean().alias("avg_gpu_used_mb"),
        pl.col("gpu_util_pct").max().alias("max_gpu_util_pct"),
        pl.col("gpu_util_pct").mean().alias("avg_gpu_util_pct"),
        pl.col("ready_chunks").max().alias("max_ready_chunks"),
        pl.col("prepare_chunks").max().alias("max_prepare_chunks"),
        pl.col("ready_deficit").max().alias("max_ready_deficit"),
        pl.col("has_inferred_counter").any().alias("has_inferred_counter"),
        pl.col("has_embed_telemetry").any().alias("has_embed_telemetry"),
        pl.col("has_vector_worker_telemetry").any().alias("has_vector_worker_telemetry"),
        pl.col("ready_batches_mixed").max().alias("max_ready_batches_mixed"),
        pl.col("oldest_ready_batch_age_ms").max().alias("max_oldest_ready_batch_age_ms"),
        pl.col("embed_attempts_total").max().alias("max_embed_attempts_total"),
        pl.col("embed_attempt_delta").sum().alias("embed_attempt_delta"),
        pl.col("embed_inflight_started_at_ms").max().alias("max_embed_inflight_started_at_ms"),
        pl.col("embed_inflight_texts").max().alias("max_embed_inflight_texts"),
        pl.col("embed_inflight_bytes").max().alias("max_embed_inflight_bytes"),
        pl.col("last_embed_attempt_wall_ms").max().alias("max_last_embed_attempt_wall_ms"),
        pl.col("vector_workers_active").max().alias("max_vector_workers_active"),
        pl.col("vector_workers_started").max().alias("max_vector_workers_started"),
        pl.col("vector_worker_restarts").max().alias("max_vector_worker_restarts"),
        pl.col("graph_queue_total").max().alias("max_graph_queue_total"),
        pl.col("graph_queue_inflight").max().alias("max_graph_queue_inflight"),
        pl.col("graph_workers_active").max().alias("max_graph_workers_active"),
        pl.col("rss_anon_mb").max().alias("max_rss_anon_mb"),
    ).row(0, named=True)

    tail = df.tail(min(4, df.height))
    tail_gpu_range = 0.0
    tail_chunk_delta = 0.0
    if tail.height:
        tail_stats = tail.select(
            (pl.col("gpu_used_mb").max() - pl.col("gpu_used_mb").min()).alias(
                "gpu_range"
            ),
            pl.col("chunk_delta").sum().alias("chunk_delta"),
        ).row(0, named=True)
        tail_gpu_range = float(tail_stats["gpu_range"] or 0.0)
        tail_chunk_delta = float(tail_stats["chunk_delta"] or 0.0)

    max_gpu = float(basic["max_gpu_used_mb"] or 0.0)
    avg_gpu_util = float(basic["avg_gpu_util_pct"] or 0.0)
    max_ready = float(basic["max_ready_chunks"] or 0.0)
    max_prepare = float(basic["max_prepare_chunks"] or 0.0)
    max_deficit = float(basic["max_ready_deficit"] or 0.0)
    max_ready_batches_mixed = float(basic["max_ready_batches_mixed"] or 0.0)
    max_oldest_ready_age = float(basic["max_oldest_ready_batch_age_ms"] or 0.0)
    max_embed_attempts = float(basic["max_embed_attempts_total"] or 0.0)
    embed_attempt_delta = float(basic["embed_attempt_delta"] or 0.0)
    max_embed_inflight_started = float(basic["max_embed_inflight_started_at_ms"] or 0.0)
    max_embed_inflight_texts = float(basic["max_embed_inflight_texts"] or 0.0)
    max_embed_inflight_bytes = float(basic["max_embed_inflight_bytes"] or 0.0)
    max_last_embed_wall = float(basic["max_last_embed_attempt_wall_ms"] or 0.0)
    max_vector_workers_active = float(basic["max_vector_workers_active"] or 0.0)
    max_vector_restarts = float(basic["max_vector_worker_restarts"] or 0.0)
    has_inferred_counter = bool(basic["has_inferred_counter"])
    has_embed_telemetry = bool(basic["has_embed_telemetry"])
    has_vector_worker_telemetry = bool(basic["has_vector_worker_telemetry"])
    graph_queue_delta = max(
        0.0, float(last["graph_queue_total"]) - float(first["graph_queue_total"])
    )
    vram_slope = linear_slope_per_min(df, "gpu_used_mb")
    rss_slope = linear_slope_per_min(df, "rss_anon_mb")

    flags: list[str] = []
    recommendations: list[str] = []
    if vector_delta <= 0:
        flags.append("no_vector_progress")
    if not has_inferred_counter or not has_embed_telemetry or not has_vector_worker_telemetry:
        flags.append("instrumentation_incomplete")
        recommendations.append(
            "Regenerate this benchmark with the current runtime; older samples lack inferred/embed/vector-worker telemetry."
        )
    if (
        vector_delta > 0
        and has_inferred_counter
        and has_embed_telemetry
        and has_vector_worker_telemetry
        and inferred_delta <= 0
        and max_embed_attempts <= 0
        and max_vector_workers_active <= 0
    ):
        flags.append("instrumentation_incomplete")
        recommendations.append(
            "Persisted chunks moved but inferred/embed/vector-worker counters stayed at zero; treat the telemetry as missing or synthesized, not as a healthy GPU signal."
        )
    if inferred_delta > vector_delta:
        flags.append("inference_persist_lag")
        recommendations.append(
            "Measure persist/finalize queues before increasing GPU throughput; inference is ahead of persisted progress."
        )
    if (
        has_embed_telemetry
        and has_vector_worker_telemetry
        and vector_delta <= 0
        and max_embed_inflight_started > 0
        and max_vector_workers_active > 0
    ):
        flags.append("gpu_embed_inflight_without_persisted_progress")
        recommendations.append(
            "Inspect TensorRT subprocess response latency and persist handoff; GPU work is visible but no persisted chunk delta was observed."
        )
    if has_embed_telemetry and max_ready > 0 and max_embed_attempts <= 0:
        flags.append("ready_work_not_consumed")
        recommendations.append(
            "Inspect vector worker admission and ready queue accounting; ready work exists but no embed attempt was recorded."
        )
    if max_vector_restarts > 0:
        flags.append("vector_worker_restarts")
        recommendations.append("Inspect VectorWorkerFault rows before changing throughput knobs.")
    if max_gpu >= thresholds.high_vram_mb:
        flags.append("vram_overshoot")
        recommendations.append("Lower admission VRAM or force GPU subprocess recycle.")
    if (
        max_gpu > 0
        and tail_gpu_range <= thresholds.plateau_range_mb
        and tail_chunk_delta <= 0
        and float(last["gpu_used_mb"]) >= max_gpu - thresholds.plateau_range_mb
    ):
        flags.append("vram_plateau_without_progress")
        recommendations.append("Recycle the GPU service when high VRAM plateaus with zero chunk delta.")
    if (
        vector_delta <= 0
        and max_deficit > 0
        and max_ready + max_prepare < thresholds.underfeed_ready_chunks
    ):
        flags.append("vector_underfeed")
        recommendations.append("Increase graph supply or ready reserve before increasing GPU batch size.")
    if graph_queue_delta > 0 and vector_delta <= 0:
        flags.append("graph_progress_without_vector_drain")
        recommendations.append("Keep TensorRT init warm long enough or inspect vector lane admission.")
    if avg_gpu_util <= thresholds.low_gpu_util_pct and max_ready > thresholds.underfeed_ready_chunks:
        flags.append("gpu_underutilized_despite_ready_work")
        recommendations.append("Increase batch token cap or microbatch items if VRAM remains bounded.")
    if aggregate_rate < thresholds.target_chunks_per_s:
        flags.append("throughput_below_target")
        recommendations.append(
            f"Average persisted throughput is below target ({aggregate_rate:.2f} < {thresholds.target_chunks_per_s:.2f} chunks/s); do not treat peak throughput as delivery."
        )
    if aggregate_rate < 1.0 and (max_ready > 0 or max_prepare > 0) and max_embed_attempts > 0:
        flags.append("embed_or_handoff_stalled")
        recommendations.append(
            "Ready/prepare work and embed attempts are visible, but persisted throughput is nearly flat; inspect TensorRT response cadence, persist handoff, and outbox finalization."
        )
    if vram_slope > 256 and aggregate_rate <= 0:
        flags.append("vram_rising_no_throughput")
        recommendations.append("Treat this as engine-build or allocator retention, not real throughput.")

    if not recommendations:
        recommendations.append("Use the top throughput run as baseline and test one parameter axis at a time.")

    diagnosis = "healthy_progress"
    if "vram_overshoot" in flags:
        diagnosis = "unsafe_vram_overshoot"
    elif "instrumentation_incomplete" in flags:
        diagnosis = "instrumentation_incomplete"
    elif "vram_plateau_without_progress" in flags:
        diagnosis = "vram_plateau_without_progress"
    elif "vector_underfeed" in flags:
        diagnosis = "vector_underfeed"
    elif "gpu_embed_inflight_without_persisted_progress" in flags:
        diagnosis = "gpu_embed_inflight_without_persisted_progress"
    elif "ready_work_not_consumed" in flags:
        diagnosis = "ready_work_not_consumed"
    elif "no_vector_progress" in flags:
        diagnosis = "no_vector_progress"
    elif "gpu_underutilized_despite_ready_work" in flags:
        diagnosis = "gpu_underutilized"
    elif "embed_or_handoff_stalled" in flags:
        diagnosis = "embed_or_handoff_stalled"
    elif "throughput_below_target" in flags:
        diagnosis = "throughput_below_target"

    return {
        "available": True,
        "diagnosis": diagnosis,
        "flags": flags,
        "samples": df.height,
        "duration_s": round(duration_s, 3),
        "vector_chunk_delta": int(vector_delta),
        "inferred_chunk_delta": int(inferred_delta),
        "persist_lag_chunks": int(persist_lag),
        "aggregate_chunks_per_s": round(aggregate_rate, 6),
        "aggregate_inferred_chunks_per_s": round(aggregate_inferred_rate, 6),
        "peak_window_chunks_per_s": round(
            float(basic["peak_window_chunks_per_s"] or 0.0), 6
        ),
        "peak_reported_chunks_per_s": round(
            float(basic["peak_reported_chunks_per_s"] or 0.0), 6
        ),
        "gpu": {
            "max_used_mb": round(max_gpu, 3),
            "avg_used_mb": round(float(basic["avg_gpu_used_mb"] or 0.0), 3),
            "final_used_mb": round(float(last["gpu_used_mb"] or 0.0), 3),
            "slope_mb_per_min": round(vram_slope, 3),
            "tail_range_mb": round(tail_gpu_range, 3),
            "max_util_pct": round(float(basic["max_gpu_util_pct"] or 0.0), 3),
            "avg_util_pct": round(avg_gpu_util, 3),
        },
        "rss": {
            "max_anon_mb": round(float(basic["max_rss_anon_mb"] or 0.0), 3),
            "slope_mb_per_min": round(rss_slope, 3),
        },
        "pipeline": {
            "max_ready_chunks": round(max_ready, 3),
            "max_ready_batches_mixed": round(max_ready_batches_mixed, 3),
            "max_oldest_ready_batch_age_ms": round(max_oldest_ready_age, 3),
            "max_prepare_chunks": round(max_prepare, 3),
            "max_ready_deficit": round(max_deficit, 3),
            "max_embed_attempts_total": round(max_embed_attempts, 3),
            "embed_attempt_delta": round(embed_attempt_delta, 3),
            "max_embed_inflight_started_at_ms": round(max_embed_inflight_started, 3),
            "max_embed_inflight_texts": round(max_embed_inflight_texts, 3),
            "max_embed_inflight_bytes": round(max_embed_inflight_bytes, 3),
            "max_last_embed_attempt_wall_ms": round(max_last_embed_wall, 3),
            "max_vector_workers_active": round(max_vector_workers_active, 3),
            "max_vector_worker_restarts": round(max_vector_restarts, 3),
            "has_inferred_counter": has_inferred_counter,
            "has_embed_telemetry": has_embed_telemetry,
            "has_vector_worker_telemetry": has_vector_worker_telemetry,
            "max_graph_queue_total": round(float(basic["max_graph_queue_total"] or 0.0), 3),
            "max_graph_queue_inflight": round(
                float(basic["max_graph_queue_inflight"] or 0.0), 3
            ),
            "max_graph_workers_active": round(
                float(basic["max_graph_workers_active"] or 0.0), 3
            ),
        },
        "recommendations": recommendations,
    }


def load_batch_runs(db_path: Path) -> pl.DataFrame:
    if not db_path.exists() or db_path.stat().st_size == 0:
        return pl.DataFrame()
    con = sqlite3.connect(db_path)
    try:
        rows = con.execute(
            """
            SELECT
                run_id,
                started_at_ms,
                finished_at_ms,
                provider,
                provider_effective,
                runner_kind,
                chunk_count,
                total_tokens,
                micro_batch_count,
                persist_queue_wait_ms,
                finalize_queue_wait_ms,
                batch_wait_for_ready_ms,
                batch_lane,
                batch_shape,
                embed_ms,
                db_write_ms,
                mark_done_ms,
                wall_ms,
                gpu_used_mb,
                chunks_inferred_total,
                chunks_persisted_total,
                inference_persist_lag_chunks,
                persist_queue_depth_current,
                finalize_queue_depth_current,
                persist_claimed_current,
                vector_workers_active_current,
                vector_worker_restarts_total,
                vector_lane_state,
                success,
                error_reason,
                ready_queue_chunks_at_gpu_start,
                prepare_inflight_chunks_at_gpu_start,
                vector_worker_admission_reason
            FROM vector_batch_run
            ORDER BY finished_at_ms ASC
            """
        ).fetchall()
    except sqlite3.Error:
        return pl.DataFrame()
    finally:
        con.close()
    if not rows:
        return pl.DataFrame()
    return pl.DataFrame(
        rows,
        schema=[
            "run_id",
            "started_at_ms",
            "finished_at_ms",
            "provider",
            "provider_effective",
            "runner_kind",
            "chunk_count",
            "total_tokens",
            "micro_batch_count",
            "persist_queue_wait_ms",
            "finalize_queue_wait_ms",
            "batch_wait_for_ready_ms",
            "batch_lane",
            "batch_shape",
            "embed_ms",
            "db_write_ms",
            "mark_done_ms",
            "wall_ms",
            "gpu_used_mb",
            "chunks_inferred_total",
            "chunks_persisted_total",
            "inference_persist_lag_chunks",
            "persist_queue_depth_current",
            "finalize_queue_depth_current",
            "persist_claimed_current",
            "vector_workers_active_current",
            "vector_worker_restarts_total",
            "vector_lane_state",
            "success",
            "error_reason",
            "ready_queue_chunks_at_gpu_start",
            "prepare_inflight_chunks_at_gpu_start",
            "vector_worker_admission_reason",
        ],
        orient="row",
    )


def summarize_batches(df: pl.DataFrame) -> dict[str, Any]:
    if df.is_empty():
        return {"available": False, "diagnosis": "no_batch_rows"}
    stats = df.select(
        pl.len().alias("batch_count"),
        pl.col("success").sum().alias("successful_batches"),
        pl.col("chunk_count").sum().alias("chunks"),
        pl.col("total_tokens").sum().alias("tokens"),
        pl.col("embed_ms").sum().alias("embed_ms_total"),
        pl.col("wall_ms").sum().alias("wall_ms_total"),
        pl.col("db_write_ms").sum().alias("db_write_ms_total"),
        pl.col("mark_done_ms").sum().alias("mark_done_ms_total"),
        pl.col("embed_ms").median().alias("embed_ms_p50"),
        pl.col("embed_ms").quantile(0.95).alias("embed_ms_p95"),
        pl.col("db_write_ms").quantile(0.95).alias("db_write_ms_p95"),
        pl.col("mark_done_ms").quantile(0.95).alias("mark_done_ms_p95"),
        pl.col("persist_queue_wait_ms").quantile(0.95).alias("persist_queue_wait_ms_p95"),
        pl.col("finalize_queue_wait_ms").quantile(0.95).alias("finalize_queue_wait_ms_p95"),
        pl.col("batch_wait_for_ready_ms").quantile(0.95).alias("batch_wait_for_ready_ms_p95"),
        pl.col("wall_ms").quantile(0.95).alias("wall_ms_p95"),
        pl.col("gpu_used_mb").max().alias("max_gpu_used_mb"),
        pl.col("inference_persist_lag_chunks").max().alias("max_inference_persist_lag_chunks"),
        pl.col("persist_queue_depth_current").max().alias("max_persist_queue_depth_current"),
        pl.col("finalize_queue_depth_current").max().alias("max_finalize_queue_depth_current"),
        pl.col("persist_claimed_current").max().alias("max_persist_claimed_current"),
        pl.col("vector_workers_active_current").max().alias("max_vector_workers_active_current"),
        pl.col("vector_worker_restarts_total").max().alias("max_vector_worker_restarts_total"),
        pl.col("ready_queue_chunks_at_gpu_start").mean().alias("avg_ready_at_gpu_start"),
        pl.col("prepare_inflight_chunks_at_gpu_start").mean().alias(
            "avg_prepare_at_gpu_start"
        ),
    ).row(0, named=True)
    embed_ms_total = float(stats["embed_ms_total"] or 0.0)
    wall_ms_total = float(stats["wall_ms_total"] or 0.0)
    db_write_ms_total = float(stats["db_write_ms_total"] or 0.0)
    mark_done_ms_total = float(stats["mark_done_ms_total"] or 0.0)
    chunks = float(stats["chunks"] or 0.0)
    lane_counts = (
        df.group_by("batch_lane")
        .agg(pl.len().alias("count"), pl.col("chunk_count").sum().alias("chunks"))
        .sort("chunks", descending=True)
        .to_dicts()
        if "batch_lane" in df.columns
        else []
    )
    persist_ratio = db_write_ms_total / wall_ms_total if wall_ms_total > 0 else 0.0
    finalize_ratio = mark_done_ms_total / wall_ms_total if wall_ms_total > 0 else 0.0
    diagnosis = "healthy_batch_store"
    batch_flags: list[str] = []
    if chunks <= 0:
        diagnosis = "no_persisted_batch_chunks"
        batch_flags.append("no_persisted_batch_chunks")
    elif float(stats["max_inference_persist_lag_chunks"] or 0.0) > 0:
        diagnosis = "inference_ahead_of_persist"
        batch_flags.append("inference_ahead_of_persist")
    elif persist_ratio > 0.35:
        diagnosis = "persist_dominates_wall_time"
        batch_flags.append("persist_dominates_wall_time")
    elif finalize_ratio > 0.35:
        diagnosis = "finalize_dominates_wall_time"
        batch_flags.append("finalize_dominates_wall_time")
    return {
        "available": True,
        "diagnosis": diagnosis,
        "flags": batch_flags,
        "batch_count": int(stats["batch_count"] or 0),
        "successful_batches": int(stats["successful_batches"] or 0),
        "success_rate": round(
            float(stats["successful_batches"] or 0.0) / float(stats["batch_count"] or 1.0),
            6,
        ),
        "chunks": int(chunks),
        "tokens": int(stats["tokens"] or 0),
        "chunks_per_embed_s": round(chunks / (embed_ms_total / 1000.0), 6)
        if embed_ms_total > 0
        else 0.0,
        "chunks_per_wall_s": round(chunks / (wall_ms_total / 1000.0), 6)
        if wall_ms_total > 0
        else 0.0,
        "embed_ms_p50": round(float(stats["embed_ms_p50"] or 0.0), 3),
        "embed_ms_p95": round(float(stats["embed_ms_p95"] or 0.0), 3),
        "db_write_ms_p95": round(float(stats["db_write_ms_p95"] or 0.0), 3),
        "mark_done_ms_p95": round(float(stats["mark_done_ms_p95"] or 0.0), 3),
        "persist_queue_wait_ms_p95": round(
            float(stats["persist_queue_wait_ms_p95"] or 0.0), 3
        ),
        "finalize_queue_wait_ms_p95": round(
            float(stats["finalize_queue_wait_ms_p95"] or 0.0), 3
        ),
        "batch_wait_for_ready_ms_p95": round(
            float(stats["batch_wait_for_ready_ms_p95"] or 0.0), 3
        ),
        "wall_ms_p95": round(float(stats["wall_ms_p95"] or 0.0), 3),
        "persist_wall_ratio": round(persist_ratio, 6),
        "finalize_wall_ratio": round(finalize_ratio, 6),
        "max_gpu_used_mb": round(float(stats["max_gpu_used_mb"] or 0.0), 3),
        "max_inference_persist_lag_chunks": round(
            float(stats["max_inference_persist_lag_chunks"] or 0.0), 3
        ),
        "max_persist_queue_depth_current": round(
            float(stats["max_persist_queue_depth_current"] or 0.0), 3
        ),
        "max_finalize_queue_depth_current": round(
            float(stats["max_finalize_queue_depth_current"] or 0.0), 3
        ),
        "max_persist_claimed_current": round(
            float(stats["max_persist_claimed_current"] or 0.0), 3
        ),
        "max_vector_workers_active_current": round(
            float(stats["max_vector_workers_active_current"] or 0.0), 3
        ),
        "max_vector_worker_restarts_total": round(
            float(stats["max_vector_worker_restarts_total"] or 0.0), 3
        ),
        "avg_ready_at_gpu_start": round(float(stats["avg_ready_at_gpu_start"] or 0.0), 3),
        "avg_prepare_at_gpu_start": round(
            float(stats["avg_prepare_at_gpu_start"] or 0.0), 3
        ),
        "lane_counts": lane_counts,
    }


def build_report(run_dir: Path, benchmark_db: Path, thresholds: Thresholds) -> dict[str, Any]:
    samples = load_samples(run_dir / "samples.ndjson")
    summary = read_json(run_dir / "summary.json")
    batches = load_batch_runs(benchmark_db)
    return {
        "tool": "analyze_vector_benchmark",
        "engine": "polars",
        "run_dir": str(run_dir),
        "summary_created_at": summary.get("created_at"),
        "summary_dominant_bottleneck": summary.get("dominant_bottleneck"),
        "summary_max_gpu_used_mb": summary.get("max_gpu_used_mb"),
        "summary_max_chunks_per_s": summary.get("max_chunk_embeddings_per_second"),
        "samples": summarize_samples(samples, thresholds),
        "batches": summarize_batches(batches),
        "benchmark_db": str(benchmark_db),
    }


def render_markdown(report: dict[str, Any]) -> str:
    samples = report["samples"]
    batches = report["batches"]
    lines = [
        "# Vector Benchmark Temporal Analysis",
        "",
        f"- run: `{report['run_dir']}`",
        f"- engine: `{report['engine']}`",
        f"- summary bottleneck: `{report.get('summary_dominant_bottleneck')}`",
        f"- diagnosis: `{samples.get('diagnosis')}`",
        "",
        "## Time Series",
    ]
    if not samples.get("available"):
        lines.append(f"- unavailable: `{samples.get('diagnosis')}`")
    else:
        gpu = samples["gpu"]
        pipeline = samples["pipeline"]
        lines.extend(
            [
                f"- samples: `{samples['samples']}` over `{samples['duration_s']}s`",
                f"- chunks: persisted `{samples['vector_chunk_delta']}`, inferred `{samples['inferred_chunk_delta']}`, persist lag `{samples['persist_lag_chunks']}`, persisted `{samples['aggregate_chunks_per_s']}` chunks/s, inferred `{samples['aggregate_inferred_chunks_per_s']}` chunks/s",
                f"- GPU VRAM: max `{gpu['max_used_mb']} MB`, final `{gpu['final_used_mb']} MB`, slope `{gpu['slope_mb_per_min']} MB/min`, tail range `{gpu['tail_range_mb']} MB`",
                f"- GPU util: avg `{gpu['avg_util_pct']}%`, max `{gpu['max_util_pct']}%`",
                f"- pipeline: ready max `{pipeline['max_ready_chunks']}`, mixed ready batches max `{pipeline['max_ready_batches_mixed']}`, oldest ready age max `{pipeline['max_oldest_ready_batch_age_ms']} ms`, prepare max `{pipeline['max_prepare_chunks']}`, ready deficit max `{pipeline['max_ready_deficit']}`, graph queue max `{pipeline['max_graph_queue_total']}`",
                f"- vector worker: active max `{pipeline['max_vector_workers_active']}`, restarts max `{pipeline['max_vector_worker_restarts']}`, embed attempts delta `{pipeline['embed_attempt_delta']}`, inflight texts max `{pipeline['max_embed_inflight_texts']}`, inflight bytes max `{pipeline['max_embed_inflight_bytes']}`",
                f"- flags: `{', '.join(samples.get('flags', [])) or 'none'}`",
            ]
        )
    lines.extend(["", "## Batch Store"])
    if not batches.get("available"):
        lines.append(f"- unavailable: `{batches.get('diagnosis')}`")
    else:
        lines.extend(
            [
                f"- batches: `{batches['batch_count']}`, success rate `{batches['success_rate']}`",
                f"- throughput: `{batches['chunks_per_embed_s']}` chunks/embed-s, `{batches['chunks_per_wall_s']}` chunks/wall-s",
                f"- latency: embed p50 `{batches['embed_ms_p50']} ms`, embed p95 `{batches['embed_ms_p95']} ms`, wall p95 `{batches['wall_ms_p95']} ms`",
                f"- persist/finalize: db-write p95 `{batches['db_write_ms_p95']} ms`, mark-done p95 `{batches['mark_done_ms_p95']} ms`, persist wait p95 `{batches['persist_queue_wait_ms_p95']} ms`, finalize wait p95 `{batches['finalize_queue_wait_ms_p95']} ms`",
                f"- ratios: persist/wall `{batches['persist_wall_ratio']}`, finalize/wall `{batches['finalize_wall_ratio']}`, diagnosis `{batches['diagnosis']}`",
                f"- runtime snapshot: inference/persist lag max `{batches['max_inference_persist_lag_chunks']}`, persist queue max `{batches['max_persist_queue_depth_current']}`, finalize queue max `{batches['max_finalize_queue_depth_current']}`, vector workers active max `{batches['max_vector_workers_active_current']}`",
                f"- GPU max in batch rows: `{batches['max_gpu_used_mb']} MB`",
                f"- queue at GPU start: ready avg `{batches['avg_ready_at_gpu_start']}`, prepare avg `{batches['avg_prepare_at_gpu_start']}`",
            ]
        )
    lines.extend(["", "## Recommendations"])
    for item in samples.get("recommendations", []):
        lines.append(f"- {item}")
    return "\n".join(lines) + "\n"


def self_test() -> int:
    df = pl.DataFrame(
        [
            {"elapsed_s": 0, "vector_chunks_total": 0, "gpu_used_mb": 1000},
            {"elapsed_s": 10, "vector_chunks_total": 10, "gpu_used_mb": 1200},
            {"elapsed_s": 20, "vector_chunks_total": 30, "gpu_used_mb": 1400},
        ]
    ).with_columns(
        pl.col("elapsed_s").diff().fill_null(0).alias("delta_s"),
        pl.col("vector_chunks_total").diff().fill_null(0).clip(0).alias("chunk_delta"),
        pl.col("gpu_used_mb").diff().fill_null(0).alias("gpu_delta_mb"),
    ).with_columns(
        pl.when(pl.col("delta_s") > 0)
        .then(pl.col("chunk_delta") / pl.col("delta_s"))
        .otherwise(0.0)
        .alias("window_chunks_per_s")
    )
    slope = linear_slope_per_min(df, "gpu_used_mb")
    if round(slope, 3) != 1200.0:
        print(f"self-test failed: slope={slope}", file=sys.stderr)
        return 1
    print("self-test passed")
    return 0


def main() -> int:
    args = parse_args()
    if args.self_test:
        return self_test()
    run_dir = resolve_run_dir(args.run_dir)
    thresholds = Thresholds(
        high_vram_mb=args.high_vram_mb,
        plateau_range_mb=args.plateau_range_mb,
        low_gpu_util_pct=args.low_gpu_util_pct,
        underfeed_ready_chunks=args.underfeed_ready_chunks,
        target_chunks_per_s=args.target_chunks_per_s,
    )
    report = build_report(run_dir, Path(args.benchmark_db), thresholds)
    if args.format == "json":
        output = json.dumps(report, indent=2, sort_keys=True) + "\n"
        suffix = "json"
    else:
        output = render_markdown(report)
        suffix = "md"
    if args.write_report:
        target = run_dir / f"vector-benchmark-analysis.{suffix}"
        target.write_text(output)
        print(target)
    else:
        print(output, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
