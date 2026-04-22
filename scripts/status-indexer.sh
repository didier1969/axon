#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

export AXON_RUNTIME_SHADOW_ROLE="indexer"
export AXON_SPLIT_SHADOW_ONLY="${AXON_SPLIT_SHADOW_ONLY:-0}"
export AXON_DASHBOARD_ENABLED="0"

exec bash "$SCRIPT_DIR/status.sh" "$@"
