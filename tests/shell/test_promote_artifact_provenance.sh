#!/usr/bin/env bash
# REQ-AXO-902464 — Le promote a publié l'ANCIEN binaire sous la NOUVELLE étiquette.
#
# Le 2026-08-23 à 00h12, un promote a rendu exit 0, phase=clean, byte-check OK et
# build_id=v0.8.0-1590-g13642f76 — en servant le binaire de v0.8.0-1586-g43880d41.
# Les quatre contrôles de `preflight.sh` étaient VERTS, et ils avaient raison : ils
# comparaient tous DEUX DÉRIVÉS DU MÊME artefact périmé.
#
#   sha du build-info vs sha réel      → le fichier périmé à lui-même
#   AXON_BUILD_ID vs `git describe`    → une ÉTIQUETTE à une ÉTIQUETTE
#   artefact vs cible canonique        → le fichier périmé à sa propre origine
#
# Aucun ne lisait le CONTENU. Ce test verrouille le seul contrôle qui n'est pas
# auto-référentiel : le binaire installé doit PORTER l'identité de la source dont
# il prétend sortir (`build.rs` l'y grave, `axon_artifact_carries_build_id` la lit).
#
# Il verrouille aussi la CAUSE : `install_release_bin` installait depuis un
# `CARGO_TARGET_DIR` que `devenv` avait réécrit pour cargo et pas pour lui — le
# build compilait ici, l'installation copiait là.
#
# Usage:
#   bash tests/shell/test_promote_artifact_provenance.sh
#   echo $?  # 0 = pass, non-zero = fail

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

FAILURES=0

fail() {
    echo "❌ FAIL: $1" >&2
    FAILURES=$((FAILURES + 1))
}

pass() {
    echo "✅ PASS: $1"
}

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

# shellcheck source=scripts/lib/axon-version.sh
source "$REPO_ROOT/scripts/lib/axon-version.sh"

# ---------------------------------------------------------------------------
# 1. La sonde de contenu : elle lit le BINAIRE, pas son étiquette.
# ---------------------------------------------------------------------------

PROMOTED_ID="v0.8.0-1590-g13642f76"
STALE_ID="v0.8.0-1586-g43880d41"

# Un « binaire » qui porte l'identité promue.
FRESH_BIN="$WORK_DIR/fresh-bin"
printf 'ELF\x00padding\x00AXON_COMPILED_BUILD_ID=%s\x00more\n' "$PROMOTED_ID" > "$FRESH_BIN"

# Le cas RÉEL de l'incident : le binaire de la veille, dont TOUTES les étiquettes
# externes annonçaient pourtant la nouvelle version.
STALE_BIN="$WORK_DIR/stale-bin"
printf 'ELF\x00padding\x00AXON_COMPILED_BUILD_ID=%s\x00more\n' "$STALE_ID" > "$STALE_BIN"

if axon_artifact_carries_build_id "$FRESH_BIN" "$PROMOTED_ID"; then
    pass "sonde de contenu : accepte le binaire compilé depuis le SHA promu"
else
    fail "sonde de contenu : elle a REFUSÉ un binaire qui porte pourtant l'identité promue"
fi

if axon_artifact_carries_build_id "$STALE_BIN" "$PROMOTED_ID"; then
    fail "sonde de contenu : elle a ACCEPTÉ le binaire de la veille — c'est exactement l'incident du 2026-08-23"
else
    pass "sonde de contenu : refuse le binaire de la veille sous l'étiquette du jour"
fi

