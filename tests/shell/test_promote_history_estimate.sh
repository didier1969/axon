#!/usr/bin/env bash
# tests/shell/test_promote_history_estimate.sh — REQ-AXO-902543
#
# Fixture tests pour l'estimation historique du promote. Aucun promote, aucun runtime :
# chaque cas est un journal `.jsonl` synthétique.
#
# Ce que ça protège
# -----------------
# Le prédicat précédent — `lease_released` avec `status: completed` — est vrai dès que le
# SCRIPT s'arrête proprement, y compris après un cutover ÉCHOUÉ suivi d'un auto-rollback.
# La phrase imprimée disait pourtant « tentative(s) réussie(s) ». Mesuré sur les 76
# tentatives du journal réel au 2026-09-05 : 22 comptées, 1 à tort.
#
# Le dernier cas est le MUTANT : il rejoue l'ANCIEN prédicat sur la MÊME fixture et
# vérifie qu'il compte bien la tentative échouée. Sans lui, ce fichier passerait avec ET
# sans le correctif (pratique 2169).
#
# Run: bash tests/shell/test_promote_history_estimate.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
EST="$ROOT_DIR/scripts/release/promote_history_estimate.py"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

PASS=0
FAIL=0
pass() { printf '  PASS  %s\n' "$1"; PASS=$(( PASS + 1 )); }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=$(( FAIL + 1 )); }

printf 'promote_history_estimate — REQ-AXO-902543\n'

# --- fixtures ---------------------------------------------------------------------
# Une tentative qui FRANCHIT le cutover : 100 s jusqu'à `cutover_prepare`, puis
# `cutover_finalize` passé.
mk_reussie() {
    local f="$1" base="$2" jusqu_au_cutover_ms="$3"
    {
        printf '{"event":"lease_acquired","phase":"init","status":"running","monotonic_ms":%d}\n' "$base"
        printf '{"event":"step_started","phase":"cutover_prepare","status":"running","monotonic_ms":%d}\n' \
            $(( base + jusqu_au_cutover_ms ))
        printf '{"event":"step_completed","phase":"cutover_finalize","status":"passed","monotonic_ms":%d}\n' \
            $(( base + jusqu_au_cutover_ms + 5000 ))
        printf '{"event":"lease_released","phase":"final","status":"completed","monotonic_ms":%d}\n' \
            $(( base + jusqu_au_cutover_ms + 6000 ))
    } > "$f"
}

# LA régression : le cutover est ENTAMÉ, il ÉCHOUE, l'auto-rollback rend la main, et le
# bail se relâche `completed`. L'ancien prédicat la comptait comme une réussite.
mk_rollback() {
    local f="$1" base="$2"
    {
        printf '{"event":"lease_acquired","phase":"init","status":"running","monotonic_ms":%d}\n' "$base"
        printf '{"event":"step_started","phase":"cutover_prepare","status":"running","monotonic_ms":%d}\n' $(( base + 900000 ))
        printf '{"event":"step_completed","phase":"cutover_prepare","status":"failed","monotonic_ms":%d}\n' $(( base + 960000 ))
        printf '{"event":"lease_released","phase":"final","status":"completed","monotonic_ms":%d}\n' $(( base + 961000 ))
    } > "$f"
}

D="$TMP/attempts"
mkdir -p "$D"
mk_reussie "$D/a-20260901.jsonl" 1000000 100000
mk_reussie "$D/b-20260902.jsonl" 2000000 200000
mk_rollback "$D/c-20260903.jsonl" 3000000

got="$(python3 "$EST" "$D")"

# La médiane de {100, 200} est 150 (statistics.median rend la moyenne des deux milieux).
# La tentative rollbackée ne doit PAS entrer dans le calcul : elle tirerait la médiane
# vers 900 s et promettrait à l'opérateur une durée qu'aucune réussite n'a jamais faite.
if [[ "$got" == *"=150s"* ]]; then
    pass "la mediane ignore la tentative rollbackee"
else
    fail "la tentative rollbackee entre dans la mediane : «$got»"
fi

if [[ "$got" == *"sur 2 tentative(s)"* ]]; then
    pass "le compte annonce 2, pas 3"
else
    fail "le compte inclut la tentative rollbackee : «$got»"
fi

if [[ "$got" == *"FRANCHI le cutover"* ]]; then
    pass "la phrase dit ce qu'elle a mesure, pas « reussie(s) »"
else
    fail "la phrase promet plus que la mesure : «$got»"
fi

# Un repertoire sans aucune tentative franchie ne doit rien promettre.
E="$TMP/vide"; mkdir -p "$E"; mk_rollback "$E/seul.jsonl" 1000000
got_vide="$(python3 "$EST" "$E")"
if [[ "$got_vide" == *"historique insuffisant"* ]]; then
    pass "aucune tentative franchie : rien n'est promis"
else
    fail "une duree est promise sans aucune reussite : «$got_vide»"
fi

# Un journal tronque par un SIGKILL — derniere ligne coupee — ne doit pas faire tomber
# l'estimation : c'est exactement le cas que ce journal existe pour survivre.
T="$TMP/tronque"; mkdir -p "$T"
mk_reussie "$T/ok.jsonl" 1000000 100000
printf '{"event":"lease_acq' >> "$T/ok.jsonl"
got_tr="$(python3 "$EST" "$T" 2>/dev/null || echo CRASH)"
if [[ "$got_tr" == *"=100s"* ]]; then
    pass "une ligne tronquee est ignoree, pas fatale"
else
    fail "un journal tronque casse l'estimation : «$got_tr»"
fi

# ---------------------------------------------------------------------------------
# LE MUTANT — l'ANCIEN prédicat, rejoué sur la MÊME fixture.
# ---------------------------------------------------------------------------------
mutant="$(python3 - "$D" <<'PY'
import json, pathlib, sys
n = 0
for p in sorted(pathlib.Path(sys.argv[1]).glob("*.jsonl")):
    rows = [json.loads(l) for l in p.read_text().splitlines() if l.strip()]
    if any(r.get("event") == "lease_released" and r.get("status") == "completed" for r in rows):
        n += 1
print(n)
PY
)"
if [[ "$mutant" == "3" ]]; then
    pass "MUTANT : l'ancien predicat compte bien 3 tentatives — la fixture reproduit le defaut"
else
    fail "MUTANT : l'ancien predicat compte $mutant — la fixture ne prouve rien"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
