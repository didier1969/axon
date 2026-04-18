#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

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

git_describe="$(git -C "$ROOT_DIR" describe --tags --always --dirty)"
if [[ "$SKIP_BUILD_MATCH" -ne 1 ]]; then
  if [[ "$AXON_BUILD_ID" != "$git_describe" ]]; then
    echo "Build info mismatch: AXON_BUILD_ID=$AXON_BUILD_ID but git describe=$git_describe" >&2
    exit 1
  fi
fi

sha256sum "$ARTIFACT_PATH" >/dev/null

echo "release preflight ok"
