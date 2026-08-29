#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=../../scripts/lib/axon-role-layout.sh
source "$ROOT_DIR/scripts/lib/axon-role-layout.sh"

assert_processes() {
    local expected="$1" instance="$2" mode="$3" actual
    actual="$(axon_start_processes "$instance" "$mode" | paste -sd, -)"
    if [[ "$actual" != "$expected" ]]; then
        printf 'FAIL  %s/%s: expected %s, got %s\n' "$instance" "$mode" "$expected" "$actual" >&2
        return 1
    fi
    printf 'PASS  %s/%s -> %s\n' "$instance" "$mode" "$actual"
}

assert_processes 'axon-brain' dev brain_only
assert_processes 'axon-indexer' dev indexer_graph
assert_processes 'axon-indexer' dev indexer_vector
assert_processes 'axon-indexer' dev indexer_full
assert_processes 'axon-brain' live brain_only
assert_processes 'axon-brain,axon-indexer' live indexer_full

if grep -Fq '[[ -x "$CARGO_TARGET/release/axon-brain" && -x "$CARGO_TARGET/release/axon-indexer" ]]' "$ROOT_DIR/scripts/start.sh"; then
    echo 'FAIL  dev launcher still auto-selects release because stale release binaries exist' >&2
    exit 1
fi
echo 'PASS  dev release selection is explicit (--release), never artifact-presence driven'

if ! grep -Fq 'http://127.0.0.1:${READYZ_PORT}/readyz' "$ROOT_DIR/scripts/start.sh"; then
    echo 'FAIL  launcher readiness is not routed through the selected role port' >&2
    exit 1
fi
if grep -Fq 'curl -sf --connect-timeout 3 "http://127.0.0.1:${AXON_BRAIN_PORT}/readyz"' "$ROOT_DIR/scripts/start.sh"; then
    echo 'FAIL  indexer-only dev launch still waits on a non-existent Brain' >&2
    exit 1
fi
echo 'PASS  readiness waits on the selected role, not an unconditional Brain'

if ! grep -Fq 'AXON_INDEXER_HEALTH_PORT:-44149' "$ROOT_DIR/scripts/start.sh"; then
    echo 'FAIL  dev launcher health port diverges from process-compose.dev.yaml' >&2
    exit 1
fi
echo 'PASS  dev launcher and child agree on indexer health port 44149'
