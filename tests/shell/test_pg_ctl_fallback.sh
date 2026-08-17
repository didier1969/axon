#!/usr/bin/env bash
# REQ-AXO-902350 — _axon_pg_ctl_fallback recovers a dead PG when `devenv up
# postgres -d` is a no-op (process-compose already running without postgres).
#
# Exercises the REAL pg_ctl recovery path against a THROWAWAY datadir (the
# function's datadir + port are parameterised precisely so this is falsifiable
# without touching a live PG). Self-skips where PostgreSQL tooling is absent —
# same constraint as the DDL tests (REQ-AXO-902328): the fixture-based shell
# suite has no database, so a PG-spinning check runs only where pg_ctl exists.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.." || exit 2

source scripts/lib/axon-instance.sh 2>/dev/null
source scripts/lib/axon-log.sh 2>/dev/null
source scripts/lib/ensure-runtime.sh 2>/dev/null

PGCTL="$(axon_resolve_pg_bin pg_ctl 2>/dev/null || true)"
if [[ -z "$PGCTL" ]]; then
  echo "SKIP  pg_ctl not available (fixture-only environment)"
  exit 0
fi
PGBIN="$(dirname "$PGCTL")"
PORT="${TEST_PG_PORT:-45999}"
TMPD="$(mktemp -d /tmp/axon-pgctl-fallback-test.XXXXXX)"
DATA="$TMPD/data"
pass=0 fail=0
cleanup() { "$PGBIN/pg_ctl" -D "$DATA" -m immediate stop >/dev/null 2>&1 || true; rm -rf "$TMPD"; }
trap cleanup EXIT
ok()   { echo "PASS  $1"; pass=$((pass+1)); }
bad()  { echo "FAIL  $1"; fail=$((fail+1)); }

"$PGBIN/initdb" -D "$DATA" -U axon --auth=trust >/dev/null 2>&1 || { echo "SKIP  initdb failed"; exit 0; }
printf "\nunix_socket_directories = '%s'\n" "$TMPD" >> "$DATA/postgresql.conf"

# Simulate the OOM case: start then hard-kill the postmaster, leaving a stale
# postmaster.pid (a clean stop would remove it — the purge is part of the fix).
"$PGBIN/pg_ctl" -D "$DATA" -o "-p $PORT" -l "$DATA/startup.log" start >/dev/null 2>&1
sleep 2
PM_PID="$(head -n1 "$DATA/postmaster.pid" 2>/dev/null | tr -d ' \t')"
kill -9 "$PM_PID" 2>/dev/null; pkill -9 -P "$PM_PID" 2>/dev/null || true
sleep 1
[[ -f "$DATA/postmaster.pid" ]] && ok "stale postmaster.pid present (OOM case reproduced)" \
                                 || bad "expected a stale postmaster.pid after hard kill"

# Negative control: a missing datadir must be refused, not silently succeeded.
if _axon_pg_ctl_fallback "/nonexistent/datadir" "$PORT" >/dev/null 2>&1; then
  bad "fallback returned 0 on a missing datadir"
else
  ok "fallback refuses a missing datadir (non-zero)"
fi

# Real recovery: dead PG + stale pid → fallback purges the pid, starts PG, serves.
if _axon_pg_ctl_fallback "$DATA" "$PORT" >/dev/null 2>&1; then
  ok "fallback recovered a dead PG (exit 0)"
else
  bad "fallback did not recover the dead PG"
fi
if "$PGBIN/pg_isready" -h 127.0.0.1 -p "$PORT" >/dev/null 2>&1; then
  ok "PG serves on :$PORT after fallback"
else
  bad "PG not serving on :$PORT after fallback"
fi

echo "$pass passed, $fail failed"
[[ "$fail" -eq 0 ]]
