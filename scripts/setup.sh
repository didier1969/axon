#!/bin/bash
set -euo pipefail

# Axon v2 - Bootstrap Script
# Use this script for first-time setup or after significant dependency changes.

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_ROOT"
# shellcheck source=scripts/lib/axon-version.sh
source "$PROJECT_ROOT/scripts/lib/axon-version.sh"
# shellcheck source=scripts/lib/axon-os-limits.sh
source "$PROJECT_ROOT/scripts/lib/axon-os-limits.sh"
# shellcheck source=scripts/lib/axon-resource-policy.sh
source "$PROJECT_ROOT/scripts/lib/axon-resource-policy.sh"

ARTIFACT_ONLY=0
WITH_TENSORRT=0
TENSORRT_QUALIFY=0
DRY_RUN=0
TENSORRT_ARGS=()

usage() {
    cat <<'EOF'
Usage: bash scripts/setup.sh [--artifact-only] [--with-tensorrt] [--tensorrt-qualify] [--dry-run]

Options:
  --artifact-only  Build only the canonical Rust release artifact and build-info, then exit.
  --with-tensorrt  Also build and validate the local TensorRT ORT artifact.
  --tensorrt-qualify
                   With --with-tensorrt, run bounded cold TensorRT qualification.
  --tensorrt-arg ARG
                   Forward one argument to scripts/setup-tensorrt.sh.
  --dry-run        Print the bootstrap plan without executing devenv shell,
                   cargo build, mix deps, or TensorRT steps. REQ-AXO-901644.

TensorRT requires the NVIDIA-approved local tarball:
  .axon/downloads/TensorRT-10.14.1.48.Linux.x86_64-gnu.cuda-12.9.tar.gz
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --artifact-only)
            ARTIFACT_ONLY=1
            shift
            ;;
        --with-tensorrt)
            WITH_TENSORRT=1
            shift
            ;;
        --tensorrt-qualify)
            WITH_TENSORRT=1
            TENSORRT_QUALIFY=1
            shift
            ;;
        --tensorrt-arg)
            TENSORRT_ARGS+=("$2")
            shift 2
            ;;
        --tensorrt-arg=*)
            TENSORRT_ARGS+=("${1#*=}")
            shift
            ;;
        --dry-run)
            DRY_RUN=1
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

echo "🚀 Starting Axon bootstrap..."

# REQ-AXO-901644 — dry-run prints the bootstrap plan without executing.
# Use this on a fresh client clone to validate that every step's prerequisites
# are present before paying the multi-minute devenv-shell + cargo + mix cost.
if [[ "$DRY_RUN" -eq 1 ]]; then
    cat <<EOF
DRY RUN: Axon bootstrap plan (no action taken)

PROJECT_ROOT       = $PROJECT_ROOT
ARTIFACT_ONLY      = $ARTIFACT_ONLY
WITH_TENSORRT      = $WITH_TENSORRT
TENSORRT_QUALIFY   = $TENSORRT_QUALIFY
TENSORRT_ARGS      = ${TENSORRT_ARGS[*]:-<none>}

Planned steps:
  1. devenv presence check (command -v devenv)
  2. devenv shell -- bash -lc './scripts/validate-devenv.sh'
  3. Nexus admission (required by promote) then devenv shell -- cargo build --release --bin <delivered artifact>...
       cwd=$PROJECT_ROOT/src/axon-core
  4. install_release_bin axon-core / axon-brain / axon-indexer / axonctl / axon-query-embed-worker
       target=$PROJECT_ROOT/bin/<name>