# REQ-AXO-902464 (2026-08-24) — LA GARDE QUI MANQUAIT : L'ÉCHELLE.
#
# Les fixtures ci-dessus font quelques dizaines d'octets. `strings` les épuise
# avant que `grep -q` n'ait le temps de sortir, donc aucun SIGPIPE, donc le test
# restait VERT sur une sonde qui échouait systématiquement en production.
#
# Le vrai binaire fait 66 Mo. `grep -q` sort à la PREMIÈRE correspondance et
# ferme le tube ; `strings`, qui a encore des dizaines de Mo à produire, reçoit
# SIGPIPE et meurt avec 141. Sous `set -o pipefail` — que preflight.sh active
# ligne 2 et que ce test active ligne 25 — le pipeline hérite du 141 : la sonde
# répondait « ne porte pas » PRÉCISÉMENT quand le binaire portait l'identité.
#
# Une garde inversée : elle échoue sur le succès. Le promote du 2026-08-24 a été
# refusé à l'étape 1b sur un binaire parfaitement conforme.
#
# La discipline était là (pipefail dans les deux). C'est la TAILLE du fixture qui
# rendait le test aveugle — un fixture qui ne peut pas déclencher le mécanisme ne
# peut pas falsifier la garde.
BIG_BIN="$WORK_DIR/big-bin"
{
    printf 'AXON_COMPILED_BUILD_ID=%s\n' "$PROMOTED_ID"
    # ~8 Mo de chaînes APRÈS la correspondance : de quoi garantir que `strings`
    # a encore beaucoup à écrire quand un `grep -q` fermerait le tube.
    head -c 8388608 /dev/urandom | base64
} > "$BIG_BIN"

if axon_artifact_carries_build_id "$BIG_BIN" "$PROMOTED_ID"; then
    pass "sonde de contenu : tient sur un artefact volumineux (pas de SIGPIPE sous pipefail)"
else
    fail "sonde de contenu : SIGPIPE sur gros artefact — la sonde échoue sur le SUCCÈS (incident du 2026-08-24)"
fi

# Contrôle positif de la même garde : sur ce même gros artefact, une identité
# absente doit toujours être refusée. Sans lui, une sonde qui rendrait `true`
# inconditionnellement passerait le test ci-dessus.
if axon_artifact_carries_build_id "$BIG_BIN" "$STALE_ID"; then
    fail "sonde de contenu : gros artefact — identité absente ACCEPTÉE, la sonde ne lit plus rien"
else
    pass "sonde de contenu : gros artefact — une identité absente reste refusée"
fi

# Un artefact absent ne doit pas rendre « vrai par défaut » : un contrôle muet est
# pire que pas de contrôle (il se lit comme un verdict).
if axon_artifact_carries_build_id "$WORK_DIR/does-not-exist" "$PROMOTED_ID"; then
    fail "sonde de contenu : artefact absent traité comme conforme"
else
    pass "sonde de contenu : artefact absent = refus, jamais un silence vert"
fi

# Une identité vide ne doit rien valider : `grep -F ''` matche TOUT.
if axon_artifact_carries_build_id "$STALE_BIN" ""; then
    fail "sonde de contenu : une identité VIDE a validé n'importe quel binaire (grep -F '' matche tout)"
else
    pass "sonde de contenu : refuse une identité de build vide"
fi

# ---------------------------------------------------------------------------
# 2. `preflight.sh` utilise réellement la sonde — vérifié en l'appelant, pas en
#    lisant le script.
# ---------------------------------------------------------------------------

# `preflight.sh` est sourçable : son corps ne s'exécute que lancé directement.
# shellcheck source=scripts/release/preflight.sh
ROOT_DIR="$REPO_ROOT"
SKIP_BUILD_MATCH=0
source "$REPO_ROOT/scripts/release/preflight.sh"

REAL_DESCRIBE="$(git -C "$REPO_ROOT" describe --tags --always --dirty)"

make_fixture() {
    # $1 = nom, $2 = identité GRAVÉE dans le binaire, $3 = identité DÉCLARÉE
    local name="$1" compiled_id="$2" declared_id="$3"
    local dir="$WORK_DIR/$name"
    mkdir -p "$dir"
    printf 'ELF\x00AXON_COMPILED_BUILD_ID=%s\x00\n' "$compiled_id" > "$dir/axon-brain"
    local sha
    sha="$(axon_file_sha256 "$dir/axon-brain")"
    cat > "$dir/axon-brain.build-info" <<EOF
AXON_RELEASE_VERSION=0.8.0
AXON_BUILD_ID=$declared_id
AXON_PACKAGE_VERSION=0.8.0
AXON_INSTALL_GENERATION=workspace
AXON_ARTIFACT_SHA256=$sha
AXON_ARTIFACT_SOURCE=$dir/axon-brain
EOF
    printf '%s\n' "$dir"
}

