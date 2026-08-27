#!/usr/bin/env bash
# REQ-AXO-902528 — no skipped, missing, timed-out, or malformed gate can qualify live.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROMOTE="$ROOT_DIR/scripts/release/promote_live_safe.sh"
CTL="$ROOT_DIR/src/axon-core/src/bin/axonctl.rs"
PASS=0; FAIL=0
pass(){ printf '  PASS  %s\n' "$1"; PASS=$((PASS+1)); }
fail(){ printf '  FAIL  %s\n' "$1" >&2; FAIL=$((FAIL+1)); }

life="$(sed -n '/lifecycle_gate_step()/,/^  }/p' "$PROMOTE")"
if grep -q 'return 77' <<<"$life" && ! grep -q 'return 0' <<<"$life"; then
  pass "lifecycle exit 77 stays a distinct non-passing gate"
else
  fail "lifecycle exit 77 is still normalized to PASS"
fi

if grep -q -- '--break-glass-reason' "$PROMOTE" && grep -q 'break_glass_refused' "$PROMOTE"; then
  pass "skip flags require an audited break-glass reason and are refused by canonical promote"
else
  fail "skip flags remain unaudited or can appear qualified"
fi

if grep -q 'timeout.*qualify-mcp' "$PROMOTE" && grep -q 'qualify_indexer_truth.*timeout' "$PROMOTE"; then
  pass "core and indexer functional gates have named timeout outcomes"
else
  fail "functional gate timeout is not bounded"
fi

if grep -q '"timeout"' "$CTL" && grep -q 'promotion_gates' "$PROMOTE"; then
  pass "gate ledger distinguishes timeout and is emitted in the final summary"
else
  fail "final verdict cannot distinguish/reconstruct gate outcomes"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