EOF
    if [[ "$ARTIFACT_ONLY" -eq 1 ]]; then
        echo "  5. SKIP dashboard + tests (--artifact-only)"
    else
        echo "  5. devenv shell -- mix deps.get && mix compile (Elixir dashboard)"
        echo "  6. devenv shell -- cargo test (Rust unit tests)"
        echo "  7. devenv shell -- mix test (Elixir dashboard tests)"
    fi
    if [[ "$WITH_TENSORRT" -eq 1 ]]; then
        if [[ "$TENSORRT_QUALIFY" -eq 1 ]]; then
            echo "  8. scripts/setup-tensorrt.sh --qualify ${TENSORRT_ARGS[*]:-}"
        else
            echo "  8. scripts/setup-tensorrt.sh ${TENSORRT_ARGS[*]:-}"
        fi
    fi
    echo ""
    echo "Prerequisite probes:"
    if command -v devenv >/dev/null 2>&1; then
        echo "  devenv : $(command -v devenv) (OK)"
    else
        echo "  devenv : NOT FOUND on PATH (install required before real run)"
    fi
    if [[ "$WITH_TENSORRT" -eq 1 ]]; then
        _tarball="$PROJECT_ROOT/.axon/downloads/TensorRT-10.14.1.48.Linux.x86_64-gnu.cuda-12.9.tar.gz"
        if [[ -f "$_tarball" ]]; then
            echo "  TensorRT tarball : $_tarball (OK)"
        else
            echo "  TensorRT tarball : NOT FOUND at $_tarball (NVIDIA download required)"
        fi
    fi
    exit 0
fi

# 0. OS-limit provisioning (REQ-AXO-901735) — idempotent, best-effort.
# Raises this shell's fd soft limit and tries to raise inotify instance/watch
# limits (root-only). On a large multi-project host the indexer's FS watcher
# otherwise hits EMFILE on inotify_init() and starts WITHOUT a watcher. Never
# fails the bootstrap; prints the exact sudo command(s) when root is required.
echo "🔧 Provisioning OS limits (fd + inotify)..."
axon_ensure_os_limits || true

# 1. Environment Check (Devenv)
if ! command -v devenv &> /dev/null; then
    echo "❌ devenv not found. Please install it first."
    exit 1
fi

echo "📦 Validating Devenv environment..."
devenv shell -- bash -lc './scripts/validate-devenv.sh'

# 2. Rust Core build
BIN_DIR="$PROJECT_ROOT/bin"
RUST_CORE_DIR="$PROJECT_ROOT/src/axon-core"
TARGET_BIN="$BIN_DIR/axon-core"
mkdir -p "$BIN_DIR"

# REQ-AXO-902464 — la cible de compilation est rapportée PAR LE BUILD, jamais
# recalculée après coup.
#
# Ici vivait `CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-$PROJECT_ROOT/.axon/cargo-target}"`,
# que plus personne ne lisait, pendant que `install_release_bin` refaisait la même
# résolution de son côté. Deux résolutions pour une seule question, et elles n'ont
# pas la même réponse : cargo tourne DANS `devenv shell`, où `devenv.nix` réassigne
# `CARGO_TARGET_DIR = DEVENV_ROOT/.axon/cargo-target` ; l'installation tourne
# DEHORS, où la variable de l'appelant est encore en place. Le 2026-08-23, un
# promote a donc compilé 8 min 24 dans le worktree figé et installé le binaire de
# la veille pris dans le workspace — toutes gardes vertes.
#
# On demande donc au shell qui compile de dire où il a écrit, et on installe de là.
AXON_EFFECTIVE_CARGO_TARGET_FILE="$(mktemp -t axon-cargo-target.XXXXXX)"
trap 'rm -f "$AXON_EFFECTIVE_CARGO_TARGET_FILE"' EXIT
AXON_EFFECTIVE_CARGO_TARGET=""

