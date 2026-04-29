#!/usr/bin/env python3
from __future__ import annotations

import json
import sys
from pathlib import Path


def fail(message: str) -> None:
    raise SystemExit(message)


def main() -> None:
    if len(sys.argv) != 2:
        fail("usage: validate_ort_manifest.py <manifest_path>")

    manifest_path = Path(sys.argv[1])
    if not manifest_path.is_file():
        fail(f"manifest not found: {manifest_path}")

    payload = json.loads(manifest_path.read_text())

    provider = payload.get("provider")
    if provider not in {"cuda", "tensorrt"}:
        fail(f"manifest provider must be 'cuda' or 'tensorrt', found: {provider!r}")

    required = ["core_lib", "cuda_provider_lib"]
    if provider == "tensorrt":
        required.append("tensorrt_provider_lib")

    missing = [key for key in required if not payload.get(key)]
    if missing:
        fail(f"manifest missing required fields: {missing}")

    for key in required:
        path = Path(payload[key])
        if not path.is_file():
            fail(f"manifest path for {key} does not exist: {path}")

    tensorrt_lib_dir = payload.get("tensorrt_lib_dir")
    if tensorrt_lib_dir and not Path(tensorrt_lib_dir).is_dir():
        fail(f"manifest tensorrt_lib_dir does not exist: {tensorrt_lib_dir}")


if __name__ == "__main__":
    main()
