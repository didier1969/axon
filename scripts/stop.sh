#!/bin/bash
set -euo pipefail

# Axon v2 - Industrial Precision Stop Script
# Kills Axon-related processes while preserving other projects.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_SLUG="${AXON_REPO_SLUG:-$(basename "$PROJECT_ROOT")}"
# shellcheck source=scripts/lib/axon-instance.sh
source "$PROJECT_ROOT/scripts/lib/axon-instance.sh"
# shellcheck source=scripts/lib/axon-role-layout.sh
source "$PROJECT_ROOT/scripts/lib/axon-role-layout.sh"
axon_load_worktree_env "$PROJECT_ROOT"
axon_resolve_instance "$PROJECT_ROOT" "$REPO_SLUG"
axon_apply_runtime_role_layout "$PROJECT_ROOT" "${AXON_RUNTIME_SHADOW_ROLE:-${AXON_RUNTIME_BOOT_ROLE:-legacy_monolith}}"
if [[ -f "$AXON_RUNTIME_STATE_FILE" ]]; then
    # shellcheck disable=SC1090
    source "$AXON_RUNTIME_STATE_FILE"
fi

AXON_TCP_PORTS=("$PHX_PORT" "$HYDRA_TCP_PORT" "$HYDRA_HTTP_PORT" "$HYDRA_ODATA_PORT" "$HYDRA_HTTP2_PORT" "$HYDRA_MCP_PORT")
case "${AXON_RUNTIME_SHADOW_ROLE:-${AXON_RUNTIME_BOOT_ROLE:-}}" in
    indexer|indexer_shadow)
        AXON_TCP_PORTS=()
        ;;
esac

HARD_MODE=0
VERIFY_ONLY=0

port_regex() {
    local port
    local first=1
    local pattern=""
    for port in "${AXON_TCP_PORTS[@]}"; do
        if [[ "$first" -eq 1 ]]; then
            pattern="$port"
            first=0
        else
            pattern="${pattern}|${port}"
        fi
    done
    printf '%s\n' "$pattern"
}

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

selected_writer_guards() {
    case "${AXON_RUNTIME_SHADOW_ROLE:-${AXON_RUNTIME_BOOT_ROLE:-legacy_monolith}}" in
        brain|brain_shadow)
            printf 'SOLL %s\n' "$AXON_DB_ROOT/.axon-soll.writer.lock"
            ;;
        indexer|indexer_shadow)
            printf 'IST %s\n' "$AXON_DB_ROOT/.axon-ist.writer.lock"
            ;;
        *)
            printf 'SOLL %s\n' "$AXON_DB_ROOT/.axon-soll.writer.lock"
            printf 'IST %s\n' "$AXON_DB_ROOT/.axon-ist.writer.lock"
            ;;
    esac
}

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

    [[ "${#AXON_TCP_PORTS[@]}" -gt 0 ]] || return 0

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

primary_listener_pid() {
    [[ "${#AXON_TCP_PORTS[@]}" -gt 0 ]] || return 0
    ss -ltnp 2>/dev/null | awk -v p="$HYDRA_HTTP_PORT" '
        $1 == "LISTEN" {
            split($4, addr_parts, ":")
            if (addr_parts[length(addr_parts)] != p) {
                next
            }
            match($0, /pid=([0-9]+)/, m)
            if (m[1] != "") {
                print m[1]
                exit
            }
        }' || true
}

