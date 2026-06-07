#!/bin/bash
set -euo pipefail

# REQ-AXO-901735 — fail-loud lifecycle. start.sh runs under `set -e`, so any
# non-zero command (or a SIGHUP/INT to a backgrounded launch) used to abort the
# script SILENTLY — the operator saw the last successful echo (e.g. "PG already
# up") and no reason. This trap converts every abort into an explicit diagnostic
# naming the stage, line, command and exit code. `_axon_stage` is advanced at
# each major step below so the message points at the real failure point.
_axon_stage="boot"
trap 'rc=$?; echo "❌ start ABORTED — stage=[${_axon_stage}] line=${LINENO} cmd=[${BASH_COMMAND}] exit=${rc}" >&2' ERR
trap 'rc=$?; [ "$rc" -ne 0 ] && echo "❌ start EXITED non-zero — stage=[${_axon_stage}] exit=${rc}" >&2 || true' EXIT

# Axon start script — rewritten session 55 (2026-05-25).
# Usage: ./scripts/axon-dev start <mode>
# Modes: brain | full
# Options: --no-dashboard, --fast

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEFAULT_PROJECTS_ROOT="$(cd "$PROJECT_ROOT/.." && pwd)"

# --- Libraries ---
# shellcheck source=scripts/lib/axon-instance.sh
source "$PROJECT_ROOT/scripts/lib/axon-instance.sh"
_SAVED_INSTANCE_KIND="${AXON_INSTANCE_KIND:-}"
_SAVED_WATCH_DIR="${AXON_WATCH_DIR:-}"
_SAVED_BULK_WRITER="${AXON_BULK_WRITER_ENABLED:-}"
axon_clear_inherited_env
[[ -n "$_SAVED_INSTANCE_KIND" ]] && export AXON_INSTANCE_KIND="$_SAVED_INSTANCE_KIND"
[[ -n "$_SAVED_WATCH_DIR" ]] && export AXON_WATCH_DIR="$_SAVED_WATCH_DIR"
[[ -n "$_SAVED_BULK_WRITER" ]] && export AXON_BULK_WRITER_ENABLED="$_SAVED_BULK_WRITER"
unset _SAVED_INSTANCE_KIND _SAVED_WATCH_DIR _SAVED_BULK_WRITER
# shellcheck source=scripts/lib/axon-role-layout.sh
source "$PROJECT_ROOT/scripts/lib/axon-role-layout.sh"
# shellcheck source=scripts/lib/axon-resource-policy.sh
source "$PROJECT_ROOT/scripts/lib/axon-resource-policy.sh"
# shellcheck source=scripts/lib/axon-ort-runtime.sh
source "$PROJECT_ROOT/scripts/lib/axon-ort-runtime.sh"
# shellcheck source=scripts/lib/axon-version.sh
source "$PROJECT_ROOT/scripts/lib/axon-version.sh"
# shellcheck source=scripts/lib/ensure-runtime.sh
source "$PROJECT_ROOT/scripts/lib/ensure-runtime.sh"
# shellcheck source=scripts/lib/axon-os-limits.sh
source "$PROJECT_ROOT/scripts/lib/axon-os-limits.sh"
# shellcheck source=scripts/lib/axon-supervisor.sh
source "$PROJECT_ROOT/scripts/lib/axon-supervisor.sh"
cd "$PROJECT_ROOT"

axon_load_worktree_env "$PROJECT_ROOT"
axon_resolve_instance "$PROJECT_ROOT" "$(basename "$PROJECT_ROOT")"
axon_resolve_resource_policy "$AXON_INSTANCE_KIND"
axon_resolve_version "$PROJECT_ROOT"

# --- Arguments ---
RUNTIME_MODE="brain_only"
START_DASHBOARD=1
RUN_MCP_TESTS=1

