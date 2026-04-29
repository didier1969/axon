#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

export AXON_RUNTIME_SHADOW_ROLE="indexer"
export AXON_SPLIT_SHADOW_ONLY="${AXON_SPLIT_SHADOW_ONLY:-0}"
export AXON_GPU_ACCESS_POLICY="${AXON_GPU_ACCESS_POLICY:-shared}"
export AXON_RUNTIME_MODE="${AXON_RUNTIME_MODE:-indexer_graph}"

runtime_flag="--${AXON_RUNTIME_MODE//_/-}"

exec bash "$SCRIPT_DIR/start.sh" "$@" "$runtime_flag" --no-dashboard --skip-mcp-tests
