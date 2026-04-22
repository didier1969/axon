#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

export AXON_RUNTIME_SHADOW_ROLE="brain"
export AXON_SPLIT_SHADOW_ONLY="${AXON_SPLIT_SHADOW_ONLY:-0}"
export AXON_GPU_ACCESS_POLICY="${AXON_GPU_ACCESS_POLICY:-avoid}"
export AXON_SPLIT_BRAIN_IST_READER_ONLY="${AXON_SPLIT_BRAIN_IST_READER_ONLY:-1}"
export AXON_DUCKDB_MEMORY_LIMIT_GB="${AXON_DUCKDB_MEMORY_LIMIT_GB:-2}"

exec bash "$SCRIPT_DIR/start.sh" "$@" --read-only --skip-mcp-tests
