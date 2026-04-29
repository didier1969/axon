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

verify_one_artifact() {
  local artifact_path="$1"
  local build_info_path="$2"
  local expected_bin_name="$3"

  [[ -f "$artifact_path" ]] || { echo "Artifact not found: $artifact_path" >&2; exit 1; }
  [[ -f "$build_info_path" ]] || { echo "Build info not found: $build_info_path" >&2; exit 1; }

  # shellcheck disable=SC1090
  source "$build_info_path"

  if [[ -z "${AXON_BUILD_ID:-}" ]]; then
    echo "Build info missing AXON_BUILD_ID: $build_info_path" >&2
    exit 1
  fi

  local artifact_sha
  artifact_sha="$(axon_file_sha256 "$artifact_path")"
  if [[ -n "${AXON_ARTIFACT_SHA256:-}" && "$AXON_ARTIFACT_SHA256" != "$artifact_sha" ]]; then
    echo "Artifact checksum mismatch: build info sha=$AXON_ARTIFACT_SHA256 actual sha=$artifact_sha" >&2
    exit 1
  fi

  local git_describe
  git_describe="$(git -C "$ROOT_DIR" describe --tags --always --dirty)"
  if [[ "$SKIP_BUILD_MATCH" -ne 1 ]]; then
    if [[ "$AXON_BUILD_ID" != "$git_describe" ]]; then
      echo "Build info mismatch: AXON_BUILD_ID=$AXON_BUILD_ID but git describe=$git_describe" >&2
      exit 1
    fi

    local workspace_release_bin
    workspace_release_bin="$(axon_workspace_release_bin_for "$ROOT_DIR" "$expected_bin_name")"
    if [[ -f "$workspace_release_bin" ]]; then
      local workspace_sha
      workspace_sha="$(axon_file_sha256 "$workspace_release_bin")"
      if [[ "$artifact_sha" != "$workspace_sha" ]]; then
        echo "Workspace artifact drift: $artifact_path sha=$artifact_sha but canonical release target sha=$workspace_sha ($workspace_release_bin)" >&2
        exit 1
      fi
      if [[ -n "${AXON_ARTIFACT_SOURCE:-}" && "$(realpath "$AXON_ARTIFACT_SOURCE")" != "$(realpath "$workspace_release_bin")" ]]; then
        echo "Artifact source mismatch: build info source=$AXON_ARTIFACT_SOURCE but canonical release target=$workspace_release_bin" >&2
        exit 1
      fi
    fi
  fi
}

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
for bin_name in axon-brain axon-indexer; do
  build_info_path="$(axon_build_info_path_for "$ROOT_DIR" "$bin_name")"
  artifact_path="$ROOT_DIR/bin/$bin_name"
  verify_one_artifact "$artifact_path" "$build_info_path" "$bin_name"
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
