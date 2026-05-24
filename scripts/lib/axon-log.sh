#!/usr/bin/env bash
# axon-log.sh — observability helpers for lifecycle scripts.
#
# Reason of existence: silent-failure pattern was the root cause of the
# 2026-05-24 PG-after-reboot incident. `devenv up postgres -d >/dev/null
# 2>&1` jeté toute l'info diagnostic, et les fonctions retournaient 0
# sur happy path sans rien echo → 94 s d'opacité pour l'opérateur.
#
# Use:
#   source scripts/lib/axon-log.sh
#   axon_log_step "Booting PG via devenv..."
#   if axon_run_logged "$AXON_RUN_ROOT/devenv-up.log" devenv up postgres -d; then
#       axon_log_ok "PG ready after ${SECONDS}s"
#   else
#       axon_log_fail_with_tail "PG boot failed" "$AXON_RUN_ROOT/devenv-up.log"
#       return 1
#   fi
#
# Re-entrant safe (`set -u`-friendly). No `set -e` so callers keep their
# own error-handling discipline.

# Idempotent sourcing guard.
if [[ -n "${_AXON_LOG_LIB_LOADED:-}" ]]; then
    return 0 2>/dev/null || exit 0
fi
_AXON_LOG_LIB_LOADED=1

# axon_log_step "<message>" — neutral step announcement.
axon_log_step() {
    printf '👉 %s\n' "$*"
}

# axon_log_ok "<message>" — success marker (symmetric with axon_log_fail*).
axon_log_ok() {
    printf '✅ %s\n' "$*"
}

# axon_log_warn "<message>" — non-fatal warning.
axon_log_warn() {
    printf '⚠️  %s\n' "$*" >&2
}

# axon_log_fail "<message>" — fatal-grade marker (no log tail).
axon_log_fail() {
    printf '❌ %s\n' "$*" >&2
}

# axon_log_fail_with_tail "<message>" "<log_path>" [n_lines=30]
# Print failure marker + last N lines of the captured log to stderr so
# the actual cause is visible to the operator without hunting in files.
axon_log_fail_with_tail() {
    local msg="$1"
    local log_path="${2:-}"
    local n_lines="${3:-30}"
    printf '❌ %s\n' "$msg" >&2
    if [[ -n "$log_path" && -s "$log_path" ]]; then
        printf '   --- tail %s of %s ---\n' "$n_lines" "$log_path" >&2
        tail -n "$n_lines" "$log_path" >&2 || true
        printf '   --- end tail ---\n' >&2
    elif [[ -n "$log_path" ]]; then
        printf '   (log %s is empty or missing)\n' "$log_path" >&2
    fi
}

# axon_run_logged "<log_path>" <command> [args...]
# Run command with stdout+stderr appended to log_path, return command's rc.
# Caller decides what to do on failure (typically: axon_log_fail_with_tail).
# Creates the log dir if absent.
axon_run_logged() {
    local log_path="$1"
    shift
    local log_dir
    log_dir="$(dirname "$log_path")"
    mkdir -p "$log_dir" 2>/dev/null || true
    local ts
    ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    {
        printf '\n=== %s : %s ===\n' "$ts" "$*"
    } >> "$log_path"
    "$@" >> "$log_path" 2>&1
}

# axon_log_check_pipefail "<message>" "${PIPESTATUS[@]}"
# Use right after a pipeline like `pg_dump ... | gzip > $out`. Echoes
# the first non-zero stage so the masked failure becomes visible.
# Returns 0 if all stages were 0, 1 otherwise.
axon_log_check_pipefail() {
    local msg="$1"
    shift
    local i=0
    local rc
    local all_ok=1
    for rc in "$@"; do
        if [[ "$rc" -ne 0 ]]; then
            axon_log_fail "$msg : stage $i exited with rc=$rc"
            all_ok=0
        fi
        i=$((i + 1))
    done
    if [[ "$all_ok" -eq 1 ]]; then
        return 0
    fi
    return 1
}
