#!/usr/bin/env bash
# REQ-AXO-902526 — process-scoped exclusive lease + durable promotion journal.
#
# This file is a library. The caller owns the EXIT trap and must call
# axon_promote_lease_release. A SIGKILL deliberately leaves the owner document and the
# unterminated JSONL journal behind; the kernel releases the flock, and the next owner
# records that stale attempt before reconciling pending/current/runtime.

AXON_PROMOTE_LEASE_FD=""
AXON_PROMOTE_LEASE_PATH=""
AXON_PROMOTE_OWNER_PATH=""
AXON_PROMOTE_JOURNAL_PATH=""
AXON_PROMOTE_PROJECTION_PATH=""
AXON_PROMOTE_RELEASE_ATTEMPT_ID=""
AXON_PROMOTE_PROJECT_CODE=""
AXON_PROMOTE_INSTANCE_KIND=""
AXON_PROMOTE_SHA=""
AXON_PROMOTE_ACTOR=""
AXON_PROMOTE_STARTED_UNIX_MS=""
AXON_PROMOTE_DEADLINE_UNIX_MS=""
AXON_PROMOTE_PREVIOUS_ATTEMPT_ID=""

_axon_promote_owner_attempt_id() {
  local owner_path="$1"
  [[ -s "$owner_path" ]] || return 0
  python3 - "$owner_path" <<'PY' 2>/dev/null || true
import json, sys
try:
    value = json.load(open(sys.argv[1], encoding="utf-8")).get("release_attempt_id")
    if isinstance(value, str):
        print(value)
except Exception:
    pass
PY
}

