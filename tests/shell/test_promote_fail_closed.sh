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

# REQ-AXO-902539 — admission may deliberately pause the indexer. The live test then
# exits 77 after its cleanup has restored the role. The promote must consume that
# recovery exactly once: retry immediately, while still failing closed when the second
# measurement skips or when either measurement reports a real failure.
run_lifecycle_case() {
  local exits="$1" expected_rc="$2" expected_calls="$3"
  local harness journal rc=0 calls
  harness="$(mktemp)"
  journal="$(mktemp)"
  {
    echo '#!/usr/bin/env bash'
    echo 'set -uo pipefail'
    echo "EXITS=($exits)"
    echo "JOURNAL=$journal"
    echo 'bash() { local i; i="$(wc -l < "$JOURNAL")"; printf "call\n" >> "$JOURNAL"; return "${EXITS[$i]:-${EXITS[${#EXITS[@]}-1]}}"; }'
    echo 'ROOT_DIR=/tmp/axon-test'
    echo "$life"
    echo 'lifecycle_gate_step'
  } > "$harness"
  bash "$harness" >/dev/null 2>&1 || rc=$?
  calls="$(wc -l < "$journal")"
  rm -f "$harness" "$journal"
  [[ "$rc" -eq "$expected_rc" && "$calls" -eq "$expected_calls" ]]
}

if run_lifecycle_case '77 0' 0 2; then
  pass "lifecycle retries once after a recovered skip and accepts a real measurement"
else
  fail "lifecycle does not turn one recovered skip into exactly one measured retry"
fi

if run_lifecycle_case '1 0' 1 1; then
  pass "lifecycle real failure is returned immediately without retry"
else
  fail "lifecycle retries or masks a real first-run failure"
fi

if run_lifecycle_case '77 77 0' 77 2; then
  pass "lifecycle second skip remains non-passing after the single retry"
else
  fail "lifecycle retries repeatedly or normalizes a second skip to PASS"
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

if grep -q 'TIER-1 AUTO-RECOVERY: restart the indexer ONLY' "$PROMOTE" \
  && grep -Fq 'if [[ "$recon_phase" != "clean" && "$recon_failed" != "indexer_alive" ]]; then' "$PROMOTE"; then
  pass "indexer-only liveness failure cannot escalate to a full Brain restart"
else
  fail "indexer-only liveness failure can still interrupt the Brain"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
