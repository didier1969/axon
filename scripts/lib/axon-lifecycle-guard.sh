#!/usr/bin/env bash
# REQ-AXO-901968 — cross-project control-plane governance.
#
# Runtime lifecycle control (stop / restart / --hard) of an Axon instance is a
# DESTRUCTIVE control-plane action: it cuts the MCP surface other tenants
# consume and interrupts indexing. It must be reserved to an agent/operator
# operating from the repo that OWNS the instance (Axon-projet = tenant zéro for
# the live instance). An LLM working in another project's cwd must not be able
# to trigger it reflexively.
#
# Mechanism (deliberate, defense-in-depth): the caller's cwd must be the owning
# repo or a subdirectory of it. Internal callers (restart, promote_live_safe.sh,
# axonctl) run FROM the repo, so they pass naturally — this guard adds no
# breakage to the deploy pipeline. A determined bypass (`cd <repo>` first, or
# `pkill`) is out of scope: the goal is to stop naive cross-tenant control, not
# a hostile operator. The operator can always override with
# AXON_ALLOW_FOREIGN_CONTROL=1.

# axon_lifecycle_authorized <caller_pwd> <repo_root> [override_flag]
# Returns 0 (authorized) when override=1 OR caller_pwd is repo_root or a
# subdirectory of it. Returns 1 (refused) otherwise. Pure function: no I/O, no
# process side effects — safe to unit-test.
axon_lifecycle_authorized() {
    local caller_pwd="${1:-}"
    local repo_root="${2:-}"
    local override="${3:-0}"

    if [[ "$override" == "1" ]]; then
        return 0
    fi
    if [[ -z "$caller_pwd" || -z "$repo_root" ]]; then
        return 1
    fi
    # Trailing slash makes the prefix match boundary-correct: "/repo" must not
    # authorize "/repo-other". "/repo/" matches "/repo/" and "/repo/sub/...".
    case "$caller_pwd/" in
        "$repo_root/"*) return 0 ;;
        *) return 1 ;;
    esac
}

# axon_assert_lifecycle_authorized <verb> <repo_root>
# Enforce the guard in a lifecycle script; exits 13 with a machine-actionable
# message when refused. Reads $PWD and $AXON_ALLOW_FOREIGN_CONTROL.
axon_assert_lifecycle_authorized() {
    local verb="${1:-stop}"
    local repo_root="${2:-}"
    if ! axon_lifecycle_authorized "$PWD" "$repo_root" "${AXON_ALLOW_FOREIGN_CONTROL:-0}"; then
        echo "axon: refused '${verb}' — runtime lifecycle control (stop/restart/--hard) is reserved to an agent operating from the owning repo (${repo_root}). Your cwd is ${PWD}. Either run from within the repo, or set AXON_ALLOW_FOREIGN_CONTROL=1 to override (REQ-AXO-901968)." >&2
        exit 13
    fi
}
