# Orphan telemetry sockets silently block `axon start`

**Status**: open / high
**Discovered**: 2026-05-01 (Claude Sonnet 4.7 session)
**SOLL refs (when MCP is back up)**: REQ-AXO-093 (silent boot failure), REQ-AXO-101 (stale sockets)
**Why this is a markdown fallback**: live brain MCP was unavailable when this needed to be recorded. Promote to SOLL when MCP is healthy.

## Symptom

`./scripts/start-dev.sh --indexer-full` (and `axon --instance dev start`) reports:

```
✅ Axon Indexer runtime is Ready.
✅ Axon Dashboard is Ready.
```

But the dev runtime is not actually running. `pgrep -af axon-indexer` returns nothing. The dev tmux pane contains only a shell prompt (and a `mise WARN` line). IST `graph_v2/ist.db` is never created. GPU is idle. The user observes "no activity, CPU and GPU near zero" — correctly.

The bug reproduced 4 times in one session before the root cause was found.

## Root cause

A precise interaction between three pieces of code:

1. `scripts/lib/axon-instance.sh` (~lines 148, 163) exports a per-instance bare path:
   ```bash
   export AXON_TELEMETRY_SOCK="/tmp/axon-dev-telemetry.sock"      # for dev
   export AXON_TELEMETRY_SOCK="/tmp/axon-live-telemetry.sock"     # for live
   ```

2. `scripts/start.sh:184` `has_live_runtime_dataplane()` for the indexer role only checks file existence:
   ```bash
   if axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
       if [[ -S "$AXON_TELEMETRY_SOCK" ]]; then
           return 0   # <-- declares "data plane already running"
       fi
       return 1
   fi
   ```

3. `scripts/stop-dev.sh`, `scripts/stop-live.sh`, and `axonctl stop` terminate processes but **do not delete** the AF_UNIX socket file at `/tmp/axon-{instance}-telemetry.sock`.

Therefore: any past run that ever created the socket leaves an orphan that the next start sees and treats as "already up". The launch line `tmux send-keys ... axonctl supervise ...` at `scripts/start.sh:835` is gated behind `if ! has_live_runtime_dataplane; then ...` — so it gets **silently skipped**. The wait loop then satisfies its readiness check on the same orphan socket and prints `Ready`.

The session evidence: `/tmp/axon-dev-telemetry.sock` was timestamped Apr 27 (orphan from a previous session) and was blocking every dev start until I `rm`'d it explicitly. After removal, `start-dev.sh --indexer-full` worked on the very next attempt, indexer pid spawned, IST grew from 0 to 30 MB in seconds.

## Proposed fix

Three layers, all needed for full robustness:

### Layer 1 — Stop scripts must clean up

In `stop-dev.sh`, `stop-live.sh`, `axonctl stop`, and the supervise teardown path:

```bash
# After processes are confirmed dead
rm -f "$AXON_TELEMETRY_SOCK"
rm -f "$AXON_MCP_SOCK"        # same problem class
rm -f "$AXON_PID_FILE"        # if not already done
```

### Layer 2 — Liveness probe instead of file existence

`has_live_runtime_dataplane()` should connect-and-probe the socket, not just check `-S`:

```bash
has_live_runtime_dataplane() {
    # ... existing pid checks ...
    if axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
        if [[ -S "$AXON_TELEMETRY_SOCK" ]] && socket_responds "$AXON_TELEMETRY_SOCK"; then
            return 0
        fi
        return 1
    fi
    # ...
}

socket_responds() {
    # nc -zU "$1"  OR  python -c "import socket; s=socket.socket(socket.AF_UNIX); s.settimeout(0.5); s.connect('$1')"
    nc -zU "$1" 2>/dev/null
}
```

### Layer 3 — Defensive orphan cleanup at start

Before checking for "already up", verify there is a live process owning the socket. If the pid file is missing or points to a dead process, the socket is by definition orphan:

```bash
if [[ -S "$AXON_TELEMETRY_SOCK" ]]; then
    if ! pidfile_alive "$AXON_PID_FILE"; then
        echo "[start] orphan telemetry socket detected (no live owner), removing"
        rm -f "$AXON_TELEMETRY_SOCK"
    fi
fi
```

## Test plan

1. Reproduce: stop dev, manually `touch` the socket file, run `start-dev.sh`. Without the fix it should report Ready and skip launch. With the fix it should detect the orphan, clean it, and proceed.
2. Regression: start dev, kill `-9` the supervise process (simulating a crash), confirm next start cleans up automatically.
3. CI: add an integration test that wraps the start/stop cycle and asserts the socket is gone after stop.

## Don't leave it like this

The user explicitly said: "il faut trouver une solution… on ne le laisse pas comme ça." This bug:
- silently disables dev runtime
- masquerades behind a green "Ready" line
- has cost real wall-clock minutes in this session alone
- aligns with the broader REQ-AXO-087 family of LLM-contract bugs (false-positive readiness signals)

**Severity: high**. Not customer-deliverable as is.
