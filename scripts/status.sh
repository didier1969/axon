#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

DASHBOARD_URL="${DASHBOARD_URL:-http://127.0.0.1:44127/}"
MCP_URL="${MCP_URL:-http://127.0.0.1:44129/mcp}"
TELEMETRY_SOCK="${TELEMETRY_SOCK:-/tmp/axon-telemetry.sock}"
MCP_SOCK="${MCP_SOCK:-/tmp/axon-mcp.sock}"

have_cmd() {
  command -v "$1" >/dev/null 2>&1
}

ok() {
  printf "OK      %s\n" "$1"
}

warn() {
  printf "WARN    %s\n" "$1"
}

fail() {
  printf "FAIL    %s\n" "$1"
}

check_process() {
  if pgrep -af axon-core >/dev/null 2>&1; then
    local pid
    pid="$(pgrep -af axon-core | head -n1 | awk '{print $1}')"
    ok "axon-core running (pid=$pid)"
    return 0
  fi
  fail "axon-core process not found"
  return 1
}

check_dashboard() {
  local body
  if body="$(curl -sS -m 3 "$DASHBOARD_URL" 2>/dev/null)"; then
    if [[ -n "$body" ]]; then
      ok "dashboard reachable ($DASHBOARD_URL)"
      return 0
    fi
  fi
  fail "dashboard unreachable ($DASHBOARD_URL)"
  return 1
}

check_mcp() {
  local payload response
  payload='{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
  if response="$(curl -sS -m 3 -H "Content-Type: application/json" -d "$payload" "$MCP_URL" 2>/dev/null)"; then
    if [[ "$response" == *'"tools"'* ]]; then
      ok "mcp reachable ($MCP_URL)"
      return 0
    fi
  fi
  fail "mcp unreachable or invalid response ($MCP_URL)"
  return 1
}

check_socket() {
  local path="$1"
  local label="$2"
  if [[ -S "$path" ]]; then
    ok "$label socket present ($path)"
    return 0
  fi
  warn "$label socket missing ($path)"
  return 0
}

main() {
  cd "$ROOT_DIR"
  printf "Axon status\n"
  printf '%s\n' "------------"

  if ! have_cmd curl; then
    fail "curl not found in PATH"
    exit 2
  fi

  local failed=0
  check_process || failed=1
  check_dashboard || failed=1
  check_mcp || failed=1
  check_socket "$TELEMETRY_SOCK" "telemetry" || true
  check_socket "$MCP_SOCK" "mcp" || true

  if [[ "$failed" -ne 0 ]]; then
    printf "STATUS  DEGRADED\n"
    exit 1
  fi

  printf "STATUS  HEALTHY\n"
}

main "$@"