# REQ-AXO-902267 — bound build parallelism by AVAILABLE memory, not by core count.
# `cargo` defaults to one rustc per core (16 on this host) and a release rustc here is
# GB-scale, so an unbounded build can commit far more than the machine has free. On
# 2026-07-27 a promote produced two global OOM kills (Chrome died twice) and left the
# laptop unresponsive; the primary cause was a fork storm (REQ-AXO-902266, fixed), but the
# host runs with swap at 7/8 GB, so the build is the remaining large consumer on this path.
# This only ever LOWERS parallelism — never above cargo's own default — and
# `AXON_BUILD_JOBS` overrides it outright.
CARGO_JOBS="$(axon_resolve_cargo_jobs)"
echo "🔨 Building Rust core (-j ${CARGO_JOBS}: $(axon_available_ram_gb) GB free, swap $(axon_swap_used_pct)%, $(axon_detect_host_cpu_cores) cores)..."
AXON_RELEASE_BUILD_RUNNER="$PROJECT_ROOT/scripts/release/run_targeted_cargo_build.sh"
AXON_DEFAULT_CARGO_TARGET="$PROJECT_ROOT/.axon/cargo-target"

# REQ-AXO-902543 — a promotion build is a host-wide resource event. The
# canonical promote sets AXON_REQUIRE_NEXUS_ADMISSION=1; setup then waits for
# Nexus' thermal, PSI and memory-reserve gates before starting rustc. A setup
# already running inside a Nexus batch unit must not nest a second admission.
axon_inside_nexus_batch() {
    grep -q '/nexus\.slice/nexus-batch\.slice/' /proc/self/cgroup 2>/dev/null
}

if [[ "${AXON_REQUIRE_NEXUS_ADMISSION:-0}" == "1" ]] && ! axon_inside_nexus_batch; then
    NEXUS_JOB_BIN="${AXON_NEXUS_JOB_BIN:-$(command -v nexus-job || true)}"
    if [[ -z "$NEXUS_JOB_BIN" || ! -x "$NEXUS_JOB_BIN" ]]; then
        echo "❌ Promotion build requires Nexus admission, but nexus-job is unavailable." >&2
        exit 1
    fi
    echo "🛡️  Waiting for Nexus admission (medium, 6G estimate / 8G ceiling, thermal + memory gates)..."
    if ! "$NEXUS_JOB_BIN" run \
            --project AXON \
            --class medium \
            --priority interactive \
            --memory 6G \
            --gpu-mib 0 \
            --timeout 90m \
            -- "$AXON_RELEASE_BUILD_RUNNER" build "$RUST_CORE_DIR" \
            "$AXON_EFFECTIVE_CARGO_TARGET_FILE" "$CARGO_JOBS" "$AXON_DEFAULT_CARGO_TARGET"; then
        echo "❌ Nexus admission/build failed; no release artifact may be installed." >&2
        exit 1
    fi
else
    if ! "$AXON_RELEASE_BUILD_RUNNER" build "$RUST_CORE_DIR" \
            "$AXON_EFFECTIVE_CARGO_TARGET_FILE" "$CARGO_JOBS" "$AXON_DEFAULT_CARGO_TARGET"; then
        echo "❌ Rust release build failed; no release artifact may be installed." >&2
        exit 1
    fi
fi

AXON_EFFECTIVE_CARGO_TARGET="$(tr -d '\n' < "$AXON_EFFECTIVE_CARGO_TARGET_FILE" 2>/dev/null || true)"
if [[ -z "$AXON_EFFECTIVE_CARGO_TARGET" || ! -d "$AXON_EFFECTIVE_CARGO_TARGET" ]]; then
    echo "❌ Le build n'a pas rapporté sa cible de compilation (CARGO_TARGET_DIR effectif)."
    echo "   Sans elle, l'installation devrait deviner d'où copier — c'est exactement"
    echo "   la devinette qui a publié l'ancien binaire le 2026-08-23 (REQ-AXO-902464)."
    exit 1
fi
echo "📦 Cible de compilation rapportée par le build : $AXON_EFFECTIVE_CARGO_TARGET"

