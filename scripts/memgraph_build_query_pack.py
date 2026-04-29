#!/usr/bin/env python3
# Copyright (c) Didier Stadelmann. All rights reserved.

from __future__ import annotations

import argparse
import json
from pathlib import Path

from memgraph_build_cypherl import build_query_pack


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build a standalone Memgraph Cypher file that installs Axon's prepared human query pack."
    )
    parser.add_argument(
        "--query-dir",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "queries" / "memgraph",
        help="Directory containing prepared .cypher queries to install in Memgraph.",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=Path(__file__).resolve().parents[1]
        / "queries"
        / "memgraph"
        / "bootstrap"
        / "axon_query_pack.cypherl",
    )
    parser.add_argument(
        "--publication-id",
        default="standalone",
        help="Publication id stamped on PreparedQuery nodes when no graph publication is being imported.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    query_dir = args.query_dir.resolve()
    if not query_dir.exists():
        raise SystemExit(f"query directory does not exist: {query_dir}")
    summary = build_query_pack(query_dir, args.out.resolve(), args.publication_id)
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
