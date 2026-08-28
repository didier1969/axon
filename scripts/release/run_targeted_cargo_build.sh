#!/usr/bin/env bash
set -euo pipefail

# REQ-AXO-902543 — stable Nexus signatures for bounded promotion workloads.
# Keep argv[1] constant per workload: Nexus learns their real peaks across SHAs
# instead of mixing them with every historical `devenv shell` invocation.

apply_nexus_cpu_quota() {
    local cgroup_path unit
    cgroup_path="$(awk -F: '$1 == "0" { print $3; exit }' /proc/self/cgroup 2>/dev/null || true)"
    [[ "$cgroup_path" == *'/nexus.slice/nexus-batch.slice/'* ]] || return 0

    unit="${cgroup_path##*/}"
    if [[ ! "$unit" =~ ^nexus-job-[A-Za-z0-9_-]+\.service$ ]]; then
        echo "cannot resolve Nexus batch unit from cgroup: $cgroup_path" >&2
        return 1
    fi
    systemctl --user set-property --runtime "$unit" CPUQuota=50%
}

ACTION="${1:-}"
apply_nexus_cpu_quota

case "$ACTION" in
    build)
        if [[ $# -ne 5 ]]; then
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
        ;;
    test-lib)
        if [[ $# -ne 2 || ! -d "$2" ]]; then
            echo "usage: $0 test-lib <rust-core-dir>" >&2
            exit 2
        fi
        printf -v BUILD_COMMAND \
            'cd %q && CARGO_BUILD_JOBS=1 cargo test --lib --no-run -j 1' "$2"
        ;;
    *)
        echo "usage: $0 {build|test-lib} ..." >&2
        exit 2
        ;;
esac

exec devenv shell -- bash -lc "$BUILD_COMMAND"
