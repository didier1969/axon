#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$ROOT_DIR/scripts/lib/axon-instance.sh"
# shellcheck source=scripts/lib/axon-resource-policy.sh
source "$ROOT_DIR/scripts/lib/axon-resource-policy.sh"
source "$ROOT_DIR/scripts/lib/axon-version.sh"
axon_load_worktree_env "$ROOT_DIR"
axon_resolve_instance "$ROOT_DIR" "$(basename "$ROOT_DIR")"
axon_resolve_resource_policy "$AXON_INSTANCE_KIND"
axon_resolve_version "$ROOT_DIR"
if [[ -f "$AXON_RUNTIME_STATE_FILE" ]]; then
  # shellcheck disable=SC1090
  source "$AXON_RUNTIME_STATE_FILE"
fi

DASHBOARD_URL="${DASHBOARD_URL:-$AXON_DASHBOARD_URL}"
MCP_URL="${MCP_URL:-$AXON_MCP_URL}"
TELEMETRY_SOCK="${TELEMETRY_SOCK:-$AXON_TELEMETRY_SOCK}"
STATUS_PROBE_WARNINGS=0

have_cmd() {
  command -v "$1" >/dev/null 2>&1
}

port_listening() {
  local port="$1"
  ss -ltn 2>/dev/null | awk -v p="$port" '
    $1 == "LISTEN" {
      split($4, addr_parts, ":")
      if (addr_parts[length(addr_parts)] == p) {
        found = 1
        exit
      }
    }
    END {
      exit(found ? 0 : 1)
    }'
}

port_pid() {
  ss -ltnp 2>/dev/null | awk -v p="$HYDRA_HTTP_PORT" '
    $1 == "LISTEN" {
      split($4, addr_parts, ":")
      if (addr_parts[length(addr_parts)] != p) {
        next
      }
      match($0, /pid=([0-9]+)/, m)
      if (m[1] != "") {
        print m[1]
        exit
      }
    }'
}

pid_matches_instance() {
  local pid="$1"
  local cmdline=""
  local listener_pid=""

  [[ -n "$pid" && -e "/proc/$pid" ]] || return 1
  cmdline="$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null || true)"
  [[ "$cmdline" == *"axon-core"* ]] || return 1

  listener_pid="$(port_pid)"
  [[ -n "$listener_pid" && "$listener_pid" == "$pid" ]]
}

ok() {
  printf "OK      %s\n" "$1"
}

warn() {
  printf "WARN    %s\n" "$1"
}

probe_warn() {
  STATUS_PROBE_WARNINGS=1
  warn "$1"
}

fail() {
  printf "FAIL    %s\n" "$1"
}

check_process() {
  if [[ -f "$AXON_PID_FILE" ]]; then
    local pid
    pid="$(cat "$AXON_PID_FILE" 2>/dev/null || true)"
    if pid_matches_instance "$pid"; then
      ok "axon-core running (pid=$pid, instance=$AXON_INSTANCE_KIND)"
      return 0
    fi
  fi
  local pid
  pid="$(port_pid)"
  if [[ -n "$pid" ]]; then
    ok "axon-core running via instance port (pid=$pid, instance=$AXON_INSTANCE_KIND)"
    return 0
  fi
  if port_listening "$HYDRA_HTTP_PORT"; then
    probe_warn "axon-core listener present on port $HYDRA_HTTP_PORT but pid is unavailable from this shell"
    return 0
  fi
  fail "axon-core process not found for instance=$AXON_INSTANCE_KIND"
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
  if port_listening "$PHX_PORT"; then
    probe_warn "dashboard listener present on port $PHX_PORT but HTTP probe failed from this shell ($DASHBOARD_URL)"
    return 0
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
  if port_listening "$HYDRA_HTTP_PORT"; then
    probe_warn "mcp listener present on port $HYDRA_HTTP_PORT but HTTP probe failed from this shell ($MCP_URL)"
    return 0
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
  printf "INSTANCE %s\n" "$AXON_INSTANCE_KIND"
  printf "SESSION  %s\n" "$TMUX_SESSION"
  printf "DB ROOT  %s\n" "$AXON_DB_ROOT"
  printf "RUN ROOT %s\n" "$AXON_RUN_ROOT"
  printf "POLICY   priority=%s budget=%s gpu=%s watcher=%s\n" \
    "$AXON_RESOURCE_PRIORITY" "$AXON_BACKGROUND_BUDGET_CLASS" "$AXON_GPU_ACCESS_POLICY" "$AXON_WATCHER_POLICY"
  printf "EMBED    %s\n" "${AXON_EMBEDDING_PROVIDER:-auto}"
  printf "WORKERS  %s\n" "${MAX_AXON_WORKERS:-auto}"
  printf "QUEUE    %s\n" "${AXON_QUEUE_MEMORY_BUDGET_BYTES:-auto}"
  printf "WATCHER  %s\n" "${AXON_WATCHER_SUBTREE_HINT_BUDGET:-auto}"
  printf "VERSION  %s\n" "${AXON_RELEASE_VERSION:-unknown}"
  printf "BUILD    %s\n" "${AXON_BUILD_ID:-unknown}"
  printf "GEN      %s\n" "${AXON_INSTALL_GENERATION:-unknown}"

  if ! have_cmd curl; then
    fail "curl not found in PATH"
    exit 2
  fi

  local failed=0
  check_process || failed=1
  if [[ "${AXON_DASHBOARD_ENABLED:-1}" == "1" ]]; then
    check_dashboard || failed=1
  else
    ok "dashboard intentionally disabled for this instance"
  fi
  check_mcp || failed=1
  check_socket "$TELEMETRY_SOCK" "telemetry" || true

  if [[ "$failed" -ne 0 ]]; then
    printf "STATUS  DEGRADED\n"
    exit 1
  fi

  if [[ "$STATUS_PROBE_WARNINGS" -ne 0 ]]; then
    printf "STATUS  WARN\n"
    exit 0
  fi

  printf "STATUS  HEALTHY\n"
}

main "$@"