axon_promote_journal_event() {
  local event="${1:?event required}"
  local phase="${2:?phase required}"
  local status="${3:?status required}"
  local detail="${4:-}"
  local previous_id="${5:-}"

  [[ -n "$AXON_PROMOTE_JOURNAL_PATH" ]] || {
    echo "axon_promote_journal_event: lease not initialized" >&2
    return 64
  }

  python3 - \
    "$AXON_PROMOTE_JOURNAL_PATH" \
    "$AXON_PROMOTE_RELEASE_ATTEMPT_ID" \
    "$AXON_PROMOTE_PROJECT_CODE" \
    "$AXON_PROMOTE_INSTANCE_KIND" \
    "$AXON_PROMOTE_SHA" \
    "$$" \
    "$AXON_PROMOTE_ACTOR" \
    "$event" \
    "$phase" \
    "$status" \
    "$detail" \
    "$previous_id" \
    "$AXON_PROMOTE_PROJECTION_PATH" \
    "$AXON_PROMOTE_STARTED_UNIX_MS" \
    "$AXON_PROMOTE_DEADLINE_UNIX_MS" <<'PY'
import datetime as dt
import json
import os
import sys
import time

(path, attempt_id, project, instance, sha, pid, actor, event, phase, status,
 detail, previous_id, projection_path, started, deadline) = sys.argv[1:]
record = {
    "schema_version": 1,
    "release_attempt_id": attempt_id,
    "project_code": project,
    "instance_kind": instance,
    "sha": sha,
    "pid": int(pid),
    "actor": actor,
    "event": event,
    "phase": phase,
    "status": status,
    "wall_time": dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z"),
    "unix_ms": time.time_ns() // 1_000_000,
    "monotonic_ms": time.monotonic_ns() // 1_000_000,
    "detail": detail,
}
if previous_id:
    record["previous_release_attempt_id"] = previous_id
payload = (json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n").encode()
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
try:
    os.write(fd, payload)
    os.fsync(fd)
finally:
    os.close(fd)

# The JSONL is the audit authority. This small atomic projection is the bounded,
# single-read status surface used by promote_status and operators.
projection = {
    "schema_version": 1,
    "release_attempt_id": attempt_id,
    "project_code": project,
    "instance_kind": instance,
    "sha": sha,
    "pid": int(pid),
    "actor": actor,
    "phase": phase,
    "status": status,
    "last_event": event,
    "last_event_detail": detail,
    "last_event_unix_ms": record["unix_ms"],
    "started_unix_ms": int(started),
    "deadline_unix_ms": int(deadline),
    "journal_path": path,
}
tmp = f"{projection_path}.tmp.{os.getpid()}"
with open(tmp, "w", encoding="utf-8") as f:
    json.dump(projection, f, sort_keys=True, separators=(",", ":"))
    f.write("\n")
    f.flush()
    os.fsync(f.fileno())
os.replace(tmp, projection_path)
PY
}

axon_promote_lease_acquire() {
  local release_dir="${1:?release directory required}"
  local instance_kind="${2:?instance kind required}"
  local project_code="${3:?project code required}"
  local sha="${4:?sha required}"
  local attempt_id="${5:?release attempt id required}"
  local ttl_seconds="${6:-14400}"

  if [[ ! "$ttl_seconds" =~ ^[1-9][0-9]*$ ]]; then
    echo "invalid promote lease TTL: $ttl_seconds" >&2
    return 64
  fi
  if ! command -v flock >/dev/null 2>&1; then
    echo "promote lease unavailable: flock command not found" >&2
    return 69
  fi

  umask 077
  mkdir -p "$release_dir"
  release_dir="$(cd "$release_dir" && pwd -P)"
  AXON_PROMOTE_LEASE_PATH="$release_dir/promote-${instance_kind}.lock"
  AXON_PROMOTE_OWNER_PATH="$release_dir/promote-${instance_kind}.owner.json"

  # Append mode avoids truncating the shared inode before ownership is known.
  exec {AXON_PROMOTE_LEASE_FD}>>"$AXON_PROMOTE_LEASE_PATH"
  if ! flock -n "$AXON_PROMOTE_LEASE_FD"; then
    local owner="unknown"
    if [[ -s "$AXON_PROMOTE_OWNER_PATH" ]]; then
      owner="$(tr '\n' ' ' < "$AXON_PROMOTE_OWNER_PATH")"
    fi
    echo "lease_busy: another ${instance_kind} promotion owns the lease; owner=${owner}" >&2
    exec {AXON_PROMOTE_LEASE_FD}>&-
    AXON_PROMOTE_LEASE_FD=""
    return 75
  fi

  AXON_PROMOTE_PREVIOUS_ATTEMPT_ID="$(_axon_promote_owner_attempt_id "$AXON_PROMOTE_OWNER_PATH")"
  AXON_PROMOTE_RELEASE_ATTEMPT_ID="$attempt_id"
  AXON_PROMOTE_PROJECT_CODE="$project_code"
  AXON_PROMOTE_INSTANCE_KIND="$instance_kind"
  AXON_PROMOTE_SHA="$sha"
  AXON_PROMOTE_ACTOR="${USER:-unknown}@$(hostname 2>/dev/null || printf 'unknown')"
  mkdir -p "$release_dir/attempts"
  AXON_PROMOTE_JOURNAL_PATH="$release_dir/attempts/${attempt_id}.jsonl"
  AXON_PROMOTE_PROJECTION_PATH="$release_dir/attempt-current.json"

  read -r AXON_PROMOTE_STARTED_UNIX_MS AXON_PROMOTE_DEADLINE_UNIX_MS < <(
    python3 - "$ttl_seconds" <<'PY'
import sys, time
now = time.time_ns() // 1_000_000
print(now, now + int(sys.argv[1]) * 1000)
PY
  )

  local owner_tmp="$AXON_PROMOTE_OWNER_PATH.tmp.$$"
  if ! python3 - \
    "$owner_tmp" "$AXON_PROMOTE_OWNER_PATH" "$attempt_id" "$project_code" \
    "$instance_kind" "$sha" "$$" "$AXON_PROMOTE_ACTOR" \
    "$AXON_PROMOTE_STARTED_UNIX_MS" "$AXON_PROMOTE_DEADLINE_UNIX_MS" <<'PY'
import json, os, sys
(tmp, target, attempt_id, project, instance, sha, pid, actor, started, deadline) = sys.argv[1:]
record = {
    "schema_version": 1,
    "release_attempt_id": attempt_id,
    "project_code": project,
    "instance_kind": instance,
    "sha": sha,
    "pid": int(pid),
    "actor": actor,
    "started_unix_ms": int(started),
    "deadline_unix_ms": int(deadline),
    "lease_model": "kernel_flock_process_fd",
    "stale_detection": "lock_acquired_while_previous_owner_record_exists",
    "phase": "lease_acquired",
}
with open(tmp, "w", encoding="utf-8") as f:
    json.dump(record, f, sort_keys=True, separators=(",", ":"))
    f.write("\n")
    f.flush()
    os.fsync(f.fileno())
os.replace(tmp, target)
PY
  then
    rm -f "$owner_tmp"
    exec {AXON_PROMOTE_LEASE_FD}>&-
    AXON_PROMOTE_LEASE_FD=""
    return 74
  fi

  if ! axon_promote_journal_event lease_acquired init running \
      "exclusive kernel lease acquired; reconcile release state before mutation"; then
    rm -f "$AXON_PROMOTE_OWNER_PATH"
    exec {AXON_PROMOTE_LEASE_FD}>&-
    AXON_PROMOTE_LEASE_FD=""
    return 74
  fi
  if [[ -n "$AXON_PROMOTE_PREVIOUS_ATTEMPT_ID" && \
        "$AXON_PROMOTE_PREVIOUS_ATTEMPT_ID" != "$attempt_id" ]]; then
    axon_promote_journal_event stale_owner_detected reconcile required \
      "previous owner is not holding the kernel lease; reconcile pending/current/runtime before new work" \
      "$AXON_PROMOTE_PREVIOUS_ATTEMPT_ID"
  fi
}

axon_promote_lease_release() {
  local terminal_status="${1:-completed}"
  local detail="${2:-promotion process exited}"
  [[ -n "$AXON_PROMOTE_LEASE_FD" ]] || return 0

  axon_promote_journal_event lease_released final "$terminal_status" "$detail" || true

  # Never remove another attempt's owner document if an operator replaced it manually.
  if [[ "$(_axon_promote_owner_attempt_id "$AXON_PROMOTE_OWNER_PATH")" == \
        "$AXON_PROMOTE_RELEASE_ATTEMPT_ID" ]]; then
    rm -f "$AXON_PROMOTE_OWNER_PATH"
  fi
  flock -u "$AXON_PROMOTE_LEASE_FD" 2>/dev/null || true
  exec {AXON_PROMOTE_LEASE_FD}>&-
  AXON_PROMOTE_LEASE_FD=""
}