pid_matches_instance() {
    local pid="$1"
    local cmdline=""
    local listener_pid=""
    local runtime_binary_name="axon-core"

    [[ -n "$pid" && -e "/proc/$pid" ]] || return 1
    cmdline="$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null || true)"
    case "${AXON_RUNTIME_SHADOW_ROLE:-${AXON_RUNTIME_BOOT_ROLE:-}}" in
        brain|brain_shadow)
            runtime_binary_name="axon-brain"
            ;;
        indexer|indexer_shadow)
            runtime_binary_name="axon-indexer"
            ;;
    esac
    [[ "$cmdline" == *"$runtime_binary_name"* || "$cmdline" == *"axon-core"* ]] || return 1

    if [[ "$runtime_binary_name" == "axon-indexer" ]]; then
        return 0
    fi

    listener_pid="$(primary_listener_pid)"
    [[ -n "$listener_pid" && "$listener_pid" == "$pid" ]]
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
    local runtime_binary_name="axon-core"
    case "${AXON_RUNTIME_SHADOW_ROLE:-${AXON_RUNTIME_BOOT_ROLE:-}}" in
        brain|brain_shadow)
            runtime_binary_name="axon-brain"
            ;;
        indexer|indexer_shadow)
            runtime_binary_name="axon-indexer"
            ;;
    esac

    # Ignore the tmux/devenv launcher shell that merely waits on the real
    # runtime child. It can survive briefly around shutdown and should not be
    # treated as the runtime process itself during stop verification.
    if [[ "$cmd" == *"bash -lc "* && "$cmd" == *'wait $core_pid'* ]]; then
        return 1
    fi
    
    # We must ONLY kill instance-qualified auxiliary processes. The shared
    # runtime binary path is not sufficient to identify live vs dev.
    if [[ "$cmd" == *"$runtime_binary_name"* && "$cmd" == *"$PROJECT_ROOT"* ]]; then
        return 0
    fi
    if [[ "$cmd" == *"$PROJECT_ROOT"* ]]; then
        [[ "$cmd" == *"_build/esbuild"* ]] && return 0
        [[ "$cmd" == *"_build/tailwind"* ]] && return 0
        [[ "$cmd" == *"${ELIXIR_NODE_NAME}"* && "$cmd" == *"beam.smp"* ]] && return 0
    fi
    
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
    if tmux has-session -t "$TMUX_SESSION" 2>/dev/null; then
        echo "Closing TMUX session '$TMUX_SESSION'..."
        tmux kill-session -t "$TMUX_SESSION" 2>/dev/null || true
        for _ in {1..5}; do
            if ! tmux has-session -t "$TMUX_SESSION" 2>/dev/null; then
                break
            fi
            sleep 0.10
        done
    fi

    # Fallback in case socket resolution fails inside the current runner context.
    local tmux_fallback_pids
    tmux_fallback_pids="$(ps -eo pid=,cmd= | grep -E "tmux .*new-session -d -s ${TMUX_SESSION}|tmux .* -t ${TMUX_SESSION}|tmux .* -s ${TMUX_SESSION}" | awk '{print $1}' | sort -u || true)"
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
    if [[ "${AXON_ALLOW_BROAD_STOP:-0}" != "1" ]]; then
        echo "Skipping 'devenv processes down' because it is not instance-safe by default."
        return 0
    fi
    if command -v devenv >/dev/null 2>&1; then
        echo "Attempting 'devenv processes down' as authoritative cleanup..."
        devenv processes down >/dev/null 2>&1 || true
        sleep 0.20
    fi
}

verify_writer_guard_release() {
    local label="$1"
    local lock_path="$2"
    local strict_missing="${3:-0}"
    local owner_pid=""
    local guard_fd=""

    if [[ ! -f "$lock_path" ]]; then
        if [[ "$strict_missing" == "1" ]]; then
            echo "❌ $label writer guard lockfile missing; release cannot be verified ($lock_path)"
            return 1
        fi
        echo "⚠️ $label writer guard lockfile missing after shutdown ($lock_path)"
        return 0
    fi

    if ! command -v flock >/dev/null 2>&1; then
        if [[ "$strict_missing" == "1" ]]; then
            echo "❌ flock unavailable; cannot strictly verify $label writer guard release ($lock_path)"
            return 1
        fi
        echo "⚠️ flock not available; cannot verify $label writer guard release ($lock_path)"
        return 0
    fi

    owner_pid="$(sed -n 's/^owner=.*;pid=\([0-9]\+\)$/\1/p' "$lock_path" 2>/dev/null | head -n1 || true)"
    for _ in {1..20}; do
        exec {guard_fd}<>"$lock_path"
        if flock -n "$guard_fd"; then
            echo "✅ $label writer guard released ($lock_path)"
            flock -u "$guard_fd" || true
            exec {guard_fd}>&-
            return 0
        fi
        exec {guard_fd}>&-
        sleep 0.10
    done

    if [[ -n "$owner_pid" ]] && ! pid_exists "$owner_pid"; then
        exec {guard_fd}<>"$lock_path"
        if flock -n "$guard_fd"; then
            echo "✅ $label writer guard released after stale owner cleanup ($lock_path)"
            flock -u "$guard_fd" || true
            exec {guard_fd}>&-
            return 0
        fi
        exec {guard_fd}>&-
        echo "⚠️ $label writer guard lockfile is stale; recorded owner pid=$owner_pid is no longer alive ($lock_path)"
        return 0
    fi

    echo "❌ $label writer guard still held after shutdown ($lock_path)"
    return 1
}

