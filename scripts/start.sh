#!/bin/bash
set -euo pipefail

# Axon v2 - Daily Start Script
# Canonical daily workflow entrypoint. Launches via process-compose (REQ-AXO-901735).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEFAULT_PROJECTS_ROOT="$(cd "$PROJECT_ROOT/.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$PROJECT_ROOT/scripts/lib/axon-instance.sh"
# REQ-AXO-109 — clear AXON_*/HYDRA_* leaked from a previous run in
# this shell before any lib re-derives instance state.
# Preserve AXON_INSTANCE_KIND across the clear — it's set by the
# scripts/axon dispatcher and must survive env sanitization.
_SAVED_INSTANCE_KIND="${AXON_INSTANCE_KIND:-}"
axon_clear_inherited_env
if [[ -n "$_SAVED_INSTANCE_KIND" ]]; then
    export AXON_INSTANCE_KIND="$_SAVED_INSTANCE_KIND"
fi
unset _SAVED_INSTANCE_KIND
# shellcheck source=scripts/lib/axon-role-layout.sh
source "$PROJECT_ROOT/scripts/lib/axon-role-layout.sh"
# shellcheck source=scripts/lib/axon-resource-policy.sh
source "$PROJECT_ROOT/scripts/lib/axon-resource-policy.sh"
# shellcheck source=scripts/lib/axon-ort-runtime.sh
source "$PROJECT_ROOT/scripts/lib/axon-ort-runtime.sh"
# shellcheck source=scripts/lib/axon-version.sh
source "$PROJECT_ROOT/scripts/lib/axon-version.sh"
# shellcheck source=scripts/lib/socket-lifecycle.sh
source "$PROJECT_ROOT/scripts/lib/socket-lifecycle.sh"
# shellcheck source=scripts/lib/ensure-runtime.sh
source "$PROJECT_ROOT/scripts/lib/ensure-runtime.sh"
cd "$PROJECT_ROOT"

axon_load_worktree_env "$PROJECT_ROOT"
axon_resolve_instance "$PROJECT_ROOT" "$(basename "$PROJECT_ROOT")"
axon_resolve_resource_policy "$AXON_INSTANCE_KIND"
axon_resolve_version "$PROJECT_ROOT"

LIVE_RELEASE_CURRENT_MANIFEST="$PROJECT_ROOT/.axon/live-release/current.json"
LIVE_RELEASE_PENDING_MANIFEST="$PROJECT_ROOT/.axon/live-release/pending.json"
LIVE_RELEASE_MANIFEST_SOURCE="${AXON_LIVE_RELEASE_MANIFEST:-$LIVE_RELEASE_CURRENT_MANIFEST}"
LIVE_RELEASE_ACTIVE=0
LIVE_RELEASE_BRAIN_ARTIFACT=""
LIVE_RELEASE_BRAIN_BUILD_INFO=""
LIVE_RELEASE_INDEXER_ARTIFACT=""
LIVE_RELEASE_INDEXER_BUILD_INFO=""

if [[ "$AXON_INSTANCE_KIND" == "live" \
    && -z "${AXON_LIVE_RELEASE_MANIFEST:-}" \
    && -f "$LIVE_RELEASE_PENDING_MANIFEST" ]]; then
    echo "❌ Refusing live start while a staged release is pending: $LIVE_RELEASE_PENDING_MANIFEST" >&2
    echo "   Complete promotion with scripts/axon promote-live --manifest <candidate> --restart-live," >&2
    echo "   or roll back/clear the pending manifest explicitly." >&2
    exit 1
fi

load_live_release_current() {
    [[ "$AXON_INSTANCE_KIND" == "live" && -f "$LIVE_RELEASE_MANIFEST_SOURCE" ]] || return 1

    local payload=""
    payload="$(python3 - "$LIVE_RELEASE_MANIFEST_SOURCE" <<'PY'
import json, pathlib, sys
manifest = json.loads(pathlib.Path(sys.argv[1]).read_text())
runtime = manifest.get("runtime_version") or {}
artifact = manifest.get("artifact") or {}
artifacts = manifest.get("artifacts") or {}
brain = artifacts.get("axon-brain") or {}
indexer = artifacts.get("axon-indexer") or {}
fields = [
    artifact.get("path", ""),
    artifact.get("build_info_path", "") or "",
    brain.get("path", ""),
    brain.get("build_info_path", "") or "",
    indexer.get("path", ""),
    indexer.get("build_info_path", "") or "",
    runtime.get("release_version", ""),
    runtime.get("package_version", ""),
    runtime.get("build_id", ""),
    runtime.get("install_generation", ""),
]
print("\n".join(fields))
PY
)"

    mapfile -t live_release_fields <<<"$payload"
    # Fields 0/1 (combined artifact/build_info) are intentionally read
    # but discarded — REQ-AXO-083: the combined-artifact contract is
    # superseded by the per-role brain/indexer fields below. The python
    # block above still emits them so the shape stays positional.
    LIVE_RELEASE_BRAIN_ARTIFACT="${live_release_fields[2]:-}"
    LIVE_RELEASE_BRAIN_BUILD_INFO="${live_release_fields[3]:-}"
    LIVE_RELEASE_INDEXER_ARTIFACT="${live_release_fields[4]:-}"
    LIVE_RELEASE_INDEXER_BUILD_INFO="${live_release_fields[5]:-}"

    [[ -n "$LIVE_RELEASE_BRAIN_ARTIFACT" && -f "$LIVE_RELEASE_BRAIN_ARTIFACT" ]] || {
        echo "❌ Live split manifest points to a missing brain artifact: ${LIVE_RELEASE_BRAIN_ARTIFACT:-<empty>}"
        exit 1
    }
    [[ -n "$LIVE_RELEASE_INDEXER_ARTIFACT" && -f "$LIVE_RELEASE_INDEXER_ARTIFACT" ]] || {
        echo "❌ Live split manifest points to a missing indexer artifact: ${LIVE_RELEASE_INDEXER_ARTIFACT:-<empty>}"
        exit 1
    }

    AXON_RELEASE_VERSION="${live_release_fields[6]:-$AXON_RELEASE_VERSION}"
    AXON_PACKAGE_VERSION="${live_release_fields[7]:-$AXON_PACKAGE_VERSION}"
    AXON_BUILD_ID="${live_release_fields[8]:-$AXON_BUILD_ID}"
    AXON_INSTALL_GENERATION="${live_release_fields[9]:-$AXON_INSTALL_GENERATION}"
    export AXON_RELEASE_VERSION AXON_PACKAGE_VERSION AXON_BUILD_ID AXON_INSTALL_GENERATION
    LIVE_RELEASE_ACTIVE=1
    return 0
}

