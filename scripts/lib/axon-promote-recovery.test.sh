#!/usr/bin/env bash
# RÉUTILISE : axon-promote-recovery.sh pour les prédicats sous test ; la forme
# pass/fail + compteur est celle des .test.sh voisins (axon-supervisor.test.sh,
# axon-resource-policy.test.sh) — vérifié via `axon query "classification des gates
# de promote par palier de reprise"`.
#
# REQ-AXO-902590 — le palier de reprise, éprouvé sur des LISTES, sans runtime vivant.
#
# Pourquoi des fonctions pures et non un `grep` du script : le test qui gardait ce
# prédicat auparavant (`tests/shell/test_promote_fail_closed.sh`) vérifiait la
# PRÉSENCE de la ligne source `[[ "$recon_failed" != "indexer_alive" ]]`. Il ne
# pouvait donc constater que son immobilité, jamais sa justesse — et c'est ainsi que
# le défaut a survécu à chaque relecture.
set -uo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=./axon-promote-recovery.sh
source "$ROOT_DIR/scripts/lib/axon-promote-recovery.sh"

PASS=0; FAIL=0
pass(){ printf '  PASS  %s\n' "$1"; PASS=$((PASS+1)); }
fail(){ printf '  FAIL  %s\n' "$1" >&2; FAIL=$((FAIL+1)); }

# $1 = liste jointe · $2 = tier1 attendu (oui/non) · $3 = tier2 attendu · $4 = libellé
cas() {
  local liste="$1" t1_attendu="$2" t2_attendu="$3" libelle="$4" t1=non t2=non
  axon_promote_is_indexer_only_failure "$liste" && t1=oui
  axon_promote_needs_full_restart "$liste" && t2=oui
  if [[ "$t1" == "$t1_attendu" && "$t2" == "$t2_attendu" ]]; then
    pass "$libelle"
  else
    fail "$libelle — attendu tier1=$t1_attendu tier2=$t2_attendu, obtenu tier1=$t1 tier2=$t2 (liste: '$liste')"
  fi
}

#                liste                                    tier1 tier2
cas "indexer_alive"                                        oui  non \
  "indexer seul → tier 1, le brain continue de servir"

cas "indexer_alive,indexer_process_stable"                 oui  non \
  "DEUX gates indexeur ensemble → toujours tier 1 (le cas que l'égalité de chaîne ratait)"

cas "indexer_alive,last_promote_attempt"                   oui  non \
  "un promote raté ne transforme PAS une panne d'indexeur en coupure du brain"

cas "last_promote_attempt,indexer_alive"                   oui  non \
  "et l'ordre des noms ne change rien (une égalité de chaîne y serait sensible)"

cas "brain_serving"                                        non  oui \
  "le brain lui-même est tombé → tier 2, c'est le cas pour lequel il existe"

cas "indexer_alive,brain_serving"                          non  oui \
  "une panne qui dépasse l'indexeur → tier 2"

cas "indexer_alive,gate_inconnu_de_demain"                 non  oui \
  "un nom INCONNU escalade toujours — le filet ne s'élargit pas par ignorance"

cas "last_promote_attempt"                                 non  non \
  "gate hors disponibilité courante SEUL → aucun palier : rien à redémarrer"

cas "qualification_passed,manifest_runtime_match"          non  non \
  "des verdicts de release ne se réparent pas par un redémarrage"

cas ""                                                     non  non \
  "liste vide → aucun palier"

# --- ce que le journal doit pouvoir dire -----------------------------------------
retenus="$(axon_promote_retained_gates 'indexer_alive,last_promote_attempt')"
ecartes="$(axon_promote_ignored_gates 'indexer_alive,last_promote_attempt')"
if [[ "$retenus" == "indexer_alive" && "$ecartes" == "last_promote_attempt" ]]; then
  pass "le journal peut nommer ce qui a été RETENU et ce qui a été ÉCARTÉ"
else
  fail "retenus='$retenus' écartés='$ecartes' — un opérateur ne peut pas reconstituer la règle"
fi

# --- anti-dérive : les trois classes doivent être deux à deux disjointes ----------
recouvrement=()
tous=("${AXON_PROMOTE_INDEXER_ONLY_GATES[@]}" "${AXON_PROMOTE_FULL_RESTART_GATES[@]}" "${AXON_PROMOTE_NON_RUNTIME_GATES[@]}")
for ((i=0; i<${#tous[@]}; i++)); do
  for ((j=i+1; j<${#tous[@]}; j++)); do
    [[ "${tous[i]}" == "${tous[j]}" ]] && recouvrement+=("${tous[i]}")
  done
done
if [[ "${#recouvrement[@]}" -eq 0 ]]; then
  pass "les trois classes sont disjointes — un gate ne peut pas appartenir à deux paliers"
else
  fail "gate(s) dans plusieurs classes : ${recouvrement[*]}"
fi

# `brain_serving` doit escalader : c'est la raison d'être du tier 2.
cas "brain_serving,last_promote_attempt"                   non  oui \
  "le brain tombe apres un promote rate → tier 2, car rien d autre ne le repare"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
