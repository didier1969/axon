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

# Determine role: use explicit env if set, otherwise stop ALL roles to
# guarantee cleanup even when the instance is already down and env vars
# are absent (the default fallback in axon_runtime_shadow_role is
# "indexer" which would miss a stopped brain).
if [[ -n "${AXON_RUNTIME_SHADOW_ROLE:-}" || -n "${AXON_RUNTIME_BOOT_ROLE:-}" ]]; then
    STOP_ROLE="$(axon_runtime_shadow_role)"
else
    STOP_ROLE="all"
fi

# For role-specific layout setup, resolve a concrete role (brain for "all"
# since we only need the layout for state file and port detection).
_LAYOUT_ROLE="$STOP_ROLE"
if [[ "$_LAYOUT_ROLE" == "all" ]]; then
    _LAYOUT_ROLE="brain"
fi
axon_apply_runtime_role_layout "$PROJECT_ROOT" "$_LAYOUT_ROLE"
if [[ -f "$AXON_RUNTIME_STATE_FILE" ]]; then
    # shellcheck disable=SC1090
    source "$AXON_RUNTIME_STATE_FILE"
fi

if [[ "$STOP_ROLE" == "all" ]]; then
    # Brain ports — indexer has no TCP listeners
    AXON_TCP_PORTS=("$PHX_PORT" "$HYDRA_TCP_PORT" "$HYDRA_HTTP_PORT" "$HYDRA_ODATA_PORT" "$HYDRA_HTTP2_PORT" "$HYDRA_MCP_PORT")
elif axon_role_is_indexer "$STOP_ROLE"; then
    AXON_TCP_PORTS=()
else
    AXON_TCP_PORTS=("$PHX_PORT" "$HYDRA_TCP_PORT" "$HYDRA_HTTP_PORT" "$HYDRA_ODATA_PORT" "$HYDRA_HTTP2_PORT" "$HYDRA_MCP_PORT")
fi

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
    if [[ "$STOP_ROLE" == "all" ]]; then
        printf 'SOLL %s\n' "$AXON_DB_ROOT/.axon-soll.writer.lock"
        printf 'IST %s\n' "$AXON_DB_ROOT/.axon-ist.writer.lock"
    elif axon_role_is_brain "$STOP_ROLE"; then
        printf 'SOLL %s\n' "$AXON_DB_ROOT/.axon-soll.writer.lock"
    elif axon_role_is_indexer "$STOP_ROLE"; then
        printf 'IST %s\n' "$AXON_DB_ROOT/.axon-ist.writer.lock"
    else
        printf 'SOLL %s\n' "$AXON_DB_ROOT/.axon-soll.writer.lock"
        printf 'IST %s\n' "$AXON_DB_ROOT/.axon-ist.writer.lock"
    fi
}

pid_exists() {
    local pid="$1"
    [ -e "/proc/$pid" ]
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
    local runtime_binary_names=()
    if [[ "$STOP_ROLE" == "all" ]]; then
        runtime_binary_names+=("$(axon_runtime_binary_name brain)")
        runtime_binary_names+=("$(axon_runtime_binary_name indexer)")
    else
        runtime_binary_names+=("$(axon_runtime_binary_name "$STOP_ROLE")")
    fi

    # Ignore the tmux/devenv launcher shell that merely waits on the real
    # runtime child. It can survive briefly around shutdown and should not be
    # treated as the runtime process itself during stop verification.
    if [[ "$cmd" == *"bash -lc "* && "$cmd" == *'wait $core_pid'* ]]; then
        return 1
    fi

    # We must ONLY kill instance-qualified auxiliary processes. The shared
    # runtime binary path is not sufficient to identify live vs dev.
    local runtime_binary_name
    for runtime_binary_name in "${runtime_binary_names[@]}"; do
        if [[ "$cmd" == *"$runtime_binary_name"* && "$cmd" == *"$PROJECT_ROOT"* ]]; then
            return 0
        fi
    done
    if [[ "$cmd" == *"$PROJECT_ROOT"* ]]; then
        [[ "$cmd" == *"_build/esbuild"* ]] && return 0
        [[ "$cmd" == *"_build/tailwind"* ]] && return 0
    fi
    # Match BEAM processes by node name regardless of PROJECT_ROOT in cmdline.
    # Orphaned beam.smp processes may lose the project root from their cmdline
    # but always retain the Erlang node name.
    if [[ "$cmd" == *"beam.smp"* && "$cmd" == *"${ELIXIR_NODE_NAME}"* ]]; then
        return 0
    fi

    return 1
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
            # After a clean shutdown, the runtime may leave no lockfile behind.
            # Verification should accept both "released existing guard" and
            # "guard file absent because nothing still owns it".
            verify_writer_guard_release "$guard_label" "$guard_path" 0 || guard_failed=1
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

if [ "$VERIFY_ONLY" = "1" ]; then
    PATTERNS=(
        "$ELIXIR_NODE_NAME"
        "$ELIXIR_NODE_NAME@127.0.0.1"
    )
    verify_only_exit_if_needed PATTERNS
    exit $?
fi

# Graceful Elixir shutdown via RPC before axonctl takes over process killing.
if command -v elixir >/dev/null 2>&1; then
    echo "Sending shutdown signal to Axon Nexus node..."
    elixir --name stop_script@127.0.0.1 --cookie axon_secret --rpc "${ELIXIR_NODE_NAME}@127.0.0.1" :init :stop >/dev/null 2>&1 || true
    sleep 0.20
fi

# Delegate all process termination, lock cleanup, and verification to axonctl.
AXONCTL_BIN="$PROJECT_ROOT/bin/axonctl"
if [[ ! -x "$AXONCTL_BIN" ]]; then
    # Fallback: try cargo target
    AXONCTL_BIN="$PROJECT_ROOT/src/axon-core/target/release/axonctl"
fi

if [[ -x "$AXONCTL_BIN" ]]; then
    AXONCTL_ARGS=(
        stop
        --project-root "$PROJECT_ROOT"
        --instance-kind "$AXON_INSTANCE_KIND"
        --role "$STOP_ROLE"
    )
    if [ "$HARD_MODE" = "1" ]; then
        AXONCTL_ARGS+=(--hard)
    fi
    "$AXONCTL_BIN" "${AXONCTL_ARGS[@]}" && AXONCTL_OK=1 || AXONCTL_OK=0

    if [[ "${AXON_DROP_WAL_ON_STOP:-0}" == "1" ]]; then
        echo "⚠️ AXON_DROP_WAL_ON_STOP=1 set: deleting DuckDB WAL files during stop."
        rm -f "$AXON_DB_ROOT/"*.wal
    fi

    if [ "$AXONCTL_OK" = "1" ]; then
        echo "✅ Axon stopped (Other projects preserved)."
        exit 0
    else
        echo "⚠️ axonctl stop reported remaining processes."
        exit 1
    fi
else
    echo "❌ axonctl binary not found at $AXONCTL_BIN"
    echo "   Build it: cargo build --manifest-path src/axon-core/Cargo.toml --release --bin axonctl"
    exit 1
fi
