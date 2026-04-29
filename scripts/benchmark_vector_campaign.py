#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import itertools
import json
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
BENCHMARK_SCRIPT = PROJECT_ROOT / "scripts" / "benchmark-vector-token-matrix.sh"
BENCHMARK_ROOT = PROJECT_ROOT / ".axon" / "benchmarks"


@dataclass(frozen=True)
class Scenario:
    tokens: int
    ready_depth: int
    pipeline_depth: int
    prepare_workers: int
    max_items: int
    max_batch_bytes: int

    def key(self) -> tuple[int, int, int, int, int, int]:
        return (
            self.tokens,
            self.ready_depth,
            self.pipeline_depth,
            self.prepare_workers,
            self.max_items,
            self.max_batch_bytes,
        )


def parse_csv_ints(raw: str) -> list[int]:
    return [int(part.strip()) for part in raw.split(",") if part.strip()]


def make_result_dir(label_prefix: str) -> Path:
    BENCHMARK_ROOT.mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    path = BENCHMARK_ROOT / f"{stamp}-{label_prefix}"
    path.mkdir(parents=True, exist_ok=True)
    return path


def seed_scenarios(
    tokens: list[int],
    ready_depths: list[int],
    pipeline_depths: list[int],
    prepare_workers: list[int],
    max_items: list[int],
    max_batch_bytes: list[int],
) -> list[Scenario]:
    low_ready = ready_depths[0]
    high_ready = ready_depths[-1]
    low_pipeline = pipeline_depths[0]
    high_pipeline = pipeline_depths[-1]
    low_prepare = prepare_workers[0]
    high_prepare = prepare_workers[-1]
    low_items = max_items[0]
    high_items = max_items[-1]
    low_bytes = max_batch_bytes[0]
    high_bytes = max_batch_bytes[-1]

    patterns = [
        (low_ready, low_pipeline, low_prepare, low_items, low_bytes),
        (high_ready, low_pipeline, low_prepare, low_items, low_bytes),
        (low_ready, high_pipeline, high_prepare, low_items, low_bytes),
        (low_ready, low_pipeline, low_prepare, high_items, low_bytes),
        (low_ready, low_pipeline, low_prepare, low_items, high_bytes),
        (high_ready, high_pipeline, high_prepare, high_items, high_bytes),
    ]

    scenarios: list[Scenario] = []
    seen: set[tuple[int, int, int, int, int, int]] = set()
    for token in tokens:
        for ready, pipeline, prepare, items, batch_bytes in patterns:
            scenario = Scenario(token, ready, pipeline, prepare, items, batch_bytes)
            if scenario.key() in seen:
                continue
            seen.add(scenario.key())
            scenarios.append(scenario)
    return scenarios


def scenario_neighbors(
    scenario: Scenario,
    axes: dict[str, list[int]],
) -> list[Scenario]:
    values = {
        "tokens": scenario.tokens,
        "ready_depth": scenario.ready_depth,
        "pipeline_depth": scenario.pipeline_depth,
        "prepare_workers": scenario.prepare_workers,
        "max_items": scenario.max_items,
        "max_batch_bytes": scenario.max_batch_bytes,
    }
    neighbors: list[Scenario] = []
    for axis, options in axes.items():
        current = values[axis]
        try:
            idx = options.index(current)
        except ValueError:
            continue
        for delta in (-1, 1):
            next_idx = idx + delta
            if next_idx < 0 or next_idx >= len(options):
                continue
            next_values = dict(values)
            next_values[axis] = options[next_idx]
            neighbors.append(
                Scenario(
                    tokens=next_values["tokens"],
                    ready_depth=next_values["ready_depth"],
                    pipeline_depth=next_values["pipeline_depth"],
                    prepare_workers=next_values["prepare_workers"],
                    max_items=next_values["max_items"],
                    max_batch_bytes=next_values["max_batch_bytes"],
                )
            )
    return neighbors


def score_row(row: dict[str, str]) -> float:
    bottleneck = row.get("dominant_bottleneck", "").strip().lower()
    if bottleneck.startswith("degraded"):
        return float("-inf")
    if bottleneck in {"vector_underfeed", "unknown"}:
        return -1_000_000.0
    window = float(row["window_chunks_per_second"])
    instant = float(row["max_chunk_embeddings_per_second"])
    return window * 1000.0 + instant


