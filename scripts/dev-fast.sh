#!/bin/bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEFAULT_CRATE="axon-core"

usage() {
    cat <<'EOF'
Usage: ./scripts/dev-fast.sh <check|test|build|changed> [args...]

Fast Rust dev loop for Axon:
  check [crate]               Run cargo check in the target crate (default: axon-core)
  test <filter> [crate]       Run a filtered cargo test in the target crate
  build [crate]               Run cargo build in the target crate
  changed [crate]             Show changed Rust files and suggested test filters

Examples:
  ./scripts/dev-fast.sh check
  ./scripts/dev-fast.sh test indexing_policy
  ./scripts/dev-fast.sh test scanner::tests axon-core
  ./scripts/dev-fast.sh changed
EOF
}

run_in_crate() {
    local crate="$1"
    shift
    local crate_dir="$PROJECT_ROOT/src/$crate"

    if [[ ! -d "$crate_dir" ]]; then
        echo "❌ Unknown crate directory: $crate_dir"
        exit 1
    fi

    devenv shell -- bash -lc "cd '$PROJECT_ROOT' && source '$PROJECT_ROOT/scripts/cargo-env.sh' && cd '$crate_dir' && $*"
}

suggest_filters() {
    local crate="${1:-$DEFAULT_CRATE}"
    local src_root="$PROJECT_ROOT/src/$crate/src"
    local found=0

    while IFS= read -r path; do
        found=1
        local rel="${path#$PROJECT_ROOT/}"
        local stem
        stem="$(basename "$path" .rs)"
        echo "$rel"
        if [[ "$stem" != "mod" && "$stem" != "lib" && "$stem" != "main" ]]; then
            echo "  test-filter: $stem"
        fi
    done < <(git -C "$PROJECT_ROOT" diff --name-only --diff-filter=ACMRTUXB -- "$src_root" '*.rs')

    if [[ "$found" -eq 0 ]]; then
        echo "No changed Rust files under src/$crate/src."
    fi
}

if [[ $# -lt 1 ]]; then
    usage
    exit 1
fi

command="$1"
shift

case "$command" in
    check)
        crate="${1:-$DEFAULT_CRATE}"
        run_in_crate "$crate" "cargo check"
        ;;
    test)
        if [[ $# -lt 1 ]]; then
            echo "❌ test requires a filter"
            usage
            exit 1
        fi
        filter="$1"
        crate="${2:-$DEFAULT_CRATE}"
        run_in_crate "$crate" "cargo test '$filter' -- --nocapture"
        ;;
    build)
        crate="${1:-$DEFAULT_CRATE}"
        run_in_crate "$crate" "cargo build"
        ;;
    changed)
        crate="${1:-$DEFAULT_CRATE}"
        suggest_filters "$crate"
        ;;
    --help|-h|help)
        usage
        ;;
    *)
        echo "❌ Unknown command: $command"
        usage
        exit 1
        ;;
esac
