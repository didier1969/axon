#!/bin/bash
set -euo pipefail

# Axon v2 - Industrial Precision Stop Script
# Kills Axon-related processes while preserving other projects.

PROJECT_ROOT="$(pwd)"
REPO_SLUG="${AXON_REPO_SLUG:-$(basename "$PROJECT_ROOT")}"
if [ -f "$PROJECT_ROOT/.env.worktree" ]; then
    source "$PROJECT_ROOT/.env.worktree"
fi

AXON_ENV="${AXON_ENV:-prod}"
TMUX_SESSION="${TMUX_SESSION:-axon}"

if [ "$AXON_ENV" = "dev" ]; then
    AXON_TCP_PORTS=(44137 44138 44139 44140 44141 44142)
    TMUX_SESSION="axon-dev"
else
    AXON_TCP_PORTS=(44127 44128 44129 44130 44131 44132)
fi

HARD_MODE=0
VERIFY_ONLY=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --hard)
            HARD_MODE=1
            ;;
        --verify)
            VERIFY_ONLY=1
            ;;
        --help|-h)
            cat <<'EOF'
Usage: ./scripts/stop.sh [--hard|--verify]

Options:
  --hard    Force a broader kill pass (patterns + pkill fallback, still best-effort).
  --verify  Audit-only mode: verify Axon not running without killing anything.
EOF
            exit 0
            ;;
        *)
            echo "⚠️  Unknown option: $1"
            echo "   Use --hard or --help."
            exit 1
            ;;
    esac
    shift
done

pid_exists() {
    local pid="$1"
    [ -e "/proc/$pid" ]
}

kill_single_pid() {
    local pid="$1"
    local sig="${2:-TERM}"

    if ! pid_exists "$pid"; then
        return 0
    fi

    kill -"$sig" "$pid" 2>/dev/null || true
}

wait_for_exit_processes() {
    local -n patterns_ref="$1"
    for _ in {1..12}; do
        if [ -z "$(collect_process_pids "$1")" ]; then
            return 0
        fi
        sleep 0.10
    done
    return 1
}

collect_listener_pids() {
    local pids=""
    local port
    local port_pids

    for port in "${AXON_TCP_PORTS[@]}"; do
        port_pids="$(ss -ltnp 2>/dev/null | awk -v p="$port" '
            $1 == "LISTEN" {
                split($4, addr_parts, ":")
                if (addr_parts[length(addr_parts)] != p) {
                    next
                }
                match($0, /pid=([0-9]+)/, m)
                if (m[1] != "") print m[1]
            }' || true)"
        if [ -n "$port_pids" ]; then
            pids="$pids
$port_pids"
        fi
    done

    echo "$pids" | tr ' ' '\n' | awk 'NF' | sort -u
}

collect_process_pids() {
    local -n patterns_ref="$1"
    local pid cmd
    local full_cmd
    local pids=""

    while IFS='|' read -r pid cmd; do
        full_cmd="$cmd"
        if is_axon_process_cmd "$full_cmd"; then
            pids="$pids $pid"
            if [ "${AXON_STOP_DEBUG_MATCH:-0}" = "1" ]; then
                echo "DEBUG_MATCH(pid=$pid): $full_cmd"
            fi
        fi
        unset -v p
    done < <(ps -eo pid=,command= | awk '{pid=$1; $1=""; sub(/^ /,"", $0); if (pid != "") print pid "|" $0}')

    echo "$pids" | tr ' ' '\n' | awk 'NF' | sort -u
}

is_axon_process_cmd() {
    local cmd="$1"

    [[ "$cmd" == *"$PROJECT_ROOT/bin/axon-core"* ]] && return 0
    [[ "$cmd" == *"bin/axon-core"* ]] && return 0
    [[ "$cmd" == *"bin/axon-mcp-tunnel"* ]] && return 0
    [[ "$cmd" == *"_build/esbuild-linux-x64"* ]] && return 0
    [[ "$cmd" == *"_build/tailwind-linux-x64"* ]] && return 0
    [[ "$cmd" == *"axon_nexus@127.0.0.1"* ]] && return 0
    [[ "$cmd" == *"axon_nexus"* && "$cmd" == *"beam.smp"* ]] && return 0
    return 1
}

kill_pids() {
    local pids="$1"
    local label="$2"
    local pid

    if [ -z "$pids" ]; then
        return 0
    fi

    echo "Killing $label process(es): $pids"
    for pid in $pids; do
        kill_single_pid "$pid" "TERM"
    done

    # Fast grace window before escalation to KILL.
    for _ in {1..6}; do
        local alive=0
        for pid in $pids; do
            if pid_exists "$pid"; then
                alive=1
                break
            fi
        done
        if [ "$alive" -eq 0 ]; then
            return 0
        fi
        sleep 0.10
    done

    for pid in $pids; do
        if pid_exists "$pid"; then
            kill_single_pid "$pid" "KILL"
        fi
    done
}

