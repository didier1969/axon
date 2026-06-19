#!/bin/bash
# Axon live auto-start guard.
#
# Idempotent: starts the FULL live instance (brain + indexer-full) only when the
# brain MCP port is not already accepting connections. Safe to call repeatedly.
#
# Invoked by:
#   - /etc/wsl.conf  [boot] command  -> live comes up automatically on WSL start
#   - bin/axon-mcp wrapper (self-heal) -> live is resurrected when MCP is called
#
# Replaces the pre-"nettoyage" tmux/start-v2.sh version (deleted in a03baa70),
# which referenced scripts that no longer exist. This uses the canonical
# `scripts/axon ... start --indexer-full` entrypoint, which self-enters devenv.

set -uo pipefail

PROJECT_ROOT="/home/dstadel/projects/axon"
BRAIN_PORT=44129
LOG="$PROJECT_ROOT/.axon/ensure-axon-running.log"
LOCK="/tmp/axon-ensure-running.lock"

ts() { date '+%Y-%m-%d %H:%M:%S'; }
mkdir -p "$(dirname "$LOG")"

# Serialize concurrent invocations (WSL boot and an MCP reconnect can race).
exec 9>"$LOCK"
if ! flock -n 9; then
    echo "[$(ts)] another ensure run holds the lock; exiting" >>"$LOG"
    exit 0
fi

# Fast path: brain already accepting TCP connections -> nothing to do.
if (exec 3<>"/dev/tcp/127.0.0.1/$BRAIN_PORT") 2>/dev/null; then
    exec 3>&- 2>/dev/null || true
    echo "[$(ts)] brain already up on :$BRAIN_PORT, no action" >>"$LOG"
    exit 0
fi

echo "[$(ts)] brain DOWN on :$BRAIN_PORT -> starting full live (indexer-full)" >>"$LOG"
cd "$PROJECT_ROOT" || { echo "[$(ts)] cannot cd to $PROJECT_ROOT" >>"$LOG"; exit 1; }

# Login-shell PATH carries the nix profile; start.sh self-enters devenv.
bash scripts/axon --instance live start --indexer-full >>"$LOG" 2>&1
rc=$?
echo "[$(ts)] start exited rc=$rc" >>"$LOG"
exit "$rc"
