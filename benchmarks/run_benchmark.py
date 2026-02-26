"""Axon benchmark CLI — measures per-phase indexing performance.

Usage:
    python benchmarks/run_benchmark.py --repo-path /path/to/repo
    python benchmarks/run_benchmark.py --repo-path /path/to/repo --no-embeddings
    python benchmarks/run_benchmark.py --repo-path /path/to/repo --json
"""

from __future__ import annotations

import argparse
import dataclasses
import json
import sys
from pathlib import Path

PHASE_NAMES: dict[str, str] = {
    "walk":         "File walking",
    "structure":    "Structure processing",
    "parsing":      "Parsing code",
    "imports":      "Resolving imports",
    "calls":        "Tracing calls",
    "heritage":     "Extracting heritage",
    "types":        "Analyzing types",
    "communities":  "Detecting communities",
    "processes":    "Detecting execution flows",
    "dead_code":    "Finding dead code",
    "coupling":     "Analyzing git history",
    "storage_load": "Loading to storage",
    "embeddings":   "Generating embeddings",
}


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Benchmark Axon indexing performance on a repository.",
    )
    parser.add_argument(
        "--repo-path",
        required=True,
        type=Path,
        help="Path to the repository to index",
    )
    parser.add_argument(
        "--no-embeddings",
        action="store_true",
        default=True,
        help="Skip embedding generation (default: True for benchmark isolation)",
    )
    parser.add_argument(
        "--with-embeddings",
        action="store_true",
        default=False,
        help="Include embedding generation in benchmark",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        default=False,
        help="Output results as JSON instead of table",
    )
    args = parser.parse_args()

    repo_path = args.repo_path.resolve()
    if not repo_path.is_dir():
        print(f"Error: --repo-path '{repo_path}' is not a directory", file=sys.stderr)
        sys.exit(1)

    run_embeddings = args.with_embeddings and not args.no_embeddings

    # Import here so the script can be run from the repo root with uv run.
    try:
        from axon.core.ingestion.pipeline import run_pipeline
    except ImportError as exc:
        print(
            f"Error: could not import axon. Run with: uv run python benchmarks/run_benchmark.py\n{exc}",
            file=sys.stderr,
        )
        sys.exit(1)

    print(f"Indexing {repo_path} ...", file=sys.stderr)
    _graph, result = run_pipeline(repo_path, embeddings=run_embeddings)

    # Build phase rows sorted by duration descending.
    timings = dataclasses.asdict(result.phase_timings)
    total = result.duration_seconds
    rows: list[tuple[str, float, float]] = []
    for field_name, duration in timings.items():
        if duration == 0.0:
            continue
        pct = (duration / total * 100) if total > 0 else 0.0
        rows.append((PHASE_NAMES.get(field_name, field_name), duration, pct))
    rows.sort(key=lambda r: r[1], reverse=True)

    bottleneck_name, bottleneck_dur, bottleneck_pct = rows[0] if rows else ("N/A", 0.0, 0.0)

    if args.json:
        output = {
            "repo": str(repo_path),
            "files": result.files,
            "symbols": result.symbols,
            "relationships": result.relationships,
            "total_seconds": round(total, 3),
            "bottleneck_phase": bottleneck_name,
            "bottleneck_seconds": round(bottleneck_dur, 3),
            "phases": [
                {"name": name, "seconds": round(dur, 3), "pct": round(pct, 1)}
                for name, dur, pct in rows
            ],
        }
        print(json.dumps(output, indent=2))
        return

    # Table output.
    width = 51
    print()
    print("═" * width)
    print("AXON BENCHMARK REPORT")
    print(f"Repo:      {repo_path}")
    print(f"Files:     {result.files:,}")
    print(f"Symbols:   {result.symbols:,}")
    print(f"Relations: {result.relationships:,}")
    print(f"Total:     {total:.2f}s")
    print("═" * width)
    print(f"{'Phase':<30} {'Duration':>8}  {'%':>6}")
    print("─" * width)
    for name, dur, pct in rows:
        print(f"{name:<30} {dur:>7.2f}s  {pct:>5.1f}%")
    print("═" * width)
    print(f"Bottleneck: {bottleneck_name} ({bottleneck_dur:.2f}s, {bottleneck_pct:.1f}% of total)")
    print("═" * width)
    print()


if __name__ == "__main__":
    main()