kill_tmux_session() {
    if tmux has-session -t axon 2>/dev/null; then
        echo "Closing TMUX session '$TMUX_SESSION'..."
        tmux kill-session -t "$TMUX_SESSION" 2>/dev/null || true
        for _ in {1..5}; do
            if ! tmux has-session -t axon 2>/dev/null; then
                break
            fi
            sleep 0.10
        done
    fi

    # Fallback in case socket resolution fails inside the current runner context.
    local tmux_fallback_pids
    tmux_fallback_pids="$(ps -eo pid=,cmd= | grep -E 'tmux .*new-session -d -s axon|tmux .* -t axon|tmux .* -s axon' | awk '{print $1}' | sort -u || true)"
    if [ -n "$tmux_fallback_pids" ]; then
        echo "Killing fallback TMUX process(es): $tmux_fallback_pids"
        for pid in $tmux_fallback_pids; do
            kill -15 "$pid" 2>/dev/null || true
        done
        sleep 0.20
        for pid in $tmux_fallback_pids; do
            kill -9 "$pid" 2>/dev/null || true
        done
    fi
}

kill_by_devenv() {
    if command -v devenv >/dev/null 2>&1; then
        echo "Attempting 'devenv processes down' as authoritative cleanup..."
        devenv processes down >/dev/null 2>&1 || true
        sleep 0.20
    fi
}

kill_hard_patterns() {
    local raw
    local pids
    local match_patterns=(
        "bin/axon-core"
        "bin/axon-mcp-tunnel"
        "axon_nexus"
        "_build/esbuild-linux-x64"
        "_build/tailwind-linux-x64"
        "axon-core"
    )

    pids=""
    for pattern in "${match_patterns[@]}"; do
        raw="$(ps -eo pid=,command= | awk -v p="$pattern" 'index($0, p) {print $1}' || true)"
        if [ -n "$raw" ]; then
            pids="$pids
$raw"
        fi
    done

    pids="$(echo "$pids" | tr ' \n' '\n' | awk 'NF' | sort -u | tr '\n' ' ')"
    if [ -n "$pids" ]; then
        echo "Hard-mode: pattern matched process(es): $pids"
        kill_pids "$pids" "hard-mode patterns"
        return
    fi

    # Final hard fallback for stubborn visible names
    for pattern in "bin/axon-core" "axon_nexus@127.0.0.1" "axon_nexus" "axon-core" "_build/esbuild-linux-x64" "_build/tailwind-linux-x64" "beam.smp.*axon_nexus@"; do
        pkill -9 -f "$pattern" 2>/dev/null || true
    done
}

verify_only_exit_if_needed() {
    local patterns_ref="$1"
    local process_pids
    local listener_pids
    local stale=""
    local pid

    process_pids="$(collect_process_pids "$patterns_ref")"
    listener_pids="$(collect_listener_pids)"

    for pid in $listener_pids; do
        if ! pid_exists "$pid"; then
            stale="$stale $pid"
        fi
    done

    if [ -z "$process_pids" ] && [ -z "$listener_pids" ]; then
        echo "✅ Stop verification OK: no visible Axon processes/listeners."
        return 0
    fi

    echo "⚠️ Stop verification failed:"
    [ -n "$process_pids" ] && echo "Process-match pids: $process_pids"
    if [ "${AXON_STOP_DEBUG_MATCH:-0}" = "1" ] && [ -n "$process_pids" ]; then
        echo "Matched process command lines:"
        for pid in $process_pids; do
            ps -p "$pid" -o pid=,cmd= || true
        done
    fi
    [ -n "$listener_pids" ] && echo "Port listener pids: $listener_pids"
    if [ -n "$stale" ]; then
        echo "⚠️ Non-visible/stale listener pids (namespace-shifted): $stale"
    fi
    ss -ltnp 2>/dev/null | rg "4412[7-9]|4413[0-2]" || true
    return 1
}

echo "🛑 Stopping Axon v2 Architecture (Chirurgical Mode)..."

# 1. Axon process signatures for checks and teardown.
PATTERNS=(
    "$PROJECT_ROOT/bin/axon-core"
    "bin/axon-core"
    "bin/axon-mcp-tunnel"
    "axon-core"
    "axon_nexus"
    "axon_nexus@127.0.0.1"
    "$PROJECT_ROOT/src/dashboard/_build/esbuild-linux-x64"
    "$PROJECT_ROOT/src/dashboard/_build/tailwind-linux-x64"
    "src/dashboard/_build/esbuild-linux-x64"
    "src/dashboard/_build/tailwind-linux-x64"
)

if [ "$VERIFY_ONLY" = "1" ]; then
    verify_only_exit_if_needed PATTERNS
    exit $?
fi

# 2. Graceful Elixir shutdown via RPC (if node is named)
if command -v elixir >/dev/null 2>&1; then
    echo "Sending shutdown signal to Axon Nexus node..."
    elixir --name stop_script@127.0.0.1 --cookie axon_secret --rpc "axon_nexus@127.0.0.1" :init :stop >/dev/null 2>&1 || true
    sleep 0.20
fi

# 3. Close TMUX session (primary path + fallback)
kill_tmux_session

