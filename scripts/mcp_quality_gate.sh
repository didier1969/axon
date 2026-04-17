#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RUNS_ROOT="$PROJECT_ROOT/.axon/mcp-measure-runs"

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
  if [[ "$ALLOW_MUTATIONS" -eq 1 ]]; then
    cmd+=(--allow-mutations)
  fi
  if [[ -n "$scenario_file" && -f "$scenario_file" ]]; then
    cmd+=(--scenario-file "$scenario_file")
  fi
  "${cmd[@]}"
  echo "   report: $out"
done

if [[ "$WITH_REGRESSION" -eq 1 ]]; then
  echo ""
  echo "-- MCP regression gate"
  measure_cmd=(
    python3 "$SCRIPT_DIR/measure_mcp_suite.py"
    --project AXO
    --label quality-gate
  )
  if [[ "$WARM_CACHE" -eq 1 ]]; then
    measure_cmd+=(--warm-cache)
  fi
  "${measure_cmd[@]}"

  candidate_summary="$(find "$RUNS_ROOT" -mindepth 2 -maxdepth 2 -name summary.json | sort | tail -n 1)"
  if [[ -z "$candidate_summary" ]]; then
    echo "❌ Unable to locate the latest MCP measurement summary." >&2
    exit 1
  fi

  compare_cmd=(
    python3 "$SCRIPT_DIR/compare_mcp_runs.py"
    --candidate "$candidate_summary"
  )
  if [[ -n "$BASELINE_SUMMARY" ]]; then
    compare_cmd+=(--base "$BASELINE_SUMMARY")
  fi
  "${compare_cmd[@]}"
fi

echo ""
echo "✅ MCP quality gate passed."
