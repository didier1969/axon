#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/dev-baseline.sh
source "$SCRIPT_DIR/lib/dev-baseline.sh"

usage() {
    cat <<'EOF'
Usage: bash scripts/reset-dev-baseline.sh

Reset dev brain+indexer into a stable, measurable baseline:
- stop brain/indexer
- clean IST dev artifacts and runtime role run roots
- restart brain/indexer
- wait for a stable measurement window:
  - brain healthy and attached to indexer feed
  - indexer healthy and canonical
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi

dev_baseline_require_dev_instance

echo "[reset-dev-baseline] stopping dev runtime roles"
dev_baseline_stop_split

echo "[reset-dev-baseline] cleaning dev IST and runtime role run roots"
dev_baseline_clean_state

echo "[reset-dev-baseline] starting brain"
AXON_INSTANCE_KIND=dev bash "$SCRIPT_DIR/lib/start-brain.sh"

echo "[reset-dev-baseline] starting indexer"
AXON_INSTANCE_KIND=dev bash "$SCRIPT_DIR/lib/start-indexer.sh"

echo "[reset-dev-baseline] waiting for stable measurement window"
baseline_status="$(dev_baseline_wait_for_stable_measurement_window 240)"

printf '%s\n' "$baseline_status"
