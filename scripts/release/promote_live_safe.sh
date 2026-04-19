#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$ROOT_DIR/scripts/lib/axon-instance.sh"
AXON_INSTANCE_KIND=live
axon_resolve_instance "$ROOT_DIR" "$(basename "$ROOT_DIR")"

PROJECT_CODE="AXO"
SKIP_BUILD=0
SKIP_QUALIFY=0
DRY_RUN=0

usage() {
  cat <<'EOF'
Usage: bash scripts/release/promote_live_safe.sh [--project <code>] [--skip-build] [--skip-qualify] [--dry-run]

One-shot promotion flow:
  1. Build canonical release artifact
  2. Run release preflight
  3. Create qualified release manifest
  4. Promote live with restart and MCP runtime post-check
  5. Run core MCP qualification and final live status
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --project) PROJECT_CODE="${2:-}"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --skip-qualify) SKIP_QUALIFY=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage; exit 1 ;;
  esac
done

[[ -n "$PROJECT_CODE" ]] || { echo "--project is required" >&2; exit 1; }

start_head="$(git -C "$ROOT_DIR" rev-parse HEAD)"

ensure_head_stable() {
  local current_head
  current_head="$(git -C "$ROOT_DIR" rev-parse HEAD)"
  if [[ "$current_head" != "$start_head" ]]; then
    echo "HEAD changed during promotion flow: start=$start_head current=$current_head" >&2
    return 1
  fi
}

run_step() {
  local label="$1"
  shift
  echo "== $label =="
  "$@"
}

if [[ "$DRY_RUN" -eq 1 ]]; then
  echo "DRY RUN: would promote current HEAD via safe one-shot flow"
  echo "DRY RUN: project=$PROJECT_CODE head=$start_head skip_build=$SKIP_BUILD skip_qualify=$SKIP_QUALIFY"
  exit 0
fi

if [[ "$SKIP_BUILD" -ne 1 ]]; then
  run_step "build canonical release artifact" "$ROOT_DIR/scripts/axon" setup --artifact-only
fi

ensure_head_stable
run_step "release preflight" "$ROOT_DIR/scripts/axon" release-preflight
ensure_head_stable

manifest_path="$(run_step "create qualified release manifest" "$ROOT_DIR/scripts/axon" create-release-manifest --state qualified | tail -n 1)"
[[ -n "$manifest_path" ]] || { echo "Failed to capture manifest path" >&2; exit 1; }
manifest_path="$(realpath "$manifest_path")"

ensure_head_stable
run_step "promote live and verify runtime truth" "$ROOT_DIR/scripts/axon" promote-live --manifest "$manifest_path" --restart-live

if [[ "$SKIP_QUALIFY" -ne 1 ]]; then
  ensure_head_stable
  run_step "qualify live MCP core surface" "$ROOT_DIR/scripts/axon" --instance live qualify-mcp --surface core --checks quality,latency --project "$PROJECT_CODE"
fi

ensure_head_stable
run_step "final live status" bash "$ROOT_DIR/scripts/status-live.sh"

echo "SAFE PROMOTION COMPLETE"
echo "manifest=$manifest_path"
