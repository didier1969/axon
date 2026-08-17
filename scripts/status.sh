#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$ROOT_DIR/scripts/lib/axon-instance.sh"
# REQ-AXO-109 — clear AXON_*/AXON_* leaked from a previous run in
# this shell before any lib re-derives instance state.
axon_clear_inherited_env
# shellcheck source=scripts/lib/axon-resource-policy.sh
source "$ROOT_DIR/scripts/lib/axon-resource-policy.sh"
# shellcheck source=scripts/lib/axon-role-layout.sh
source "$ROOT_DIR/scripts/lib/axon-role-layout.sh"
source "$ROOT_DIR/scripts/lib/axon-version.sh"
# shellcheck source=scripts/lib/axon-supervisor.sh
source "$ROOT_DIR/scripts/lib/axon-supervisor.sh"
axon_load_worktree_env "$ROOT_DIR"
axon_resolve_instance "$ROOT_DIR" "$(basename "$ROOT_DIR")"
axon_resolve_resource_policy "$AXON_INSTANCE_KIND"
axon_resolve_version "$ROOT_DIR"
# REQ-AXO-178 — auto-detect role from pid files when env override is absent
# (fresh shell calling status sees no AXON_RUNTIME_* vars; the previous
# default was 'indexer' which masked a healthy brain-only runtime).
STATUS_ROLE=""
if [[ -z "${AXON_RUNTIME_SHADOW_ROLE:-}" && -z "${AXON_RUNTIME_BOOT_ROLE:-}" && -z "${AXON_RUNTIME_MODE:-}" ]]; then
  STATUS_ROLE="$(axon_detect_role_from_pid_files "$ROOT_DIR" "$AXON_INSTANCE_KIND" 2>/dev/null || true)"
fi
if [[ -z "$STATUS_ROLE" ]]; then
  STATUS_ROLE="$(axon_runtime_shadow_role)"
fi
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
# REQ-AXO-901735 — dead-brain detection: a process-compose supervisor that is
# up (answers its /live management endpoint) while the canonical brain port is
# NOT listening means the brain process died and the supervisor failed to bring
# it back. Surface this explicitly so the operator/LLM doesn't read a partial
# "supervisor alive" signal as healthy.
AXON_DEAD_BRAIN="0"
_STATUS_PC_PORT="$(axon_pc_port_for_instance "$AXON_INSTANCE_KIND")"
if axon_supervisor_healthy "$_STATUS_PC_PORT" && ! axon_brain_healthy "$AXON_BRAIN_PORT"; then
  if axon_port_is_free "$AXON_BRAIN_PORT"; then
    AXON_DEAD_BRAIN="1"
  fi
fi
export AXON_DEAD_BRAIN AXON_BRAIN_PORT
export AXON_PC_PORT="$_STATUS_PC_PORT"

# REQ-AXO-902264 — survey EVERY supervised role, not just the one this invocation
# happens to be about. `axon status` derives a single role from the pid files, so a
# runtime whose indexer had exhausted `max_restarts` hours earlier still printed
# HEALTHY. Self-healing that gives up must not look like self-healing that worked.
AXON_ROLE_SURVEY="$(axon_role_survey "$ROOT_DIR" "$AXON_INSTANCE_KIND" 2>/dev/null || true)"
AXON_ROLE_SURVEY_RENDER=""
AXON_ROLE_SURVEY_DEGRADED="0"
if [[ -n "$AXON_ROLE_SURVEY" ]]; then
  _survey_rc=0
  AXON_ROLE_SURVEY_RENDER="$(printf '%s\n' "$AXON_ROLE_SURVEY" \
    | AXON_PC_PORT="$_STATUS_PC_PORT" AXON_INSTANCE_KIND="$AXON_INSTANCE_KIND" \
      python3 "$ROOT_DIR/scripts/lib/axon-role-survey-render.py")" || _survey_rc=$?
  # Exit 2 is the renderer's "a role is abandoned" contract, not an error.
  [[ "$_survey_rc" -eq 2 ]] && AXON_ROLE_SURVEY_DEGRADED="1"
  unset _survey_rc
fi
export AXON_ROLE_SURVEY_RENDER AXON_ROLE_SURVEY_DEGRADED