load_live_release_current || true

WATCH_ROOT="${AXON_WATCH_DIR:-$DEFAULT_PROJECTS_ROOT}"
PROJECTS_ROOT="${AXON_PROJECTS_ROOT:-$WATCH_ROOT}"
PROJECT_CODE="${AXON_PROJECT_CODE:-}"
if [[ -z "$PROJECT_CODE" && -f "$PROJECT_ROOT/.axon/meta.json" ]]; then
    PROJECT_CODE="$(python3 -c 'import json; print(json.load(open("'"$PROJECT_ROOT"'/.axon/meta.json")).get("code",""))' 2>/dev/null || true)"
fi
PROJECT_CODE="${PROJECT_CODE:-$(basename "$PROJECT_ROOT")}"
# REQ-AXO-150 — runtime mode resolution priority: env override > last-known
# good (instance-state.json) > customer-facing default (brain_only). The
# previous default was `indexer_graph`, which left live MCP socket missing
# whenever a stale runtime triggered a fresh `start` without flags — bricking
# the customer-facing `axon init` workflow (observed 2026-05-03).
case "${AXON_INSTANCE_KIND:-live}" in
    live) AXON_INSTANCE_STATE_FILE="$PROJECT_ROOT/.axon/instance-state.json" ;;
    dev)  AXON_INSTANCE_STATE_FILE="$PROJECT_ROOT/.axon-dev/instance-state.json" ;;
    *)    AXON_INSTANCE_STATE_FILE="$PROJECT_ROOT/.axon/instance-state.json" ;;
esac

# Per-instance env-var overrides loaded from a sourced config file. Holds
# AXON_LIVE_DATABASE_URL / AXON_SOLL_SEED_PATH and any future runtime-flag
# persistence. Files are .gitignore'd because AXON_LIVE_DATABASE_URL contains
# credentials. Operator writes them once via the canonical wrapper or manually;
# start.sh propagates them to the brain process automatically on every start.
case "${AXON_INSTANCE_KIND:-live}" in
    live) AXON_RUNTIME_CONFIG_FILE="$PROJECT_ROOT/.axon/runtime-config.live.env" ;;
    dev)  AXON_RUNTIME_CONFIG_FILE="$PROJECT_ROOT/.axon-dev/runtime-config.dev.env" ;;
    *)    AXON_RUNTIME_CONFIG_FILE="" ;;
esac
if [[ -n "$AXON_RUNTIME_CONFIG_FILE" && -f "$AXON_RUNTIME_CONFIG_FILE" ]]; then
    # shellcheck disable=SC1090
    set -o allexport
    . "$AXON_RUNTIME_CONFIG_FILE"
    set +o allexport
    echo "🔧 Loaded runtime config from $AXON_RUNTIME_CONFIG_FILE"
fi

# Auto-bootstrap PG / role / DB before any binary check.
# Without this, a fresh WSL, a wiped .devenv/state, or a competing
# Docker container holding :44144 forces operator into a 5-step manual
# recovery. ensure_runtime_ready is idempotent — safe to call on every
# start. Runs inline (no devenv shell wrap) by resolving psql /
# pg_isready / devenv from /nix/store + PATH directly, saving the
# ~10-15s cost of a devenv shell entry on this machine.
if [[ "${AXON_SKIP_RUNTIME_BOOTSTRAP:-0}" != "1" ]]; then
    if ! ensure_runtime_ready "$AXON_INSTANCE_KIND"; then
        echo "❌ Runtime bootstrap (ensure_runtime_ready) failed; refusing to start." >&2
        exit 1
    fi
