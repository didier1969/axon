#!/usr/bin/env python3
"""Validate live MCP runtime identity against a release manifest."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys


REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "scripts"))

from mcp_probe_common import call_tool, initialize_session, response_data  # noqa: E402


def load_manifest(path: pathlib.Path) -> dict:
    manifest = json.loads(path.read_text())
    if not isinstance(manifest, dict):
        raise SystemExit("manifest payload is not an object")
    return manifest


def require_str(mapping: dict, key: str) -> str:
    value = mapping.get(key)
    if not isinstance(value, str) or not value:
        raise SystemExit(f"manifest missing string field: {key}")
    return value


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Check that live MCP status matches a target release manifest."
    )
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--url", required=True)
    parser.add_argument("--timeout", type=int, default=8)
    parser.add_argument("--expect-instance", default="live")
    parser.add_argument("--install-generation")
    args = parser.parse_args()

    manifest = load_manifest(pathlib.Path(args.manifest).resolve())
    runtime_version = manifest.get("runtime_version")
    if not isinstance(runtime_version, dict):
        raise SystemExit("manifest missing runtime_version object")

    expected = {
        "release_version": require_str(runtime_version, "release_version"),
        "package_version": require_str(runtime_version, "package_version"),
        "build_id": require_str(runtime_version, "build_id"),
        "install_generation": args.install_generation
        or require_str(runtime_version, "install_generation"),
    }

    initialize_session(args.url, args.timeout, "release-runtime-check")
    _, response = call_tool(args.url, args.timeout, "status", {"mode": "brief"})
    data = response_data(response)
    live_runtime = data.get("runtime_version")
    if not isinstance(live_runtime, dict):
        raise SystemExit("status missing data.runtime_version")
    instance_identity = data.get("instance_identity")
    if not isinstance(instance_identity, dict):
        raise SystemExit("status missing data.instance_identity")

    actual_instance = instance_identity.get("instance_kind")
    if actual_instance != args.expect_instance:
        raise SystemExit(
            f"instance mismatch: expected {args.expect_instance}, got {actual_instance}"
        )

    mismatches: list[str] = []
    for key, expected_value in expected.items():
        actual_value = live_runtime.get(key)
        if actual_value != expected_value:
            mismatches.append(f"{key}: expected {expected_value}, got {actual_value}")

    if mismatches:
        raise SystemExit("runtime_version mismatch: " + "; ".join(mismatches))

    print(
        json.dumps(
            {
                "status": "ok",
                "instance_kind": actual_instance,
                "runtime_version": {
                    key: live_runtime.get(key) for key in expected.keys()
                },
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
