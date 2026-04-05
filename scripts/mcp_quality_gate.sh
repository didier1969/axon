#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

PROJECTS=("BookingSystem" "axon")
QUERY_BY_PROJECT=("booking" "axon")

echo "== MCP Quality Gate =="
echo "Non-intrusive validation (write-capable tools skipped)."

for i in "${!PROJECTS[@]}"; do
  project="${PROJECTS[$i]}"
  query="${QUERY_BY_PROJECT[$i]}"
  out="/tmp/mcp_quality_gate_${project}.json"
  scenario_file=""
  case "$project" in
    BookingSystem)
      scenario_file="$SCRIPT_DIR/mcp_scenarios/booking_system.json"
      ;;
  esac
  echo ""
  echo "-- project=${project} query=${query}"
  cmd=(
    python3 "$SCRIPT_DIR/mcp_validate.py"
    --project "$project"
    --query "$query"
    --strict
    --timeout 60
    --json-out "$out"
  )
  if [[ "$*" == *"--allow-mutations"* ]]; then
    cmd+=(--allow-mutations)
  fi
  if [[ -n "$scenario_file" && -f "$scenario_file" ]]; then
    cmd+=(--scenario-file "$scenario_file")
  fi
  "${cmd[@]}"
  echo "   report: $out"
done

echo ""
echo "✅ MCP quality gate passed."
