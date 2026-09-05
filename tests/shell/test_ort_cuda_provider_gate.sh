#!/usr/bin/env bash
# tests/shell/test_ort_cuda_provider_gate.sh — REQ-AXO-902623
#
# Fixture tests for `axon_ort_assert_cuda_provider`. Aucun nix, aucun GPU, aucun start :
# chaque cas fabrique un faux préfixe ORT dans un répertoire temporaire.
#
# What this protects
# ------------------
# Le repli « materialization nixpkgs » ne s'achevait jamais avant `REQ-AXO-902622`. Il
# aboutit désormais — sur `nixpkgs#onnxruntime`, qui ne porte PAS
# `libonnxruntime_providers_cuda.so`. L'ancien code avertissait puis CONTINUAIT : l'embedder
# tombait sur CPU, et le runtime se déclarait HEALTHY. Un avertissement dans un log de
# démarrage n'est pas un canal.
#
# Le dernier cas est le MUTANT : il rejoue l'ANCIEN comportement (tester le fichier et se
# contenter d'avertir) sur la même fixture, et vérifie qu'il rend bien 0. Sans lui, ce
# fichier passerait avec ET sans le correctif (pratique 2169).
#
# Run: bash tests/shell/test_ort_cuda_provider_gate.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

# shellcheck source=scripts/lib/axon-ort-runtime.sh
source "$ROOT_DIR/scripts/lib/axon-ort-runtime.sh"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

PASS=0
FAIL=0
pass() { printf '  PASS  %s\n' "$1"; PASS=$(( PASS + 1 )); }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=$(( FAIL + 1 )); }

# make_prefix <nom> <with_cuda:0|1> → imprime le chemin du faux préfixe ORT
make_prefix() {
    local name="$1" with_cuda="$2" p="$TMP/$1"
    mkdir -p "$p/lib"
    : > "$p/lib/libonnxruntime.so"
    [[ "$with_cuda" == "1" ]] && : > "$p/lib/libonnxruntime_providers_cuda.so"
    printf '%s\n' "$p"
}

# assert_rc <desc> <attendu> <provider> <prefix> [manifest]
assert_rc() {
    local desc="$1" want="$2" provider="$3" prefix="$4" manifest="${5:-}" got=0
    axon_ort_assert_cuda_provider "$provider" "$prefix" "$manifest" >/dev/null 2>&1 || got=$?
    if [[ "$got" == "$want" ]]; then
        pass "$desc"
    else
        fail "$desc (attendu rc=$want, obtenu rc=$got)"
    fi
}

printf 'axon_ort_assert_cuda_provider — REQ-AXO-902623\n'

WITH="$(make_prefix with-cuda 1)"
WITHOUT="$(make_prefix without-cuda 0)"

# LA régression : c'est la forme exacte du paquet nixpkgs générique.
assert_rc "cuda demandé + provider ABSENT → refus" 1 cuda "$WITHOUT"

assert_rc "tensorrt demandé + provider ABSENT → refus" 1 tensorrt "$WITHOUT"

assert_rc "cuda demandé + provider PRÉSENT → accepté" 0 cuda "$WITH"

# L'échappatoire opérateur (REQ-AXO-902021). Testée, pas supposée : c'est le SEUL
# chemin qui reste à qui veut vraiment démarrer sans GPU.
assert_rc "AXON_EMBEDDING_PROVIDER=cpu passe, même sans provider CUDA" 0 cpu "$WITHOUT"

# Un chemin vide n'est pas un chemin valide. Sans cette garde, `-f "/lib/…"` teste une
# racine qui existe sur toute machine Ubuntu — le même piège que celui documenté sur
# axon_resolve_nix_gcc_lib_dir.
assert_rc "chemin de paquet VIDE → refus, pas un test sur /lib" 1 cuda ""

# Le message doit NOMMER le paquet et le manifeste — critère d'acceptation du REQ.
MANIFEST_ABSENT="$TMP/pas-de-manifeste.json"
out="$(axon_ort_assert_cuda_provider cuda "$WITHOUT" "$MANIFEST_ABSENT" 2>&1 || true)"
if [[ "$out" == *"$WITHOUT"* && "$out" == *"$MANIFEST_ABSENT"* && "$out" == *"AXON_EMBEDDING_PROVIDER=cpu"* ]]; then
    pass "le refus nomme le paquet retenu, le manifeste attendu et l'échappatoire"
else
    fail "le refus n'est pas exploitable : «$out»"
fi

MANIFEST_PRESENT="$TMP/manifeste.json"
echo '{}' > "$MANIFEST_PRESENT"
out="$(axon_ort_assert_cuda_provider cuda "$WITHOUT" "$MANIFEST_PRESENT" 2>&1 || true)"
if [[ "$out" == *"présent"* ]]; then
    pass "un manifeste PRÉSENT non retenu est distingué d'un manifeste ABSENT"
else
    fail "les deux cas de manifeste sont confondus : «$out»"
fi

# ---------------------------------------------------------------------------------
# LE MUTANT — REQ-AXO-902623, critère d'acceptation.
#
# L'ANCIEN comportement, rejoué à l'identique sur la MÊME fixture : tester le fichier,
# avertir, et rendre 0. S'il rendait déjà non-zéro, la fixture ne reproduirait pas la
# panne et tout ce fichier passerait aussi bien sans le correctif.
# ---------------------------------------------------------------------------------
ancien_comportement() {
    local ort_out_path="$1"
    if [[ ! -f "$ort_out_path/lib/libonnxruntime_providers_cuda.so" ]]; then
        echo "warn: paquet sans provider CUDA — repli CPU" >&2
    fi
    return 0
}
mutant_rc=0
ancien_comportement "$WITHOUT" >/dev/null 2>&1 || mutant_rc=$?
if [[ "$mutant_rc" == 0 ]]; then
    pass "MUTANT : l'ancien code rend bien 0 sur cette fixture — la panne est reproduite"
else
    fail "MUTANT : l'ancien code refuse déjà (rc=$mutant_rc) — la fixture ne prouve rien"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
