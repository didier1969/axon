#!/bin/bash
# Axon live auto-start guard + autonomous indexer self-heal.
#
# Idempotent: starts the FULL live instance (brain + indexer-full) only when the
# brain MCP port is not already accepting connections. Safe to call repeatedly.
#
# REQ-AXO-902277 — it ALSO owns the indexer restart-budget regeneration. When the
# brain is up but process-compose has ABANDONED the indexer (max_restarts spent
# after repeated TensorRT hangs — the 2026-08-06 ~2-day silent outage), this
# guard restarts it within a rolling window, out-of-band of the very supervisor
# that gave up. Two triggers: (1) event-driven, on every invocation below;
# (2) a detached `--watch` loop it keeps alive, for the operator-away idle case.
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
# REQ-AXO-902277 — self-heal watcher (detached periodic loop).
WATCH_INTERVAL="${AXON_SELF_HEAL_INTERVAL:-120}"
WATCH_LOCK="/tmp/axon-self-heal-watch.lock"
WATCH_PIDFILE="$PROJECT_ROOT/.axon/self-heal-watch.pid"
SUPERVISOR_LIB="$PROJECT_ROOT/scripts/lib/axon-supervisor.sh"
PAUSE_FILE="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/nexus-admission/axon-indexer.paused"

ts() { date '+%Y-%m-%d %H:%M:%S'; }
mkdir -p "$(dirname "$LOG")"

# REQ-AXO-902536 — a session is never the runtime owner.  The historical
# self-heal inherited whichever tmux/Codex cgroup made the first MCP call, so a
# later OOM killed the service together with that interactive session.  Calls
# outside the owning unit now delegate to systemd; AXON_SYSTEMD_OWNER breaks
# the intentional recursion when axon-live.service invokes this script.
if [[ "${1:-}" != "--watch" && "${AXON_SYSTEMD_OWNER:-0}" != "1" ]] \
    && command -v systemctl >/dev/null 2>&1 \
    && systemctl --user cat axon-live.service >/dev/null 2>&1; then
    if (exec 3<>"/dev/tcp/127.0.0.1/$BRAIN_PORT") 2>/dev/null; then
        exec 3>&- 2>/dev/null || true
        systemctl --user start --no-block axon-self-heal.service >/dev/null 2>&1 || true
        exit 0
    fi
    systemctl --user restart axon-live.service
    systemctl --user start --no-block axon-self-heal.service >/dev/null 2>&1 || true
    exit 0
fi

# --------------------------------------------------------------------------
# REQ-AXO-902277 — `--watch` mode: the periodic self-heal loop. Handled BEFORE
# the bootstrap lock below (it must NOT hold that lock — it is long-lived). Lives
# OUTSIDE process-compose on purpose: a healer inside the supervisor it heals
# shares that supervisor's abandonment bug (advisor guidance).
# --------------------------------------------------------------------------
if [[ "${1:-}" == "--watch" ]]; then
    exec 8>"$WATCH_LOCK"
    if ! flock -n 8; then
        echo "[$(ts)] self-heal watcher already running; exiting" >>"$LOG"
        exit 0
    fi
    echo "$$" > "$WATCH_PIDFILE"
    trap 'rm -f "$WATCH_PIDFILE" 2>/dev/null || true' EXIT
    echo "[$(ts)] self-heal watcher started (interval=${WATCH_INTERVAL}s)" >>"$LOG"
    # shellcheck source=lib/axon-supervisor.sh
    if ! source "$SUPERVISOR_LIB" 2>>"$LOG"; then
        echo "[$(ts)] cannot source $SUPERVISOR_LIB — watcher aborting" >>"$LOG"
        exit 1
    fi
    while :; do
        if [[ -f "$PAUSE_FILE" ]]; then
            echo "[$(ts)] indexer self-heal inhibited by admission pressure" >>"$LOG"
            sleep "$WATCH_INTERVAL"
            continue
        fi
        # A DOWN supervisor yields rc 2 (no-op): the watcher never fights an
        # intentional stop, it only restarts a role the supervisor abandoned.
        axon_self_heal_indexer "$PROJECT_ROOT" live >>"$LOG" 2>&1 || true
        # REQ-AXO-902348/902332 — same survey layer records WHY any role exited,
        # so `axon status` can report the cause. Best-effort, never breaks the loop.
        axon_persist_role_exit_events "$PROJECT_ROOT" live >>"$LOG" 2>&1 || true
        sleep "$WATCH_INTERVAL"
    done