# 4. Kill lingering Axon processes by direct patterns.
PROCESS_PIDS="$(collect_process_pids PATTERNS)"
kill_pids "$PROCESS_PIDS" "pattern-matching"
if [ -n "$PROCESS_PIDS" ]; then
    wait_for_exit_processes PATTERNS || true
fi

if [ "$HARD_MODE" = "1" ]; then
    kill_hard_patterns
    wait_for_exit_processes PATTERNS || true
fi

# 3b. Kill lingering processes still bound to Axon TCP ports (authoritative cleanup path)
PORT_PIDS="$(collect_listener_pids)"
ALIVE_PORT_PIDS=""
STALE_PORT_PIDS=""

if [ -n "$PORT_PIDS" ]; then
    for pid in $PORT_PIDS; do
        if pid_exists "$pid"; then
            ALIVE_PORT_PIDS="$ALIVE_PORT_PIDS $pid"
        else
            STALE_PORT_PIDS="$STALE_PORT_PIDS $pid"
        fi
    done

    if [ -n "$ALIVE_PORT_PIDS" ]; then
        kill_pids "$ALIVE_PORT_PIDS" "port-listening"
    fi
fi

# 4. Clean up sockets, ports and locks (final safety net)
echo "Cleaning up sockets, ports and locks..."
for port in "${AXON_TCP_PORTS[@]}"; do
    fuser -k "${port}/tcp" 2>/dev/null || true &
done
wait || true
rm -f /tmp/axon-mcp.sock /tmp/axon-telemetry.sock /tmp/axon-v2.sock
rm -f "$PROJECT_ROOT/.axon/graph_v2/"*.lock

if [[ "${AXON_DROP_WAL_ON_STOP:-0}" == "1" ]]; then
    echo "⚠️ AXON_DROP_WAL_ON_STOP=1 set: deleting DuckDB WAL files during stop."
    rm -f "$PROJECT_ROOT/.axon/graph_v2/"*.wal
fi

# 5. Final verification
AFTER_PATTERN_PIDS="$(collect_process_pids PATTERNS)"
AFTER_PORT_PIDS="$(collect_listener_pids)"
AFTER_STALE=""

if [ -n "$AFTER_PORT_PIDS" ]; then
    for pid in $AFTER_PORT_PIDS; do
        if pid_exists "$pid"; then
            :
        else
            AFTER_STALE="$AFTER_STALE $pid"
        fi
    done
fi

if [ -n "$AFTER_PATTERN_PIDS" ] || [ -n "$AFTER_PORT_PIDS" ]; then
    if [ -n "$AFTER_PORT_PIDS" ]; then
        echo "⚠️ Port listeners still present after cleanup:"
        ss -ltnp 2>/dev/null | rg "4412[7-9]|4413[0-2]" || true
    fi
    if [ -n "$AFTER_STALE" ]; then
        echo "⚠️ Some listener PIDs are stale/non-visible from this execution context: $AFTER_STALE"
        echo "   This usually means the listener process runs in another PID namespace/runner."
        echo "   Run from host context: pkill -f 'axon-core|axon_nexus|bin/axon-core|_build/esbuild|_build/tailwind'"
        echo "   or run: devenv processes down"
    fi
fi

if [ -n "$AFTER_PATTERN_PIDS" ] || [ -n "$AFTER_PORT_PIDS" ]; then
    echo "⚠️ Stopping still detected after first cleanup. Retrying with process supervisor..."
    kill_by_devenv
    kill_tmux_session
    wait_for_exit_processes PATTERNS || true
    if [ "$HARD_MODE" = "1" ]; then
        kill_hard_patterns
        wait_for_exit_processes PATTERNS || true
    fi
    AFTER_PATTERN_PIDS="$(collect_process_pids PATTERNS)"
    AFTER_PORT_PIDS="$(collect_listener_pids)"
fi

if [ -n "$AFTER_PORT_PIDS" ]; then
    STALE_PORT_PIDS=""
    for pid in $AFTER_PORT_PIDS; do
        if pid_exists "$pid"; then
            :
        else
            STALE_PORT_PIDS="$STALE_PORT_PIDS $pid"
        fi
    done
fi

if tmux has-session -t axon 2>/dev/null; then
    echo "⚠️ TMUX session 'axon' still present after cleanup."
    echo "   If this is stale, please run: tmux kill-session -t axon"
    exit 1
fi

if [ -n "$AFTER_PATTERN_PIDS" ] || [ -n "$AFTER_PORT_PIDS" ]; then
    echo "⚠️ Axon-related processes still running after cleanup."
    [ -n "$AFTER_PATTERN_PIDS" ] && echo "Pattern-match pids: $AFTER_PATTERN_PIDS"
    [ -n "$AFTER_PORT_PIDS" ] && echo "Port listeners pids: $AFTER_PORT_PIDS"
    if [ -n "${STALE_PORT_PIDS:-}" ]; then
        echo "⚠️ Non-visible listener pids (likely namespace-isolated):$STALE_PORT_PIDS"
    fi
    exit 1
fi

echo "✅ Axon stopped (Other projects preserved)."