fi
AXON_LAST_RUNTIME_MODE=""
if [[ -f "$AXON_INSTANCE_STATE_FILE" ]]; then
    AXON_LAST_RUNTIME_MODE="$(python3 -c 'import json,sys
try:
    print(json.load(open(sys.argv[1])).get("last_mode",""))
except Exception:
    pass' "$AXON_INSTANCE_STATE_FILE" 2>/dev/null || true)"
fi
RUNTIME_MODE="${AXON_RUNTIME_MODE:-${AXON_LAST_RUNTIME_MODE:-brain_only}}"
# axon_runtime_shadow_role reads AXON_RUNTIME_MODE; export so the resolved
# mode (after env > state > default) is what the helper sees.
export AXON_RUNTIME_MODE="$RUNTIME_MODE"
RUNTIME_SHADOW_ROLE="$(axon_runtime_shadow_role)"
RUNTIME_SHADOW_ONLY="${AXON_SPLIT_SHADOW_ONLY:-0}"
RUNTIME_EXECUTABLE="bin/axon-core"
RUNTIME_EXECUTABLE_NAME="$(axon_runtime_binary_name "$RUNTIME_SHADOW_ROLE")"
SELECTED_DEBUG_RUNTIME_BIN=""
SELECTED_RELEASE_RUNTIME_BIN=""
START_DASHBOARD=1
RUN_MCP_TESTS=1
SKIP_ELIXIR_PREWARM="${AXON_SKIP_ELIXIR_PREWARM:-0}"
REQUEST_TENSORRT=0

detect_accessible_gpu() {
    if command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi -L >/dev/null 2>&1; then
        return 0
    fi
    if [ -x /usr/lib/wsl/lib/nvidia-smi ] && /usr/lib/wsl/lib/nvidia-smi -L >/dev/null 2>&1; then
        return 0
    fi
    return 1
}

# REQ-AXO-901641 — resolve NVML library across WSL2 + native Linux without
# pinning a maintainer-specific default. Returns the first existing candidate ;
# empty string if no NVML library found (caller decides whether that's fatal).
resolve_nvml_library_path() {
    local candidate
    for candidate in \
        "${AXON_NVML_LIBRARY_PATH:-}" \
        "/usr/lib/wsl/lib/libnvidia-ml.so.1" \
        "/usr/lib/x86_64-linux-gnu/libnvidia-ml.so.1" \
        "/usr/lib64/libnvidia-ml.so.1" \
        "/usr/lib/libnvidia-ml.so.1"
    do
        if [[ -n "$candidate" && -f "$candidate" ]]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    # Last resort : ldconfig cache (covers nix-store paths or non-standard installs).
    if command -v ldconfig >/dev/null 2>&1; then
        candidate="$(ldconfig -p 2>/dev/null | awk '/libnvidia-ml\.so\.1/ { print $NF; exit }')"
        if [[ -n "$candidate" && -f "$candidate" ]]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    fi
    return 1
}

has_live_runtime() {
    if nc -z localhost "$HYDRA_HTTP_PORT" 2>/dev/null; then
        return 0
    fi
    if [[ -f "$AXON_PID_FILE" ]]; then
        local pid
        pid="$(cat "$AXON_PID_FILE" 2>/dev/null || true)"
        if [[ -n "$pid" && -e "/proc/$pid" ]]; then
            return 0
        fi
    fi
    return 1
}

probe_writer_guard() {
    local label="$1"
    local lock_path="$2"
    local owner=""

    command -v flock >/dev/null 2>&1 || return 0
    [[ -f "$lock_path" ]] || return 0

    # Advisory preflight only. Rust startup enforcement remains authoritative.
    # Do not create lockfiles here, otherwise a refused or aborted shell launch
    # would leave stale-looking scaffolding before the runtime even starts.
    exec {guard_fd}<>"$lock_path"
    if ! flock -n "$guard_fd"; then
        owner="$(tr '\n' ';' < "$lock_path" 2>/dev/null || true)"
        echo "❌ Preflight: $label writer guard is already held."
        echo "   Lock: $lock_path"
        if [[ -n "$owner" ]]; then
            echo "   Recorded owner: $owner"
        fi
        echo "   Startup aborted before runtime launch. Rust writer enforcement remains authoritative."
        exit 1
    fi
    flock -u "$guard_fd" || true
    exec {guard_fd}>&-
}

selected_writer_guards() {
    if axon_role_is_brain "$RUNTIME_SHADOW_ROLE"; then
        printf 'SOLL %s\n' "$AXON_DB_ROOT/.axon-soll.writer.lock"
    elif axon_role_is_indexer "$RUNTIME_SHADOW_ROLE"; then
        printf 'IST %s\n' "$AXON_DB_ROOT/.axon-ist.writer.lock"
    else
        printf 'SOLL %s\n' "$AXON_DB_ROOT/.axon-soll.writer.lock"
        printf 'IST %s\n' "$AXON_DB_ROOT/.axon-ist.writer.lock"
    fi
}

