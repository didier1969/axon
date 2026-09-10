#!/usr/bin/env bash
set -euo pipefail

# Détecter rust/cargo nix si présent
NIX_RUST_BIN="/nix/store/b5qdmhli012ya96x8nyh551n5v94hhlm-rust-stable-1.94.1-1.94.1/bin"
if [ -d "$NIX_RUST_BIN" ]; then
    export PATH="$NIX_RUST_BIN:$PATH"
fi

exec cargo fmt --manifest-path src/axon-core/Cargo.toml --all -- --check
