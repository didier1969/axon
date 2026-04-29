#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/dev-baseline.sh
source "$SCRIPT_DIR/lib/dev-baseline.sh"

usage() {
    cat <<'EOF'
Usage: bash scripts/reset-dev-indexer-baseline.sh

Reset the dev runtime into an indexer-only stable, measurable baseline:
- stop brain/indexer
- clean IST dev artifacts and runtime role run roots
- start indexer only
- wait for an indexer-only stable measurement window:
  - indexer healthy and canonical
  - brain absent
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi

dev_baseline_require_dev_instance

echo "[reset-dev-indexer-baseline] stopping dev runtime roles"
dev_baseline_stop_split

echo "[reset-dev-indexer-baseline] cleaning dev IST and runtime role run roots"
dev_baseline_clean_state

echo "[reset-dev-indexer-baseline] starting indexer"
AXON_INSTANCE_KIND=dev bash "$SCRIPT_DIR/start-indexer.sh"

echo "[reset-dev-indexer-baseline] waiting for stable measurement window"
baseline_status="$(dev_baseline_wait_for_indexer_measurement_window 240)"

printf '%s\n' "$baseline_status"
