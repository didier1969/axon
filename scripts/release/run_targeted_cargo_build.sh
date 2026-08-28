#!/usr/bin/env bash
set -euo pipefail

# REQ-AXO-902543 — stable Nexus signature for the bounded release build.
# Keep argv[1] constant (`build`): Nexus learns the real peak across SHAs instead
# of mixing this path with every historical `devenv shell` invocation.

if [[ "${1:-}" != "build" || $# -ne 5 ]]; then
    echo "usage: $0 build <rust-core-dir> <target-report-file> <jobs> <default-target>" >&2
    exit 2
fi

RUST_CORE_DIR="$2"
TARGET_REPORT_FILE="$3"
CARGO_JOBS="$4"
DEFAULT_TARGET="$5"

if [[ ! -d "$RUST_CORE_DIR" || ! "$CARGO_JOBS" =~ ^[1-9][0-9]*$ ]]; then
    echo "invalid targeted release build arguments" >&2
    exit 2
fi

printf -v BUILD_COMMAND \
    'cd %q && cargo build --release -j %q --bin axon-core --bin axon-brain --bin axon-indexer --bin axonctl --bin axon-query-embed-worker && printf '\''%%s\\n'\'' "${CARGO_TARGET_DIR:-%s}" > %q' \
    "$RUST_CORE_DIR" "$CARGO_JOBS" "$DEFAULT_TARGET" "$TARGET_REPORT_FILE"

exec devenv shell -- bash -lc "$BUILD_COMMAND"
