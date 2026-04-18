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

tracked_dirty="$(git -C "$ROOT_DIR" status --short --untracked-files=no)"
if [[ -n "$tracked_dirty" ]]; then
  echo "Tracked git state is dirty; release preflight failed." >&2
  git -C "$ROOT_DIR" status --short --untracked-files=no >&2
  exit 1
fi

[[ -f "$ARTIFACT_PATH" ]] || { echo "Artifact not found: $ARTIFACT_PATH" >&2; exit 1; }
[[ -f "$BUILD_INFO_PATH" ]] || { echo "Build info not found: $BUILD_INFO_PATH" >&2; exit 1; }

if [[ "$CHECK_PENDING" -eq 1 && -f "$ROOT_DIR/.axon/live-release/pending.json" ]]; then
  echo "Stale pending live release exists at .axon/live-release/pending.json; clear it before continuing." >&2
  exit 1
fi

# shellcheck disable=SC1090
source "$BUILD_INFO_PATH"

if [[ -z "${AXON_BUILD_ID:-}" ]]; then
  echo "Build info missing AXON_BUILD_ID: $BUILD_INFO_PATH" >&2
  exit 1
fi

artifact_sha="$(axon_file_sha256 "$ARTIFACT_PATH")"
if [[ -n "${AXON_ARTIFACT_SHA256:-}" && "$AXON_ARTIFACT_SHA256" != "$artifact_sha" ]]; then
  echo "Artifact checksum mismatch: build info sha=$AXON_ARTIFACT_SHA256 actual sha=$artifact_sha" >&2
  exit 1
fi

git_describe="$(git -C "$ROOT_DIR" describe --tags --always --dirty)"
if [[ "$SKIP_BUILD_MATCH" -ne 1 ]]; then
  if [[ "$AXON_BUILD_ID" != "$git_describe" ]]; then
    echo "Build info mismatch: AXON_BUILD_ID=$AXON_BUILD_ID but git describe=$git_describe" >&2
    exit 1
  fi

  workspace_release_bin="$(axon_workspace_release_bin "$ROOT_DIR")"
  if [[ "$(realpath "$ARTIFACT_PATH")" == "$(realpath "$ROOT_DIR/bin/axon-core")" && -f "$workspace_release_bin" ]]; then
    workspace_sha="$(axon_file_sha256 "$workspace_release_bin")"
    if [[ "$artifact_sha" != "$workspace_sha" ]]; then
      echo "Workspace artifact drift: bin/axon-core sha=$artifact_sha but canonical release target sha=$workspace_sha ($workspace_release_bin)" >&2
      exit 1
    fi
    if [[ -n "${AXON_ARTIFACT_SOURCE:-}" && "$(realpath "$AXON_ARTIFACT_SOURCE")" != "$(realpath "$workspace_release_bin")" ]]; then
      echo "Artifact source mismatch: build info source=$AXON_ARTIFACT_SOURCE but canonical release target=$workspace_release_bin" >&2
      exit 1
    fi
  fi
fi

echo "release preflight ok"