def choose_next_adaptive(
    rows: list[dict[str, str]],
    seen: set[tuple[int, int, int, int, int, int]],
    all_scenarios: list[Scenario],
    axes: dict[str, list[int]],
) -> Scenario | None:
    if not rows:
        for scenario in all_scenarios:
            if scenario.key() not in seen:
                return scenario
        return None

    ranked = sorted(rows, key=score_row, reverse=True)
    candidates: list[Scenario] = []
    candidate_seen: set[tuple[int, int, int, int, int, int]] = set()

    def add_candidate(candidate: Scenario) -> None:
        key = candidate.key()
        if key in seen or key in candidate_seen:
            return
        candidate_seen.add(key)
        candidates.append(candidate)

    for row in ranked[:8]:
        base = Scenario(
            tokens=int(row["tokens"]),
            ready_depth=int(row["ready_depth"]),
            pipeline_depth=int(row["pipeline_depth"]),
            prepare_workers=int(row["prepare_workers"]),
            max_items=int(row["max_items"]),
            max_batch_bytes=int(row["max_batch_bytes"]),
        )
        for neighbor in scenario_neighbors(base, axes):
            add_candidate(neighbor)

    if candidates:
        parent_score = {
            (
                int(row["tokens"]),
                int(row["ready_depth"]),
                int(row["pipeline_depth"]),
                int(row["prepare_workers"]),
                int(row["max_items"]),
                int(row["max_batch_bytes"]),
            ): score_row(row)
            for row in ranked[:8]
        }

        def candidate_score(candidate: Scenario) -> float:
            best_parent = max(
                (
                    parent_score[parent.key()]
                    for parent in (
                        Scenario(
                            tokens=int(row["tokens"]),
                            ready_depth=int(row["ready_depth"]),
                            pipeline_depth=int(row["pipeline_depth"]),
                            prepare_workers=int(row["prepare_workers"]),
                            max_items=int(row["max_items"]),
                            max_batch_bytes=int(row["max_batch_bytes"]),
                        )
                        for row in ranked[:8]
                    )
                    if candidate in scenario_neighbors(parent, axes)
                ),
                default=0.0,
            )
            distance_penalty = (
                abs(candidate.tokens)
                + abs(candidate.ready_depth)
                + abs(candidate.pipeline_depth) * 10
                + abs(candidate.prepare_workers) * 20
                + abs(candidate.max_items)
                + abs(candidate.max_batch_bytes // (1024 * 1024))
            )
            return best_parent - distance_penalty * 0.001

        candidates.sort(key=candidate_score, reverse=True)
        return candidates[0]

    for scenario in all_scenarios:
        if scenario.key() not in seen:
            return scenario
    return None


def read_single_result(results_tsv: Path) -> dict[str, str]:
    rows = list(csv.DictReader(results_tsv.read_text().splitlines(), delimiter="\t"))
    if len(rows) != 1:
        raise RuntimeError(f"Expected 1 row in {results_tsv}, found {len(rows)}")
    return rows[0]


def append_campaign_row(path: Path, row: dict[str, str]) -> None:
    exists = path.exists()
    fieldnames = [
        "run_index",
        "elapsed_wall_seconds",
        "gpu_backend",
        "tokens",
        "ready_depth",
        "pipeline_depth",
        "prepare_workers",
        "max_items",
        "max_batch_bytes",
        "graph_workers",
        "max_chunk_embeddings_per_second",
        "window_chunk_delta",
        "window_chunks_per_second",
        "max_graph_projection_queue_runtime_inflight",
        "max_graph_workers_active_current",
        "dominant_bottleneck",
        "summary_path",
        "nested_results_tsv",
        "scenario_label",
    ]
    with path.open("a", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames, delimiter="\t")
        if not exists:
            writer.writeheader()
        writer.writerow(row)


def write_leaderboard(results_path: Path, leaderboard_path: Path) -> None:
    rows = list(csv.DictReader(results_path.read_text().splitlines(), delimiter="\t"))
    ranked = sorted(rows, key=lambda row: float(row["window_chunks_per_second"]), reverse=True)
    with leaderboard_path.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=rows[0].keys(), delimiter="\t")
        writer.writeheader()
        writer.writerows(ranked[:20])


