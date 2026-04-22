#!/usr/bin/env python3
"""Seed the canonical current project into the dev brain MCP registry."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

SCRIPT_ROOT = Path(__file__).resolve().parent
PROJECT_ROOT = SCRIPT_ROOT.parent
sys.path.insert(0, str(SCRIPT_ROOT))

from mcp_probe_common import call_tool, initialize_session, response_text  # noqa: E402


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--meta",
        type=Path,
        default=PROJECT_ROOT / ".axon" / "meta.json",
        help="Canonical project metadata file",
    )
    parser.add_argument(
        "--url",
        default="http://127.0.0.1:44139/mcp",
        help="Brain MCP endpoint",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=20,
        help="RPC timeout in seconds",
    )
    return parser


def load_meta(path: Path) -> dict[str, object]:
    raw = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(raw, dict):
        raise RuntimeError(f"Invalid meta payload in {path}")
    return raw


def main() -> int:
    args = build_parser().parse_args()
    meta = load_meta(args.meta)
    project_path = str(meta.get("path") or "").strip()
    project_code = str(meta.get("code") or "").strip()

    if not project_path:
        raise RuntimeError(f"Missing project path in {args.meta}")

    initialize_session(args.url, args.timeout, "seed_dev_project_registry")
    _, response = call_tool(
        args.url,
        args.timeout,
        "axon_init_project",
        {
            "project_path": project_path,
            **({"project_code": project_code} if project_code else {}),
        },
    )

    if response.get("error") is not None:
        raise RuntimeError(f"axon_init_project returned error: {response['error']}")

    print(response_text(response))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