honest_dir="$(make_fixture honest "$REAL_DESCRIBE" "$REAL_DESCRIBE")"
if ( verify_one_artifact "$honest_dir/axon-brain" "$honest_dir/axon-brain.build-info" axon-brain ) 2>/dev/null; then
    pass "preflight : accepte un artefact dont le CONTENU porte le SHA déclaré"
else
    fail "preflight : il a refusé un artefact pourtant honnête (contenu == étiquette == git describe)"
fi

# LE cas de l'incident : toutes les étiquettes sont justes, le contenu ne l'est pas.
liar_dir="$(make_fixture liar "$STALE_ID" "$REAL_DESCRIBE")"
if ( verify_one_artifact "$liar_dir/axon-brain" "$liar_dir/axon-brain.build-info" axon-brain ) 2>/dev/null; then
    fail "preflight : étiquettes toutes justes, contenu périmé — ACCEPTÉ. C'est l'incident du 2026-08-23, non corrigé."
else
    pass "preflight : refuse un artefact dont les étiquettes sont justes mais le contenu périmé"
fi

# ---------------------------------------------------------------------------
# 3. La CAUSE : le promote ne détourne plus la cible de compilation.
# ---------------------------------------------------------------------------

# `build_from_frozen_worktree` exportait `CARGO_TARGET_DIR=$ROOT_DIR/...` (le
# workspace) en croyant ne déplacer que les sources. `devenv.nix` réécrit cette
# variable pour cargo (DEVENV_ROOT = le worktree) mais PAS pour `install_release_bin`,
# resté hors du shell : on compilait ici et on installait là.
if grep -qE '^[[:space:]]*CARGO_TARGET_DIR="\$ROOT_DIR' "$REPO_ROOT/scripts/release/promote_live_safe.sh"; then
    fail "promote : CARGO_TARGET_DIR est encore détourné vers le workspace — la cause de REQ-AXO-902464 est intacte"
else
    pass "promote : plus de cible de compilation détournée vers le workspace"
fi

# `install_release_bin` doit installer depuis la cible que le build a RÉELLEMENT
# utilisée, telle que ce build la rapporte — pas depuis une variable que quelqu'un
# d'autre a pu réécrire entre-temps.
if grep -q 'AXON_EFFECTIVE_CARGO_TARGET' "$REPO_ROOT/scripts/setup.sh"; then
    pass "setup : l'installation lit la cible rapportée par le build lui-même"
else
    fail "setup : l'installation résout encore sa source indépendamment du build (deux résolutions, deux résultats)"
fi

# ---------------------------------------------------------------------------
# 4. L'archive ne réinjecte plus le build-info d'une AUTRE génération.
# ---------------------------------------------------------------------------

# `create_manifest.py` archive par sha256 du binaire et ne recopiait le build-info
# que si l'archive n'existait pas. Deux binaires identiques ⇒ le build-info de la
# veille (build_id 1586) restait, et le cutover le réinjectait dans `bin/`. C'est
# pourquoi le build-info sur disque contredisait le manifeste.
if grep -q 'archived_build_info_is_stale\|_build_info_matches' "$REPO_ROOT/scripts/release/create_manifest.py"; then
    pass "manifeste : un build-info archivé d'une autre génération est rafraîchi, pas réutilisé"
else
    fail "manifeste : l'archive réutilise encore un build-info d'une autre génération"
fi

echo
if [[ "$FAILURES" -eq 0 ]]; then
    echo "✅ test_promote_artifact_provenance : toutes les gardes tiennent"
    exit 0
fi
echo "❌ test_promote_artifact_provenance : $FAILURES garde(s) en défaut" >&2
exit 1