def run_scenario(
    scenario: Scenario,
    run_index: int,
    args: argparse.Namespace,
    result_dir: Path,
    started_at: float,
) -> dict[str, str]:
    scenario_label = (
        f"{args.label_prefix}-r{run_index:03d}"
        f"-t{scenario.tokens}"
        f"-rq{scenario.ready_depth}"
        f"-pp{scenario.pipeline_depth}"
        f"-pw{scenario.prepare_workers}"
        f"-mi{scenario.max_items}"
        f"-bb{scenario.max_batch_bytes}"
    )
    cmd = [
        "bash",
        str(BENCHMARK_SCRIPT),
        "--mode",
        "warm",
        "--gpu-backend",
        args.gpu_backend,
        "--tokens",
        str(scenario.tokens),
        "--duration",
        str(args.duration),
        "--interval",
        str(args.interval),
        "--label-prefix",
        scenario_label,
        "--max-items",
        str(scenario.max_items),
        "--max-batch-bytes",
        str(scenario.max_batch_bytes),
        "--graph-workers",
        str(args.graph_workers),
        "--prepare-workers",
        str(scenario.prepare_workers),
        "--ready-depth",
        str(scenario.ready_depth),
        "--pipeline-depth",
        str(scenario.pipeline_depth),
    ]
    if args.manifest:
        cmd.extend(["--manifest", args.manifest])
    subprocess.run(cmd, cwd=PROJECT_ROOT, check=True)

    nested_dir = max(
        BENCHMARK_ROOT.glob(f"*{scenario_label}"),
        key=lambda path: path.stat().st_mtime,
    )
    nested_results_tsv = nested_dir / "results.tsv"
    nested_row = read_single_result(nested_results_tsv)
    return {
        "run_index": str(run_index),
        "elapsed_wall_seconds": f"{time.time() - started_at:.0f}",
        "gpu_backend": nested_row["gpu_backend"],
        "tokens": str(scenario.tokens),
        "ready_depth": str(scenario.ready_depth),
        "pipeline_depth": str(scenario.pipeline_depth),
        "prepare_workers": str(scenario.prepare_workers),
        "max_items": str(scenario.max_items),
        "max_batch_bytes": str(scenario.max_batch_bytes),
        "graph_workers": str(args.graph_workers),
        "max_chunk_embeddings_per_second": nested_row["max_chunk_embeddings_per_second"],
        "window_chunk_delta": nested_row["window_chunk_delta"],
        "window_chunks_per_second": nested_row["window_chunks_per_second"],
        "max_graph_projection_queue_runtime_inflight": nested_row[
            "max_graph_projection_queue_runtime_inflight"
        ],
        "max_graph_workers_active_current": nested_row["max_graph_workers_active_current"],
        "dominant_bottleneck": nested_row["dominant_bottleneck"],
        "summary_path": nested_row["summary_path"],
        "nested_results_tsv": str(nested_results_tsv),
        "scenario_label": scenario_label,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Run an 8h adaptive vector benchmark campaign.")
    parser.add_argument("--budget-seconds", type=int, default=8 * 60 * 60)
    parser.add_argument("--duration", type=int, default=120)
    parser.add_argument("--interval", type=int, default=1)
    parser.add_argument("--graph-workers", type=int, default=2)
    parser.add_argument("--label-prefix", default="vector-campaign-8h")
    parser.add_argument("--gpu-backend", choices=("cuda", "tensorrt"), default="cuda")
    parser.add_argument("--manifest", default="")
    parser.add_argument("--tokens", default="12000,16000,32000,48000")
    parser.add_argument("--ready-depths", default="96,160,256")
    parser.add_argument("--pipeline-depths", default="12,24")
    parser.add_argument("--prepare-workers", default="8,12")
    parser.add_argument("--max-items-values", default="128,192,256")
    parser.add_argument("--max-batch-bytes-values", default="8388608,12582912")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    axes = {
        "tokens": parse_csv_ints(args.tokens),
        "ready_depth": parse_csv_ints(args.ready_depths),
        "pipeline_depth": parse_csv_ints(args.pipeline_depths),
        "prepare_workers": parse_csv_ints(args.prepare_workers),
        "max_items": parse_csv_ints(args.max_items_values),
        "max_batch_bytes": parse_csv_ints(args.max_batch_bytes_values),
    }

    all_scenarios = [
        Scenario(*combo)
        for combo in itertools.product(
            axes["tokens"],
            axes["ready_depth"],
            axes["pipeline_depth"],
            axes["prepare_workers"],
            axes["max_items"],
            axes["max_batch_bytes"],
        )
    ]
    seeds = seed_scenarios(
        axes["tokens"],
        axes["ready_depth"],
        axes["pipeline_depth"],
        axes["prepare_workers"],
        axes["max_items"],
        axes["max_batch_bytes"],
    )

    if args.dry_run:
        for idx, scenario in enumerate(seeds, start=1):
            print(idx, scenario)
        return 0

    result_dir = make_result_dir(args.label_prefix)
    results_path = result_dir / "campaign-results.tsv"
    leaderboard_path = result_dir / "leaderboard.tsv"
    manifest_path = result_dir / "manifest.json"
    manifest_path.write_text(
        json.dumps(
            {
                "created_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "budget_seconds": args.budget_seconds,
                "duration_seconds": args.duration,
                "interval_seconds": args.interval,
                "graph_workers": args.graph_workers,
                "gpu_backend": args.gpu_backend,
                "manifest": args.manifest,
                "axes": axes,
                "seed_count": len(seeds),
                "strategy": "seed_matrix_then_adaptive_neighbors",
            },
            indent=2,
        )
    )

    started_at = time.time()
    deadline = started_at + args.budget_seconds
    seen: set[tuple[int, int, int, int, int, int]] = set()
    rows: list[dict[str, str]] = []
    run_index = 0
    seed_index = 0

    while time.time() < deadline:
        if seed_index < len(seeds):
            scenario = seeds[seed_index]
            seed_index += 1
            if scenario.key() in seen:
                continue
        else:
            scenario = choose_next_adaptive(rows, seen, all_scenarios, axes)
            if scenario is None:
                break

        run_index += 1
        seen.add(scenario.key())
        print(
            f"[vector-campaign] run={run_index} "
            f"tokens={scenario.tokens} ready={scenario.ready_depth} "
            f"pipeline={scenario.pipeline_depth} prepare={scenario.prepare_workers} "
            f"items={scenario.max_items} bytes={scenario.max_batch_bytes}",
            flush=True,
        )
        row = run_scenario(scenario, run_index, args, result_dir, started_at)
        rows.append(row)
        append_campaign_row(results_path, row)
        write_leaderboard(results_path, leaderboard_path)

    print(f"[vector-campaign] campaign results: {results_path}")
    print(f"[vector-campaign] leaderboard: {leaderboard_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
