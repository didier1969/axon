#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/axon-resource-policy.sh
source "$ROOT_DIR/scripts/lib/axon-resource-policy.sh"

assert_eq() {
    local actual="$1"
    local expected="$2"
    local label="$3"
    if [[ "$actual" != "$expected" ]]; then
        echo "FAIL: $label (expected=$expected actual=$actual)"
        exit 1
    fi
}

assert_int_lt() {
    local left="$1"
    local right="$2"
    local label="$3"
    if (( left >= right )); then
        echo "FAIL: $label (expected $left < $right)"
        exit 1
    fi
}

assert_bytes_lt() {
    local left="$1"
    local right="$2"
    local label="$3"
    if (( left >= right )); then
        echo "FAIL: $label (expected $left < $right)"
        exit 1
    fi
}

# REQ-AXO-902275 — `test_policy_asymmetry` SUPPRIMÉE. Elle comparait dev vs live sur le
# budget de queue, dont la variable est retirée côté Rust depuis REQ-AXO-290 S3 et que
# `axonctl preflight` rejette désormais. Le test prouvait donc une asymétrie sur un
# réglage que le runtime refuse. L'asymétrie live>dev qui compte encore porte sur les
# quatre knobs vérifiés plus bas, tous réellement lus par le runtime.

unset AXON_RESOURCE_PRIORITY AXON_BACKGROUND_BUDGET_CLASS AXON_GPU_ACCESS_POLICY AXON_WATCHER_POLICY
unset AXON_EMBEDDING_PROVIDER
unset AXON_RESOURCE_POLICY_COMPUTED_INSTANCE
unset AXON_POLICY_SOURCE_AXON_RESOURCE_PRIORITY AXON_POLICY_SOURCE_AXON_BACKGROUND_BUDGET_CLASS
unset AXON_POLICY_SOURCE_AXON_GPU_ACCESS_POLICY AXON_POLICY_SOURCE_AXON_WATCHER_POLICY
unset AXON_POLICY_SOURCE_AXON_EMBEDDING_PROVIDER

axon_detect_host_cpu_cores() { printf '8\n'; }
axon_resolve_resource_policy dev

assert_eq "$AXON_RESOURCE_PRIORITY" "best_effort" "default dev priority"
assert_eq "$AXON_BACKGROUND_BUDGET_CLASS" "conservative" "default dev budget class"
assert_eq "$AXON_GPU_ACCESS_POLICY" "preferred" "default dev gpu policy aligns with REQ-AXO-221 GPU baseline"
assert_eq "$AXON_WATCHER_POLICY" "bounded" "default dev watcher policy"
assert_eq "${AXON_EMBEDDING_PROVIDER:-<unset>}" "<unset>" "REQ-AXO-184 #1: no avoid→cpu auto-coercion; canonical knob is AXON_EMBEDDING_PROVIDER, runtime decides default"

# REQ-AXO-184 #1 regression: AXON_GPU_ACCESS_POLICY=avoid must NOT silently
# coerce AXON_EMBEDDING_PROVIDER to cpu. Operators wanting cpu set the
# canonical knob explicitly.
unset AXON_RESOURCE_PRIORITY AXON_BACKGROUND_BUDGET_CLASS AXON_GPU_ACCESS_POLICY AXON_WATCHER_POLICY
unset AXON_EMBEDDING_PROVIDER
unset AXON_RESOURCE_POLICY_COMPUTED_INSTANCE
unset AXON_POLICY_SOURCE_AXON_RESOURCE_PRIORITY AXON_POLICY_SOURCE_AXON_BACKGROUND_BUDGET_CLASS
unset AXON_POLICY_SOURCE_AXON_GPU_ACCESS_POLICY AXON_POLICY_SOURCE_AXON_WATCHER_POLICY
unset AXON_POLICY_SOURCE_AXON_EMBEDDING_PROVIDER
AXON_GPU_ACCESS_POLICY="avoid"
axon_resolve_resource_policy dev
assert_eq "$AXON_GPU_ACCESS_POLICY" "avoid" "explicit avoid policy preserved"
assert_eq "${AXON_EMBEDDING_PROVIDER:-<unset>}" "<unset>" "REQ-AXO-184 #1 regression: avoid does NOT coerce provider to cpu"

AXON_RESOURCE_PRIORITY="critical"
AXON_WATCHER_POLICY="off"
axon_resolve_resource_policy dev
assert_eq "$AXON_RESOURCE_PRIORITY" "critical" "explicit priority override preserved"
# REQ-AXO-902275 — l'override opérateur se teste sur un knob RÉELLEMENT consommé par le
# runtime. Les deux cibles précédentes ne l'étaient pas : `MAX_AXON_WORKERS` (aucun
# lecteur) puis `AXON_QUEUE_MEMORY_BUDGET_BYTES` (retirée, et rejetée par preflight).
assert_eq "$AXON_WATCHER_POLICY" "off" "explicit watcher policy override preserved"

echo "PASS: axon resource policy"
