#!/usr/bin/env bash
# REQ-AXO-241 — env var single source of truth. Smoke test:
#   1. axon_clear_inherited_env preserves operator-provided AXON_* knobs
#      (allowlist-by-prefix), even brand-new names not previously listed.
#   2. axon_clear_inherited_env unsets DERIVED per-instance vars (PID
#      files, sockets, runtime identities, etc.) so live/dev separation
#      holds across consecutive runs in the same shell.
#   3. PASS_THROUGH_EXPORTS would propagate the operator knob via prefix
#      match. Asserted via the same predicates the start.sh path uses.
#
# Catches whitelist drift on PR merge: if a refactor accidentally
# narrows the prefix-allowlist, this test fails on a synthetic AXON_FOO
# that no source-of-truth contract knows about.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$ROOT_DIR/scripts/lib/axon-instance.sh"
# shellcheck source=scripts/lib/axon-env-vars.sh
source "$ROOT_DIR/scripts/lib/axon-env-vars.sh"

assert_eq() {
    local actual="$1"
    local expected="$2"
    local label="$3"
    if [[ "$actual" != "$expected" ]]; then
        echo "FAIL: $label (expected=$expected actual=$actual)"
        exit 1
    fi
}

# ─── Phase 1: operator knobs survive axon_clear_inherited_env ────────────
unset AXON_FOO AXON_BAR AXON_VECTOR_WORKERS HYDRA_GRPC_PORT 2>/dev/null || true
export AXON_FOO="synthetic-not-on-any-allowlist"
export AXON_BAR="another-synthetic-knob"
export AXON_VECTOR_WORKERS="42"
export HYDRA_GRPC_PORT="50051"

axon_clear_inherited_env

assert_eq "${AXON_FOO:-<unset>}" "synthetic-not-on-any-allowlist" \
    "AXON_FOO must survive (allowlist-by-prefix, today: 0 places to update)"
assert_eq "${AXON_BAR:-<unset>}" "another-synthetic-knob" \
    "AXON_BAR must survive"
assert_eq "${AXON_VECTOR_WORKERS:-<unset>}" "42" \
    "AXON_VECTOR_WORKERS (operator tuning knob) must survive"
assert_eq "${HYDRA_GRPC_PORT:-<unset>}" "50051" \
    "HYDRA_GRPC_PORT (operator-overridable) must survive"

# ─── Phase 2: derived per-instance vars are cleared ──────────────────────
export AXON_PID_FILE="/tmp/leftover-from-previous-run.pid"
export AXON_RUNTIME_IDENTITY="leftover-id"
export HYDRA_HTTP_PORT="33999"
export AXON_POLICY_SOURCE_AXON_RESOURCE_PRIORITY="policy_default"
export AXON_RESOURCE_POLICY_COMPUTED_INSTANCE="dev"

axon_clear_inherited_env

assert_eq "${AXON_PID_FILE:-<unset>}" "<unset>" \
    "AXON_PID_FILE (derived per-instance) must be unset"
assert_eq "${AXON_RUNTIME_IDENTITY:-<unset>}" "<unset>" \
    "AXON_RUNTIME_IDENTITY (derived per-run) must be unset"
assert_eq "${HYDRA_HTTP_PORT:-<unset>}" "<unset>" \
    "HYDRA_HTTP_PORT (derived per-instance) must be unset"
assert_eq "${AXON_POLICY_SOURCE_AXON_RESOURCE_PRIORITY:-<unset>}" "<unset>" \
    "AXON_POLICY_SOURCE_* (resource policy marker) must be unset by wildcard match"
assert_eq "${AXON_RESOURCE_POLICY_COMPUTED_INSTANCE:-<unset>}" "<unset>" \
    "AXON_RESOURCE_POLICY_* must be unset by wildcard match"

# ─── Phase 3: predicate semantics align between clear and pass-through ───
# start.sh's PASS_THROUGH path uses the same two predicates; verify they
# would propagate AXON_FOO and skip a derived name.
if axon_env_var_in_prefix_allowlist "AXON_FOO" \
    && ! axon_env_var_is_derived "AXON_FOO"; then
    : # AXON_FOO will be propagated — correct
else
    echo "FAIL: PASS_THROUGH predicates would drop AXON_FOO (the synthetic operator knob)"
    exit 1
fi

if axon_env_var_in_prefix_allowlist "AXON_PID_FILE" \
    && ! axon_env_var_is_derived "AXON_PID_FILE"; then
    echo "FAIL: PASS_THROUGH predicates would propagate AXON_PID_FILE (derived per-instance)"
    exit 1
fi

# Foreign prefix: a non-AXON, non-HYDRA, non-OMP_NUM_THREADS/OMP_WAIT_POLICY
# var must NOT match the prefix-allowlist. This guards against accidental
# scope creep where unrelated env vars (PATH, USER, …) start being
# propagated.
if axon_env_var_in_prefix_allowlist "USER"; then
    echo "FAIL: USER must NOT be in prefix-allowlist"
    exit 1
fi

echo "PASS: axon env var single-source-of-truth (REQ-AXO-241)"
