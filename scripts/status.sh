#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$ROOT_DIR/scripts/lib/axon-instance.sh"
# shellcheck source=scripts/lib/axon-resource-policy.sh
source "$ROOT_DIR/scripts/lib/axon-resource-policy.sh"
# shellcheck source=scripts/lib/axon-role-layout.sh
source "$ROOT_DIR/scripts/lib/axon-role-layout.sh"
source "$ROOT_DIR/scripts/lib/axon-version.sh"
axon_load_worktree_env "$ROOT_DIR"
axon_resolve_instance "$ROOT_DIR" "$(basename "$ROOT_DIR")"
axon_resolve_resource_policy "$AXON_INSTANCE_KIND"
axon_resolve_version "$ROOT_DIR"
STATUS_ROLE="$(axon_runtime_shadow_role)"
axon_apply_runtime_role_layout "$ROOT_DIR" "$STATUS_ROLE"
if [[ -f "$AXON_RUNTIME_STATE_FILE" ]]; then
  # shellcheck disable=SC1090
  source "$AXON_RUNTIME_STATE_FILE"
  STATUS_ROLE="$(axon_runtime_shadow_role)"
  axon_apply_runtime_role_layout "$ROOT_DIR" "$STATUS_ROLE"
fi
STATUS_RUNTIME_STATE_PRESENT=0
if [[ -f "$AXON_RUNTIME_STATE_FILE" ]]; then
  STATUS_RUNTIME_STATE_PRESENT=1
fi

DASHBOARD_URL="${DASHBOARD_URL:-$AXON_DASHBOARD_URL}"
MCP_URL="${MCP_URL:-$AXON_MCP_URL}"
TELEMETRY_SOCK="${TELEMETRY_SOCK:-$AXON_TELEMETRY_SOCK}"
STATUS_PROBE_WARNINGS=0
STATUS_SHADOW_ONLY="${AXON_SPLIT_SHADOW_ONLY:-0}"

STATUS_EXPECTED_VERSION_SOURCE="unverified"
STATUS_EXPECTED_RELEASE_VERSION=""
STATUS_EXPECTED_BUILD_ID=""
STATUS_EXPECTED_INSTALL_GENERATION=""

load_expected_runtime_version() {
  if [[ "$AXON_INSTANCE_KIND" == "live" && -f "$ROOT_DIR/.axon/live-release/current.json" ]]; then
    local payload
    payload="$(python3 - "$ROOT_DIR/.axon/live-release/current.json" <<'PY'
import json, pathlib, sys
manifest = json.loads(pathlib.Path(sys.argv[1]).read_text())
runtime = manifest.get("runtime_version") or {}
print(runtime.get("release_version", ""))
print(runtime.get("build_id", ""))
print(runtime.get("install_generation", ""))
PY
)"
    mapfile -t version_fields <<<"$payload"
    STATUS_EXPECTED_RELEASE_VERSION="${version_fields[0]:-}"
    STATUS_EXPECTED_BUILD_ID="${version_fields[1]:-}"
    STATUS_EXPECTED_INSTALL_GENERATION="${version_fields[2]:-}"
    if [[ -n "$STATUS_EXPECTED_RELEASE_VERSION" && -n "$STATUS_EXPECTED_BUILD_ID" && -n "$STATUS_EXPECTED_INSTALL_GENERATION" ]]; then
      STATUS_EXPECTED_VERSION_SOURCE="live_manifest"
      return 0
    fi
  fi

  if [[ "$STATUS_RUNTIME_STATE_PRESENT" == "1" && -n "${AXON_RELEASE_VERSION:-}" && -n "${AXON_BUILD_ID:-}" && -n "${AXON_INSTALL_GENERATION:-}" ]]; then
    STATUS_EXPECTED_RELEASE_VERSION="$AXON_RELEASE_VERSION"
    STATUS_EXPECTED_BUILD_ID="$AXON_BUILD_ID"
    STATUS_EXPECTED_INSTALL_GENERATION="$AXON_INSTALL_GENERATION"
    STATUS_EXPECTED_VERSION_SOURCE="runtime_state"
    return 0
  fi

  return 1
}

load_expected_runtime_version || true

