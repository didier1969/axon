#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Check if the active IST database exists before running tests
DB_PATH="${HOME}/.local/share/axon/db/ist.db"
if [ ! -f "$DB_PATH" ]; then
    echo "⚠️  Base de données IST introuvable ($DB_PATH)."
    echo "   Skipping MCP tests car l'indexation n'a pas encore été effectuée ou la base est vide."
    exit 0
fi

PROJECTS=("BookingSystem")
QUERY_BY_PROJECT=("booking")

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