require_writer_guard_probe() {
    local label="$1"
    local lock_path="$2"
    local owner=""

    command -v flock >/dev/null 2>&1 || {
        echo "❌ Strict preflight requires flock for $label writer guard verification."
        exit 1
    }

    if [[ ! -f "$lock_path" ]]; then
        echo "❌ Strict preflight: $label writer guard lockfile is missing."
        echo "   Lock: $lock_path"
        echo "   Combined-runtime rollback is unsupported; split writer ownership must be explicit."
        exit 1
    fi

    exec {guard_fd}<>"$lock_path"
    if ! flock -n "$guard_fd"; then
        owner="$(tr '\n' ';' < "$lock_path" 2>/dev/null || true)"
        echo "❌ Strict preflight: $label writer guard is still held."
        echo "   Lock: $lock_path"
        if [[ -n "$owner" ]]; then
            echo "   Recorded owner: $owner"
        fi
        echo "   Combined-runtime rollback aborted until writer ownership is provably released."
        exit 1
    fi
    flock -u "$guard_fd" || true
    exec {guard_fd}>&-
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        brain|--brain-only|--brainonly)
            RUNTIME_MODE="brain_only"
            ;;
        indexer|--indexer-full|--indexerfull|--full|full)
            RUNTIME_MODE="indexer_full"
            ;;
        --indexer-graph|--indexergraph)
            RUNTIME_MODE="indexer_graph"
            ;;
        --indexer-vector|--indexervector)
            RUNTIME_MODE="indexer_vector"
            ;;
        --no-dashboard)
            START_DASHBOARD=0
            ;;
        --skip-mcp-tests)
            RUN_MCP_TESTS=0
            ;;
        --skip-elixir-prewarm)
            SKIP_ELIXIR_PREWARM=1
            ;;
        --fast)
            # Dev-iteration shorthand: brain serves MCP, nothing else.
            # Skips dashboard (Elixir/Phoenix not needed for MCP-only
            # work), Elixir Hex/Rebar prewarm, and the post-start MCP
            # quality gate. ~5× faster restart for pipeline / GPU work.
            START_DASHBOARD=0
            RUN_MCP_TESTS=0
            SKIP_ELIXIR_PREWARM=1
            ;;
        --tensorrt)
            REQUEST_TENSORRT=1
            ;;
        --help|-h)
            cat <<'EOF'
Usage: ./scripts/axon-dev start <mode> [options]
       ./scripts/axon-live start <mode> [options]

Modes:
  brain             MCP server only (no indexation, no GPU)
  indexer | full     Brain + indexer + GPU embedder + dashboard (default: TensorRT when GPU detected)

Options:
  --no-dashboard     Disable Elixir LiveView dashboard
  --skip-mcp-tests   Skip automatic MCP quality gate validation after startup
  --fast             Dev shorthand (=--no-dashboard --skip-mcp-tests --skip-elixir-prewarm)
  --tensorrt         Force TensorRT (redundant — auto-detected when GPU present)

Examples:
  ./scripts/axon-dev start brain        # MCP only
  ./scripts/axon-dev start full         # brain + indexer + GPU
  ./scripts/axon-dev stop
EOF
            exit 0
            ;;
        --use-process-compose)
            # Ignored — process-compose is now the only path (REQ-AXO-901735).
            ;;
        *)
            echo "❌ Unknown option: $1"
            echo "   Use --help to list supported modes."
            exit 1
            ;;
    esac
    shift
done


# TensorRT is the default when GPU is detected and mode uses vectors.
# --tensorrt flag is now redundant but still accepted for backward compat.
if [[ "$RUNTIME_MODE" == "indexer_full" || "$RUNTIME_MODE" == "indexer_vector" ]]; then
    if [[ "$REQUEST_TENSORRT" == "1" ]] || detect_accessible_gpu; then
        export AXON_EMBEDDING_PROVIDER="tensorrt"
        export AXON_GPU_TELEMETRY_BACKEND="${AXON_GPU_TELEMETRY_BACKEND:-nvml}"
    fi
elif [[ "$REQUEST_TENSORRT" == "1" ]]; then
    echo "❌ --tensorrt requires indexer or full mode."
    echo "   The current mode is $RUNTIME_MODE, which does not run the vector lane."
    exit 1
fi

if [[ "${AXON_EMBEDDING_PROVIDER:-}" == "tensorrt" ]]; then
    # REQ-AXO-901641 — resolve NVML lib across WSL/native Linux. If nothing
    # found, leave the var unset and let the runtime fall back per its own
    # discipline (TensorRT path requires NVML — operator gets a clear error
    # from the embedder, not a silent dlopen of a non-existent WSL path).
    if [[ -z "${AXON_NVML_LIBRARY_PATH:-}" ]]; then
        _resolved_nvml="$(resolve_nvml_library_path 2>/dev/null || true)"
        if [[ -n "$_resolved_nvml" ]]; then
            export AXON_NVML_LIBRARY_PATH="$_resolved_nvml"
        fi
    else
        export AXON_NVML_LIBRARY_PATH
    fi
    export AXON_OPT_MAX_VRAM_USED_MB="${AXON_OPT_MAX_VRAM_USED_MB:-2048}"
    export AXON_CUDA_MEMORY_SOFT_LIMIT_MB="${AXON_CUDA_MEMORY_SOFT_LIMIT_MB:-$AXON_OPT_MAX_VRAM_USED_MB}"
    export AXON_CUDA_MEMORY_LIMIT_MB="${AXON_CUDA_MEMORY_LIMIT_MB:-1024}"
    export AXON_GPU_PRIMARY_WORKER_MAX_USED_MB="${AXON_GPU_PRIMARY_WORKER_MAX_USED_MB:-1536}"
    export AXON_GPU_TELEMETRY_CACHE_TTL_MS="${AXON_GPU_TELEMETRY_CACHE_TTL_MS:-250}"
    export AXON_TENSORRT_OVERSHOOT_MB="${AXON_TENSORRT_OVERSHOOT_MB:-7900}"
    export AXON_QUALIFY_STOP_ON_VRAM_OVERSHOOT="${AXON_QUALIFY_STOP_ON_VRAM_OVERSHOOT:-1}"
