#!/usr/bin/env bash
# axon-os-limits.sh — OS-limit provisioning for the Axon runtime.
#
# Reason of existence: on a large host (57 projects, ~546K files) the dev
# indexer collapsed because the kernel's inotify + fd limits were too low for
# the watcher to function:
#   1. fs.inotify.max_user_instances = 128 (system default), most of it
#      consumed by leaked dbus-daemon processes → the indexer hit EMFILE on
#      inotify_init() and started WITHOUT a filesystem watcher (silent
#      degradation — IST never refreshed).
#   2. The indexer inherited a LOW fd soft limit (ulimit -n 65536) because the
#      launch context never raised it toward the (much higher) hard limit.
#
# `axon_ensure_os_limits` makes each runtime launch (and `setup.sh`) self-heal:
#   - raise the calling shell's `ulimit -n` soft limit to the hard max (no sudo)
#   - check inotify instances/watches and TRY `sysctl -w` (root-only); on
#     failure print the EXACT sudo command + a persistence hint
#   - report current inotify-instance headroom (used vs limit)
#
# It is BEST-EFFORT: it never fails the calling script (always returns 0) and
# never kills any process. Raising the limit is the correct, safe fix — killing
# dbus or other non-Axon processes is out of scope and unsafe.

# Idempotent sourcing guard.
if [[ -n "${_AXON_OS_LIMITS_LIB_LOADED:-}" ]]; then
    return 0 2>/dev/null || exit 0
fi
_AXON_OS_LIMITS_LIB_LOADED=1

# Safe thresholds. Below these the inotify-heavy watcher on a large multi-
# project host (REQ-AXO-901735 diagnosis) starves and falls back to no watcher.
: "${AXON_INOTIFY_MIN_INSTANCES:=1024}"
: "${AXON_INOTIFY_MIN_WATCHES:=524288}"
# Target soft fd limit floor. We always try to raise to the hard max, but warn
# only when the achievable soft limit is still below this floor.
: "${AXON_FD_SOFT_FLOOR:=262144}"

# Minimal logging that works whether or not axon-log.sh is sourced.
_axon_oslim_step() {
    if declare -F axon_log_step >/dev/null 2>&1; then
        axon_log_step "$*"
    else
        printf '👉 %s\n' "$*"
    fi
}
_axon_oslim_ok() {
    if declare -F axon_log_ok >/dev/null 2>&1; then
        axon_log_ok "$*"
    else
        printf '✅ %s\n' "$*"
    fi
}
_axon_oslim_warn() {
    if declare -F axon_log_warn >/dev/null 2>&1; then
        axon_log_warn "$*"
    else
        printf '⚠️  %s\n' "$*" >&2
    fi
}

