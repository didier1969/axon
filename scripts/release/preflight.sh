#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=scripts/lib/axon-version.sh
source "$ROOT_DIR/scripts/lib/axon-version.sh"

ARTIFACT_PATH="$ROOT_DIR/bin/axon-core"
BUILD_INFO_PATH="$ROOT_DIR/bin/axon-core.build-info"
CHECK_PENDING=0
SKIP_BUILD_MATCH=0

usage() {
  cat <<'EOF'
Usage: bash scripts/release/preflight.sh [--artifact <path>] [--build-info <path>] [--check-pending] [--skip-build-match]
EOF
}

preflight_parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --artifact) ARTIFACT_PATH="${2:-}"; shift 2 ;;
      --build-info) BUILD_INFO_PATH="${2:-}"; shift 2 ;;
      --check-pending) CHECK_PENDING=1; shift ;;
      --skip-build-match) SKIP_BUILD_MATCH=1; shift ;;
      --help|-h) usage; exit 0 ;;
      *) echo "Unknown option: $1" >&2; usage; exit 1 ;;
    esac
  done

  ARTIFACT_PATH="$(realpath "$ARTIFACT_PATH")"
  BUILD_INFO_PATH="$(realpath "$BUILD_INFO_PATH")"
}

# REQ-AXO-902464 — les contrôles ci-dessous rendent `return 1`, jamais `exit 1` :
# une garde qu'on ne peut pas appeler hors du script ne peut pas être falsifiée, et
# une garde non falsifiée n'a jamais prouvé qu'elle mordait. Les appelants relaient.
# Test : `tests/shell/test_promote_artifact_provenance.sh`.
verify_one_artifact() {
  local artifact_path="$1"
  local build_info_path="$2"
  local expected_bin_name="$3"

  [[ -f "$artifact_path" ]] || { echo "Artifact not found: $artifact_path" >&2; return 1; }
  [[ -f "$build_info_path" ]] || { echo "Build info not found: $build_info_path" >&2; return 1; }

  # shellcheck disable=SC1090
  source "$build_info_path"

  if [[ -z "${AXON_BUILD_ID:-}" ]]; then
    echo "Build info missing AXON_BUILD_ID: $build_info_path" >&2
    return 1
  fi

  local artifact_sha
  artifact_sha="$(axon_file_sha256 "$artifact_path")"
  if [[ -n "${AXON_ARTIFACT_SHA256:-}" && "$AXON_ARTIFACT_SHA256" != "$artifact_sha" ]]; then
    echo "Artifact checksum mismatch: build info sha=$AXON_ARTIFACT_SHA256 actual sha=$artifact_sha" >&2
    return 1
  fi

  local git_describe
  git_describe="$(git -C "$ROOT_DIR" describe --tags --always --dirty)"
  if [[ "$SKIP_BUILD_MATCH" -ne 1 ]]; then
    if [[ "$AXON_BUILD_ID" != "$git_describe" ]]; then
      echo "Build info mismatch: AXON_BUILD_ID=$AXON_BUILD_ID but git describe=$git_describe" >&2
      return 1
    fi

    # REQ-AXO-902464 — LE contrôle qui manquait, et le seul qui ne soit pas
    # auto-référentiel. Les trois précédents comparent des dérivés du MÊME fichier :
    # son sha à son sha, son étiquette à une étiquette. Le 2026-08-23 ils étaient
    # tous verts sur un binaire vieux d'un jour. Celui-ci lit le CONTENU : le
    # binaire doit porter l'identité gravée par `build.rs` au moment où il a été
    # compilé. C'est ce que `PIL-AXO-005` exige — « l'artefact correspond-il au SHA
    # promu » — et qui n'était jusqu'ici que déclaré.
    if ! axon_artifact_carries_build_id "$artifact_path" "$AXON_BUILD_ID"; then
      echo "Artifact provenance mismatch: $artifact_path ne porte PAS l'identité de build $AXON_BUILD_ID." >&2
      echo "  Le binaire n'a pas été compilé depuis la source que ce build-info annonce." >&2
      echo "  Contrôle : strings -a '$artifact_path' | grep -F '$AXON_BUILD_ID'" >&2
      return 1
    fi

    # La cible canonique est celle que le BUILD a rapportée (`AXON_ARTIFACT_SOURCE`),
    # pas une cible recalculée ici. Recalculer supposait que le build compile
    # toujours dans le target du workspace : faux depuis REQ-AXO-902391, où le
    # promote compile dans un worktree détaché — et c'est cette supposition qui
    # faisait rougir l'étape 4 (REQ-AXO-902460) chaque fois qu'une étape
    # intermédiaire recompilait dans le target partagé.
    local recorded_source="${AXON_ARTIFACT_SOURCE:-}"
    if [[ -n "$recorded_source" && -f "$recorded_source" ]]; then
      local recorded_sha
      recorded_sha="$(axon_file_sha256 "$recorded_source")"
      if [[ "$artifact_sha" != "$recorded_sha" ]]; then
        echo "Artifact drift: $artifact_path sha=$artifact_sha but its recorded build source sha=$recorded_sha ($recorded_source)" >&2
        return 1
      fi
    fi
  fi
}

