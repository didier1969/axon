#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$PROJECT_ROOT/scripts/lib/axon-instance.sh"
# shellcheck source=scripts/lib/axon-role-layout.sh
source "$PROJECT_ROOT/scripts/lib/axon-role-layout.sh"

cd "$PROJECT_ROOT"
axon_resolve_instance "$PROJECT_ROOT" "$(basename "$PROJECT_ROOT")"

TARGET_MODE=""
START_DASHBOARD=1

usage() {
  cat <<'EOF'
Usage: ./scripts/upgrade-topology.sh [--brain-only|--indexer-graph|--indexer-vector|--indexer-full] [--no-dashboard]

Behavior:
  Legacy alias: starts only the missing runtime role needed for the requested mode.
  It does not stop a running brain or indexer.

Examples:
  ./scripts/upgrade-topology.sh --indexer-full
  ./scripts/upgrade-topology.sh --brain-only
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --brain-only|--brainonly)
      TARGET_MODE="brain_only"
      ;;
    --indexer-graph|--indexergraph)
      TARGET_MODE="indexer_graph"
      ;;
    --indexer-vector|--indexervector)
      TARGET_MODE="indexer_vector"
      ;;
    --indexer-full|--indexerfull)
      TARGET_MODE="indexer_full"
      ;;
    --no-dashboard)
      START_DASHBOARD=0
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "❌ Unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
  shift
done

if [[ -z "$TARGET_MODE" ]]; then
  echo "❌ Missing target runtime mode." >&2
  usage >&2
  exit 1
fi

role_pid_file() {
  local role="$1"
  (
    export AXON_RUNTIME_IDENTITY="upgrade-topology-probe"
    axon_apply_runtime_role_layout "$PROJECT_ROOT" "$role" "$(axon_runtime_binary_name "$role")"
    printf '%s\n' "$AXON_PID_FILE"
  )
}

role_state_file() {
  local role="$1"
  (
    export AXON_RUNTIME_IDENTITY="upgrade-topology-probe"
    axon_apply_runtime_role_layout "$PROJECT_ROOT" "$role" "$(axon_runtime_binary_name "$role")"
    printf '%s\n' "$AXON_RUNTIME_STATE_FILE"
  )
}

role_running() {
  local role="$1"
  local pid_file
  pid_file="$(role_pid_file "$role")"
  [[ -f "$pid_file" ]] || return 1
  local pid
  pid="$(cat "$pid_file" 2>/dev/null || true)"
  [[ -n "$pid" && -e "/proc/$pid" ]]
}

role_mode() {
  local role="$1"
  local state_file
  state_file="$(role_state_file "$role")"
  [[ -f "$state_file" ]] || return 1
  # shellcheck disable=SC1090
  source "$state_file"
  printf '%s\n' "${AXON_RUNTIME_MODE:-}"
}

start_brain() {
  local brain_mode="${1:-brain_only}"
  local args=()
  if [[ "$START_DASHBOARD" != "1" ]]; then
    args+=(--no-dashboard)
  fi
  AXON_RUNTIME_MODE="$brain_mode" bash "$PROJECT_ROOT/scripts/start-brain.sh" "${args[@]}"
}

start_indexer() {
  local indexer_mode="$1"
  AXON_RUNTIME_MODE="$indexer_mode" bash "$PROJECT_ROOT/scripts/start-indexer.sh"
}

ensure_brain() {
  local desired_mode="${1:-brain_only}"
  if role_running brain; then
    local current_mode=""
    current_mode="$(role_mode brain || true)"
    if [[ -n "$current_mode" && "$current_mode" != "$desired_mode" ]]; then
      echo "❌ Brain already running in mode '$current_mode'; non-disruptive upgrade refuses to restart it." >&2
      exit 1
    fi
    echo "ℹ️  Brain already running; leaving it untouched."
    return 0
  fi
  start_brain "$desired_mode"
}

ensure_indexer() {
  local desired_mode="$1"
  if role_running indexer; then
    local current_mode=""
    current_mode="$(role_mode indexer || true)"
    if [[ -n "$current_mode" && "$current_mode" != "$desired_mode" ]]; then
      echo "❌ Indexer already running in mode '$current_mode'; non-disruptive upgrade refuses to restart it." >&2
      exit 1
    fi
    echo "ℹ️  Indexer already running; leaving it untouched."
    return 0
  fi
  start_indexer "$desired_mode"
}

case "$TARGET_MODE" in
  brain_only)
    ensure_brain brain_only
    ;;
  indexer_graph|indexer_vector|indexer_full)
    ensure_indexer "$TARGET_MODE"
    ;;
  split)
    ensure_brain brain_only
    ensure_indexer indexer_full
    ;;
esac
