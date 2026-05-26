#!/bin/bash
set -euo pipefail

# Axon v2 - Industrial Precision Stop Script
# Kills Axon-related processes while preserving other projects.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_SLUG="${AXON_REPO_SLUG:-$(basename "$PROJECT_ROOT")}"
# shellcheck source=scripts/lib/axon-instance.sh
source "$PROJECT_ROOT/scripts/lib/axon-instance.sh"
# Preserve AXON_INSTANCE_KIND across env sanitization (same fix as start.sh).
_SAVED_INSTANCE_KIND="${AXON_INSTANCE_KIND:-}"
axon_clear_inherited_env
if [[ -n "$_SAVED_INSTANCE_KIND" ]]; then
    export AXON_INSTANCE_KIND="$_SAVED_INSTANCE_KIND"
fi
unset _SAVED_INSTANCE_KIND
# shellcheck source=scripts/lib/axon-role-layout.sh
source "$PROJECT_ROOT/scripts/lib/axon-role-layout.sh"
# shellcheck source=scripts/lib/socket-lifecycle.sh
source "$PROJECT_ROOT/scripts/lib/socket-lifecycle.sh"
axon_load_worktree_env "$PROJECT_ROOT"
axon_resolve_instance "$PROJECT_ROOT" "$REPO_SLUG"

# REQ-AXO-209 — explicit role-scoped stop. Operator wants
# `./scripts/axon-{live,dev} stop --role indexer` to stop ONLY the
# indexer (brain stays up + MCP responsive), and `--role brain` to
# stop ONLY the brain (indexer keeps indexing). CLI flag takes
# priority over the env-driven defaults below.
CLI_ROLE_OVERRIDE=""
_pending_args=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --role)
            if [[ $# -lt 2 ]]; then
                echo "stop.sh: --role requires brain|indexer|all" >&2
                exit 2
            fi
            CLI_ROLE_OVERRIDE="$2"
            shift 2
            ;;
        --role=*)
            CLI_ROLE_OVERRIDE="${1#*=}"
            shift
            ;;
        *)
            _pending_args+=("$1")
            shift
            ;;
    esac
done
# Restore unconsumed args for the post-resolution argument loop below.
set -- "${_pending_args[@]+"${_pending_args[@]}"}"

# Determine role: CLI override > explicit env > default ALL.
# axon_runtime_shadow_role's "indexer" fallback would miss a stopped
# brain when no role context is available, so the default stays "all".
if [[ -n "$CLI_ROLE_OVERRIDE" ]]; then
    case "$CLI_ROLE_OVERRIDE" in
        brain|indexer|all)
            STOP_ROLE="$CLI_ROLE_OVERRIDE"
            ;;
        *)
            echo "stop.sh: --role must be brain|indexer|all (got '$CLI_ROLE_OVERRIDE')" >&2
            exit 2
            ;;
    esac
elif [[ -n "${AXON_RUNTIME_SHADOW_ROLE:-}" || -n "${AXON_RUNTIME_BOOT_ROLE:-}" ]]; then
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

# DEC-AXO-901598 + REQ-AXO-901636 : canonical/derived TCP port split.
# Canonical ports belong to PIL-AXO-008 sub-products (brain + indexer).
# Derived ports belong to PIL-AXO-009 non-canonical surfaces (dashboard).
# --verify mode (audit-only) checks AXON_CANONICAL_TCP_PORTS only ;
# normal stop path does not actively touch derived surfaces either
# (axonctl manages brain + indexer ; dashboard runs out-of-band).
if [[ "$STOP_ROLE" == "all" ]]; then
    AXON_CANONICAL_TCP_PORTS=("$AXON_BRAIN_PORT")
    AXON_DERIVED_TCP_PORTS=("$PHX_PORT")
elif axon_role_is_indexer "$STOP_ROLE"; then
    AXON_CANONICAL_TCP_PORTS=()
    AXON_DERIVED_TCP_PORTS=()
else
    AXON_CANONICAL_TCP_PORTS=("$AXON_BRAIN_PORT")
    AXON_DERIVED_TCP_PORTS=("$PHX_PORT")
fi
# Backward-compat union for the existing kill path (normal stop mode).
AXON_TCP_PORTS=("${AXON_CANONICAL_TCP_PORTS[@]}" "${AXON_DERIVED_TCP_PORTS[@]:+${AXON_DERIVED_TCP_PORTS[@]}}")

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
Usage: ./scripts/stop.sh [--role brain|indexer|all] [--hard|--verify]

Options:
  --role    Scope the stop to one Axon role only. `--role brain` keeps the
            indexer running (and vice versa); default is `all` when no role
            is set via env or CLI. (REQ-AXO-209)
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