fi

# Spawn the detached watcher if it is not already running. Best-effort liveness
# check via the pidfile; the watcher's own flock is the true single-instance
# guard, so a lost race just spawns a process that immediately exits.
ensure_watcher() {
    # systemd owns the watcher on the workstation.  The unit is enabled and an
    # external self-heal call starts it with --no-block; never detach a second
    # watcher from inside axon-live.service.
    if [[ "${AXON_SYSTEMD_OWNER:-0}" == "1" ]] \
        && systemctl --user cat axon-self-heal.service >/dev/null 2>&1; then
        return 0
    fi
    local wpid=""
    [[ -f "$WATCH_PIDFILE" ]] && wpid="$(cat "$WATCH_PIDFILE" 2>/dev/null)"
    if [[ -n "$wpid" ]] && kill -0 "$wpid" 2>/dev/null; then
        return 0
    fi
    # Close the bootstrap flock descriptor explicitly.  A detached fallback
    # watcher inheriting fd 9 would otherwise keep the one-shot lock alive
    # after its parent exits and make every later bootstrap a no-op.
    setsid bash "$PROJECT_ROOT/scripts/ensure-axon-running.sh" --watch \
        9>&- >>"$LOG" 2>&1 < /dev/null &
    disown 2>/dev/null || true
    echo "[$(ts)] spawned self-heal watcher" >>"$LOG"
}

# One-shot indexer self-heal (event-driven). Best-effort: a missing lib or a DOWN
# supervisor is a no-op, never an abort.
heal_indexer_once() {
    [[ -f "$PAUSE_FILE" ]] && {
        echo "[$(ts)] one-shot indexer heal inhibited by admission pressure" >>"$LOG"
        return 0
    }
    [[ -f "$SUPERVISOR_LIB" ]] || return 0
    # shellcheck source=lib/axon-supervisor.sh
    source "$SUPERVISOR_LIB" 2>>"$LOG" || return 0
    axon_self_heal_indexer "$PROJECT_ROOT" live >>"$LOG" 2>&1 || true
    # REQ-AXO-902348/902332 — record WHY any role exited (survives the role's death).
    axon_persist_role_exit_events "$PROJECT_ROOT" live >>"$LOG" 2>&1 || true
}

# Serialize concurrent invocations (WSL boot and an MCP reconnect can race).
exec 9>"$LOCK"
if ! flock -n 9; then
    echo "[$(ts)] another ensure run holds the lock; exiting" >>"$LOG"
    exit 0
fi

# Fast path: brain already accepting TCP connections -> the runtime is up, so the
# only thing that can still be wrong is an abandoned indexer. Heal it (REQ-AXO-
# 902277) and make sure the periodic watcher is alive, then done.
if (exec 3<>"/dev/tcp/127.0.0.1/$BRAIN_PORT") 2>/dev/null; then
    exec 3>&- 2>/dev/null || true
    echo "[$(ts)] brain already up on :$BRAIN_PORT, checking indexer self-heal" >>"$LOG"
    heal_indexer_once
    ensure_watcher
    exit 0
fi

BOOT_MODE="${AXON_BOOT_MODE:---indexer-full}"
if [[ -f "$PAUSE_FILE" && "$BOOT_MODE" != "--brain-only" ]]; then
    echo "[$(ts)] admission pressure marker present; booting brain-only" >>"$LOG"
    BOOT_MODE="--brain-only"
fi
echo "[$(ts)] brain DOWN on :$BRAIN_PORT -> starting live ($BOOT_MODE)" >>"$LOG"
cd "$PROJECT_ROOT" || { echo "[$(ts)] cannot cd to $PROJECT_ROOT" >>"$LOG"; exit 1; }

# Login-shell PATH carries the nix profile; start.sh self-enters devenv.
bash scripts/axon --instance live start "$BOOT_MODE" >>"$LOG" 2>&1
rc=$?
echo "[$(ts)] start exited rc=$rc" >>"$LOG"

# A fresh start brings the indexer up with a clean budget, so no heal is needed
# now — but keep the periodic watcher alive for the next abandonment.
ensure_watcher
exit "$rc"
