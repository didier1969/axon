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

test_policy_asymmetry() {
    local cpu_cores="$1"
    local ram_gb="$2"
    local live_budget=""
    local dev_budget=""

    # REQ-AXO-902275 — le volet "workers" de cette asymétrie s'appuyait sur
    # `axon_compute_worker_cap`, supprimée : elle alimentait `MAX_AXON_WORKERS`, que rien
    # ne lisait. L'asymétrie live>dev sur les workers est décidée côté Rust
    # (`runtime_capacity_profile::recommend_sizing`) et se teste là-bas. Ce qui reste ici
    # est le budget mémoire de queue, lui réellement consommé par le runtime.
    live_budget="$(axon_compute_queue_memory_budget_bytes balanced "$ram_gb")"
    dev_budget="$(axon_compute_queue_memory_budget_bytes conservative "$ram_gb")"

    assert_bytes_lt "$dev_budget" "$live_budget" "dev queue budget lower than live for ${cpu_cores}c/${ram_gb}g"
}

test_policy_asymmetry 4 8
test_policy_asymmetry 8 16
test_policy_asymmetry 16 32

unset AXON_RESOURCE_PRIORITY AXON_BACKGROUND_BUDGET_CLASS AXON_GPU_ACCESS_POLICY AXON_WATCHER_POLICY
unset AXON_QUEUE_MEMORY_BUDGET_BYTES AXON_EMBEDDING_PROVIDER
unset AXON_RESOURCE_POLICY_COMPUTED_INSTANCE
unset AXON_POLICY_SOURCE_AXON_RESOURCE_PRIORITY AXON_POLICY_SOURCE_AXON_BACKGROUND_BUDGET_CLASS
unset AXON_POLICY_SOURCE_AXON_GPU_ACCESS_POLICY AXON_POLICY_SOURCE_AXON_WATCHER_POLICY
unset AXON_POLICY_SOURCE_AXON_QUEUE_MEMORY_BUDGET_BYTES AXON_POLICY_SOURCE_AXON_EMBEDDING_PROVIDER

axon_detect_host_cpu_cores() { printf '8\n'; }
axon_detect_host_ram_gb() { printf '16\n'; }
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
unset AXON_QUEUE_MEMORY_BUDGET_BYTES AXON_EMBEDDING_PROVIDER
unset AXON_RESOURCE_POLICY_COMPUTED_INSTANCE
unset AXON_POLICY_SOURCE_AXON_RESOURCE_PRIORITY AXON_POLICY_SOURCE_AXON_BACKGROUND_BUDGET_CLASS
unset AXON_POLICY_SOURCE_AXON_GPU_ACCESS_POLICY AXON_POLICY_SOURCE_AXON_WATCHER_POLICY
unset AXON_POLICY_SOURCE_AXON_QUEUE_MEMORY_BUDGET_BYTES AXON_POLICY_SOURCE_AXON_EMBEDDING_PROVIDER
AXON_GPU_ACCESS_POLICY="avoid"
axon_resolve_resource_policy dev
assert_eq "$AXON_GPU_ACCESS_POLICY" "avoid" "explicit avoid policy preserved"
assert_eq "${AXON_EMBEDDING_PROVIDER:-<unset>}" "<unset>" "REQ-AXO-184 #1 regression: avoid does NOT coerce provider to cpu"

AXON_RESOURCE_PRIORITY="critical"
AXON_QUEUE_MEMORY_BUDGET_BYTES="123456789"
axon_resolve_resource_policy dev
assert_eq "$AXON_RESOURCE_PRIORITY" "critical" "explicit priority override preserved"
# REQ-AXO-902275 — l'override opérateur se teste désormais sur une variable RÉELLEMENT
# consommée (3 lecteurs côté Rust). L'ancien cas portait sur `MAX_AXON_WORKERS`, que
# personne ne lisait : il prouvait que la politique préserve un réglage sans effet.
assert_eq "$AXON_QUEUE_MEMORY_BUDGET_BYTES" "123456789" "explicit queue budget override preserved"

echo "PASS: axon resource policy"
