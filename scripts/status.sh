#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$ROOT_DIR/scripts/lib/axon-instance.sh"
# REQ-AXO-109 — clear AXON_*/HYDRA_* leaked from a previous run in
# this shell before any lib re-derives instance state.
axon_clear_inherited_env
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

# ---------------------------------------------------------------------------
# Find axonctl binary
# ---------------------------------------------------------------------------
AXONCTL=""
for candidate in \
    "$ROOT_DIR/bin/axonctl" \
    "$ROOT_DIR/src/axon-core/target/release/axonctl" \
    "$ROOT_DIR/src/axon-core/target/debug/axonctl"; do
  if [[ -x "$candidate" ]]; then
    AXONCTL="$candidate"
    break
  fi
done

if [[ -z "$AXONCTL" ]]; then
  printf "ERROR   axonctl binary not found (checked bin/ and target/release/)\n" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Call axonctl status --json
# ---------------------------------------------------------------------------
JSON_OUTPUT="$("$AXONCTL" status \
  --project-root "$ROOT_DIR" \
  --instance-kind "$AXON_INSTANCE_KIND" \
  --role "$STATUS_ROLE" \
  --json 2>&1)" || true

if [[ -z "$JSON_OUTPUT" ]] || ! python3 -c "import json,sys; json.loads(sys.stdin.read())" <<<"$JSON_OUTPUT" 2>/dev/null; then
  printf "ERROR   axonctl status returned invalid JSON:\n%s\n" "$JSON_OUTPUT" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Format human-readable output from JSON
# ---------------------------------------------------------------------------
AXONCTL_JSON="$JSON_OUTPUT" python3 - "$AXON_INSTANCE_KIND" "$STATUS_ROLE" <<'PY'
import json
import os
import sys

instance_kind = sys.argv[1]
role_hint = sys.argv[2]
data = json.loads(os.environ["AXONCTL_JSON"])

instance = data.get("instance_kind", instance_kind)
role = data.get("role", role_hint)
overall = data.get("overall", "unknown")

print("Axon status")
print("------------")
print(f"INSTANCE {instance}")
print(f"ROLE     {role}")
print(f"OVERALL  {overall.upper()}")
print()

# Process
proc = data.get("process", {})
pid = proc.get("pid")
alive = proc.get("alive", False)
match = proc.get("cmdline_matches", False)
# REQ-AXO-097 — when the role process is dead, print FAIL not OK so an
# operator scanning the output (or an LLM parsing it) cannot misread the
# `OK process pid=X dead` line as healthy. cmdline mismatch is a soft
# warning (process alive but probably not ours) — surface as WARN.
if pid is not None:
    if alive and match:
        print(f"OK      process pid={pid} running")
    elif alive:
        print(f"WARN    process pid={pid} alive (cmdline mismatch — probably reused pid)")
    else:
        print(f"FAIL    process pid={pid} dead (stale pid file points to a process that is not running)")
else:
    print("FAIL    process: no pid file")

# Ports
ports = data.get("ports", [])
listening = [p for p in ports if p.get("listening")]
not_listening = [p for p in ports if not p.get("listening")]
if listening:
    port_list = ", ".join(str(p["port"]) for p in listening)
    print(f"OK      ports listening: {port_list}")
if not_listening:
    port_list = ", ".join(str(p["port"]) for p in not_listening)
    print(f"--      ports not listening: {port_list}")

# Sockets
for s in data.get("sockets", []):
    name = s.get("name", "?")
    path = s.get("path", "?")
    if s.get("exists"):
        print(f"OK      {name} socket present ({path})")
    else:
        print(f"WARN    {name} socket missing ({path})")

# Writer guards
for g in data.get("writer_guards", []):
    target = g.get("target", "?")
    if not g.get("exists"):
        continue
    owner_pid = g.get("owner_pid")
    stale = g.get("stale", False)
    if stale:
        print(f"WARN    guard {target}: STALE (pid={owner_pid})")
    else:
        print(f"OK      guard {target}: held (pid={owner_pid})")

# REQ-AXO-151 — print role contract violations so operators see why an
# alive process is still `degraded` (e.g. brain with no MCP socket).
violations = data.get("role_contract_violations", [])
for v in violations:
    print(f"FAIL    role contract: {v}")

print()
print(f"STATUS  {overall.upper()}")

sys.exit(0 if overall == "healthy" else 1)
PY
