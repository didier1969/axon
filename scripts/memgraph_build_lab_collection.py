#!/usr/bin/env python3
# Copyright (c) Didier Stadelmann. All rights reserved.

from __future__ import annotations

import argparse
import json
from pathlib import Path

from memgraph_build_cypherl import build_lab_collection


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build a Memgraph Lab importable query collection from Axon's prepared human queries."
    )
    parser.add_argument(
        "--query-dir",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "queries" / "memgraph",
        help="Directory containing prepared .cypher queries.",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=Path(__file__).resolve().parents[1]
        / "queries"
        / "memgraph"
        / "bootstrap"
        / "axon_lab_query_collection.json",
    )
    parser.add_argument(
        "--publication-id",
        default="standalone",
        help="Publication id stamped in the exported Lab collection metadata.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    query_dir = args.query_dir.resolve()
    if not query_dir.exists():
        raise SystemExit(f"query directory does not exist: {query_dir}")
    summary = build_lab_collection(query_dir, args.out.resolve(), args.publication_id)
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