canonical_axon_processes_alive_pids() {
    # REQ-AXO-901637 + DEC-AXO-901598 rule 2 (binary-anchored process identity).
    # Match cmdlines ending in ${PROJECT_ROOT}/bin/axon-brain or ${PROJECT_ROOT}/bin/axon-indexer
    # (regex `( |$)` = followed by space or end). Excludes :
    #   - axon-bench-* / axon-mcp-tunnel-static (other binaries in bin/)
    #   - dashboard BEAM (different cmdline shape entirely)
    #   - third-party processes whose cmdline contains 'axon' but not in $PROJECT_ROOT/bin/
    # Also matches the axonctl supervisor (its cmdline ends with `-- bin/axon-brain`
    # or `-- bin/axon-indexer`), which is correct : supervisor alive == canonical
    # sub-product still managed.
    # Note: pgrep returns 1 on no-match, which would trip `set -euo pipefail` when
    # this function is captured via $(...). Catch the no-match case explicitly so
    # the helper always returns 0 with an empty stdout when nothing is alive.
    local pg_out
    pg_out="$(pgrep -af "${PROJECT_ROOT}/bin/axon-brain( |\$)|${PROJECT_ROOT}/bin/axon-indexer( |\$)|(^|[[:space:]])bin/axon-brain\$|(^|[[:space:]])bin/axon-indexer\$" 2>/dev/null || true)"
    [[ -z "$pg_out" ]] && return 0
    printf '%s\n' "$pg_out" | awk '{print $1}' | sort -u
}

collect_canonical_listener_pids() {
    # REQ-AXO-901636 : verify scope = canonical TCP ports only.
    local pids=""
    local port
    local port_pids

    [[ "${#AXON_CANONICAL_TCP_PORTS[@]}" -gt 0 ]] || return 0

    for port in "${AXON_CANONICAL_TCP_PORTS[@]}"; do
        # `|| true` on the whole pipeline absorbs ss exit codes (when no listener
        # matches) so the no-match path returns 0 with empty stdout instead of
        # tripping `set -euo pipefail`.
        port_pids="$(ss -ltnp 2>/dev/null | awk -v p="$port" '
            $1 == "LISTEN" {
                split($4, addr_parts, ":")
                if (addr_parts[length(addr_parts)] != p) {
                    next
                }
                match($0, /pid=([0-9]+)/, m)
                if (m[1] != "") print m[1]
            }' 2>/dev/null || true)"
        if [ -n "$port_pids" ]; then
            pids="$pids
$port_pids"
        fi
    done

    [[ -z "$pids" ]] && return 0
    printf '%s\n' "$pids" | tr ' ' '\n' | awk 'NF' | sort -u
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
    # DEC-AXO-901598 + REQ-AXO-901636 + REQ-AXO-901637 :
    # Canonical-only scope for `--verify` audit.
    # Identity = binary-anchored cmdline match in ${PROJECT_ROOT}/bin/.
    # Listener scope = AXON_CANONICAL_TCP_PORTS (PHX_PORT excluded —
    # dashboard PIL-AXO-009 non-canonical never blocks runtime verify).
    local _unused_patterns_ref="$1"  # kept for backward call signature
    local canonical_pids
    local canonical_listener_pids
    local stale=""
    local pid

    canonical_pids="$(canonical_axon_processes_alive_pids)"
    canonical_listener_pids="$(collect_canonical_listener_pids)"

    for pid in $canonical_listener_pids; do
        if ! pid_exists "$pid"; then
            stale="$stale $pid"
        fi
    done

    if [ -z "$canonical_pids" ] && [ -z "$canonical_listener_pids" ]; then
        local guard_failed=0
        while read -r guard_label guard_path; do
            [[ -n "${guard_label:-}" ]] || continue
            # After a clean shutdown, the runtime may leave no lockfile behind.
            # Verification accepts both "released existing guard" and
            # "guard file absent because nothing still owns it".
            verify_writer_guard_release "$guard_label" "$guard_path" 0 || guard_failed=1
        done < <(selected_writer_guards)
        if [ "$guard_failed" -eq 1 ]; then
            return 1
        fi
        echo "✅ Stop verification OK: no canonical Axon processes/listeners (PIL-AXO-008 scope)."
        return 0
    fi

    echo "⚠️ Stop verification failed (canonical scope PIL-AXO-008):"
    if [ -n "$canonical_pids" ]; then
        echo "Canonical process pids: $canonical_pids"
        if [ "${AXON_STOP_DEBUG_MATCH:-0}" = "1" ]; then
            echo "Matched canonical command lines:"
            for pid in $canonical_pids; do
                ps -p "$pid" -o pid=,cmd= || true
            done
        fi
    fi
    [ -n "$canonical_listener_pids" ] && echo "Canonical port listener pids: $canonical_listener_pids"
    if [ -n "$stale" ]; then
        echo "⚠️ Non-visible/stale listener pids (namespace-shifted): $stale"
    fi
    if [[ "${#AXON_CANONICAL_TCP_PORTS[@]}" -gt 0 ]]; then
        local canonical_regex
        canonical_regex="$(IFS='|'; echo "${AXON_CANONICAL_TCP_PORTS[*]}")"
        ss -ltnp 2>/dev/null | rg "$canonical_regex" || true
    fi
    return 1
}