preflight_main() {
  preflight_parse_args "$@"

  local tracked_dirty
  tracked_dirty="$(git -C "$ROOT_DIR" status --short --untracked-files=no)"
  if [[ -n "$tracked_dirty" ]]; then
    echo "Tracked git state is dirty; release preflight failed." >&2
    git -C "$ROOT_DIR" status --short --untracked-files=no >&2
    exit 1
  fi

  if [[ "$CHECK_PENDING" -eq 1 && -f "$ROOT_DIR/.axon/live-release/pending.json" ]]; then
    echo "Stale pending live release exists at .axon/live-release/pending.json; clear it before continuing." >&2
    exit 1
  fi

  declare -A split_build_ids=()
  declare -A split_release_versions=()
  declare -A split_package_versions=()
  local bin_name build_info_path artifact_path
  for bin_name in axon-brain axon-indexer; do
    build_info_path="$(axon_build_info_path_for "$ROOT_DIR" "$bin_name")"
    artifact_path="$ROOT_DIR/bin/$bin_name"
    verify_one_artifact "$artifact_path" "$build_info_path" "$bin_name" || exit 1
    # shellcheck disable=SC1090
    source "$build_info_path"
    split_build_ids["$bin_name"]="${AXON_BUILD_ID:-}"
    split_release_versions["$bin_name"]="${AXON_RELEASE_VERSION:-}"
    split_package_versions["$bin_name"]="${AXON_PACKAGE_VERSION:-}"
  done
  if [[ "${split_build_ids[axon-brain]}" != "${split_build_ids[axon-indexer]}" ]]; then
    echo "Split build mismatch: brain=${split_build_ids[axon-brain]} indexer=${split_build_ids[axon-indexer]}" >&2
    exit 1
  fi
  if [[ "${split_release_versions[axon-brain]}" != "${split_release_versions[axon-indexer]}" ]]; then
    echo "Split release version mismatch: brain=${split_release_versions[axon-brain]} indexer=${split_release_versions[axon-indexer]}" >&2
    exit 1
  fi
  if [[ "${split_package_versions[axon-brain]}" != "${split_package_versions[axon-indexer]}" ]]; then
    echo "Split package version mismatch: brain=${split_package_versions[axon-brain]} indexer=${split_package_versions[axon-indexer]}" >&2
    exit 1
  fi

  echo "release preflight ok"
}

# REQ-AXO-902464 — le corps ne s'exécute que lancé DIRECTEMENT, pour que les gardes
# ci-dessus soient appelables (donc falsifiables) depuis un test. Tous les appelants
# existants font `bash preflight.sh`, ce chemin est inchangé.
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  preflight_main "$@"
fi