fi

# Brain-only defaults: avoid GPU unless explicitly overridden.
if [[ "$RUNTIME_MODE" == "brain_only" ]]; then
    export AXON_GPU_ACCESS_POLICY="${AXON_GPU_ACCESS_POLICY:-avoid}"
fi

# REQ-AXO-91563 slice 1 — cap glibc per-thread mmap arenas. Without this,
# 170+ threads (Tokio + ORT inference + watcher + ingester) trigger glibc
# to create up to 64 arenas × 64 MB each (~10 GB virtual, mostly resident
# under bursty allocation). Once allocated, glibc NEVER returns these
# arenas to the OS, so RSS climbs monotonically and never recovers in
# idle. Session 42 measurement (graph-only, T+9m) : 4.39 GB → 2.59 GB
# (-41 %), 111 → 5 × 64MB arenas (-95 %). Indexer_full equivalent saving
# is expected ~5-7 GB. Override only if profiling identifies arena
# contention as a bottleneck (multi-threaded malloc-heavy workloads —
# not the case here, ORT + Rust runtime allocate large chunks rarely).
export MALLOC_ARENA_MAX="${MALLOC_ARENA_MAX:-2}"

CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-$PROJECT_ROOT/.axon/cargo-target}"
DEVENV_DEBUG_BIN_ROOT="$CARGO_TARGET_ROOT/debug"

# Single-process: always axon-brain. AXON_RUNTIME_MODE tells it what to do.
RUNTIME_EXECUTABLE_NAME="axon-brain"
if [[ "$AXON_INSTANCE_KIND" == "live" && -x "$PROJECT_ROOT/bin/axon-brain" ]]; then
    RUNTIME_EXECUTABLE="bin/axon-brain"
else
    RUNTIME_EXECUTABLE="${DEVENV_DEBUG_BIN_ROOT#"$PROJECT_ROOT"/}/axon-brain"
fi
SELECTED_DEBUG_RUNTIME_BIN="$PROJECT_ROOT/$RUNTIME_EXECUTABLE"
SELECTED_RELEASE_RUNTIME_BIN="$CARGO_TARGET_ROOT/release/axon-brain"

# Shadow role determines which writer locks and DB paths the binary uses.
case "$RUNTIME_MODE" in
    brain_only) RUNTIME_SHADOW_ROLE="brain" ;;
    *)          RUNTIME_SHADOW_ROLE="indexer" ;;
esac
export AXON_RUNTIME_SHADOW_ROLE="$RUNTIME_SHADOW_ROLE"
axon_apply_runtime_role_layout "$PROJECT_ROOT" "$RUNTIME_SHADOW_ROLE" "$RUNTIME_EXECUTABLE_NAME"

export AXON_SKIP_ELIXIR_PREWARM="$SKIP_ELIXIR_PREWARM"

# Provider already resolved at lines 384-395 (TensorRT auto-detect).
EMBEDDING_PROVIDER_REQUEST="${AXON_EMBEDDING_PROVIDER:-cpu}"

if [[ "$EMBEDDING_PROVIDER_REQUEST" == "cpu" ]]; then
    if [[ -z "${AXON_VECTOR_WORKERS:-}" ]]; then
        export AXON_VECTOR_WORKERS=1
    fi
    if [[ -z "${AXON_CHUNK_BATCH_SIZE:-}" ]]; then
        export AXON_CHUNK_BATCH_SIZE=24
    fi
    if [[ -z "${AXON_FILE_VECTORIZATION_BATCH_SIZE:-}" ]]; then
        export AXON_FILE_VECTORIZATION_BATCH_SIZE=8
    fi
    if [[ -z "${OMP_NUM_THREADS:-}" ]]; then
        export OMP_NUM_THREADS=4
    fi
    if [[ -z "${OMP_WAIT_POLICY:-}" ]]; then
        export OMP_WAIT_POLICY=PASSIVE
    fi
fi

STARTUP_TIMEOUT_S="${AXON_STARTUP_TIMEOUT_S:-}"
if [[ -z "$STARTUP_TIMEOUT_S" ]]; then
    if [[ "$RUNTIME_MODE" == "indexer_full" ]]; then
        # REQ-AXO-91570 : bumped 240 → 900 s. Defense-in-depth pour
        # absorber un cold-compile TensorRT BGE-Large (5-15 min typique
        # quand le hash d'engine cache change : profile bump, ORT/TRT
        # upgrade, model file change). Cache HIT = 30-60 s usual.
        # Override via `AXON_STARTUP_TIMEOUT_S`.
        STARTUP_TIMEOUT_S=900
    else
        STARTUP_TIMEOUT_S=120
    fi