FORCE_RELEASE=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        brain)             RUNTIME_MODE="brain_only" ;;
        full|indexer)      RUNTIME_MODE="indexer_full" ;;
        # REQ-AXO-901796 — long-form flags advertised by ./scripts/axon usage().
        # AxonRuntimeMode variants : BrainOnly, IndexerGraph, IndexerVector, IndexerFull.
        --brain-only)      RUNTIME_MODE="brain_only" ;;
        --indexer-graph)   RUNTIME_MODE="indexer_graph" ;;
        --indexer-vector)  RUNTIME_MODE="indexer_vector" ;;
        --indexer-full)    RUNTIME_MODE="indexer_full" ;;
        --release)         FORCE_RELEASE=1 ;;
        --no-dashboard)    START_DASHBOARD=0 ;;
        --skip-mcp-tests)  RUN_MCP_TESTS=0 ;;
        --fast)            START_DASHBOARD=0; RUN_MCP_TESTS=0 ;;
        --help|-h)
            cat <<'EOF'
Usage: ./scripts/axon-dev start <mode> [options]

Modes (positional aliases or long flags):
  brain | --brain-only        MCP server only (no indexation)
  --indexer-graph             Graph pipeline only (A1/A2/A3, no GPU/embed)
  --indexer-vector            Vector pipeline only (B1/B2/B3, embed-only)
  full | indexer | --indexer-full
                              Brain + indexer + GPU embedder + dashboard

Options:
  --release        Use release binaries (10× less RAM, 10× faster)
  --no-dashboard   Disable dashboard
  --fast           No dashboard, no MCP tests
EOF
            exit 0 ;;
        *) echo "❌ Unknown: $1. Use --help."; exit 1 ;;
    esac
    shift
done

export AXON_RUNTIME_MODE="$RUNTIME_MODE"

# --- GPU detection (auto TensorRT when GPU present + mode uses vectors) ---
detect_gpu() {
    command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi -L >/dev/null 2>&1 && return 0
    [[ -x /usr/lib/wsl/lib/nvidia-smi ]] && /usr/lib/wsl/lib/nvidia-smi -L >/dev/null 2>&1 && return 0
    return 1
}

if [[ "$RUNTIME_MODE" == indexer_full || "$RUNTIME_MODE" == indexer_vector ]]; then
    if detect_gpu; then
        export AXON_EMBEDDING_PROVIDER="tensorrt"
        for candidate in /usr/lib/wsl/lib/libnvidia-ml.so.1 /usr/lib/x86_64-linux-gnu/libnvidia-ml.so.1; do
            [[ -f "$candidate" ]] && export AXON_NVML_LIBRARY_PATH="$candidate" && break
        done
    else
        export AXON_EMBEDDING_PROVIDER="cpu"
        export AXON_VECTOR_WORKERS=1
        export OMP_NUM_THREADS=4
        export OMP_WAIT_POLICY=PASSIVE
    fi
else
    export AXON_EMBEDDING_PROVIDER="cpu"
fi

# --- Load per-instance runtime config ---
case "${AXON_INSTANCE_KIND:-live}" in
    dev)  RUNTIME_CONFIG="$PROJECT_ROOT/.axon-dev/runtime-config.dev.env" ;;
    live) RUNTIME_CONFIG="$PROJECT_ROOT/.axon/runtime-config.live.env" ;;
    *)    RUNTIME_CONFIG="" ;;
esac
if [[ -n "$RUNTIME_CONFIG" && -f "$RUNTIME_CONFIG" ]]; then
    set -o allexport; . "$RUNTIME_CONFIG"; set +o allexport
fi

# --- Preflight ---

# 0. OS-limit provisioning (REQ-AXO-901735) — best-effort, never fatal.
# Raises THIS shell's fd soft limit toward the hard max so every
# process-compose child (brain/indexer/dashboard) inherits the higher
# ulimit -n, and tries to raise inotify instance/watch limits (root-only).
# Without this the indexer's FS watcher hits EMFILE on inotify_init() on a
# large multi-project host and starts WITHOUT a watcher (silent IST staleness).
axon_ensure_os_limits || true

# 1. Nix daemon
if ! nix store info >/dev/null 2>&1; then
    echo "⚠️  Nix daemon not responding, attempting start..."
    if command -v systemctl >/dev/null && systemctl is-system-running >/dev/null 2>&1; then
        sudo systemctl start nix-daemon || true
    else
        sudo bash -c "/nix/var/nix/profiles/default/bin/nix-daemon --daemon &" || true
        sleep 2
    fi
    nix store info >/dev/null 2>&1 || { echo "❌ Nix daemon still down."; exit 1; }