install_release_bin() {
    local bin_name="$1"
    local release_bin
    local target_bin
    local build_info_path
    release_bin="$AXON_EFFECTIVE_CARGO_TARGET/release/$bin_name"
    target_bin="$BIN_DIR/$bin_name"
    build_info_path="$(axon_build_info_path_for "$PROJECT_ROOT" "$bin_name")"
    if [[ ! -x "$release_bin" ]]; then
        echo "❌ Canonical release binary missing after build: $release_bin"
        exit 1
    fi
    if ! axon_stamp_artifact_build_id "$release_bin" "$AXON_BUILD_ID"; then
        echo "❌ Unable to stamp build identity into $release_bin"
        exit 1
    fi
    if ! axon_artifact_carries_build_id "$release_bin" "$AXON_BUILD_ID"; then
        echo "❌ Stamped artifact does not carry build identity: $release_bin"
        exit 1
    fi
    install -m 755 "$release_bin" "$target_bin"
    AXON_ARTIFACT_SHA256="$(axon_file_sha256 "$target_bin")"
    axon_write_export_file "$build_info_path" \
        AXON_RELEASE_VERSION "$AXON_PACKAGE_VERSION" \
        AXON_BUILD_ID "$AXON_BUILD_ID" \
        AXON_PACKAGE_VERSION "$AXON_PACKAGE_VERSION" \
        AXON_INSTALL_GENERATION workspace \
        AXON_ARTIFACT_SHA256 "$AXON_ARTIFACT_SHA256" \
        AXON_ARTIFACT_SOURCE "$release_bin"
    echo "✅ Rust binary available at bin/$bin_name"
}

AXON_BUILD_ID="$(axon_workspace_build_id "$PROJECT_ROOT")"
AXON_PACKAGE_VERSION="$(axon_package_version "$PROJECT_ROOT")"
install_release_bin "axon-core"
install_release_bin "axon-brain"
install_release_bin "axon-indexer"
install_release_bin "axon-query-embed-worker"
# REQ-AXO-153 — axonctl supervises the runtime processes and exposes the
# status JSON consumed by `axon status` / qualify-mcp. Including it in the
# release artifact set ensures every promotion ships the supervisor that
# matches the runtime contract; without it, axonctl-side fixes (e.g.
# REQ-AXO-151 role_contract_violations) compile and commit but stay inert
# in production.
install_release_bin "axonctl"

if [[ "$ARTIFACT_ONLY" -eq 1 ]]; then
    if [[ "$WITH_TENSORRT" -eq 1 ]]; then
        echo "🧩 Building requested TensorRT artifact..."
        if [[ "$TENSORRT_QUALIFY" -eq 1 ]]; then
            bash "$PROJECT_ROOT/scripts/setup-tensorrt.sh" --qualify "${TENSORRT_ARGS[@]}"
        else
            bash "$PROJECT_ROOT/scripts/setup-tensorrt.sh" "${TENSORRT_ARGS[@]}"
        fi
    fi
    echo "🏁 Artifact-only bootstrap complete."
    exit 0
fi

# 3. Dashboard dependencies and compile
DASHBOARD_DIR="$PROJECT_ROOT/src/dashboard"
echo "💧 Preparing Elixir dashboard..."
devenv shell -- bash -lc "cd '$DASHBOARD_DIR' && mix local.hex --force >/dev/null && mix local.rebar --force >/dev/null && mix deps.get && mix compile"
echo "✅ Elixir dashboard compiled"

# 4. Core validation
echo "🧪 Running validation suite..."

echo "--- Rust Unit Tests ---"
devenv shell -- bash -lc "cd '$RUST_CORE_DIR' && cargo test"

echo "--- Elixir Dashboard Tests ---"
devenv shell -- bash -lc "cd '$DASHBOARD_DIR' && mix test"

if [[ "$WITH_TENSORRT" -eq 1 ]]; then
    echo "🧩 Building requested TensorRT artifact..."
    if [[ "$TENSORRT_QUALIFY" -eq 1 ]]; then
        bash "$PROJECT_ROOT/scripts/setup-tensorrt.sh" --qualify "${TENSORRT_ARGS[@]}"
    else
        bash "$PROJECT_ROOT/scripts/setup-tensorrt.sh" "${TENSORRT_ARGS[@]}"
    fi
fi

echo "🏁 Bootstrap complete."
echo "Next step: ./scripts/start.sh"
echo "Stop running services with: ./scripts/stop.sh"