echo "🛑 Stopping Axon v2 Architecture (Chirurgical Mode)..."

# REQ-AXO-901735 — stop process-compose daemon if running for this instance.
# Process-compose manages restart policies; killing children without stopping
# the supervisor causes immediate respawn.
case "${AXON_INSTANCE_KIND:-live}" in
    live) _PC_PORT=8080 ;;
    dev)  _PC_PORT=8081 ;;
    *)    _PC_PORT=8080 ;;
esac
if curl -sf "http://localhost:${_PC_PORT}/live" >/dev/null 2>&1; then
    _PC_BIN="$(command -v process-compose 2>/dev/null || true)"
    if [[ -z "$_PC_BIN" ]]; then
        _PC_BIN="$(devenv shell --no-reload --no-tui -- bash -c 'which process-compose' 2>/dev/null | tail -1 || true)"
    fi
    if [[ -x "${_PC_BIN:-}" ]]; then
        echo "   Stopping process-compose on :${_PC_PORT}..."
        "$_PC_BIN" down -p "$_PC_PORT" 2>/dev/null || true
        sleep 1
    fi
fi
unset _PC_PORT _PC_BIN

if [ "$VERIFY_ONLY" = "1" ]; then
    PATTERNS=(
        "$ELIXIR_NODE_NAME"
        "$ELIXIR_NODE_NAME@127.0.0.1"
    )
    verify_only_exit_if_needed PATTERNS
    exit $?
fi

# Graceful Elixir shutdown via RPC before axonctl takes over process killing.
# REQ-AXO-901638 polling discipline : replace the legacy `sleep 0.20` defensive
# wait with a bounded poll loop on `epmd -names` (returns the live Elixir node
# list). Most shutdowns settle in ~20-100ms ; cap at 2s to fail loud if the BEAM
# refuses to release the node name.
if command -v elixir >/dev/null 2>&1; then
    echo "Sending shutdown signal to Axon Nexus node..."
    _erlang_cookie="${AXON_ERLANG_COOKIE:-axon_secret}"
    elixir --name stop_script@127.0.0.1 --cookie "$_erlang_cookie" --rpc "${ELIXIR_NODE_NAME}@127.0.0.1" :init :stop >/dev/null 2>&1 || true
    unset _erlang_cookie
    if command -v epmd >/dev/null 2>&1; then
        _elixir_node_short="${ELIXIR_NODE_NAME%%@*}"
        _stop_end_ms=$(( $(date +%s%N) / 1000000 + 2000 ))
        while true; do
            if ! epmd -names 2>/dev/null | grep -q "name ${_elixir_node_short} "; then
                break
            fi
            (( $(date +%s%N) / 1000000 >= _stop_end_ms )) && break
            sleep 0.05
        done
    else
        # epmd absent (unusual) — fall back to a single conservative sleep.
        sleep 0.20
    fi
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

    # REQ-AXO-093 — orphan-socket guard: axonctl stop kills processes but
    # does not always unlink the AF_UNIX sockets. Leftover sockets cause
    # the next start to misread "data plane already up" and silently skip
    # its launch, producing a green "Ready" line on a dead runtime. The
    # cleanup helpers live in scripts/lib/socket-lifecycle.sh.
    if [[ "${AXON_INSTANCE_KIND:-live}" == "dev" ]]; then
        _AXON_RUN_ROOT_BASE="$PROJECT_ROOT/.axon-dev"
    else
        _AXON_RUN_ROOT_BASE="$PROJECT_ROOT/.axon"
    fi
    if [[ "$STOP_ROLE" == "all" ]]; then
        axon_cleanup_role_state "$AXON_INSTANCE_KIND" brain "$_AXON_RUN_ROOT_BASE"
        axon_cleanup_role_state "$AXON_INSTANCE_KIND" indexer "$_AXON_RUN_ROOT_BASE"
    else
        axon_cleanup_role_state "$AXON_INSTANCE_KIND" "$STOP_ROLE" "$_AXON_RUN_ROOT_BASE"
    fi
    axon_cleanup_legacy_instance_paths "$AXON_INSTANCE_KIND"

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
