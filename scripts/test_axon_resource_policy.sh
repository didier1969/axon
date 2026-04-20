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
    local live_workers=""
    local dev_workers=""
    local live_budget=""
    local dev_budget=""

    live_workers="$(axon_compute_worker_cap live balanced "$cpu_cores")"
    dev_workers="$(axon_compute_worker_cap dev conservative "$cpu_cores")"
    live_budget="$(axon_compute_queue_memory_budget_bytes balanced "$ram_gb")"
    dev_budget="$(axon_compute_queue_memory_budget_bytes conservative "$ram_gb")"

    assert_int_lt "$dev_workers" "$live_workers" "dev workers lower than live for ${cpu_cores}c/${ram_gb}g"
    assert_bytes_lt "$dev_budget" "$live_budget" "dev queue budget lower than live for ${cpu_cores}c/${ram_gb}g"
}

test_policy_asymmetry 4 8
test_policy_asymmetry 8 16
test_policy_asymmetry 16 32

unset AXON_RESOURCE_PRIORITY AXON_BACKGROUND_BUDGET_CLASS AXON_GPU_ACCESS_POLICY AXON_WATCHER_POLICY
unset MAX_AXON_WORKERS AXON_QUEUE_MEMORY_BUDGET_BYTES AXON_WATCHER_SUBTREE_HINT_BUDGET AXON_EMBEDDING_PROVIDER
unset AXON_RESOURCE_POLICY_COMPUTED_INSTANCE
unset AXON_POLICY_SOURCE_AXON_RESOURCE_PRIORITY AXON_POLICY_SOURCE_AXON_BACKGROUND_BUDGET_CLASS
unset AXON_POLICY_SOURCE_AXON_GPU_ACCESS_POLICY AXON_POLICY_SOURCE_AXON_WATCHER_POLICY
unset AXON_POLICY_SOURCE_MAX_AXON_WORKERS AXON_POLICY_SOURCE_AXON_QUEUE_MEMORY_BUDGET_BYTES
unset AXON_POLICY_SOURCE_AXON_WATCHER_SUBTREE_HINT_BUDGET AXON_POLICY_SOURCE_AXON_EMBEDDING_PROVIDER

axon_detect_host_cpu_cores() { printf '8\n'; }
axon_detect_host_ram_gb() { printf '16\n'; }
axon_resolve_resource_policy dev

assert_eq "$AXON_RESOURCE_PRIORITY" "best_effort" "default dev priority"
assert_eq "$AXON_BACKGROUND_BUDGET_CLASS" "conservative" "default dev budget class"
assert_eq "$AXON_GPU_ACCESS_POLICY" "avoid" "default dev gpu policy"
assert_eq "$AXON_WATCHER_POLICY" "bounded" "default dev watcher policy"
assert_eq "$AXON_EMBEDDING_PROVIDER" "cpu" "dev gpu avoidance forces cpu provider"
assert_eq "$MAX_AXON_WORKERS" "2" "default dev worker cap on 8 cores"

AXON_RESOURCE_PRIORITY="critical"
MAX_AXON_WORKERS="9"
axon_resolve_resource_policy dev
assert_eq "$AXON_RESOURCE_PRIORITY" "critical" "explicit priority override preserved"
assert_eq "$MAX_AXON_WORKERS" "9" "explicit worker cap override preserved"

echo "PASS: axon resource policy"
