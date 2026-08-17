#!/usr/bin/env bash
# REQ-AXO-902348/902332 — the role exit ledger: axon_persist_role_exit_events
# records WHY each role exited (deduped, clean exits excluded) and
# axon_recent_role_exits reads it back. Exercised against a THROWAWAY PG with the
# /processes body substituted (AXON_PC_PROCESSES_BODY_OVERRIDE) so the persist +
# dedup path is falsifiable without a live supervisor. Self-skips where PG tooling
# is absent (same constraint as REQ-AXO-902328 / test_pg_ctl_fallback).
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.." || exit 2
ROOT="$PWD"
source scripts/lib/axon-instance.sh 2>/dev/null
source scripts/lib/axon-log.sh 2>/dev/null
source scripts/lib/ensure-runtime.sh 2>/dev/null
source scripts/lib/axon-supervisor.sh 2>/dev/null

pass=0 fail=0
ok()  { echo "PASS  $1"; pass=$((pass+1)); }
bad() { echo "FAIL  $1"; fail=$((fail+1)); }

# ── pure unit: exit-code reason mapping (no PG) ───────────────────────────────
[[ "$(axon_exit_code_reason -1)"  == *"death by signal"* ]] && ok "reason(-1)=signal"      || bad "reason(-1)"
[[ "$(axon_exit_code_reason 75)"  == *"TensorRT"* ]]        && ok "reason(75)=TensorRT-hang" || bad "reason(75)"
[[ "$(axon_exit_code_reason 137)" == *"SIGKILL"* ]]        && ok "reason(137)=SIGKILL"      || bad "reason(137)"
[[ "$(axon_exit_code_reason 2)"   == *"exit code 2"* ]]    && ok "reason(2)=exit code 2"    || bad "reason(2)"

PGCTL="$(axon_resolve_pg_bin pg_ctl 2>/dev/null || true)"
if [[ -z "$PGCTL" ]]; then echo "SKIP  pg tooling absent"; echo "$pass passed, $fail failed"; [[ "$fail" -eq 0 ]]; exit $?; fi
PGBIN="$(dirname "$PGCTL")"
export PATH="$PGBIN:$PATH"     # the persister resolves psql via `command -v psql`
PORT="${TEST_PG_PORT:-45998}"
TMPD="$(mktemp -d /tmp/axon-exit-ledger-test.XXXXXX)"; DATA="$TMPD/data"
cleanup() { "$PGBIN/pg_ctl" -D "$DATA" -m immediate stop >/dev/null 2>&1 || true; rm -rf "$TMPD"; }
trap cleanup EXIT
"$PGBIN/initdb" -D "$DATA" -U axon --auth=trust >/dev/null 2>&1 || { echo "SKIP  initdb failed"; exit 0; }
printf "\nunix_socket_directories = '%s'\n" "$TMPD" >> "$DATA/postgresql.conf"
"$PGBIN/pg_ctl" -D "$DATA" -o "-p $PORT" -l "$DATA/startup.log" start >/dev/null 2>&1
for i in $(seq 1 20); do "$PGBIN/pg_isready" -h 127.0.0.1 -p "$PORT" -q 2>/dev/null && break; sleep 1; done

export PGPORT="$PORT"
Q() { "$PGBIN/psql" -h 127.0.0.1 -p "$PORT" -U axon -d axon_test -tAXc "$1" 2>/dev/null; }
"$PGBIN/psql" -h 127.0.0.1 -p "$PORT" -U axon -d postgres -c "CREATE DATABASE axon_test" >/dev/null 2>&1
"$PGBIN/psql" -h 127.0.0.1 -p "$PORT" -U axon -d axon_test -c "CREATE SCHEMA IF NOT EXISTS axon" >/dev/null 2>&1
Q "CREATE TABLE IF NOT EXISTS axon.role_exit_event (id BIGSERIAL PRIMARY KEY, role TEXT NOT NULL, instance_kind TEXT NOT NULL, observed_ms BIGINT NOT NULL, exit_code INTEGER NOT NULL, pc_status TEXT NOT NULL, restarts INTEGER NOT NULL DEFAULT 0, reason TEXT)" >/dev/null

# body: brain clean (exit 0), indexer crashed (-1, restarts 3), dashboard failed (1, restarts 1)
BODY='{"data":[{"name":"axon-brain","status":"Running","exit_code":0,"restarts":0},{"name":"axon-indexer","status":"Completed","exit_code":-1,"restarts":3},{"name":"dashboard","status":"Restarting","exit_code":1,"restarts":1}]}'

AXON_PC_PROCESSES_BODY_OVERRIDE="$BODY" axon_persist_role_exit_events "$ROOT" test >/dev/null 2>&1
n_total="$(Q "SELECT count(*) FROM axon.role_exit_event")"
n_brain="$(Q "SELECT count(*) FROM axon.role_exit_event WHERE role='axon-brain'")"
[[ "$n_total" == "2" ]]  && ok "2 exit events recorded (indexer+dashboard)"      || bad "expected 2 events, got ${n_total}"
[[ "$n_brain" == "0" ]]  && ok "clean role (exit 0) recorded NOTHING — neg control" || bad "clean brain wrongly recorded (${n_brain})"

# dedup: same body again → no new rows
AXON_PC_PROCESSES_BODY_OVERRIDE="$BODY" axon_persist_role_exit_events "$ROOT" test >/dev/null 2>&1
n_after="$(Q "SELECT count(*) FROM axon.role_exit_event")"
[[ "$n_after" == "2" ]] && ok "re-poll of same state is deduped (still 2)" || bad "dedup failed (${n_after})"

# a NEW crash (indexer restarts 3→4) → one new row
BODY2='{"data":[{"name":"axon-indexer","status":"Completed","exit_code":-1,"restarts":4}]}'
AXON_PC_PROCESSES_BODY_OVERRIDE="$BODY2" axon_persist_role_exit_events "$ROOT" test >/dev/null 2>&1
n_idx="$(Q "SELECT count(*) FROM axon.role_exit_event WHERE role='axon-indexer'")"
[[ "$n_idx" == "2" ]] && ok "a new crash (restarts changed) records a new row" || bad "new crash not recorded (${n_idx})"

# reader: latest per role, with reason
READ="$(axon_recent_role_exits "$ROOT" test 24 2>/dev/null)"
echo "$READ" | grep -q "axon-indexer|.*|-1|death by signal" && ok "reader returns latest indexer exit + reason" || bad "reader missing indexer line: ${READ}"
echo "$READ" | grep -q "axon-brain" && bad "reader wrongly returned clean brain" || ok "reader omits the clean role"

echo "$pass passed, $fail failed"
[[ "$fail" -eq 0 ]]
