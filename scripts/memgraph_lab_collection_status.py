#!/usr/bin/env python3
# Copyright (c) Didier Stadelmann. All rights reserved.

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


PLACEHOLDER_TOKENS = ("$project_code", "$target", "$limit", "$min_degree")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Validate Axon's Memgraph Lab query collection artifact.")
    parser.add_argument(
        "--collection",
        type=Path,
        default=Path(__file__).resolve().parents[1]
        / "queries"
        / "memgraph"
        / "bootstrap"
        / "axon_lab_query_collection.json",
    )
    parser.add_argument(
        "--url",
        default="http://127.0.0.1:3000/axon_lab_query_collection.json",
        help="Optional Lab-served URL to probe.",
    )
    parser.add_argument(
        "--installer-url",
        default="http://127.0.0.1:3000/axon_lab_install_collection.html",
        help="Optional Lab-served browser installer URL to probe.",
    )
    parser.add_argument("--skip-url", action="store_true", help="Only validate the local JSON file.")
    return parser.parse_args()


def load_collection(path: Path) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise SystemExit(f"collection file missing: {path}") from exc
    except json.JSONDecodeError as exc:
        raise SystemExit(f"collection file is not valid JSON: {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise SystemExit("collection root must be a JSON object")
    return data


def collection_queries(data: dict[str, Any]) -> list[dict[str, Any]]:
    collections = data.get("sampleCollections")
    if not isinstance(collections, list) or not collections:
        raise SystemExit("collection must contain sampleCollections[0]")
    queries = collections[0].get("queries")
    if not isinstance(queries, list):
        raise SystemExit("collection must contain sampleCollections[0].queries")
    return [query for query in queries if isinstance(query, dict)]


def validate_queries(queries: list[dict[str, Any]]) -> dict[str, Any]:
    unresolved = []
    for query in queries:
        code = str(query.get("query", ""))
        tokens = [token for token in PLACEHOLDER_TOKENS if token in code]
        if tokens:
            unresolved.append({"title": query.get("title"), "tokens": tokens})
    return {
        "query_count": len(queries),
        "first_query": queries[0].get("title") if queries else None,
        "last_query": queries[-1].get("title") if queries else None,
        "unresolved_placeholders": unresolved,
        "click_ready": len(queries) == 27 and not unresolved,
    }


def probe_url(url: str) -> dict[str, Any]:
    try:
        with urllib.request.urlopen(url, timeout=5) as response:
            body = response.read(1024 * 1024)
            served = json.loads(body.decode("utf-8"))
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as exc:
        return {"available": False, "url": url, "error": str(exc)}
    queries = collection_queries(served)
    status = validate_queries(queries)
    status.update({"available": True, "url": url})
    return status


def probe_installer_url(url: str) -> dict[str, Any]:
    try:
        with urllib.request.urlopen(url, timeout=5) as response:
            body = response.read(256 * 1024).decode("utf-8", errors="replace")
    except (urllib.error.URLError, TimeoutError) as exc:
        return {"available": False, "url": url, "error": str(exc)}
    markers = [
        "Axon query collection installer",
        "/axon_lab_query_collection.json",
        "memgraph-lab-db",
        "collections",
    ]
    missing = [marker for marker in markers if marker not in body]
    return {
        "available": True,
        "url": url,
        "installer_ready": not missing,
        "missing_markers": missing,
    }


def main() -> int:
    args = parse_args()
    data = load_collection(args.collection.resolve())
    queries = collection_queries(data)
    local_status = validate_queries(queries)
    result = {
        "collection": str(args.collection.resolve()),
        "local": local_status,
        "lab_url": None if args.skip_url else probe_url(args.url),
        "installer_url": None if args.skip_url else probe_installer_url(args.installer_url),
    }
    print(json.dumps(result, indent=2, sort_keys=True))
    if not local_status["click_ready"]:
        return 1
    if result["lab_url"] is not None and not result["lab_url"].get("available"):
        return 2
    if result["installer_url"] is not None and not result["installer_url"].get("installer_ready"):
        return 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