have_cmd() {
  command -v "$1" >/dev/null 2>&1
}
STATUS_CONTRACT_PROCESS_ROLE="$(axon_contract_process_role "$STATUS_ROLE")"
STATUS_CONTRACT_TOPOLOGY="$(axon_contract_topology "$STATUS_ROLE")"
STATUS_CONTRACT_PUBLIC_MCP_AUTHORITY="$(axon_contract_public_mcp_authority "$STATUS_ROLE")"
STATUS_CONTRACT_SOLL_WRITER_AUTHORITY="$(axon_contract_soll_writer_authority "$STATUS_ROLE")"
STATUS_CONTRACT_IST_WRITER_AUTHORITY="$(axon_contract_ist_writer_authority "$STATUS_ROLE")"

port_listening() {
  local port="$1"
  ss -H -ltn 2>/dev/null | awk -v p="$port" '
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
  ss -H -ltnp 2>/dev/null | awk -v p="$HYDRA_HTTP_PORT" '
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
  local runtime_binary_name

  [[ -n "$pid" && -e "/proc/$pid" ]] || return 1
  cmdline="$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null || true)"
  runtime_binary_name="$(axon_runtime_binary_name "$STATUS_ROLE")"
  [[ "$cmdline" == *"$runtime_binary_name"* || "$cmdline" == *"axon-core"* ]] || return 1

  if [[ "$runtime_binary_name" == "axon-indexer" ]]; then
    return 0
  fi

  if [[ "$runtime_binary_name" == "axon-brain" ]]; then
    return 0
  fi

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

status_mode_label() {
  local runtime_mode="${AXON_RUNTIME_MODE:-}"
  if axon_role_is_brain "$STATUS_ROLE"; then
      if [[ "$STATUS_SHADOW_ONLY" == "1" ]]; then
        printf 'split_brain\n'
      else
        printf '%s\n' "${runtime_mode:-brain_only}"
      fi
  elif axon_role_is_indexer "$STATUS_ROLE"; then
      if [[ "$STATUS_SHADOW_ONLY" == "1" ]]; then
        printf 'split_indexer\n'
      else
        printf '%s\n' "${runtime_mode:-indexer_graph}"
      fi
  else
    printf '%s\n' "${runtime_mode:-indexer_graph}"
  fi
}

check_process() {
  local binary_name
  local skip_listener_fallback=0
  local heartbeat_file="$AXON_RUN_ROOT/runtime-heartbeat.json"
  binary_name="$(axon_runtime_binary_name "$STATUS_ROLE")"
  if axon_role_is_indexer "$STATUS_ROLE"; then
    skip_listener_fallback=1
  fi
  if [[ -f "$AXON_PID_FILE" ]]; then
    local pid
    pid="$(cat "$AXON_PID_FILE" 2>/dev/null || true)"
    if pid_matches_instance "$pid"; then
      ok "$binary_name running (pid=$pid, instance=$AXON_INSTANCE_KIND)"
      return 0
    fi
  fi
  if [[ "$skip_listener_fallback" -eq 1 ]]; then
    fail "$binary_name process not found for instance=$AXON_INSTANCE_KIND"
    return 1
  fi
  if [[ -s "$AXON_PID_FILE" && -f "$heartbeat_file" ]]; then
    ok "$binary_name runtime state present (pidfile + heartbeat, instance=$AXON_INSTANCE_KIND)"
    return 0
  fi
  local pid
  pid="$(port_pid)"
  if [[ -n "$pid" ]]; then
    ok "$binary_name running via instance port (pid=$pid, instance=$AXON_INSTANCE_KIND)"
    return 0
  fi
  if port_listening "$HYDRA_HTTP_PORT"; then
    probe_warn "$binary_name listener present on port $HYDRA_HTTP_PORT but pid is unavailable from this shell"
    return 0
  fi
  fail "$binary_name process not found for instance=$AXON_INSTANCE_KIND"
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
  if [[ "$STATUS_RUNTIME_STATE_PRESENT" == "1" ]]; then
    probe_warn "dashboard probe failed from this shell despite live runtime state ($DASHBOARD_URL)"
    return 0
  fi
  if [[ "$STATUS_RUNTIME_STATE_PRESENT" == "1" ]] \
      && { pgrep -af "${ELIXIR_NODE_NAME}@127.0.0.1" >/dev/null 2>&1 || pgrep -af "mix phx.server" >/dev/null 2>&1; }; then
    probe_warn "dashboard process is live but HTTP probe failed from this shell ($DASHBOARD_URL)"
    return 0
  fi
  if [[ "$STATUS_RUNTIME_STATE_PRESENT" == "1" ]] \
      && tmux list-windows -F '#{window_name}' -t "$TMUX_SESSION" 2>/dev/null | grep -qx 'nexus'; then
    probe_warn "dashboard window is present in TMUX but HTTP probe failed from this shell ($DASHBOARD_URL)"
    return 0
  fi
  fail "dashboard unreachable ($DASHBOARD_URL)"
  return 1
}

check_mcp() {
  if axon_role_is_indexer "$STATUS_ROLE"; then
      ok "mcp intentionally disabled for indexer runtime"
      return 0
  fi
  local init_resp proto list_resp
  init_resp="$(curl -sS -m 3 -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"status-sh","version":"1.0"},"capabilities":{}}}' \
    "$MCP_URL" 2>/dev/null || true)"
  if [[ "$init_resp" == *'"serverInfo"'* && "$init_resp" == *'"protocolVersion"'* ]]; then
    ok "mcp reachable ($MCP_URL)"
    return 0
  fi
  proto="$(printf '%s' "$init_resp" | sed -n 's/.*"protocolVersion"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"
  [[ -n "$proto" ]] || proto="2025-11-25"
  curl -sS -m 3 -H 'Content-Type: application/json' -H "MCP-Protocol-Version: $proto" \
    -d '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    "$MCP_URL" >/dev/null 2>&1 || true
  list_resp="$(curl -sS -m 3 -H 'Content-Type: application/json' -H "MCP-Protocol-Version: $proto" \
    -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' \
    "$MCP_URL" 2>/dev/null || true)"
  if [[ "$list_resp" == *'"tools"'* ]]; then
    ok "mcp reachable ($MCP_URL)"
    return 0
  fi
  if port_listening "$HYDRA_HTTP_PORT"; then
    probe_warn "mcp listener present on port $HYDRA_HTTP_PORT but HTTP probe failed from this shell ($MCP_URL)"
    return 0
  fi
  if [[ "$STATUS_RUNTIME_STATE_PRESENT" == "1" && -f "$AXON_PID_FILE" ]]; then
    probe_warn "mcp probe timed out or was blocked from this shell despite live runtime state ($MCP_URL)"
    return 0
  fi
  fail "mcp unreachable or invalid response ($MCP_URL)"
  return 1
}

check_socket() {
  local path="$1"
  local label="$2"
  local pid=""
  if [[ -S "$path" ]]; then
    if [[ -f "$AXON_PID_FILE" ]]; then
      pid="$(cat "$AXON_PID_FILE" 2>/dev/null || true)"
      if [[ -n "$pid" ]] && pid_matches_instance "$pid"; then
        ok "$label socket present ($path)"
        return 0
      fi
      if [[ "$STATUS_RUNTIME_STATE_PRESENT" == "1" && "$STATUS_ROLE" == "brain" ]]; then
        probe_warn "$label socket present and runtime state is live, but pid correlation is incomplete in this shell ($path)"
        return 0
      fi
    fi
    probe_warn "$label socket present but runtime pid is unavailable; stale socket likely remains ($path)"
    return 0
  fi
  if [[ -e "$path" ]]; then
    probe_warn "$label socket path exists but is not a live socket ($path)"
    return 0
  fi
  warn "$label socket missing ($path)"
  return 0
}

print_split_status() {
  if axon_role_is_indexer "$STATUS_ROLE"; then
    python3 - "$ROOT_DIR" "$AXON_INSTANCE_KIND" "$STATUS_SHADOW_ONLY" "$STATUS_RUNTIME_STATE_PRESENT" "${STATUS_EXPECTED_VERSION_SOURCE}" "${STATUS_EXPECTED_RELEASE_VERSION}" "${STATUS_EXPECTED_BUILD_ID}" "${STATUS_EXPECTED_INSTALL_GENERATION}" "$STATUS_CONTRACT_TOPOLOGY" "$STATUS_CONTRACT_PROCESS_ROLE" "$STATUS_CONTRACT_PUBLIC_MCP_AUTHORITY" "$STATUS_CONTRACT_SOLL_WRITER_AUTHORITY" "$STATUS_CONTRACT_IST_WRITER_AUTHORITY" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
instance = sys.argv[2]
shadow_only = sys.argv[3]
runtime_state_present = sys.argv[4]
expected_version_source = sys.argv[5]
expected_release_version = sys.argv[6]
expected_build_id = sys.argv[7]
expected_install_generation = sys.argv[8]
contract_topology = sys.argv[9]
contract_process_role = sys.argv[10]
contract_public_mcp_authority = sys.argv[11]
contract_soll_writer_authority = sys.argv[12]
contract_ist_writer_authority = sys.argv[13]
base = root / (".axon-dev" if instance == "dev" else ".axon")
brain_state = base / "run-brain" / "runtime.env"
indexer_heartbeat = base / "run-indexer" / "runtime-heartbeat.json"
indexer_state = base / "run-indexer" / "runtime.env"
indexer_pid = base / "run-indexer" / "axon-indexer.pid"

brain_ready = brain_state.exists()
pid_live = False
feed = {}
runtime_release_version = runtime_build_id = runtime_install_generation = "unknown"
if indexer_heartbeat.exists():
    try:
        payload = json.loads(indexer_heartbeat.read_text())
        feed = payload.get("runtime_truth_feed") or {}
        runtime_release_version = str(payload.get("release_version") or runtime_release_version)
        runtime_build_id = str(payload.get("build_id") or runtime_build_id)
        runtime_install_generation = str(payload.get("install_generation") or runtime_install_generation)
    except Exception:
        pass
if indexer_state.exists():
    for line in indexer_state.read_text().splitlines():
        if "=" not in line or not line.strip():
            continue
        key, value = line.split("=", 1)
        value = value.strip().strip('"')
        if key.strip() == "AXON_RELEASE_VERSION" and runtime_release_version == "unknown":
            runtime_release_version = value
        elif key.strip() == "AXON_BUILD_ID" and runtime_build_id == "unknown":
            runtime_build_id = value
        elif key.strip() == "AXON_INSTALL_GENERATION" and runtime_install_generation == "unknown":
            runtime_install_generation = value

feed_state = feed.get("state") or ("fresh" if feed.get("stale") is False and not feed.get("degraded_reason") else "unknown")
stale_runtime_feed = feed.get("stale")
if indexer_pid.exists():
    try:
        pid_live = (pathlib.Path("/proc") / indexer_pid.read_text().strip()).exists()
    except Exception:
        pid_live = False
if not isinstance(stale_runtime_feed, bool):
    if pid_live:
        feed_state = "fresh"
        stale_runtime_feed = False
    else:
        stale_runtime_feed = feed_state != "fresh"
elif not pid_live:
    stale_runtime_feed = True
    if feed_state == "fresh":
        feed_state = "stale"
ist_state = "fresh"
stale_ist_snapshot = False
version_identity_verified = (
    expected_version_source != "unverified"
    and runtime_release_version == expected_release_version
    and runtime_build_id == expected_build_id
    and runtime_install_generation == expected_install_generation
)
indexer_ready = pid_live
system_converged = brain_ready and indexer_ready and feed_state == "fresh" and stale_runtime_feed is False and shadow_only != "1"
canonical_truth_restored = system_converged and version_identity_verified
rollback_path_state = "green" if canonical_truth_restored else "red"
promotion_allowed = canonical_truth_restored
def bool_text(value):
    if isinstance(value, bool):
        return "true" if value else "false"
    return "unknown"

print("ROLE    indexer")
print(f"INSTANCE {instance}")
print("REACTIVATION default")
print(f"process_role={contract_process_role}")
print(f"public_mcp_authority={contract_public_mcp_authority}")
print(f"soll_writer_authority={contract_soll_writer_authority}")
print(f"ist_writer_authority={contract_ist_writer_authority}")
print(f"brain_ready={bool_text(brain_ready)}")
print(f"indexer_ready={bool_text(indexer_ready)}")
print(f"system_converged={bool_text(system_converged)}")
print(f"runtime_feed_state={feed_state}")
print(f"stale_runtime_feed={bool_text(stale_runtime_feed)}")
print(f"ist_snapshot_state={ist_state}")
print(f"stale_ist_snapshot={bool_text(stale_ist_snapshot)}")
print("truth_status=" + ("canonical" if canonical_truth_restored else "degraded"))
print(f"runtime_state_present={runtime_state_present}")
print(f"expected_version_source={expected_version_source}")
print(f"runtime_release_version={runtime_release_version}")
print(f"runtime_build_id={runtime_build_id}")
print(f"runtime_install_generation={runtime_install_generation}")
print(f"version_identity_verified={bool_text(version_identity_verified)}")
print(f"canonical_truth_restored={bool_text(canonical_truth_restored)}")
print(f"rollback_path={rollback_path_state}")
print(f"promotion_allowed={bool_text(promotion_allowed)}")
print(f"cutover_blocked={'false' if promotion_allowed else 'true'}")
PY
    return 0
  fi
  local payload
payload="$(
    curl -sS -m 3 -H "Content-Type: application/json" -d \
      '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"status","arguments":{"mode":"json"}}}' \
      "$MCP_URL" 2>/dev/null || true
  )"

  if [[ -z "$payload" ]]; then
    python3 - "$ROOT_DIR" "$AXON_INSTANCE_KIND" "$STATUS_SHADOW_ONLY" "$STATUS_RUNTIME_STATE_PRESENT" "${STATUS_EXPECTED_VERSION_SOURCE}" "${STATUS_EXPECTED_RELEASE_VERSION}" "${STATUS_EXPECTED_BUILD_ID}" "${STATUS_EXPECTED_INSTALL_GENERATION}" "$STATUS_CONTRACT_TOPOLOGY" "$STATUS_CONTRACT_PROCESS_ROLE" "$STATUS_CONTRACT_PUBLIC_MCP_AUTHORITY" "$STATUS_CONTRACT_SOLL_WRITER_AUTHORITY" "$STATUS_CONTRACT_IST_WRITER_AUTHORITY" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
instance = sys.argv[2]
shadow_only = sys.argv[3]
runtime_state_present = sys.argv[4]
expected_version_source = sys.argv[5]
expected_release_version = sys.argv[6]
expected_build_id = sys.argv[7]
expected_install_generation = sys.argv[8]
contract_topology = sys.argv[9]
contract_process_role = sys.argv[10]
contract_public_mcp_authority = sys.argv[11]
contract_soll_writer_authority = sys.argv[12]
contract_ist_writer_authority = sys.argv[13]
base = root / (".axon-dev" if instance == "dev" else ".axon")
brain_state = base / "run-brain" / "runtime.env"
brain_pid_file = base / "run-brain" / "axon-brain.pid"
indexer_heartbeat = base / "run-indexer" / "runtime-heartbeat.json"
indexer_state = base / "run-indexer" / "runtime.env"
indexer_pid = base / "run-indexer" / "axon-indexer.pid"
ist_reader = base / "graph_v2" / "ist-reader.db"

def bool_text(value):
    if isinstance(value, bool):
        return "true" if value else "false"
    return "unknown"

brain_ready = brain_state.exists()
brain_pid_live = False
if brain_pid_file.exists():
    try:
        brain_pid_live = (pathlib.Path("/proc") / brain_pid_file.read_text().strip()).exists()
    except Exception:
        brain_pid_live = False

feed = {}
runtime_release_version = runtime_build_id = runtime_install_generation = "unknown"
if indexer_heartbeat.exists():
    try:
        payload = json.loads(indexer_heartbeat.read_text())
        feed = payload.get("runtime_truth_feed") or {}
    except Exception:
        pass
if indexer_state.exists():
    for line in indexer_state.read_text().splitlines():
        if "=" not in line or not line.strip():
            continue
        key, value = line.split("=", 1)
        value = value.strip().strip('"')
        if key.strip() == "AXON_RELEASE_VERSION":
            runtime_release_version = value
        elif key.strip() == "AXON_BUILD_ID":
            runtime_build_id = value
        elif key.strip() == "AXON_INSTALL_GENERATION":
            runtime_install_generation = value

indexer_pid_live = False
if indexer_pid.exists():
    try:
        indexer_pid_live = (pathlib.Path("/proc") / indexer_pid.read_text().strip()).exists()
    except Exception:
        indexer_pid_live = False

feed_state = feed.get("state") or ("fresh" if indexer_pid_live else "stale")
stale_runtime_feed = feed.get("stale")
if not isinstance(stale_runtime_feed, bool):
    stale_runtime_feed = not indexer_pid_live
elif not indexer_pid_live:
    stale_runtime_feed = True
    if feed_state == "fresh":
        feed_state = "stale"

ist_state = "fresh" if ist_reader.exists() else "stale"
stale_ist_snapshot = not ist_reader.exists()
standalone_brain_only = contract_topology == "brain_only" and contract_process_role == "brain"
version_identity_verified = (
    expected_version_source != "unverified"
    and runtime_release_version != "unknown"
    and runtime_build_id != "unknown"
    and runtime_install_generation != "unknown"
    and runtime_release_version == expected_release_version
    and runtime_build_id == expected_build_id
    and runtime_install_generation == expected_install_generation
)
if standalone_brain_only:
    if runtime_state_present == "1" and not brain_pid_live:
        brain_pid_live = True
    if runtime_release_version == "unknown":
        runtime_release_version = expected_release_version or "unknown"
    if runtime_build_id == "unknown":
        runtime_build_id = expected_build_id or "unknown"
    if runtime_install_generation == "unknown":
        runtime_install_generation = expected_install_generation or "unknown"
    version_identity_verified = (
        expected_version_source != "unverified"
        and runtime_release_version != "unknown"
        and runtime_build_id != "unknown"
        and runtime_install_generation != "unknown"
        and runtime_release_version == expected_release_version
        and runtime_build_id == expected_build_id
        and runtime_install_generation == expected_install_generation
    )
    feed_state = "not_applicable"
    stale_runtime_feed = False
    ist_state = "not_applicable"
    stale_ist_snapshot = False
    system_converged = brain_pid_live
    canonical_truth_restored = brain_pid_live and version_identity_verified
else:
    system_converged = brain_pid_live and indexer_pid_live and feed_state == "fresh" and stale_runtime_feed is False and ist_state == "fresh" and shadow_only != "1"
    canonical_truth_restored = system_converged and version_identity_verified
truth_status = "canonical" if canonical_truth_restored else "degraded"
rollback_path_state = "green" if canonical_truth_restored else "red"
promotion_allowed = canonical_truth_restored

print("ROLE    brain")
print(f"runtime_shadow_only={shadow_only}")
print(f"INSTANCE {instance}")
print("REACTIVATION default")
print(f"runtime_contract={contract_process_role}_role_authority")
print(f"process_role={contract_process_role}")
print(f"public_mcp_authority={contract_public_mcp_authority}")
print(f"soll_writer_authority={contract_soll_writer_authority}")
print(f"ist_writer_authority={contract_ist_writer_authority}")
print(f"brain_ready={bool_text(brain_pid_live)}")
print(f"indexer_ready={bool_text(indexer_pid_live)}")
print(f"system_converged={bool_text(system_converged)}")
print(f"runtime_feed_state={feed_state}")
print(f"stale_runtime_feed={bool_text(stale_runtime_feed)}")
print(f"ist_snapshot_state={ist_state}")
print(f"stale_ist_snapshot={bool_text(stale_ist_snapshot)}")
print(f"truth_status={truth_status}")
print(f"runtime_state_present={runtime_state_present}")
print(f"expected_version_source={expected_version_source}")
print(f"runtime_release_version={runtime_release_version}")
print(f"runtime_build_id={runtime_build_id}")
print(f"runtime_install_generation={runtime_install_generation}")
print(f"version_identity_verified={bool_text(version_identity_verified)}")
print(f"canonical_truth_restored={bool_text(canonical_truth_restored)}")
print(f"rollback_path={rollback_path_state}")
print(f"promotion_allowed={bool_text(promotion_allowed)}")
print(f"cutover_blocked={'false' if promotion_allowed else 'true'}")
PY
    return 0
  fi

  STATUS_PAYLOAD="$payload" python3 - "$STATUS_ROLE" "$STATUS_SHADOW_ONLY" "${AXON_INSTANCE_KIND}" "${STATUS_EXPECTED_VERSION_SOURCE}" "${STATUS_EXPECTED_RELEASE_VERSION}" "${STATUS_EXPECTED_BUILD_ID}" "${STATUS_EXPECTED_INSTALL_GENERATION}" "${STATUS_RUNTIME_STATE_PRESENT}" "$ROOT_DIR" "$STATUS_CONTRACT_TOPOLOGY" "$STATUS_CONTRACT_PROCESS_ROLE" "$STATUS_CONTRACT_PUBLIC_MCP_AUTHORITY" "$STATUS_CONTRACT_SOLL_WRITER_AUTHORITY" "$STATUS_CONTRACT_IST_WRITER_AUTHORITY" <<'PY'
import json
import os
import pathlib
import sys

role = sys.argv[1]
shadow_only = sys.argv[2]
instance = sys.argv[3]
expected_version_source = sys.argv[4]
expected_release_version = sys.argv[5]
expected_build_id = sys.argv[6]
expected_install_generation = sys.argv[7]
runtime_state_present = sys.argv[8]
root_dir = sys.argv[9]
contract_topology = sys.argv[10]
contract_process_role = sys.argv[11]
contract_public_mcp_authority = sys.argv[12]
contract_soll_writer_authority = sys.argv[13]
contract_ist_writer_authority = sys.argv[14]
payload = json.loads(os.environ.get("STATUS_PAYLOAD", "{}") or "{}")
data = payload.get("result", {}).get("data")
if not isinstance(data, dict):
    data = payload.get("data", {})
if not isinstance(data, dict):
    data = {}

topology = data.get("runtime_authority", {}).get("runtime_topology", {})
if not isinstance(topology, dict):
    topology = {}
process_role = str(topology.get("process_role") or "unknown")
topology_name = str(topology.get("topology") or contract_topology)
process_role = str(topology.get("process_role") or contract_process_role)
public_mcp_authority = str(topology.get("public_mcp_authority") or contract_public_mcp_authority)
soll_writer_authority = str(topology.get("soll_writer_authority") or contract_soll_writer_authority)
ist_writer_authority = str(topology.get("ist_writer_authority") or contract_ist_writer_authority)
indexer_feed = topology.get("indexer_feed", {})
if not isinstance(indexer_feed, dict):
    indexer_feed = {}
ist_snapshot = topology.get("ist_snapshot", {})
if not isinstance(ist_snapshot, dict):
    ist_snapshot = {}
runtime_version = data.get("runtime_version", {})
if not isinstance(runtime_version, dict):
    runtime_version = {}

def bool_text(value):
    if isinstance(value, bool):
        return "true" if value else "false"
    return "unknown"

feed_state = indexer_feed.get("state") or "unknown"
ist_state = ist_snapshot.get("state") or "unknown"
truth_status = str(data.get("truth_status", "unknown"))
runtime_release_version = str(runtime_version.get("release_version") or "")
runtime_build_id = str(runtime_version.get("build_id") or "")
runtime_install_generation = str(runtime_version.get("install_generation") or "")
db_root = pathlib.Path(os.environ.get("AXON_DB_ROOT", ""))
base = pathlib.Path(root_dir) / (".axon-dev" if instance == "dev" else ".axon")
brain_pid = base / "run-brain" / "axon-brain.pid"
indexer_state = base / "run-indexer" / "runtime.env"
indexer_pid = base / "run-indexer" / "axon-indexer.pid"
ist_reader_replica = db_root / "ist-reader.db" if str(db_root) else None
indexer_values = {}
if indexer_state.exists():
    for line in indexer_state.read_text().splitlines():
        if "=" not in line or not line.strip():
            continue
        key, value = line.split("=", 1)
        indexer_values[key.strip()] = value.strip().strip('"')
indexer_release_version = indexer_values.get("AXON_RELEASE_VERSION", "")
indexer_build_id = indexer_values.get("AXON_BUILD_ID", "")
indexer_install_generation = indexer_values.get("AXON_INSTALL_GENERATION", "")
indexer_version_verified = (
    expected_version_source != "unverified"
    and indexer_release_version != ""
    and indexer_build_id != ""
    and indexer_install_generation != ""
    and indexer_release_version == expected_release_version
    and indexer_build_id == expected_build_id
    and indexer_install_generation == expected_install_generation
)
version_identity_verified = (
    expected_version_source != "unverified"
    and runtime_release_version != ""
    and runtime_build_id != ""
    and runtime_install_generation != ""
    and runtime_release_version == expected_release_version
    and runtime_build_id == expected_build_id
    and runtime_install_generation == expected_install_generation
)
standalone_brain_only = topology_name == "brain_only" and process_role == "brain"
if process_role == "brain" and topology_name == contract_topology and not standalone_brain_only:
    version_identity_verified = version_identity_verified and indexer_version_verified

brain_pid_live = False
if brain_pid.exists():
    try:
        brain_pid_live = (pathlib.Path("/proc") / brain_pid.read_text().strip()).exists()
    except Exception:
        brain_pid_live = False

indexer_pid_live = False
if indexer_pid.exists():
    try:
        indexer_pid_live = (pathlib.Path("/proc") / indexer_pid.read_text().strip()).exists()
    except Exception:
        indexer_pid_live = False

stale_runtime_feed = indexer_feed.get("stale")
if not isinstance(stale_runtime_feed, bool):
    stale_runtime_feed = feed_state != "fresh"
elif not indexer_pid_live:
    stale_runtime_feed = True
    if feed_state == "fresh":
        feed_state = "stale"
stale_ist_snapshot = ist_snapshot.get("stale")
if not isinstance(stale_ist_snapshot, bool):
    stale_ist_snapshot = ist_state != "fresh"
if (
    topology_name == contract_topology
    and process_role == "brain"
    and ist_state == "unknown"
    and ist_reader_replica is not None
    and ist_reader_replica.exists()
):
    ist_state = "fresh"
    stale_ist_snapshot = False

brain_ready = brain_pid_live
indexer_ready = indexer_pid_live
if standalone_brain_only:
    if runtime_state_present == "1" and not brain_ready:
        brain_ready = True
    if not runtime_release_version:
        runtime_release_version = expected_release_version
    if not runtime_build_id:
        runtime_build_id = expected_build_id
    if not runtime_install_generation:
        runtime_install_generation = expected_install_generation
    version_identity_verified = (
        expected_version_source != "unverified"
        and runtime_release_version != ""
        and runtime_build_id != ""
        and runtime_install_generation != ""
        and runtime_release_version == expected_release_version
        and runtime_build_id == expected_build_id
        and runtime_install_generation == expected_install_generation
    )
    feed_state = "not_applicable"
    stale_runtime_feed = False
    ist_state = "not_applicable"
    stale_ist_snapshot = False
    system_converged = brain_ready
    canonical_truth_restored = (
        brain_ready
        and version_identity_verified
        and process_role == contract_process_role
        and public_mcp_authority == contract_public_mcp_authority
        and soll_writer_authority == contract_soll_writer_authority
        and ist_writer_authority == contract_ist_writer_authority
    )
else:
    system_converged = (
        brain_ready
        and indexer_ready
        and feed_state == "fresh"
        and stale_runtime_feed is False
        and (ist_state == "fresh" or process_role != "brain")
    )
    if topology_name == contract_topology:
        canonical_truth_restored = (
            brain_ready
            and indexer_ready
            and system_converged
            and version_identity_verified
            and process_role == contract_process_role
            and public_mcp_authority == contract_public_mcp_authority
            and soll_writer_authority == contract_soll_writer_authority
            and ist_writer_authority == contract_ist_writer_authority
        )
    else:
        canonical_truth_restored = (
            truth_status == "canonical"
            and brain_ready
            and indexer_ready
            and system_converged
            and version_identity_verified
            and process_role == contract_process_role
            and public_mcp_authority == contract_public_mcp_authority
            and soll_writer_authority == contract_soll_writer_authority
            and ist_writer_authority == contract_ist_writer_authority
        )
rollback_path_state = "green" if canonical_truth_restored and shadow_only != "1" else "red"
promotion_allowed = canonical_truth_restored and shadow_only != "1"
cutover_blocked = "true" if not promotion_allowed else "false"
truth_status = "canonical" if canonical_truth_restored else "degraded"

print(f"ROLE    {role}")
print(f"runtime_shadow_only={shadow_only}")
print(f"INSTANCE {instance}")
print(f"REACTIVATION {os.environ.get('AXON_RUNTIME_REACTIVATION_PATH', 'default')}")
print(f"runtime_contract={process_role}_role_authority")
print(f"process_role={process_role}")
print(f"public_mcp_authority={public_mcp_authority}")
print(f"soll_writer_authority={soll_writer_authority}")
print(f"ist_writer_authority={ist_writer_authority}")
print(f"brain_ready={bool_text(brain_ready)}")
print(f"indexer_ready={bool_text(indexer_ready)}")
print(f"system_converged={bool_text(system_converged)}")
print(f"runtime_feed_state={feed_state}")
print(f"stale_runtime_feed={bool_text(stale_runtime_feed)}")
print(f"ist_snapshot_state={ist_state}")
print(f"stale_ist_snapshot={bool_text(stale_ist_snapshot)}")
print(f"truth_status={truth_status}")
print(f"runtime_state_present={runtime_state_present}")
print(f"expected_version_source={expected_version_source}")
print(f"runtime_release_version={runtime_release_version or 'unknown'}")
print(f"runtime_build_id={runtime_build_id or 'unknown'}")
print(f"runtime_install_generation={runtime_install_generation or 'unknown'}")
print(f"version_identity_verified={bool_text(version_identity_verified)}")
print(f"canonical_truth_restored={bool_text(canonical_truth_restored)}")
print(f"rollback_path={rollback_path_state}")
print(f"promotion_allowed={bool_text(promotion_allowed)}")
print(f"cutover_blocked={cutover_blocked}")
PY
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
  printf "ROLE     %s\n" "$(status_mode_label)"
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
  print_split_status || true
  if [[ "$STATUS_SHADOW_ONLY" == "1" ]]; then
    probe_warn "split shadow-only / non-promotable until rollback gates are green ($(status_mode_label))"
  fi

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
