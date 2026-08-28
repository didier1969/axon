#!/usr/bin/env bash
# REQ-AXO-902530 — monotonic budgets and diagnostic readiness samples.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROMOTE="$ROOT_DIR/scripts/release/promote_live_safe.sh"
START="$ROOT_DIR/scripts/start.sh"
PASS=0; FAIL=0
pass(){ printf '  PASS  %s\n' "$1"; PASS=$((PASS+1)); }
fail(){ printf '  FAIL  %s\n' "$1" >&2; FAIL=$((FAIL+1)); }

if grep -q 'axon_monotonic_ms' "$START" && ! grep -q 'date +%s%3N.*AXON_LAUNCH' "$START"; then
  pass "launch-to-ready timing uses one monotonic clock and cannot become negative"
else
  fail "launch-to-ready timing still mixes broken wall-clock units"
fi

if grep -q 'deadline_monotonic_ms' "$PROMOTE" && grep -q 'step_timeout_seconds' "$PROMOTE"; then
  pass "each named step publishes a monotonic deadline and timeout budget"
else
  fail "step journal lacks monotonic deadlines"
fi

if grep -q 'PROMOTE_LIVE_BUILD_TIMEOUT_S="${PROMOTE_LIVE_BUILD_TIMEOUT_S:-3600}"' "$PROMOTE" && \
   grep -q 'PROMOTE_LIVE_BUILD_TIMEOUT_S < 60' "$PROMOTE" && \
   grep -q 'build) echo "$PROMOTE_LIVE_BUILD_TIMEOUT_S"' "$PROMOTE"; then
  pass "cold release builds have a validated configurable deadline"
else
  fail "build deadline remains hard-coded or accepts invalid values"
fi

for state in connection_refused timeout http_5xx invalid_json mcp_nonready functional_failure; do
  if ! grep -q "$state" "$PROMOTE"; then fail "readiness sampler lacks state=$state"; fi
done
[[ "$FAIL" -gt 0 ]] || pass "sampler distinguishes transport, HTTP, JSON, MCP, and functional failures"

if grep -q 'historical_promote_estimate' "$PROMOTE" && ! grep -q 'dans ~3-6 min' "$PROMOTE"; then
  pass "customer estimate is derived from prior attempt journals"
else
  fail "customer estimate remains hard-coded"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
