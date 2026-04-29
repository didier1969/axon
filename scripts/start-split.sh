#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

export AXON_SPLIT_SHADOW_ONLY=1

AXON_RUNTIME_MODE=brain_only bash "$SCRIPT_DIR/start-brain.sh" "$@"
AXON_RUNTIME_MODE=indexer_graph bash "$SCRIPT_DIR/start-indexer.sh" "$@"
