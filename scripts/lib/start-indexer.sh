#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

export AXON_RUNTIME_SHADOW_ROLE="indexer"
export AXON_SPLIT_SHADOW_ONLY="${AXON_SPLIT_SHADOW_ONLY:-0}"
export AXON_GPU_ACCESS_POLICY="${AXON_GPU_ACCESS_POLICY:-shared}"
# Default to indexer_full so live promote-restart resumes embedding by
# default (DEC-AXO-NNN, operator directive 2026-05-14). Override via
# `AXON_RUNTIME_MODE=indexer_graph` for CPU-only ingest.
export AXON_RUNTIME_MODE="${AXON_RUNTIME_MODE:-indexer_full}"

runtime_flag="--${AXON_RUNTIME_MODE//_/-}"

# REQ-AXO-901737 : tensorrt is the default for live vector modes when the
# operator hasn't explicitly set AXON_EMBEDDING_PROVIDER. No more
# AXON_REQUEST_TENSORRT indirection ; the canonical knob is the single
# source of truth. Operator opts out via AXON_EMBEDDING_PROVIDER={cpu,cuda}.
if [[ "$AXON_RUNTIME_MODE" == "indexer_full" || "$AXON_RUNTIME_MODE" == "indexer_vector" ]]; then
    : "${AXON_EMBEDDING_PROVIDER:=tensorrt}"
    export AXON_EMBEDDING_PROVIDER
fi

exec bash "$SCRIPT_DIR/../start.sh" "$@" "$runtime_flag" --no-dashboard --skip-mcp-tests