kill_hard_patterns() {
    local raw
    local pids
    local match_patterns=(
        "${ELIXIR_NODE_NAME}"
        "$PROJECT_ROOT/src/dashboard/_build/esbuild"
        "$PROJECT_ROOT/src/dashboard/_build/tailwind"
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
    for pattern in "${ELIXIR_NODE_NAME}@127.0.0.1" "$PROJECT_ROOT/src/dashboard/_build"; do
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
        local guard_failed=0
        while read -r guard_label guard_path; do
            [[ -n "${guard_label:-}" ]] || continue
            verify_writer_guard_release "$guard_label" "$guard_path" 1 || guard_failed=1
        done < <(selected_writer_guards)
        if [ "$guard_failed" -eq 1 ]; then
            return 1
        fi
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
    ss -ltnp 2>/dev/null | rg "$(port_regex)" || true
    return 1
}

echo "🛑 Stopping Axon v2 Architecture (Chirurgical Mode)..."
echo "Shutdown order: RPC -> TMUX -> tracked processes -> listener cleanup -> writer-guard verification"
if [[ "${AXON_SPLIT_SHADOW_ONLY:-0}" == "1" ]]; then
    echo "Split rollback note: stop must fully release SOLL/IST writer ownership before monolith reactivation."
fi

# 1. Axon process signatures for checks and teardown.
PATTERNS=(
    "$ELIXIR_NODE_NAME"
    "$ELIXIR_NODE_NAME@127.0.0.1"
    "$PROJECT_ROOT/src/dashboard/_build/esbuild-linux-x64"
    "$PROJECT_ROOT/src/dashboard/_build/tailwind-linux-x64"
    "src/dashboard/_build/esbuild-linux-x64"
    "src/dashboard/_build/tailwind-linux-x64"
)

if [ "$VERIFY_ONLY" = "1" ]; then
    verify_only_exit_if_needed PATTERNS
    exit $?
fi

# 1b. Kill the tracked core pid first when available.
if [[ -f "$AXON_PID_FILE" ]]; then
    TRACKED_PID="$(cat "$AXON_PID_FILE" 2>/dev/null || true)"
    if pid_matches_instance "${TRACKED_PID:-}"; then
        kill_pids "$TRACKED_PID" "tracked core"
    fi
fi

# 2. Graceful Elixir shutdown via RPC (if node is named)
if command -v elixir >/dev/null 2>&1; then
    echo "Sending shutdown signal to Axon Nexus node..."
    elixir --name stop_script@127.0.0.1 --cookie axon_secret --rpc "${ELIXIR_NODE_NAME}@127.0.0.1" :init :stop >/dev/null 2>&1 || true
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

# 4. Clean up sockets and ports. Writer guard lockfiles are intentionally left
# on disk; the kernel lock is released with process exit and the file content is
# useful for operator diagnostics on the next startup.
echo "Cleaning up sockets and ports..."
for port in "${AXON_TCP_PORTS[@]}"; do
    fuser -k "${port}/tcp" 2>/dev/null || true &
done
wait || true
rm -f "$AXON_MCP_SOCK" "$AXON_TELEMETRY_SOCK" "$AXON_PID_FILE" "$AXON_RUNTIME_STATE_FILE" /tmp/axon-v2.sock

WRITER_GUARD_RELEASE_FAILED=0
STRICT_GUARD_RELEASE=0
if [[ "${AXON_SPLIT_SHADOW_ONLY:-0}" == "1" ]]; then
    STRICT_GUARD_RELEASE=1
fi
while read -r guard_label guard_path; do
    [[ -n "${guard_label:-}" ]] || continue
    verify_writer_guard_release "$guard_label" "$guard_path" "$STRICT_GUARD_RELEASE" || WRITER_GUARD_RELEASE_FAILED=1
done < <(selected_writer_guards)

if [[ "${AXON_DROP_WAL_ON_STOP:-0}" == "1" ]]; then
    echo "⚠️ AXON_DROP_WAL_ON_STOP=1 set: deleting DuckDB WAL files during stop."
    rm -f "$AXON_DB_ROOT/"*.wal
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
        ss -ltnp 2>/dev/null | rg "$(port_regex)" || true
    fi
    if [ -n "$AFTER_STALE" ]; then
        echo "⚠️ Some listener PIDs are stale/non-visible from this execution context: $AFTER_STALE"
        echo "   This usually means the listener process runs in another PID namespace/runner."
        echo "   Run from host context: pkill -f 'axon-core|axon-brain|axon-indexer|$ELIXIR_NODE_NAME|bin/axon-core|_build/esbuild|_build/tailwind'"
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

if tmux has-session -t "$TMUX_SESSION" 2>/dev/null; then
    echo "⚠️ TMUX session '$TMUX_SESSION' still present after cleanup."
    echo "   If this is stale, please run: tmux kill-session -t $TMUX_SESSION"
    exit 1
fi

if [[ "$WRITER_GUARD_RELEASE_FAILED" -ne 0 ]]; then
    echo "⚠️ Writer guards still held after stop; rollback/reactivation is blocked until the runtime exits cleanly."
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
