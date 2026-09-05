#!/usr/bin/env bash
# tests/shell/test_ort_store_path_extraction.sh — REQ-AXO-902622
#
# Fixture tests for `axon_ort_last_store_path`. No nix, no build, no promote : chaque cas
# est un flux synthétique poussé sur stdin.
#
# What this protects
# ------------------
# Le promote du 2026-09-05 a échoué au step 2 sur « Unable to materialize a valid ONNX
# Runtime output path » alors que le build nix avait RÉUSSI (`exit_code: 0`,
# `state: "succeeded"`, chemin imprimé). L'extraction se faisait par `tail -n 1` sur un
# flux où `2>&1` avait fusionné stderr — et le `nix` du PATH est un shim d'admission qui
# écrit son rapport APRÈS le résultat. La dernière ligne n'était plus le chemin.
#
# Le cas 1 est LA régression : sa fixture est le tail réel du log
# /tmp/axon-ort-build.g7uRHL.log. Le dernier cas est le MUTANT — il vérifie que l'ancien
# `tail -n 1` échoue sur cette même fixture. Sans lui, ce fichier passerait avec ET sans
# le correctif, et ne prouverait rien.
#
# Run: bash tests/shell/test_ort_store_path_extraction.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

# shellcheck source=scripts/lib/axon-ort-runtime.sh
source "$ROOT_DIR/scripts/lib/axon-ort-runtime.sh"

PASS=0
FAIL=0
pass() { printf '  PASS  %s\n' "$1"; PASS=$(( PASS + 1 )); }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=$(( FAIL + 1 )); }

# assert_extract <desc> "<chemin attendu>" <<< flux-sur-stdin
assert_extract() {
    local desc="$1" expected="$2" got
    got="$(axon_ort_last_store_path)"
    if [[ "$got" == "$expected" ]]; then
        pass "$desc"
    else
        fail "$desc (attendu «$expected», obtenu «$got»)"
    fi
}

printf 'axon_ort_last_store_path — REQ-AXO-902622\n'

# LA régression. Le chemin du store, puis les deux lignes du courtier, puis son JSON —
# l'ordre exact du log réel. Le JSON est tronqué ici : ce qui compte est qu'il commence
# par `{` et contienne des `/nix/store/…` en incise, comme le vrai (~10 Ko).
assert_extract "le rapport du courtier écrit APRÈS le chemin ne masque pas le chemin" \
    '/nix/store/bqs4pjxbw1jp2gq48m33myy9iq19m0ws-onnxruntime-1.27.1' <<'STREAM'
nexus-job: 1788596161-187e0e6a — running
these 8 paths will be fetched (60.6 MiB download, 218.5 MiB unpacked):
  /nix/store/l6gapc4fk3sk0jw4nl0a2vv5kg524pyp-abseil-cpp-20260107.1
  /nix/store/bqs4pjxbw1jp2gq48m33myy9iq19m0ws-onnxruntime-1.27.1
copying path '/nix/store/l6gapc4fk3sk0jw4nl0a2vv5kg524pyp-abseil-cpp-20260107.1' from 'https://cache.nixos.org'...
copying path '/nix/store/bqs4pjxbw1jp2gq48m33myy9iq19m0ws-onnxruntime-1.27.1' from 'https://cache.nixos.org'...
/nix/store/bqs4pjxbw1jp2gq48m33myy9iq19m0ws-onnxruntime-1.27.1
nexus-job: réservé 12288 Mio, pic réel 425 Mio (3 %). Tu peux demander moins.
{"argv": "[\"/usr/bin/nix\", \"build\"]", "cwd": "/home/dstadel/projects/axon", "state": "succeeded", "exit_code": 0}
STREAM

# Le flux propre — sans courtier — doit rendre le même chemin. Le correctif ne doit rien
# changer là où `tail -n 1` avait raison.
assert_extract "un flux sans courtier rend le même chemin" \
    '/nix/store/bqs4pjxbw1jp2gq48m33myy9iq19m0ws-onnxruntime-1.27.1' <<'STREAM'
copying path '/nix/store/bqs4pjxbw1jp2gq48m33myy9iq19m0ws-onnxruntime-1.27.1' from 'https://cache.nixos.org'...
/nix/store/bqs4pjxbw1jp2gq48m33myy9iq19m0ws-onnxruntime-1.27.1
STREAM

# Cible multi-sorties : `--print-out-paths` en imprime plusieurs, une par ligne. On garde
# la DERNIÈRE — la sémantique que `tail -n 1` portait et qu'il ne faut pas perdre.
assert_extract "plusieurs sorties : la dernière gagne" \
    '/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-onnxruntime-1.27.1-lib' <<'STREAM'
/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-onnxruntime-1.27.1
/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-onnxruntime-1.27.1-lib
nexus-job: réservé 12288 Mio, pic réel 425 Mio (3 %).
STREAM

# Un build réellement échoué ne doit rien rendre. Le `[[ -z "$ORT_OUT_PATH" ]]` en aval
# est la seule chose qui distingue « échec » de « succès mal lu » : s'il reçoit une
# chaîne, il conclut au succès et le message d'erreur porte sur le mauvais objet.
assert_extract "un build échoué ne rend RIEN, pas la dernière ligne venue" \
    '' <<'STREAM'
error: builder for '/nix/store/xxxx-onnxruntime-1.27.1.drv' failed with exit code 1
       last 10 log lines:
       > cmake: command not found
nexus-job: réservé 12288 Mio, pic réel 118 Mio (1 %).
{"state": "failed", "exit_code": 1}
STREAM

# Un chemin cité DANS une phrase n'est pas un résultat. Sans l'ancrage en début et en fin
# de ligne, `copying path '…'` et le JSON du courtier seraient tous deux des candidats.
assert_extract "un chemin en incise dans une phrase n'est pas un résultat" \
    '' <<'STREAM'
copying path '/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello-2.12.3' from 'https://cache.nixos.org'...
  /nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-hello-2.12.3
STREAM

# ---------------------------------------------------------------------------------
# LE MUTANT — REQ-AXO-902622, critère d'acceptation.
#
# Un test d'absence doit fabriquer lui-même ce qu'il interdit (pratique 2169). Sur la
# fixture du cas 1, l'ANCIENNE extraction doit rendre le JSON du courtier. Si elle rendait
# le bon chemin, c'est que la fixture ne reproduit pas la panne et que tout ce fichier
# passerait aussi bien sans le correctif.
# ---------------------------------------------------------------------------------
mutant_got="$(tail -n 1 <<'STREAM'
/nix/store/bqs4pjxbw1jp2gq48m33myy9iq19m0ws-onnxruntime-1.27.1
nexus-job: réservé 12288 Mio, pic réel 425 Mio (3 %). Tu peux demander moins.
{"argv": "[\"/usr/bin/nix\", \"build\"]", "cwd": "/home/dstadel/projects/axon", "state": "succeeded", "exit_code": 0}
STREAM
)"
if [[ "$mutant_got" == /nix/store/* ]]; then
    fail "MUTANT : l'ancien «tail -n 1» rend le bon chemin — la fixture ne reproduit pas la panne"
else
    pass "MUTANT : l'ancien «tail -n 1» rend bien le JSON du courtier, pas le chemin"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
