#!/usr/bin/env bash
# ==============================================================================
# axon-mailbox-watch.sh — Watcher d'événements pour sessions d'agents LLM
# ==============================================================================
# Écoute les signaux PostgreSQL NOTIFY et les interruptions UDS d'Axon.
# Dès réception d'un message pour le projet spécifié, émet une alerte sur stdout
# pour déclencher le réveil réactif (Reactive Wakeup) d'Antigravity sans polling.
#
# Usage :
#   ./scripts/axon-mailbox-watch.sh [CODE_PROJET] [--once]
#   Exemple : ./scripts/axon-mailbox-watch.sh EXU
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AXON_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

export PGHOST="${PGHOST:-127.0.0.1}"
export PGPORT="${PGPORT:-44144}"
export PGDATABASE="${PGDATABASE:-axon_live}"
export PGUSER="${PGUSER:-axon}"
export AXON_MAILBOX_SOCK="${AXON_MAILBOX_SOCK:-/tmp/axon_mailbox.sock}"

BIN_PATH="${AXON_ROOT}/bin/axon-mailbox-watcher"
CARGO_TARGET="${AXON_ROOT}/src/axon-core/target/release/axon-mailbox-watcher"
CARGO_DEBUG_TARGET="${AXON_ROOT}/src/axon-core/target/debug/axon-mailbox-watcher"

if [[ -f "${BIN_PATH}" ]]; then
    EXEC_BIN="${BIN_PATH}"
elif [[ -f "${CARGO_TARGET}" ]]; then
    EXEC_BIN="${CARGO_TARGET}"
elif [[ -f "${CARGO_DEBUG_TARGET}" ]]; then
    EXEC_BIN="${CARGO_DEBUG_TARGET}"
else
    echo "[axon-mailbox-watch] Binaire non trouvé, compilation rapide..." >&2
    PATH="/home/dstadel/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH" \
    cargo build --manifest-path "${AXON_ROOT}/src/axon-core/Cargo.toml" --bin axon-mailbox-watcher >&2
    EXEC_BIN="${CARGO_DEBUG_TARGET}"
fi

ARGS=()
if [[ $# -gt 0 && "$1" != --* ]]; then
    ARGS+=(--project "$1")
    shift
fi

exec "${EXEC_BIN}" "${ARGS[@]}" "$@"
