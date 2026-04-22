#!/usr/bin/env bash
set -euo pipefail

DEV_BASELINE_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEV_BASELINE_SCRIPT_DIR="$(cd "$DEV_BASELINE_LIB_DIR/.." && pwd)"
DEV_BASELINE_PROJECT_ROOT="$(cd "$DEV_BASELINE_SCRIPT_DIR/.." && pwd)"

# shellcheck source=scripts/lib/axon-instance.sh
source "$DEV_BASELINE_LIB_DIR/axon-instance.sh"

dev_baseline_require_dev_instance() {
    export AXON_INSTANCE_KIND="${AXON_INSTANCE_KIND:-dev}"
    axon_resolve_instance "$DEV_BASELINE_PROJECT_ROOT" "$(basename "$DEV_BASELINE_PROJECT_ROOT")"
    if [[ "${AXON_INSTANCE_KIND}" != "dev" ]]; then
        echo "This command is dev-only. Resolved instance: ${AXON_INSTANCE_KIND}" >&2
        return 1
    fi
}

dev_baseline_graph_root() {
    printf '%s\n' "$DEV_BASELINE_PROJECT_ROOT/.axon-dev/graph_v2"
}

dev_baseline_cleanup_targets() {
    local graph_root
    graph_root="$(dev_baseline_graph_root)"
    cat <<EOF
$graph_root/ist.db
$graph_root/ist.db.wal
$graph_root/ist.db.tmp
$graph_root/ist-reader.db
$graph_root/ist-reader.db.tmp
$graph_root/ist-reader.publish.tmp.db
$graph_root/.axon-ist.writer.lock
$graph_root/.axon-soll.writer.lock
$DEV_BASELINE_PROJECT_ROOT/.axon-dev/run
$DEV_BASELINE_PROJECT_ROOT/.axon-dev/run-brain
$DEV_BASELINE_PROJECT_ROOT/.axon-dev/run-indexer
EOF
}

dev_baseline_stop_split() {
    AXON_INSTANCE_KIND=dev bash "$DEV_BASELINE_SCRIPT_DIR/stop-indexer.sh" "$@"
    AXON_INSTANCE_KIND=dev bash "$DEV_BASELINE_SCRIPT_DIR/stop-brain.sh" "$@"
}

dev_baseline_remove_target() {
    local target="$1"
    if [[ -L "$target" || -f "$target" ]]; then
        rm -f "$target"
        return 0
    fi
    if [[ -d "$target" ]]; then
        rm -rf "$target"
    fi
}

dev_baseline_clean_state() {
    local target=""
    while IFS= read -r target; do
        [[ -n "$target" ]] || continue
        dev_baseline_remove_target "$target"
    done < <(dev_baseline_cleanup_targets)
    mkdir -p "$(dev_baseline_graph_root)"
}

dev_baseline_start_split() {
    AXON_INSTANCE_KIND=dev bash "$DEV_BASELINE_SCRIPT_DIR/start-brain.sh"
    AXON_INSTANCE_KIND=dev bash "$DEV_BASELINE_SCRIPT_DIR/start-indexer.sh"
}

dev_baseline_wait_for_role() {
    local role="$1"
    local timeout_s="${2:-180}"
    local deadline=$((SECONDS + timeout_s))
    local script="$DEV_BASELINE_SCRIPT_DIR/status-${role}.sh"
    local output=""

    while (( SECONDS < deadline )); do
        output="$(AXON_INSTANCE_KIND=dev bash "$script" 2>&1 || true)"
        if grep -q "STATUS  HEALTHY" <<<"$output"; then
            printf '%s\n' "$output"
            return 0
        fi
        sleep 2
    done

    printf '%s\n' "$output" >&2
    return 1
}

dev_baseline_wait_for_split_converged() {
    local timeout_s="${1:-240}"
    local deadline=$((SECONDS + timeout_s))
    local output=""

    while (( SECONDS < deadline )); do
        output="$(AXON_INSTANCE_KIND=dev bash "$DEV_BASELINE_SCRIPT_DIR/status-brain.sh" 2>&1 || true)"
        if grep -q "STATUS  HEALTHY" <<<"$output" \
            && grep -q "system_converged=true" <<<"$output" \
            && grep -q "truth_status=canonical" <<<"$output"; then
            printf '%s\n' "$output"
            return 0
        fi
        sleep 2
    done

    printf '%s\n' "$output" >&2
    return 1
}

dev_baseline_wait_for_stable_measurement_window() {
    local timeout_s="${1:-240}"
    local deadline=$((SECONDS + timeout_s))
    local brain_output=""
    local indexer_output=""

    while (( SECONDS < deadline )); do
        brain_output="$(AXON_INSTANCE_KIND=dev bash "$DEV_BASELINE_SCRIPT_DIR/status-brain.sh" 2>&1 || true)"
        indexer_output="$(AXON_INSTANCE_KIND=dev bash "$DEV_BASELINE_SCRIPT_DIR/status-indexer.sh" 2>&1 || true)"

        if grep -q "STATUS  HEALTHY" <<<"$brain_output" \
            && grep -q "brain_ready=true" <<<"$brain_output" \
            && grep -q "indexer_ready=true" <<<"$brain_output" \
            && grep -q "STATUS  HEALTHY" <<<"$indexer_output" \
            && grep -q "system_converged=true" <<<"$indexer_output" \
            && grep -q "truth_status=canonical" <<<"$indexer_output" \
            && grep -q "ist_snapshot_state=fresh" <<<"$indexer_output"; then
            printf '### brain_status\n%s\n### indexer_status\n%s\n' "$brain_output" "$indexer_output"
            return 0
        fi
        sleep 2
    done

    {
        printf '### brain_status\n%s\n' "$brain_output"
        printf '### indexer_status\n%s\n' "$indexer_output"
    } >&2
    return 1
}

dev_baseline_wait_for_indexer_measurement_window() {
    local timeout_s="${1:-240}"
    local deadline=$((SECONDS + timeout_s))
    local indexer_output=""

    while (( SECONDS < deadline )); do
        indexer_output="$(AXON_INSTANCE_KIND=dev bash "$DEV_BASELINE_SCRIPT_DIR/status-indexer.sh" 2>&1 || true)"

        if grep -q "STATUS  HEALTHY" <<<"$indexer_output" \
            && grep -q "brain_ready=false" <<<"$indexer_output" \
            && grep -q "indexer_ready=true" <<<"$indexer_output" \
            && grep -q "process_role=indexer" <<<"$indexer_output" \
            && grep -q "ist_writer_authority=indexer" <<<"$indexer_output" \
            && grep -q "ist_snapshot_state=fresh" <<<"$indexer_output"; then
            printf '### indexer_status\n%s\n' "$indexer_output"
            return 0
        fi
        sleep 2
    done

    printf '### indexer_status\n%s\n' "$indexer_output" >&2
    return 1
}
