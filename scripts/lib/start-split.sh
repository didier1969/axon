#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

export AXON_SPLIT_SHADOW_ONLY=1

AXON_RUNTIME_MODE=brain_only bash "$SCRIPT_DIR/start-brain.sh" "$@"
# Operator directive 2026-05-14: indexer_full + tensorrt is the live default
# (DEC-AXO-NNN). start-indexer.sh honors AXON_REQUEST_TENSORRT=1 by default.
AXON_RUNTIME_MODE=indexer_full bash "$SCRIPT_DIR/start-indexer.sh" "$@"
