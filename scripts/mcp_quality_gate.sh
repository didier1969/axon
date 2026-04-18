#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ALLOW_MUTATIONS=0
WITH_REGRESSION=1
WARM_CACHE=1
BASELINE_SUMMARY=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --allow-mutations)
      ALLOW_MUTATIONS=1
      shift
      ;;
    --skip-regression)
      WITH_REGRESSION=0
      shift
      ;;
    --cold)
      WARM_CACHE=0
      shift
      ;;
    --baseline)
      BASELINE_SUMMARY="${2:-}"
      shift 2
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

if [[ "$ALLOW_MUTATIONS" -eq 1 ]]; then
  echo "quality-mcp is now a core compatibility wrapper and does not support --allow-mutations." >&2
  echo "Use ./scripts/axon qualify-mcp --surface soll --checks quality --mutations dry-run|safe-live|full instead." >&2
  exit 2
fi

echo "== MCP Quality Gate =="
echo "Compatibility wrapper around ./scripts/axon qualify-mcp"

cmd=(
  python3 "$SCRIPT_DIR/qualify_mcp.py"
  --surface core
  --checks quality,latency
  --project AXO
  --strict
)
if [[ "$WARM_CACHE" -eq 1 ]]; then
  cmd+=(--mode steady-state)
else
  cmd+=(--mode cold)
fi
if [[ "$WITH_REGRESSION" -eq 0 ]]; then
  cmd+=(--skip-regression)
fi
if [[ -n "$BASELINE_SUMMARY" ]]; then
  cmd+=(--baseline "$BASELINE_SUMMARY")
fi
"${cmd[@]}"

echo ""
echo "✅ MCP quality gate passed."