fi

# 2. Devenv shell helper
run_devenv() {
    devenv shell --no-reload --no-tui -- bash -lc "$1"
}

# 3. Validate devenv
echo "📦 Validating devenv..."
run_devenv './scripts/validate-devenv.sh'

_axon_stage="pg_bootstrap"
# 4. PG bootstrap
if ! ensure_runtime_ready "$AXON_INSTANCE_KIND"; then
    echo "❌ PG bootstrap failed."; exit 1
fi

# 5. Already running check — distinguish a HEALTHY instance from a stale orphan.
# REQ-AXO-901735: a previous stop that left an orphan process-compose supervisor
# (or a leftover axon-brain) holding $AXON_BRAIN_PORT must NOT block a fresh
# start. Abort ONLY when a healthy brain is actually serving /readyz; otherwise
# reap the repo-scoped orphan by PID and proceed.
if ! axon_port_is_free "$AXON_BRAIN_PORT"; then
    if axon_brain_healthy "$AXON_BRAIN_PORT"; then
        echo "ℹ️  Healthy Axon already serving on :$AXON_BRAIN_PORT. Stop first."
        exit 0
    fi
    echo "⚠️  Port :$AXON_BRAIN_PORT held by a stale Axon orphan (not serving /readyz). Reclaiming..."
    _early_pc_bin="$(command -v process-compose 2>/dev/null || true)"
    if ! axon_reap_supervisor_tree \
            "$PROJECT_ROOT" "$AXON_INSTANCE_KIND" "$AXON_BRAIN_PORT" \
            "${_early_pc_bin:-}" "${ELIXIR_NODE_NAME:-}"; then
        echo "❌ Could not reclaim :$AXON_BRAIN_PORT from the stale orphan. Run: ./scripts/axon --instance $AXON_INSTANCE_KIND stop --hard"
        exit 1
    fi
    unset _early_pc_bin
    echo "✅ Reclaimed :$AXON_BRAIN_PORT from stale orphan; proceeding with start."
fi

# --- Resolve binaries ---
CARGO_TARGET="$PROJECT_ROOT/.axon/cargo-target"

if [[ "$AXON_INSTANCE_KIND" == "live" ]]; then
    BRAIN_BIN="$PROJECT_ROOT/bin/axon-brain"
    INDEXER_BIN="$PROJECT_ROOT/bin/axon-indexer"

    # Live release manifest: install promoted binaries + propagate identity
    MANIFEST="${AXON_LIVE_RELEASE_MANIFEST:-$PROJECT_ROOT/.axon/live-release/current.json}"
    if [[ -f "$MANIFEST" ]]; then
        eval "$(python3 -c "
import json, os
m = json.load(open('$MANIFEST'))
arts = m.get('artifacts', {})
rv = m.get('runtime_version', {})
bp = arts.get('axon-brain', {}).get('path', '')
ip = arts.get('axon-indexer', {}).get('path', '')
ig = rv.get('install_generation', '')
if bp: print(f'BRAIN_ARTIFACT={bp}')
if ip: print(f'INDEXER_ARTIFACT={ip}')
if ig: print(f'export AXON_INSTALL_GENERATION={ig}')
")"
        if [[ -n "${BRAIN_ARTIFACT:-}" && -f "$BRAIN_ARTIFACT" ]]; then
            mkdir -p bin
            install -m 755 "$BRAIN_ARTIFACT" "$BRAIN_BIN"
            if [[ -n "${INDEXER_ARTIFACT:-}" && -f "$INDEXER_ARTIFACT" ]]; then
                install -m 755 "$INDEXER_ARTIFACT" "$INDEXER_BIN"
            fi
        fi
    fi
