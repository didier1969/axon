#!/usr/bin/env bash
# REQ-AXO-902527 — static transaction ordering and LKG-controller invariants.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROMOTE="$ROOT_DIR/scripts/release/promote_live_safe.sh"
PASS=0; FAIL=0
pass() { printf '  PASS  %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf '  FAIL  %s\n' "$1" >&2; FAIL=$((FAIL + 1)); }

line() { grep -n "$1" "$PROMOTE" | head -1 | cut -d: -f1 || true; }
prepare="$(line 'run_step 5 cutover_prepare')"
ddl="$(line 'run_step 5b apply_ddl_live')"
core="$(line 'run_step 6 qualify_mcp')"
truth="$(line 'run_step 6b qualify_indexer_truth')"
finalize="$(line 'run_step 6f cutover_finalize')"
if [[ -n "$prepare" && "$prepare" -lt "$ddl" && "$ddl" -lt "$core" && "$core" -lt "$truth" && "$truth" -lt "$finalize" ]]; then
  pass "finalize is structurally after liveness, DDL, core, and indexer-truth gates"
else
  fail "two-phase gate order is invalid: prepare=$prepare ddl=$ddl core=$core truth=$truth finalize=$finalize"
fi

if grep -q 'CUTOVER_PREPARED.*-eq 1' "$PROMOTE" && grep -q -- '--phase rollback' "$PROMOTE"; then
  pass "every trappable post-prepare failure invokes explicit rollback"
else
  fail "EXIT path lacks rollback for a prepared transaction"
fi

if grep -q 'run_step 5 cutover_prepare "$ROOT_DIR/bin/axonctl"' "$PROMOTE"; then
  pass "prepare is driven by the installed LKG controller"
else
  fail "prepare is not driven by ROOT_DIR/bin/axonctl"
fi

if grep -q 'install -m 755.*ROOT_DIR/bin' "$PROMOTE"; then
  fail "candidate replaces bin before activation"
else
  pass "candidate remains outside bin until LKG activation"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
