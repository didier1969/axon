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

# REQ-AXO-902590 — cette assertion vérifiait la PRÉSENCE de la ligne
# `[[ "$recon_phase" != "clean" && "$recon_failed" != "indexer_alive" ]]`. Elle ne
# pouvait donc constater que son immobilité, jamais sa justesse : le prédicat était
# FAUX (égalité de chaîne sur une liste jointe) et le test le gardait tel quel.
# Le comportement est désormais éprouvé sur des listes réelles par
# `scripts/lib/axon-promote-recovery.test.sh` ; ici on garde deux choses qu'un test
# unitaire ne peut pas voir : que le promote passe bien par ces prédicats, et que
# l'ancienne forme n'est pas revenue.
if grep -q 'TIER-1 AUTO-RECOVERY: restart the indexer ONLY' "$PROMOTE" \
  && grep -q 'axon_promote_is_indexer_only_failure "\$recon_failed"' "$PROMOTE" \
  && grep -q 'axon_promote_needs_full_restart "\$recon_failed"' "$PROMOTE"; then
  pass "les deux paliers se decident par appartenance, via les predicats eprouves"
else
  fail "un palier ne passe plus par axon-promote-recovery.sh"
fi

if grep -Fq '"$recon_failed" == "indexer_alive"' "$PROMOTE" \
  || grep -Fq '"$recon_failed" != "indexer_alive"' "$PROMOTE"; then
  fail "l egalite de chaine sur la liste jointe est REVENUE — deux gates rouges couperaient le brain"
else
  pass "aucune egalite de chaine ne subsiste sur recon_failed"
fi

if bash "$ROOT_DIR/scripts/lib/axon-promote-recovery.test.sh" >/dev/null 2>&1; then
  pass "les predicats de palier passent leur propre suite"
else
  fail "scripts/lib/axon-promote-recovery.test.sh echoue"
fi

if grep -q 'ecartes (hors disponibilite courante)' "$PROMOTE" \
  || grep -q 'écartés (hors disponibilité courante)' "$PROMOTE"; then
  pass "le journal nomme les gates ECARTES de la decision, pas seulement les rouges"
else
  fail "le journal tait sa propre regle de decision"
fi

if grep -Fq 'if [[ -f "$ADMISSION_PAUSE_FILE" ]]; then' "$PROMOTE" \
  && ! grep -Fq 'if [[ "$recon_phase" != "clean" && -f "$ADMISSION_PAUSE_FILE" ]]; then' "$PROMOTE"; then
  pass "admission desired-state overrides a fresh heartbeat instead of accepting stale clean"
else
  fail "fresh heartbeat can still mask an admission-paused indexer"
fi

guard_line="$(grep -n '^post_finalize_runtime_guard$' "$PROMOTE" | head -1 | cut -d: -f1)"
complete_line="$(grep -n 'PROMOTE COMPLETE' "$PROMOTE" | tail -1 | cut -d: -f1)"
if [[ -n "$guard_line" && -n "$complete_line" && "$guard_line" -lt "$complete_line" ]] \
  && grep -q 'indexer /readyz returned' "$PROMOTE"; then
  pass "final verdict rechecks actual indexer readiness after heartbeat reconciliation"
else
  fail "PROMOTE COMPLETE can still race a stopped indexer"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
