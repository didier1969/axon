#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/dev-baseline.sh
source "$SCRIPT_DIR/lib/dev-baseline.sh"

dev_baseline_require_dev_instance

duration="60"
interval="5"
label="dev-cold"
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
Usage: bash scripts/qualify-dev-cold.sh [--duration N] [--interval N] [--label NAME] [extra qualify args...]

Runs a cold dev qualification:
- reset the dev split baseline
- attach qualification to the converged split runtime through brain shadow
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

echo "[qualify-dev-cold] resetting dev baseline"
bash "$SCRIPT_DIR/reset-dev-baseline.sh"

echo "[qualify-dev-cold] running cold qualification"
exec python3 "$SCRIPT_DIR/qualify_ingestion_run.py" \
    --reuse-runtime \
    --mode brain_shadow \
    --duration "$duration" \
    --interval "$interval" \
    --label "$label" \
    "${extra_args[@]}"
