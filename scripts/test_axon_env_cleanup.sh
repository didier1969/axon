#!/usr/bin/env bash
# REQ-AXO-109 — verify scripts/lib/axon-instance.sh's
# axon_clear_inherited_env unsets AXON_* env vars carried over
# from a previous lifecycle run while preserving the documented
# user-input allowlist (instance kind, project scope, role/mode
# selectors, GPU/embedding overrides, resource-policy knobs, etc.).
#
# Without this contract, a `dev` start followed by a `live` start in
# the same shell leaks dev tuning vars into the live BEAM dashboard
# and runtime, which violates Pillar PIL-AXO-004 (Dual-Instance
# Operational Discipline).
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$ROOT_DIR/scripts/lib/axon-instance.sh"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

assert_unset() {
    local name="$1"
    if [[ -n "${!name:-}" ]]; then
        fail "$name expected unset, still '${!name}'"
    fi
}

assert_eq() {
    local name="$1"
    local expected="$2"
    if [[ "${!name:-}" != "$expected" ]]; then
        fail "$name expected '$expected', got '${!name:-<unset>}'"
    fi
}

# --- Setup: simulate a polluted shell after a previous dev run --------------

# Vars from axon_resolve_instance (derived per-instance — must be unset).
export AXON_RUNTIME_IDENTITY="axon-dev-stale"
export AXON_DB_ROOT="/stale/axon-dev/graph_v2"
export AXON_RUN_ROOT="/stale/axon-dev/run"
export AXON_TELEMETRY_SOCK="/tmp/axon-dev-telemetry.sock"
export AXON_MCP_SOCK="/tmp/axon-dev-mcp.sock"
export AXON_MUTATION_POLICY="advisory_mutable"
export AXON_PID_FILE="/stale/.axon-dev/run/axon-core.pid"
export AXON_RUNTIME_STATE_FILE="/stale/.axon-dev/run/runtime.env"
export AXON_DASHBOARD_URL="http://127.0.0.1:44137/"
export AXON_SQL_URL="http://127.0.0.1:44139/sql"
export AXON_MCP_URL="http://127.0.0.1:44139/mcp"
export AXON_PUBLIC_HOST_SOURCE="derived"
export AXON_PUBLIC_ENDPOINTS_AVAILABLE="1"
export AXON_MCP_PUBLIC_URL="http://stale.example/mcp"
export AXON_SQL_PUBLIC_URL="http://stale.example/sql"
export AXON_DASHBOARD_PUBLIC_URL="http://stale.example/"
export AXON_RELEASE_VERSION="stale-version"
export AXON_PACKAGE_VERSION="0.0.0-stale"
export AXON_BUILD_ID="stale-build"
export AXON_INSTALL_GENERATION="stale-gen"
export AXON_BRAIN_PORT="44139"

# Vars from start.sh that are derived (must be unset).
export AXON_GPU_EMBED_SERVICE_TENSORRT="1"

# Vars from axon_resolve_instance that are ALSO in the derived list.
# These get unset by axon_clear_inherited_env; production scripts
# (start.sh, stop.sh) save/restore them around the clear call.
export AXON_INSTANCE_KIND="live"
export AXON_PROJECT_ROOT="/home/user/projects/axon"
export AXON_RUNTIME_SHADOW_ROLE="brain"
export AXON_RUNTIME_MODE="brain_only"

# Vars from the user-input allowlist (must be preserved).
export AXON_PROJECT_CODE="AXO"
export AXON_GPU_BACKEND="tensorrt"
export AXON_VECTOR_WORKERS="4"
export AXON_PUBLIC_HOST="public.example.com"
export AXON_LIVE_RELEASE_MANIFEST="/path/to/live/current.json"
export AXON_BENCHMARK_ACTIVE="1"
export AXON_GRPC_PORT="55555"

# Non-AXON env (must remain untouched as a safety check).
export PATH_TEST_SENTINEL="should-not-be-touched"
export USERS_OWN_VAR="hello"

# --- Run cleanup -----------------------------------------------------------

axon_clear_inherited_env

# --- Assert derived vars are unset -----------------------------------------

assert_unset AXON_RUNTIME_IDENTITY
assert_unset AXON_DB_ROOT
assert_unset AXON_RUN_ROOT
assert_unset AXON_TELEMETRY_SOCK
assert_unset AXON_MCP_SOCK
assert_unset AXON_MUTATION_POLICY
assert_unset AXON_PID_FILE
assert_unset AXON_RUNTIME_STATE_FILE
assert_unset AXON_DASHBOARD_URL
assert_unset AXON_SQL_URL
assert_unset AXON_MCP_URL
assert_unset AXON_PUBLIC_HOST_SOURCE
assert_unset AXON_PUBLIC_ENDPOINTS_AVAILABLE
assert_unset AXON_MCP_PUBLIC_URL
assert_unset AXON_SQL_PUBLIC_URL
assert_unset AXON_DASHBOARD_PUBLIC_URL
assert_unset AXON_RELEASE_VERSION
assert_unset AXON_PACKAGE_VERSION
assert_unset AXON_BUILD_ID
assert_unset AXON_INSTALL_GENERATION
assert_unset AXON_BRAIN_PORT
assert_unset AXON_GPU_EMBED_SERVICE_TENSORRT
assert_unset AXON_INSTANCE_KIND
assert_unset AXON_PROJECT_ROOT
assert_unset AXON_RUNTIME_SHADOW_ROLE
assert_unset AXON_RUNTIME_MODE

# --- Assert user-input allowlist is preserved ------------------------------

assert_eq AXON_PROJECT_CODE "AXO"
assert_eq AXON_GPU_BACKEND "tensorrt"
assert_eq AXON_VECTOR_WORKERS "4"
assert_eq AXON_PUBLIC_HOST "public.example.com"
assert_eq AXON_LIVE_RELEASE_MANIFEST "/path/to/live/current.json"
assert_eq AXON_BENCHMARK_ACTIVE "1"
assert_eq AXON_GRPC_PORT "55555"

# --- Assert non-AXON env is not touched ------------------------------------

assert_eq PATH_TEST_SENTINEL "should-not-be-touched"
assert_eq USERS_OWN_VAR "hello"

# --- Cycle 2: idempotent on already-clean env ------------------------------

axon_clear_inherited_env
assert_eq AXON_PROJECT_CODE "AXO"
assert_unset AXON_DB_ROOT

# --- Cycle 3: re-export, re-clear pattern works repeatedly -----------------

export AXON_DB_ROOT="/stale/again"
export AXON_TELEMETRY_SOCK="/tmp/axon-dev-telemetry.sock"
axon_clear_inherited_env
assert_unset AXON_DB_ROOT
assert_unset AXON_TELEMETRY_SOCK
assert_eq AXON_PROJECT_CODE "AXO"

echo "PASS: axon env cleanup (REQ-AXO-109)"
