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

# Implicit --tensorrt when running a vector lane on live (operator directive
# 2026-05-14: "dorénavant c'est l'indexeur full avec tensor rt par défaut").
# Honors explicit --tensorrt / --no-tensorrt passthrough and AXON_REQUEST_TENSORRT
# env override.
tensorrt_flag=""
if [[ "$AXON_RUNTIME_MODE" == "indexer_full" || "$AXON_RUNTIME_MODE" == "indexer_vector" ]]; then
    if [[ "${AXON_REQUEST_TENSORRT:-1}" == "1" ]] && ! printf '%s\n' "$@" | grep -qE '^--(no-)?tensorrt$'; then
        tensorrt_flag="--tensorrt"
    fi
fi

exec bash "$SCRIPT_DIR/../start.sh" "$@" "$runtime_flag" $tensorrt_flag --no-dashboard --skip-mcp-tests