else
    if [[ "$FORCE_RELEASE" == "1" ]] || [[ -x "$CARGO_TARGET/release/axon-brain" && -x "$CARGO_TARGET/release/axon-indexer" ]]; then
        BRAIN_BIN="$CARGO_TARGET/release/axon-brain"
        INDEXER_BIN="$CARGO_TARGET/release/axon-indexer"
        BUILD_PROFILE="--release"
    else
        BRAIN_BIN="$CARGO_TARGET/debug/axon-brain"
        INDEXER_BIN="$CARGO_TARGET/debug/axon-indexer"
        BUILD_PROFILE=""
    fi
fi

_axon_stage="build"
# 6. Auto-rebuild (dev only)
if [[ "$AXON_INSTANCE_KIND" != "live" ]]; then
    if [[ ! -x "$BRAIN_BIN" || ! -x "$INDEXER_BIN" ]]; then
        echo "🔨 Binary missing, building ${BUILD_PROFILE:-debug}..."
        run_devenv "cargo build --manifest-path src/axon-core/Cargo.toml ${BUILD_PROFILE:-} --bin axon-brain --bin axon-indexer"
    elif find "$PROJECT_ROOT/src/axon-core/src" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) -newer "$BRAIN_BIN" -print -quit | grep -q .; then
        echo "🔨 Sources newer than binary, rebuilding ${BUILD_PROFILE:-debug}..."
        run_devenv "cargo build --manifest-path src/axon-core/Cargo.toml ${BUILD_PROFILE:-} --bin axon-brain --bin axon-indexer"
    fi
fi

# Check binaries exist
for bin in "$BRAIN_BIN" "$INDEXER_BIN"; do
    [[ -x "$bin" ]] || { echo "❌ Missing: $bin"; exit 1; }
done

# --- Env exports ---
# Limit glibc per-thread mmap arenas. Without this, 170+ threads create
# up to 64 arenas × 64 MB each (~10 GB virtual). REQ-AXO-91563.
export MALLOC_ARENA_MAX="${MALLOC_ARENA_MAX:-2}"
# ORT loads libonnxruntime from ORT_DYLIB_PATH instead of bundling it.
export ORT_STRATEGY=system
# Root directory containing all projects to watch/index.
export AXON_PROJECTS_ROOT="${AXON_PROJECTS_ROOT:-$DEFAULT_PROJECTS_ROOT}"
# Filesystem scope for the file watcher. Defaults to all projects.
export AXON_WATCH_DIR="${AXON_WATCH_DIR:-$DEFAULT_PROJECTS_ROOT}"
# REQ-AXO-901893 — Watchman file source toggle. 1 = Watchman clock/cursor
# reconciliation (per-repo watch-project, no missed events); 0 = legacy
# notify/inotify + ingress_buffer + sweeps. Default ON — the dev gate validated
# connect/subscribe/clocks/fresh-instance + live deltas (create+delete) and the
# legacy-watcher cutover. Set =0 only as an emergency rollback to the (also
# stall-prone) notify path. Propagated to the indexer via process-compose env.
export AXON_USE_WATCHMAN="${AXON_USE_WATCHMAN:-1}"
# Resolve the watchman binary to an ABSOLUTE path so the indexer's Connector
# (which shells out `<bin> get-sockname`) can find it — the indexer runs OUTSIDE
# the devenv PATH (process-compose inherits start.sh's non-devenv PATH, same as
# the ORT libs which are passed by absolute env path). Prefer the project-local
# devenv profile symlink (stable, NOT a hardcoded nix-store hash path); fall
# back to PATH lookup then the bare name.
if [[ -z "${AXON_WATCHMAN_BIN:-}" ]]; then
    if [[ -x "$PROJECT_ROOT/.devenv/profile/bin/watchman" ]]; then
        export AXON_WATCHMAN_BIN="$PROJECT_ROOT/.devenv/profile/bin/watchman"
    else
        export AXON_WATCHMAN_BIN="$(command -v watchman 2>/dev/null || echo watchman)"
    fi
