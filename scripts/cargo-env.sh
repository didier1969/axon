#!/bin/bash

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    echo "source scripts/cargo-env.sh"
    exit 1
fi

_axon_cargo_env_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$_axon_cargo_env_root/.axon/cargo-target}"
AXON_RUST_CACHE_MODE="${AXON_RUST_CACHE_MODE:-incremental}"

case "$AXON_RUST_CACHE_MODE" in
    incremental)
        export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-1}"
        unset RUSTC_WRAPPER
        ;;
    sccache)
        export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
        if command -v sccache >/dev/null 2>&1; then
            export RUSTC_WRAPPER="${RUSTC_WRAPPER:-$(command -v sccache)}"
        fi
        ;;
    *)
        echo "Unsupported AXON_RUST_CACHE_MODE=$AXON_RUST_CACHE_MODE" >&2
        return 1 2>/dev/null || exit 1
        ;;
esac

unset _axon_cargo_env_root