# REQ-AXO-902348/902332 — WHY a role fell, from the persisted exit ledger (written
# by the self-heal survey that survives the death). Best-effort; empty on a clean
# runtime. Rendered as an informational section under the roles.
AXON_ROLE_EXITS="$(axon_recent_role_exits "$ROOT_DIR" "$AXON_INSTANCE_KIND" 24 2>/dev/null || true)"
export AXON_ROLE_EXITS

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
# REQ-AXO-902264 — an abandoned role degrades the runtime BEFORE the header is printed.
# axonctl's `overall` only knows the single role this invocation is about, so leaving the
# survey to degrade further down produced `OVERALL HEALTHY` above `STATUS DEGRADED` in the
# same output — two answers to one question, which is the ambiguity this REQ removes.
if os.environ.get("AXON_ROLE_SURVEY_DEGRADED", "0") == "1":
    overall = "degraded"

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
elif data.get("effective_alive"):
    # REQ-AXO-901879 — process-compose launches the role binary directly and
    # does not write the legacy pid file ; liveness is backed by canonical
    # evidence (mcp surface / writer guard) per PIL-AXO-001, not the pid file.
    print(f"OK      process live via {data.get('liveness_source', 'signal')} (no pid file)")
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

# Sockets. REQ-AXO-902242 — WARN only when the surface is genuinely unavailable.
# Nothing ever binds the MCP unix socket (MCP is served over HTTP by design), so a
# bare `exists` check printed a permanent WARN on a healthy runtime — noise that
# trains operators and LLMs to ignore warnings. `satisfied_by` / `applicable` come
# from axonctl, which already resolves this for the role contract (REQ-AXO-156).
for s in data.get("sockets", []):
    name = s.get("name", "?")
    path = s.get("path", "?")
    if s.get("exists"):
        print(f"OK      {name} socket present ({path})")
    elif not s.get("applicable", True):
        print(f"--      {name} socket n/a for this role ({path})")
    elif s.get("satisfied_by"):
        print(f"OK      {name} served via {s['satisfied_by']} (no socket file by design)")
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

# REQ-AXO-185 #5 — surface heartbeat degraded_reason so operators see silent
# fallbacks (e.g. embedder_provider_fallback: requested=cuda effective=cpu) at
# `axon status` time instead of after a probe window. Heartbeat path comes
# from AXON_RUN_ROOT exported by axon_apply_runtime_role_layout.
run_root = os.environ.get("AXON_RUN_ROOT", "").strip()
if run_root:
    heartbeat_path = os.path.join(run_root, "runtime-heartbeat.json")
    try:
        with open(heartbeat_path, "r", encoding="utf-8") as fh:
            heartbeat = json.load(fh)
        degraded_reason = heartbeat.get("degraded_reason")
        if isinstance(degraded_reason, str) and degraded_reason.strip():
            print(f"WARN    heartbeat degraded_reason: {degraded_reason.strip()}")
    except (OSError, json.JSONDecodeError):
        # Heartbeat absent or malformed: silent — the process-state lines
        # above already convey liveness; this surface is additive.
        pass

# REQ-AXO-902264 — supervised-role survey. Rendered LAST (just above STATUS) because it
# is the section that decides whether a role has been silently abandoned. The lines and
# the degrade decision are produced by scripts/lib/axon-role-survey-render.py (fixture-
# tested in tests/shell/test_role_survey_render.sh); this block only places them.
survey_render = os.environ.get("AXON_ROLE_SURVEY_RENDER", "").rstrip()
if survey_render:
    print()
    print("Supervisor roles")
    print(survey_render)

# REQ-AXO-902348/902332 — the exit ledger: WHY a role fell, not just THAT it did.
# One line per role that has a recorded exit in the window; empty on a clean run.
role_exits = os.environ.get("AXON_ROLE_EXITS", "").strip()
if role_exits:
    print()
    print("Recent role exits (why a role fell — axon.role_exit_event)")
    for line in role_exits.splitlines():
        parts = line.split("|")
        if len(parts) < 4:
            continue
        role, iso, code, reason = parts[0], parts[1], parts[2], "|".join(parts[3:])
        print(f"  {role:16} {iso}  exit={code}  {reason}")

# REQ-AXO-901735 — dead-brain condition computed in shell (supervisor up but
# canonical brain port not listening). This is a runtime failure the supervisor
# did not self-heal; surface it as FAIL and force a non-healthy exit code.
dead_brain = os.environ.get("AXON_DEAD_BRAIN", "0") == "1"
if dead_brain:
    brain_port = os.environ.get("AXON_BRAIN_PORT", "?")
    pc_port = os.environ.get("AXON_PC_PORT", "?")
    print(
        f"FAIL    dead brain: process-compose supervisor up on :{pc_port} but "
        f"brain port :{brain_port} not listening — the brain died and was not "
        f"restarted. Recover: ./scripts/axon stop --hard && ./scripts/axon start"
    )
    overall = "degraded"

print()
print(f"STATUS  {overall.upper()}")

sys.exit(0 if overall == "healthy" else 1)
PY