fi


run_devenv_shell() {
    local cmd="$1"
    local attempt=1
    while true; do
        if devenv shell --no-reload --no-tui -- bash -lc "$cmd"; then
            return 0
        fi
        if (( attempt >= 2 )); then
            return 1
        fi
        axon_log_warn "devenv shell failed; running 'devenv gc' once before retrying..."
        # REQ-AXO-901740 — capture devenv gc output ; previously >/dev/null
        # 2>&1 masked GC failures (disk full, permission denied) and we
        # only saw the second devenv shell attempt fail with the same
        # underlying cause.
        local gc_log="${AXON_RUN_ROOT:-${PROJECT_ROOT:-${PWD}}/.axon/run}/devenv-gc.log"
        mkdir -p "$(dirname "$gc_log")" 2>/dev/null || true
        if ! devenv gc >>"$gc_log" 2>&1; then
            axon_log_warn "devenv gc returned non-zero (see $gc_log)"
        fi
        attempt=$((attempt + 1))
        sleep 1
    done
}

# REQ-AXO-149 — verify nix-daemon BEFORE the first devenv shell call. Without
# the daemon (e.g. fresh WSL boot), `devenv shell` fails with a cryptic
# `cannot connect to socket /nix/var/nix/daemon-socket/socket` and the
# customer-facing `axon init` cold-start cannot complete. Previous code did
# this check ~70 lines later, after the first devenv invocation had already
# failed.
if ! nix store info >/dev/null 2>&1; then
    axon_log_warn "Nix daemon is not responding. Attempting to start it..."
    if command -v systemctl >/dev/null && systemctl is-system-running >/dev/null 2>&1; then
        sudo systemctl start nix-daemon || true
    else
        # WSL2 / non-systemd hosts — direct daemon launch.
        sudo bash -c "/nix/var/nix/profiles/default/bin/nix-daemon --daemon &" || true
        sleep 2
    fi
    if ! nix store info >/dev/null 2>&1; then
        echo "❌ nix-daemon is still not responding after auto-start attempt."
        echo "   Check that /nix/var/nix/profiles/default/bin/nix-daemon exists, then retry."
        echo "   On WSL2 you may need: sudo /nix/var/nix/profiles/default/bin/nix-daemon &"
        exit 1
    fi
fi

echo "📦 Validating Devenv environment..."
run_devenv_shell './scripts/validate-devenv.sh'

if [[ "$SKIP_ELIXIR_PREWARM" == "1" ]]; then
    echo "⏭️ Skipping Elixir pre-warm (AXON_SKIP_ELIXIR_PREWARM=1)."
else
    echo "📦 Pre-warming Elixir environment (Hex/Rebar)..."
    run_devenv_shell "cd '$PROJECT_ROOT/src/dashboard' && mix local.hex --force >/dev/null && mix local.rebar --force >/dev/null"
fi

if [ ! -x "$RUNTIME_EXECUTABLE" ]; then
    echo "❌ Missing $RUNTIME_EXECUTABLE_NAME"
    echo "   Expected executable: $RUNTIME_EXECUTABLE"
    if [[ "$RUNTIME_EXECUTABLE_NAME" == "axon-core" ]]; then
        echo "   Run ./scripts/setup.sh first."
    else
        echo "   Run cargo build --manifest-path src/axon-core/Cargo.toml --bin $RUNTIME_EXECUTABLE_NAME first."
    fi
    exit 1
fi

if [[ -n "$SELECTED_DEBUG_RUNTIME_BIN" ]] && find "$PROJECT_ROOT/src/axon-core/src" \
    -type f \( -name '*.rs' -o -name 'Cargo.toml' \) \
    -newer "$SELECTED_DEBUG_RUNTIME_BIN" -print -quit | grep -q .; then
    axon_log_warn "Detected newer axon-core sources than $SELECTED_DEBUG_RUNTIME_BIN"
    echo "   Rebuilding selected runtime role binary..."
    # REQ-AXO-174: dev launches the debug binary at .axon/cargo-target/debug/,
    # but the rebuild used to always pass --release — producing a fresh
    # release binary while leaving the debug binary stale. Match the build
    # profile to the binary the script is about to launch.
    if [[ "$AXON_INSTANCE_KIND" == "live" ]]; then
        BUILD_PROFILE_FLAG="--release"
    else
        BUILD_PROFILE_FLAG=""
    fi
    if ! run_devenv_shell "cd '$PROJECT_ROOT/src/axon-core' && cargo build --bin $RUNTIME_EXECUTABLE_NAME $BUILD_PROFILE_FLAG"; then
        echo "❌ Failed to rebuild $RUNTIME_EXECUTABLE_NAME"
        exit 1
    fi
    if [[ "$AXON_INSTANCE_KIND" == "live" && -n "$SELECTED_RELEASE_RUNTIME_BIN" && -f "$SELECTED_RELEASE_RUNTIME_BIN" ]]; then
        echo "🔄 Updating live $RUNTIME_EXECUTABLE from rebuilt runtime role artifact..."
        mkdir -p "$(dirname "$RUNTIME_EXECUTABLE")"
        install -m 755 "$SELECTED_RELEASE_RUNTIME_BIN" "$RUNTIME_EXECUTABLE"
    fi
