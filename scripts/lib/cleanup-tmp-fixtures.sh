#!/usr/bin/env bash
# REQ-AXO-255 / DEC-AXO-076 — automated /tmp test-fixture cleanup.
# Sourced by scripts/axon (clean-tmp verb) and invoked as a hook before start.sh.
# CPT-AXO-047 explains the lifecycle hygiene rationale.

# Usage (when invoked as standalone): scripts/axon clean-tmp [--dry-run] [--age-hours=N] [--quiet]
# Usage (when sourced): axon_cleanup_tmp_fixtures [--dry-run] [--age-hours=N] [--quiet]

# Patterns are an explicit allowlist — no glob /tmp/*, no rm -rf /tmp.
# Safety floor: --age-hours minimum 1 (cannot pass 0) so concurrent test runs are never racy.

AXON_CLEANUP_LOG="${AXON_CLEANUP_LOG:-/tmp/axon-cleanup.log}"
AXON_CLEANUP_LOG_MAX_BYTES="${AXON_CLEANUP_LOG_MAX_BYTES:-1048576}"  # 1 MiB

# DEC-AXO-076 §2: allowlist patterns (exact `find -name` glob syntax).
# Each entry is one pattern. Comments document the leak source.
_axon_cleanup_patterns() {
    cat <<'PATTERNS'
axon_test_db*
.tmp??????
axon-legacy-ist-*
axon-embedding-soft-reset-*
axon-ingestion-soft-reset-*
axon-memgraph-publications
axon-memgraph-*
axon-brain.promoted-original-*
hydra_db_test
hydra_db_ts
hydra_db_*
soll-fresh-test.db
soll-*test*.db
soll.db.backup-*
soll.db.before-*
PATTERNS
}

# Logs a line to AXON_CLEANUP_LOG with timestamp prefix; rotates if too big.
_axon_cleanup_log() {
    local msg="$1"
    local ts
    ts="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    if [[ -f "$AXON_CLEANUP_LOG" ]]; then
        local size
        size="$(stat -c '%s' "$AXON_CLEANUP_LOG" 2>/dev/null || echo 0)"
        if (( size > AXON_CLEANUP_LOG_MAX_BYTES )); then
            mv -f "$AXON_CLEANUP_LOG" "${AXON_CLEANUP_LOG}.1" 2>/dev/null || true
        fi
    fi
    printf '%s %s\n' "$ts" "$msg" >>"$AXON_CLEANUP_LOG" 2>/dev/null || true
}

# Run the cleanup. Returns 0 on success (even when nothing to delete or partial failure).
# Echoes a summary line on stdout: "axon-cleanup: deleted=N freed=BYTES dry_run=BOOL"
# Echoes nothing on stdout when --quiet is passed.
axon_cleanup_tmp_fixtures() {
    local dry_run=0
    local age_hours=1
    local quiet=0
    local target_dir="${AXON_CLEANUP_DIR:-/tmp}"

    while (( $# > 0 )); do
        case "$1" in
            --dry-run) dry_run=1 ;;
            --age-hours=*) age_hours="${1#*=}" ;;
            --age-hours)
                shift
                age_hours="${1:-1}"
                ;;
            --quiet) quiet=1 ;;
            --dir=*) target_dir="${1#*=}" ;;  # for tests
            --help|-h)
                cat <<'HELP'
axon_cleanup_tmp_fixtures — REQ-AXO-255 automated /tmp leak sweep

Options:
  --dry-run          Show what would be deleted, do not delete.
  --age-hours=N      Only delete entries older than N hours (default 1, minimum 1).
  --quiet            Suppress stdout summary (logs still go to AXON_CLEANUP_LOG).
  --dir=PATH         Sweep PATH instead of /tmp (used by tests).
  --help             Show this help.

Patterns (allowlist only, leading-dot included):
HELP
                _axon_cleanup_patterns | sed 's/^/  /'
                return 0
                ;;
            *) ;;  # ignore unknown to be forward-compatible
        esac
        shift
    done

    # Safety floor — never accept age <1h (concurrent test runs guard).
    if [[ ! "$age_hours" =~ ^[0-9]+$ ]] || (( age_hours < 1 )); then
        age_hours=1
    fi

    local age_minutes=$(( age_hours * 60 ))

    local deleted=0
    local freed_bytes=0

    # Build a -name expression chain for find.
    local -a name_args=()
    local first=1
    while IFS= read -r pat; do
        [[ -z "$pat" ]] && continue
        if (( first )); then
            name_args+=( -name "$pat" )
            first=0
        else
            name_args+=( -o -name "$pat" )
        fi
    done < <(_axon_cleanup_patterns)

    # Wrap the OR-chain in parentheses for find precedence.
    local -a find_args=( "$target_dir" -mindepth 1 -maxdepth 1 \( "${name_args[@]}" \) -mmin +"$age_minutes" )

    # Iterate matches; account size before deletion.
    while IFS= read -r -d '' entry; do
        [[ -z "$entry" ]] && continue
        local size_bytes=0
        if [[ -e "$entry" ]]; then
            # du -sb gives bytes for files and dirs uniformly.
            size_bytes="$(du -sb --apparent-size "$entry" 2>/dev/null | awk '{print $1}')"
            size_bytes="${size_bytes:-0}"
        fi
        if (( dry_run )); then
            _axon_cleanup_log "DRY would delete $entry ($size_bytes B)"
        else
            if rm -rf -- "$entry" 2>/dev/null; then
                _axon_cleanup_log "deleted $entry ($size_bytes B)"
                deleted=$(( deleted + 1 ))
                freed_bytes=$(( freed_bytes + size_bytes ))
            else
                _axon_cleanup_log "FAILED to delete $entry"
            fi
        fi
        if (( dry_run )); then
            deleted=$(( deleted + 1 ))
            freed_bytes=$(( freed_bytes + size_bytes ))
        fi
    done < <(find "${find_args[@]}" -print0 2>/dev/null)

    _axon_cleanup_log "summary deleted=$deleted freed_bytes=$freed_bytes dry_run=$dry_run age_hours=$age_hours dir=$target_dir"

    if (( ! quiet )); then
        printf 'axon-cleanup: deleted=%d freed=%d dry_run=%d age_hours=%d\n' \
            "$deleted" "$freed_bytes" "$dry_run" "$age_hours"
    fi

    return 0
}

# When sourced for hook usage, expose a non-fatal wrapper that NEVER blocks start.
axon_cleanup_tmp_fixtures_safe() {
    if axon_cleanup_tmp_fixtures "$@" 2>/dev/null; then
        return 0
    fi
    _axon_cleanup_log "WARN axon_cleanup_tmp_fixtures returned non-zero (ignored)"
    return 0
}

# When invoked directly (./scripts/lib/cleanup-tmp-fixtures.sh), run the cleanup.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    axon_cleanup_tmp_fixtures "$@"
fi