fi
# Use COPY BINARY for PG writes instead of INSERT VALUES SQL text.
export AXON_BULK_WRITER_ENABLED="${AXON_BULK_WRITER_ENABLED:-1}"
# Absolute path to this Axon repo root.
export AXON_PROJECT_ROOT="$PROJECT_ROOT"
# HTTP health port for the indexer process (default: brain port + 1).
# Explicit per-instance: live=44130, dev=44140. Yaml overrides via AXON_INDEXER_HEALTH_PORT env.
export AXON_INDEXER_HEALTH_PORT="${AXON_INDEXER_HEALTH_PORT:-$((AXON_BRAIN_PORT + 1))}"
# Absolute paths to the brain and indexer binaries for process-compose.
export AXON_BRAIN_BIN="$BRAIN_BIN"
export AXON_INDEXER_BIN="$INDEXER_BIN"
# Writer lock role: brain=SOLL writer, indexer=IST writer.
export AXON_RUNTIME_SHADOW_ROLE="$( [[ "$RUNTIME_MODE" == "brain_only" ]] && echo brain || echo indexer )"
# Random cookie for Erlang node authentication (dashboard).
export AXON_ERLANG_COOKIE="${AXON_ERLANG_COOKIE:-$(head -c 32 /dev/urandom | base64 | tr -d '/+=' | head -c 20)}"
# Dashboard toggle for process-compose YAML.
[[ "$START_DASHBOARD" == "1" ]] && export AXON_DASHBOARD_DISABLED=false || export AXON_DASHBOARD_DISABLED=true

# Resolve ORT runtime (sets ORT_DYLIB_PATH, LD_LIBRARY_PATH)
axon_resolve_ort_runtime "$PROJECT_ROOT" "${AXON_EMBEDDING_PROVIDER:-cpu}" || exit 1
# The ort resolver sets PRELAUNCH_LD_LIBRARY_PATH_EXPORT as a string
# "export LD_LIBRARY_PATH=...". Eval it so process-compose children inherit it.
if [[ -n "${PRELAUNCH_LD_LIBRARY_PATH_EXPORT:-}" ]]; then
    eval "$PRELAUNCH_LD_LIBRARY_PATH_EXPORT"
fi

# Role layout (sets DB paths, socket paths per instance)
axon_apply_runtime_role_layout "$PROJECT_ROOT" "$AXON_RUNTIME_SHADOW_ROLE" "axon-brain"

mkdir -p "$AXON_DB_ROOT" "${AXON_RUN_ROOT:-/tmp}"
rm -f "${AXON_TELEMETRY_SOCK:-}" "${AXON_MCP_SOCK:-}" "${AXON_PID_FILE:-}"

# --- Process selection ---
PC_PROCESSES=(axon-brain)
READYZ_PORT="$AXON_BRAIN_PORT"

if [[ "$RUNTIME_MODE" == indexer_* ]]; then
    PC_PROCESSES+=(axon-indexer)
    READYZ_PORT="$AXON_INDEXER_HEALTH_PORT"
fi
[[ "$START_DASHBOARD" == "1" ]] && PC_PROCESSES+=(dashboard)

# --- Launch ---
PC_YAML="$PROJECT_ROOT/process-compose.${AXON_INSTANCE_KIND}.yaml"
[[ -f "$PC_YAML" ]] || { echo "❌ Missing: $PC_YAML"; exit 1; }

# REQ-AXO-901735 — single source of truth for the PC management port.
PC_PORT="$(axon_pc_port_for_instance "$AXON_INSTANCE_KIND")"

_axon_stage="resolve_pc_bin"
# Resolve process-compose binary from devenv
PC_BIN="$(run_devenv 'which process-compose' 2>/dev/null | tail -1)"
[[ -x "${PC_BIN:-}" ]] || { echo "❌ process-compose not found in devenv."; exit 1; }
export AXON_PGREADY_BIN="$(run_devenv 'which pg_isready' 2>/dev/null | tail -1)"

echo "🚀 Starting Axon (instance=$AXON_INSTANCE_KIND, mode=$RUNTIME_MODE)"
echo "   Brain: $BRAIN_BIN | Indexer: $INDEXER_BIN"
echo "   MCP: http://127.0.0.1:$AXON_BRAIN_PORT/mcp"
echo "   Embedding: ${AXON_EMBEDDING_PROVIDER:-cpu}"