fi

if has_live_runtime; then
    echo "ℹ️ Axon is already running on port $HYDRA_HTTP_PORT."
    echo "   Stop first: ./scripts/axon --instance $AXON_INSTANCE_KIND stop"
    exit 0
fi

DEVENV_TUNNEL_BIN="$CARGO_TARGET_ROOT/release/axon-mcp-tunnel"



if [[ "${AXON_SKIP_BIN_SYNC:-0}" != "1" ]]; then
    if [[ "$LIVE_RELEASE_ACTIVE" -eq 1 ]]; then
        echo "🔄 Updating live split binaries from promoted artifacts..."
        mkdir -p bin
        rm -f bin/axon-brain bin/axon-indexer
        install -m 755 "$LIVE_RELEASE_BRAIN_ARTIFACT" bin/axon-brain
        install -m 755 "$LIVE_RELEASE_INDEXER_ARTIFACT" bin/axon-indexer
        if [[ -n "$LIVE_RELEASE_BRAIN_BUILD_INFO" && -f "$LIVE_RELEASE_BRAIN_BUILD_INFO" ]]; then
            install -m 644 "$LIVE_RELEASE_BRAIN_BUILD_INFO" "$(axon_build_info_path_for "$PROJECT_ROOT" "axon-brain")"
        fi
        if [[ -n "$LIVE_RELEASE_INDEXER_BUILD_INFO" && -f "$LIVE_RELEASE_INDEXER_BUILD_INFO" ]]; then
            install -m 644 "$LIVE_RELEASE_INDEXER_BUILD_INFO" "$(axon_build_info_path_for "$PROJECT_ROOT" "axon-indexer")"
        fi
    elif [[ "$RUNTIME_EXECUTABLE_NAME" == "axon-core" && -f "$DEVENV_RELEASE_BIN" ]]; then
        echo "🔄 Updating bin/axon-core safely..."
        mkdir -p bin && rm -f bin/axon-core && install -m 755 "$DEVENV_RELEASE_BIN" bin/axon-core
        AXON_BUILD_ID="$(axon_workspace_build_id "$PROJECT_ROOT")"
        export AXON_BUILD_ID
        axon_write_export_file "$AXON_BUILD_INFO_FILE" \
            AXON_RELEASE_VERSION "$AXON_RELEASE_VERSION" \
            AXON_BUILD_ID "$AXON_BUILD_ID" \
            AXON_PACKAGE_VERSION "$AXON_PACKAGE_VERSION" \
            AXON_INSTALL_GENERATION "$AXON_INSTALL_GENERATION"
    fi
fi

if [[ "${AXON_SKIP_BIN_SYNC:-0}" != "1" ]] \
    && ! axon_role_is_indexer "$RUNTIME_SHADOW_ROLE" \
    && [ -f "$DEVENV_TUNNEL_BIN" ]; then
    mkdir -p bin && install -m 755 "$DEVENV_TUNNEL_BIN" bin/axon-mcp-tunnel
fi

# ---------------------------------------------------------------------------
# Phase: env export + writer guard + process-compose launch
# ---------------------------------------------------------------------------

mkdir -p "$AXON_DB_ROOT" "$AXON_RUN_ROOT"
rm -f "$AXON_TELEMETRY_SOCK" "$AXON_MCP_SOCK" "$AXON_PID_FILE"

while read -r guard_label guard_path; do
    [[ -n "${guard_label:-}" ]] || continue
    probe_writer_guard "$guard_label" "$guard_path"
done < <(selected_writer_guards)

axon_resolve_ort_runtime "$PROJECT_ROOT" "$EMBEDDING_PROVIDER_REQUEST" || exit 1

if [[ "$RUNTIME_MODE" == "indexer_full" ]]; then
    export AXON_ENABLE_AUTONOMOUS_INGESTOR=true
    export AXON_RUNTIME_PROFILE=full_autonomous
fi

export ORT_STRATEGY=system
export AXON_PROJECTS_ROOT="$PROJECTS_ROOT"
export AXON_WATCH_DIR="$WATCH_ROOT"
export AXON_PROJECT_ROOT="$PROJECT_ROOT"
export AXON_MCP_MUTATION_JOBS=1
export SQL_URL="$AXON_SQL_URL"
export AXON_INDEXER_HEALTH_PORT=$((HYDRA_HTTP_PORT + 10))

# Binary paths for process-compose YAML.
if [[ "$AXON_INSTANCE_KIND" == "live" ]]; then
    export AXON_BRAIN_BIN="$PROJECT_ROOT/bin/axon-brain"
    export AXON_INDEXER_BIN="$PROJECT_ROOT/bin/axon-indexer"