# axon_count_inotify_instances — count live inotify instances across all
# processes by enumerating anon_inode:inotify fds under /proc/*/fd. Prints an
# integer (0 on any error). Best-effort: unreadable /proc entries are skipped.
axon_count_inotify_instances() {
    local count=0
    local fd target
    shopt -s nullglob
    for fd in /proc/[0-9]*/fd/*; do
        target="$(readlink "$fd" 2>/dev/null || true)"
        if [[ "$target" == "anon_inode:inotify" ]]; then
            count=$((count + 1))
        fi
    done
    shopt -u nullglob
    printf '%s\n' "$count"
}

# _axon_read_sysctl <key> — print the current value of a sysctl key, or empty.
_axon_read_sysctl() {
    local key="$1"
    local path="/proc/sys/${key//.//}"
    if [[ -r "$path" ]]; then
        tr -d '[:space:]' < "$path" 2>/dev/null || true
    fi
}

# _axon_try_raise_sysctl <key> <min> — raise a sysctl key to <min> when below.
# Returns 0 if the key is already adequate OR was successfully raised; 1 if a
# raise was needed but failed (typically: not root). Never prints on success.
_axon_try_raise_sysctl() {
    local key="$1"
    local min="$2"
    local cur
    cur="$(_axon_read_sysctl "$key")"
    # If unreadable / non-numeric, treat as "needs raising" so the operator
    # still gets the actionable command, but don't error out the script.
    if [[ "$cur" =~ ^[0-9]+$ ]] && (( cur >= min )); then
        return 0
    fi
    # Attempt the raise, then VERIFY by re-reading. On WSL2 (and namespaced
    # containers) `sysctl -w` can exit 0 WITHOUT actually changing the value, so
    # the exit code alone is not proof — claiming success on it falsely reports
    # "raised 128 → 1024" while the kernel stays at 128. Only declare success
    # when the kernel itself now reports >= min.
    sysctl -w "${key}=${min}" >/dev/null 2>&1 || true
    local after
    after="$(_axon_read_sysctl "$key")"
    if [[ "$after" =~ ^[0-9]+$ ]] && (( after >= min )); then
        _axon_oslim_ok "raised ${key} ${cur:-?} → ${after}"
        return 0
    fi
    return 1
}

# axon_ensure_os_limits — main entry point. Best-effort, always returns 0.
axon_ensure_os_limits() {
    # --- 1. Raise the calling shell's fd soft limit toward the hard max. ---
    local soft hard
    soft="$(ulimit -Sn 2>/dev/null || echo unknown)"
    hard="$(ulimit -Hn 2>/dev/null || echo unknown)"
    if [[ "$hard" == "unlimited" ]]; then
        # Pick a generous concrete target; unlimited is not directly settable
        # as a soft cap on some kernels and offers no extra benefit here.
        if [[ "$soft" != "unlimited" ]] && [[ "$soft" =~ ^[0-9]+$ ]]; then
            if ulimit -n 1048576 2>/dev/null; then
                _axon_oslim_ok "fd soft limit raised ${soft} → 1048576 (hard=unlimited)"
            else
                _axon_oslim_warn "could not raise fd soft limit (current soft=${soft}, hard=unlimited)"
            fi
        fi
    elif [[ "$soft" =~ ^[0-9]+$ && "$hard" =~ ^[0-9]+$ ]]; then
        if (( soft < hard )); then
            if ulimit -n "$hard" 2>/dev/null; then
                _axon_oslim_ok "fd soft limit raised ${soft} → ${hard} (hard max)"
            else
                _axon_oslim_warn "could not raise fd soft limit toward hard max (soft=${soft}, hard=${hard})"
            fi
        fi
        # After the attempt, warn if we are still below the working floor.
        local soft_after
        soft_after="$(ulimit -Sn 2>/dev/null || echo "$soft")"
        if [[ "$soft_after" =~ ^[0-9]+$ ]] && (( soft_after < AXON_FD_SOFT_FLOOR )); then
            _axon_oslim_warn "fd soft limit ${soft_after} is below the recommended floor ${AXON_FD_SOFT_FLOOR}; raise the hard limit in /etc/security/limits.d/99-axon.conf (e.g. '* hard nofile 1048576')."
        fi
    else
        _axon_oslim_warn "could not read fd limits (soft=${soft}, hard=${hard})"
    fi

    # --- 2. inotify instances / watches: try sysctl -w, else actionable warn. ---
    local need_sudo_keys=()
    local inst_cur watch_cur
    inst_cur="$(_axon_read_sysctl fs.inotify.max_user_instances)"
    watch_cur="$(_axon_read_sysctl fs.inotify.max_user_watches)"

    if ! _axon_try_raise_sysctl fs.inotify.max_user_instances "$AXON_INOTIFY_MIN_INSTANCES"; then
        need_sudo_keys+=("fs.inotify.max_user_instances=${AXON_INOTIFY_MIN_INSTANCES}")
        _axon_oslim_warn "fs.inotify.max_user_instances=${inst_cur:-?} is below the safe threshold ${AXON_INOTIFY_MIN_INSTANCES} and could not be raised (needs root)."
    fi
    if ! _axon_try_raise_sysctl fs.inotify.max_user_watches "$AXON_INOTIFY_MIN_WATCHES"; then
        need_sudo_keys+=("fs.inotify.max_user_watches=${AXON_INOTIFY_MIN_WATCHES}")
        _axon_oslim_warn "fs.inotify.max_user_watches=${watch_cur:-?} is below the safe threshold ${AXON_INOTIFY_MIN_WATCHES} and could not be raised (needs root)."
    fi

    if (( ${#need_sudo_keys[@]} > 0 )); then
        local k
        printf '   ─── Run ONCE with sudo to fix the kernel limits ───\n' >&2
        for k in "${need_sudo_keys[@]}"; do
            printf '     sudo sysctl -w %s\n' "$k" >&2
        done
        printf '   To persist across reboots, write them to /etc/sysctl.d/99-axon.conf:\n' >&2
        printf '     sudo tee /etc/sysctl.d/99-axon.conf >/dev/null <<EOF\n' >&2
        for k in "${need_sudo_keys[@]}"; do
            printf '     %s\n' "$k" >&2
        done
        printf '     EOF\n' >&2
        printf '     sudo sysctl --system\n' >&2
        printf '   WSL2 note: /etc/sysctl.d is honoured only when systemd is enabled in\n' >&2
        printf '   /etc/wsl.conf ([boot] systemd=true). Otherwise add the lines above to a\n' >&2
        printf '   [boot] command in /etc/wsl.conf, then run `wsl --shutdown` from Windows.\n' >&2
    fi

    # --- 3. Report inotify-instance headroom (used vs limit). ---
    local used limit
    used="$(axon_count_inotify_instances)"
    limit="$(_axon_read_sysctl fs.inotify.max_user_instances)"
    if [[ "$limit" =~ ^[0-9]+$ ]]; then
        local free=$(( limit - used ))
        if (( free < 0 )); then free=0; fi
        _axon_oslim_step "inotify instances in use: ${used} / ${limit} (headroom: ${free})"
        if (( limit > 0 )) && (( used * 100 >= limit * 80 )); then
            _axon_oslim_warn "inotify-instance usage ≥ 80% of the limit (${used}/${limit}); a swarm of leaked processes (e.g. dbus-daemon) can starve the watcher. The fix is raising fs.inotify.max_user_instances (see above), NOT killing those processes."
        fi
    else
        _axon_oslim_step "inotify instances in use: ${used} (limit unknown)"
    fi

    return 0
}
