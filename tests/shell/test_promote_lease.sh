#!/usr/bin/env bash
# REQ-AXO-902526 — a live promotion is a single-owner transaction.
#
# This test uses a real kernel flock and a SIGKILL, not a mocked predicate. It proves
# the three states the operator must be able to distinguish:
#   1. active owner: a contender is refused without touching release state;
#   2. dead owner: the kernel releases the lease but the durable owner record remains;
#   3. controlled recovery: the next owner records the incomplete attempt before work.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
LEASE_LIB="$ROOT_DIR/scripts/lib/axon-promote-lease.sh"
PROMOTE="$ROOT_DIR/scripts/release/promote_live_safe.sh"

PASS=0
FAIL=0
pass() { printf '  PASS  %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf '  FAIL  %s\n' "$1" >&2; FAIL=$((FAIL + 1)); }

WORK_DIR="$(mktemp -d)"
HOLDER_PID=""
cleanup() {
  if [[ -n "$HOLDER_PID" ]]; then
    kill -9 "$HOLDER_PID" 2>/dev/null || true
    wait "$HOLDER_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

if [[ ! -f "$LEASE_LIB" ]]; then
  fail "lease library exists"
  printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
fi

acquire_line="$(grep -n '^axon_promote_lease_acquire ' "$PROMOTE" | head -1 | cut -d: -f1 || true)"
first_step_line="$(grep -n '^[[:space:]]*run_step 1 build ' "$PROMOTE" | head -1 | cut -d: -f1 || true)"
if [[ -n "$acquire_line" && -n "$first_step_line" && "$acquire_line" -lt "$first_step_line" ]]; then
  pass "promote acquires the lease before its first build/mutation step"
else
  fail "promote does not acquire the lease before build (acquire=$acquire_line build=$first_step_line)"
fi

if grep -q 'axon_promote_lease_release completed' "$PROMOTE" && \
   grep -q 'axon_promote_lease_release failed' "$PROMOTE"; then
  pass "EXIT path journals and releases both successful and failed attempts"
else
  fail "EXIT path does not close both successful and failed attempt journals"
fi

STATE_DIR="$WORK_DIR/live-release"
mkdir -p "$STATE_DIR" "$WORK_DIR/bin"
printf '{"build_id":"old"}\n' > "$STATE_DIR/current.json"
printf '{"build_id":"pending"}\n' > "$STATE_DIR/pending.json"
printf 'old-runtime-bytes\n' > "$WORK_DIR/bin/axon-brain"
before_state="$(sha256sum "$STATE_DIR/current.json" "$STATE_DIR/pending.json" "$WORK_DIR/bin/axon-brain")"

HOLDER_ID="attempt-holder"
CONTENDER_ID="attempt-contender"
RECOVERY_ID="attempt-recovery"
SHA="0123456789abcdef0123456789abcdef01234567"
holder_out="$WORK_DIR/holder.out"

bash -c '
  set -euo pipefail
  source "$1"
  axon_promote_lease_acquire "$2" live AXO "$3" "$4" 60
  printf "READY\n"
  exec sleep 60
' _ "$LEASE_LIB" "$STATE_DIR" "$SHA" "$HOLDER_ID" >"$holder_out" 2>&1 &
HOLDER_PID=$!

for _ in $(seq 1 100); do
  grep -q '^READY$' "$holder_out" 2>/dev/null && break
  kill -0 "$HOLDER_PID" 2>/dev/null || break
  sleep 0.05
done

if grep -q '^READY$' "$holder_out" 2>/dev/null; then
  pass "first promote acquires the live lease"
else
  fail "first promote did not acquire the live lease: $(tr '\n' ' ' < "$holder_out")"
fi

set +e
contender_out="$(bash -c '
  set -euo pipefail
  source "$1"
  axon_promote_lease_acquire "$2" live AXO "$3" "$4" 60
' _ "$LEASE_LIB" "$STATE_DIR" "$SHA" "$CONTENDER_ID" 2>&1)"
contender_rc=$?
set -e

if [[ "$contender_rc" -eq 75 ]]; then
  pass "concurrent promote is refused with the dedicated lease-busy exit code"
else
  fail "concurrent promote returned rc=$contender_rc instead of 75: $contender_out"
fi

if [[ "$contender_out" == *"$HOLDER_ID"* && "$contender_out" == *"lease_busy"* ]]; then
  pass "refusal names the active owner and machine-readable reason"
else
  fail "refusal does not identify the active owner: $contender_out"
fi

after_state="$(sha256sum "$STATE_DIR/current.json" "$STATE_DIR/pending.json" "$WORK_DIR/bin/axon-brain")"
if [[ "$after_state" == "$before_state" && ! -e "$STATE_DIR/attempts/${CONTENDER_ID}.jsonl" ]]; then
  pass "refused contender changes no manifest, artifact, or attempt journal"
else
  fail "refused contender mutated release state or created its own journal"
fi

owner_file="$STATE_DIR/promote-live.owner.json"
if python3 - "$owner_file" "$HOLDER_ID" "$SHA" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
assert d["release_attempt_id"] == sys.argv[2]
assert d["sha"] == sys.argv[3]
assert d["instance_kind"] == "live"
assert isinstance(d["pid"], int) and d["pid"] > 1
assert d["actor"]
assert d["deadline_unix_ms"] > d["started_unix_ms"]
assert d["lease_model"] == "kernel_flock_process_fd"
PY
then
  pass "owner record carries attempt, SHA, PID, actor, deadline, and lease model"
else
  fail "owner record misses the required lease identity"
fi

# SIGKILL bypasses every EXIT trap. The kernel must release the flock while the owner
# record and unterminated journal remain as durable evidence of the interrupted attempt.
kill -9 "$HOLDER_PID" 2>/dev/null || true
wait "$HOLDER_PID" 2>/dev/null || true
HOLDER_PID=""

bash -c '
  set -euo pipefail
  source "$1"
  axon_promote_lease_acquire "$2" live AXO "$3" "$4" 60
  axon_promote_journal_event reconcile pending running "pending/current/runtime reconciliation required"
  axon_promote_lease_release completed "test recovery complete"
' _ "$LEASE_LIB" "$STATE_DIR" "$SHA" "$RECOVERY_ID"

recovery_journal="$STATE_DIR/attempts/${RECOVERY_ID}.jsonl"
if python3 - "$recovery_journal" "$HOLDER_ID" "$RECOVERY_ID" <<'PY'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
events = [row["event"] for row in rows]
assert events[0] == "lease_acquired"
assert "stale_owner_detected" in events
assert "reconcile" in events
assert events[-1] == "lease_released"
assert any(row.get("previous_release_attempt_id") == sys.argv[2] for row in rows)
assert all(row["release_attempt_id"] == sys.argv[3] for row in rows)
times = [row["monotonic_ms"] for row in rows]
assert times == sorted(times)
assert all(row.get("sha") for row in rows)
PY
then
  pass "recovery journal distinguishes stale owner and records monotonic phase evidence"
else
  fail "recovery journal does not prove stale-owner reconciliation"
fi

if [[ ! -e "$owner_file" ]]; then
  pass "clean release removes the owner record after journalling the terminal state"
else
  fail "owner record remains after clean lease release"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