else
    export AXON_BRAIN_BIN="$PROJECT_ROOT/${DEVENV_DEBUG_BIN_ROOT#"$PROJECT_ROOT"/}/axon-brain"
    export AXON_INDEXER_BIN="$PROJECT_ROOT/${DEVENV_DEBUG_BIN_ROOT#"$PROJECT_ROOT"/}/axon-indexer"
fi

# Dashboard control.
if [[ "$START_DASHBOARD" == "1" ]]; then
    export AXON_DASHBOARD_DISABLED=false
else
    export AXON_DASHBOARD_DISABLED=true
fi

# Erlang cookie — generated per-instance, never hardcoded.
export AXON_ERLANG_COOKIE="${AXON_ERLANG_COOKIE:-$(head -c 32 /dev/urandom | base64 | tr -d '/+=' | head -c 20)}"

# Persist runtime state for next start default.
axon_write_export_file "$AXON_RUNTIME_STATE_FILE" \
  AXON_RUNTIME_MODE "$RUNTIME_MODE" \
  AXON_INSTANCE_KIND "$AXON_INSTANCE_KIND" \
  AXON_EMBEDDING_PROVIDER "${AXON_EMBEDDING_PROVIDER:-}"

# Select which processes to start based on mode.
PC_PROCESSES=()
case "$RUNTIME_MODE" in
    brain_only)
        PC_PROCESSES+=(axon-brain)
        ;;
    indexer_graph|indexer_vector|indexer_full)
        PC_PROCESSES+=(axon-brain axon-indexer)
        ;;
    *)
        PC_PROCESSES+=(axon-brain)
        ;;
esac

if [[ "$START_DASHBOARD" == "1" ]]; then
    PC_PROCESSES+=(dashboard)
fi

PC_YAML="$PROJECT_ROOT/process-compose.${AXON_INSTANCE_KIND}.yaml"
if [[ ! -f "$PC_YAML" ]]; then
    echo "❌ Missing process-compose YAML: $PC_YAML"
    exit 1
fi

# Process-compose port — distinct per instance to allow cohabitation.
case "$AXON_INSTANCE_KIND" in
    live) PC_PORT=8080 ;;
    dev)  PC_PORT=8081 ;;
    *)    PC_PORT=8080 ;;
esac

echo "🚀 Starting Axon (instance=$AXON_INSTANCE_KIND, mode=$RUNTIME_MODE)"
echo "   Binary: $AXON_BRAIN_BIN"
echo "   MCP: http://127.0.0.1:$HYDRA_HTTP_PORT/mcp"
echo "   Embedding: ${AXON_EMBEDDING_PROVIDER:-cpu}"
echo ""

# Resolve devenv-only binaries needed by process-compose children.
_devenv_bin_resolve="$(run_devenv_shell 'echo "PC=$(which process-compose)" && echo "PGREADY=$(which pg_isready)"' 2>/dev/null | grep -E '^PC=|^PGREADY=')"
PC_BIN="$(echo "$_devenv_bin_resolve" | grep '^PC=' | cut -d= -f2-)"
PGREADY_BIN="$(echo "$_devenv_bin_resolve" | grep '^PGREADY=' | cut -d= -f2-)"
unset _devenv_bin_resolve

if [[ ! -x "${PC_BIN:-}" ]]; then
    echo "❌ process-compose not found in devenv shell."
    exit 1
fi
export AXON_PGREADY_BIN="${PGREADY_BIN:-pg_isready}"

# Launch process-compose in detached mode.
# All env vars are already exported in this shell — process-compose
# and its children inherit them directly. No env file needed.
"$PC_BIN" up \
    -f "$PC_YAML" \
    -p "$PC_PORT" \
    -t=false \
    -D \
    --ordered-shutdown \
    --disable-dotenv \
    "${PC_PROCESSES[@]}"

# Wait for readiness. For full mode, wait on the indexer (last to be ready).
if [[ "$RUNTIME_MODE" == indexer_* ]]; then
    READYZ_PORT="$AXON_INDEXER_HEALTH_PORT"
else
    READYZ_PORT="$HYDRA_HTTP_PORT"
fi

echo "⏳ Waiting for readiness on :${READYZ_PORT}/readyz (timeout ${STARTUP_TIMEOUT_S}s)..."
READY=false
for ((i=1; i<=STARTUP_TIMEOUT_S; i++)); do
    if curl -sf "http://127.0.0.1:${READYZ_PORT}/readyz" >/dev/null 2>&1; then
        READY=true
        break
    fi
    if (( i % 15 == 0 )); then
        echo "  ⏳ ${i}s/${STARTUP_TIMEOUT_S}s..."
    fi
    sleep 1
done

if [[ "$READY" == "true" ]]; then
    echo "✅ Axon ready (instance=$AXON_INSTANCE_KIND, mode=$RUNTIME_MODE)"
    echo "   MCP: http://127.0.0.1:$HYDRA_HTTP_PORT/mcp"
    echo "   process-compose: http://localhost:$PC_PORT"
    echo "   Stop: ./scripts/axon --instance $AXON_INSTANCE_KIND stop"
else
    echo "❌ Timeout waiting for readiness on :${READYZ_PORT}/readyz"
    echo "   Check logs: process-compose -p $PC_PORT process logs axon-brain"
    exit 1
fi
