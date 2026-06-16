#!/usr/bin/env bash
# REQ-AXO-902009 — reproducible static build of the MCP stdio<->HTTP tunnel.
#
# Produces a fully-static musl binary (portable to a fresh host, no Nix-store
# interpreter) and installs it at bin/axon-mcp-tunnel-static via an atomic
# rename — safe to run while a tunnel process is executing the old inode.
#
# Requires the musl target, provisioned reproducibly by devenv.nix
# (languages.rust.targets = [ "x86_64-unknown-linux-musl" ]). Run inside the
# devenv shell:
#   devenv shell --no-reload --no-tui -- bash scripts/release/build_tunnel_static.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

TARGET="x86_64-unknown-linux-musl"
MANIFEST="src/axon-mcp-tunnel/Cargo.toml"
TARGET_DIR="${CARGO_TARGET_DIR:-.axon/cargo-target}"
OUT="${TARGET_DIR}/${TARGET}/release/axon-mcp-tunnel"
DEST="bin/axon-mcp-tunnel-static"

echo "==> building ${TARGET} static tunnel"
cargo build --release --target "${TARGET}" --manifest-path "${MANIFEST}"

[ -f "${OUT}" ] || { echo "ERROR: built binary not found at ${OUT}" >&2; exit 1; }
file "${OUT}" | grep -q "static-pie" || {
  echo "ERROR: ${OUT} is not static-pie linked (got: $(file "${OUT}"))" >&2
  exit 1
}

# Atomic install: rename relinks the directory entry, so a running tunnel keeps
# its old inode (no ETXTBSY, no disruption); new invocations use the new binary.
cp -f "${OUT}" "${DEST}.new"
mv -f "${DEST}.new" "${DEST}"
echo "==> installed ${DEST}"
file "${DEST}"