# REQ-AXO-901762 — Ensure the process-compose management API port is free
# before launching. A stale daemon from a previous run that didn't fully
# release its port causes the new `process-compose up -D` to fail silently
# (detached mode swallows the bind error and the parent returns 0).
if axon_supervisor_healthy "$PC_PORT"; then
    echo "⚠️  Stale process-compose daemon on :${PC_PORT}. Sending down..."
    "$PC_BIN" down -p "$PC_PORT" 2>/dev/null || true
    for ((w=1; w<=10; w++)); do
        axon_supervisor_healthy "$PC_PORT" || break
        sleep 0.5
    done
fi
# REQ-AXO-901735 — also reap a WEDGED orphan supervisor: holds :${PC_PORT}
# (visible in ss) but no longer answers /live, so the `down` above is a no-op.
# Reaping is PID-anchored + scoped to THIS repo's instance config.
if ! axon_port_is_free "$PC_PORT"; then
    _pc_orphans="$(axon_pc_supervisor_pids "$PROJECT_ROOT" "$AXON_INSTANCE_KIND")"
    _pc_port_pids="$(axon_port_listener_pids "$PC_PORT")"
    if [[ -n "$_pc_orphans" || -n "$_pc_port_pids" ]]; then
        echo "⚠️  Wedged orphan supervisor on :${PC_PORT} (pids: ${_pc_orphans//$'\n'/ } ${_pc_port_pids//$'\n'/ }). Reaping by PID..."
        # shellcheck disable=SC2086
        axon_kill_pids_graceful $_pc_orphans $_pc_port_pids
    fi
    unset _pc_orphans _pc_port_pids
fi
if axon_supervisor_healthy "$PC_PORT" || ! axon_port_is_free "$PC_PORT"; then
    echo "❌ Cannot reclaim process-compose port :${PC_PORT}. Kill it manually (ss -ltnp | grep ${PC_PORT})."
    exit 1
fi

_axon_stage="pc_up"
"$PC_BIN" up -f "$PC_YAML" -p "$PC_PORT" -t=false -D --ordered-shutdown --disable-dotenv "${PC_PROCESSES[@]}"

# Verify process-compose daemon actually started (catches silent -D failures)
for ((w=1; w<=10; w++)); do
    curl -sf --connect-timeout 3 "http://127.0.0.1:${PC_PORT}/live" >/dev/null 2>&1 && break
    sleep 0.5
done
if ! curl -sf --connect-timeout 3 "http://127.0.0.1:${PC_PORT}/live" >/dev/null 2>&1; then
    echo "❌ process-compose daemon failed to start on :${PC_PORT}."
    echo "   Check if port is occupied: ss -ltnp | grep ${PC_PORT}"
    exit 1
fi

# --- Wait for readiness ---
# Brain readiness is the gate — MCP is usable as soon as the brain is up.
# Indexer init (GPU model load) continues in background; process-compose
# monitors it independently via its own readiness_probe.
BRAIN_TIMEOUT_S=120
_axon_stage="wait_readyz"
echo "⏳ Waiting for brain :${AXON_BRAIN_PORT}/readyz (timeout ${BRAIN_TIMEOUT_S}s)..."
for ((i=1; i<=BRAIN_TIMEOUT_S; i++)); do
    curl -sf --connect-timeout 3 "http://127.0.0.1:${AXON_BRAIN_PORT}/readyz" >/dev/null 2>&1 && {
        echo "✅ Axon ready (instance=$AXON_INSTANCE_KIND, mode=$RUNTIME_MODE)"
        echo "   MCP: http://127.0.0.1:$AXON_BRAIN_PORT/mcp"
        if [[ "$RUNTIME_MODE" == indexer_* ]]; then
            echo "   Indexer: initializing in background (GPU model load)"
        fi
        echo "   Stop: ./scripts/axon --instance $AXON_INSTANCE_KIND stop"
        exit 0
    }
    (( i % 15 == 0 )) && echo "  ⏳ ${i}s/${BRAIN_TIMEOUT_S}s..."
    sleep 1
done

echo "❌ Timeout on :${READYZ_PORT}/readyz"
echo "   Logs: process-compose -p $PC_PORT process logs axon-brain"
exit 1
