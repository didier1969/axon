#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/dev-baseline.sh
source "$SCRIPT_DIR/lib/dev-baseline.sh"

dev_baseline_require_dev_instance

duration="60"
interval="5"
label="dev-indexer-cold"
extra_args=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --duration)
            duration="$2"
            shift 2
            ;;
        --duration=*)
            duration="${1#*=}"
            shift
            ;;
        --interval)
            interval="$2"
            shift 2
            ;;
        --interval=*)
            interval="${1#*=}"
            shift
            ;;
        --label)
            label="$2"
            shift 2
            ;;
        --label=*)
            label="${1#*=}"
            shift
            ;;
        --help|-h)
            cat <<'EOF'
Usage: bash scripts/qualify-dev-indexer-cold.sh [--duration N] [--interval N] [--label NAME] [extra qualify args...]

Runs an indexer-only cold dev qualification:
- reset the dev runtime into an indexer-only baseline
- attach qualification to the indexer-only runtime
- archive the run under .axon/qualification-runs
EOF
            exit 0
            ;;
        *)
            extra_args+=("$1")
            shift
            ;;
    esac
done

echo "[qualify-dev-indexer-cold] resetting dev indexer baseline"
AXON_RUNTIME_MODE=indexer_full bash "$SCRIPT_DIR/reset-dev-indexer-baseline.sh"

echo "[qualify-dev-indexer-cold] running indexer-only cold qualification"
exec python3 "$SCRIPT_DIR/qualify_ingestion_run.py" \
    --reuse-runtime \
    --mode indexer_full \
    --duration "$duration" \
    --interval "$interval" \
    --label "$label" \
    "${extra_args[@]}"
