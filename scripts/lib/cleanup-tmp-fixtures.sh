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

# ---------------------------------------------------------------------------
# REQ-AXO-902272 — orphan PostgreSQL test databases.
#
# The sweep above reclaims /tmp FILES. Test DATABASES leak too, and until session 107
# nothing reclaimed them outside the suite's own end-of-run sweep — which is precisely
# the moment a killed run never reaches.
#
# What that cost, measured: 246 leaked `axon_test_*` databases (2.5 GB). The in-suite
# sweep reclaims them ONE AT A TIME, spawning a `dropdb` process per database; it was
# observed stalled for over ten minutes and the full `--lib` run could never conclude.
# The loop is self-feeding: the slower the sweep, the likelier the run is killed, and a
# killed run leaks its own databases on the way out. No amount of host capacity fixes
# that — it is a stock problem, not a speed problem.
#
# Two design choices follow from it:
#   * ONE SQL pass, not one process per database. The same 246 that defeated the in-suite
#     sweep are handled in a single `psql` round trip.
#   * Run at STARTUP (this file is already hooked before `start.sh`), not only at the end.
#     Cleanup that only runs on the happy path never runs when it is most needed.
#
# Safety: the WHERE clause is a literal `axon_test\_%` prefix match, and every candidate
# is re-checked against that prefix in shell before it is emitted. `axon_dev`, `axon_live`,
# `postgres` and the templates cannot match either filter. This is the only thing standing
# between a cleanup helper and the operator's real data — it is deliberately doubled.
AXON_TEST_DB_PREFIX='axon_test_'

# _axon_test_db_is_reclaimable <name> — the shell-side half of the double check.
# Exposed (underscore-prefixed but stable) so the tests can exercise it directly.
_axon_test_db_is_reclaimable() {
    local name="${1:-}"
    [[ -n "$name" ]] || return 1
    [[ "$name" == "${AXON_TEST_DB_PREFIX}"* ]] || return 1
    # Belt and braces: never the real databases, whatever the prefix says.
    case "$name" in
        axon_dev|axon_live|postgres|template0|template1) return 1 ;;
    esac
    return 0
}

# axon_cleanup_orphan_test_databases [--dry-run] [--quiet] [--url=URL]
#
# Returns 0 always (never blocks a start). Echoes:
#   axon-cleanup-db: dropped=N skipped=M dry_run=B
axon_cleanup_orphan_test_databases() {
    local dry_run=0 quiet=0
    local url="${AXON_CLEANUP_DB_URL:-postgres://axon@127.0.0.1:44144/postgres}"

    while (( $# > 0 )); do
        case "$1" in
            --dry-run) dry_run=1 ;;
            --quiet) quiet=1 ;;
            --url=*) url="${1#*=}" ;;
            *) ;;
        esac
        shift
    done

    if ! command -v psql >/dev/null 2>&1; then
        # Outside `devenv shell` psql is not on PATH. Not an error: the file sweep still
        # ran, and saying so beats failing a start over a cleanup helper.
        _axon_cleanup_log "db-sweep skipped: psql not on PATH"
        (( quiet )) || printf 'axon-cleanup-db: skipped (psql not on PATH)\n'
        return 0
    fi

    # The `_` in the prefix is a LIKE WILDCARD, so it must be escaped INSIDE the prefix —
    # `axon\_test\_%`. A first version appended the escape instead (`axon_test_\_%`), which
    # asks for the prefix followed by a LITERAL underscore and therefore matched nothing at
    # all. It reported `dropped=0` on a host that had a matching database sitting right
    # there, i.e. it looked exactly like a clean host. Caught only by a positive control
    # (create a fixture database, sweep, assert it is gone) — the negative result alone was
    # indistinguishable from success, which is the whole reason that control exists.
    local like_prefix="${AXON_TEST_DB_PREFIX//_/\\_}"

    local names
    if ! names="$(psql "$url" -X -q -tAc \
        "SELECT datname FROM pg_database WHERE datname LIKE '${like_prefix}%' ORDER BY datname;" \
        2>/dev/null)"; then
        _axon_cleanup_log "db-sweep skipped: could not query pg_database"
        (( quiet )) || printf 'axon-cleanup-db: skipped (database unreachable)\n'
        return 0
    fi

    local dropped=0 skipped=0 name statements=''
    while IFS= read -r name; do
        [[ -n "$name" ]] || continue
        if ! _axon_test_db_is_reclaimable "$name"; then
            skipped=$(( skipped + 1 ))
            _axon_cleanup_log "db-sweep REFUSED $name (failed the prefix re-check)"
            continue
        fi
        statements+="DROP DATABASE IF EXISTS \"$name\" WITH (FORCE);"$'\n'
        dropped=$(( dropped + 1 ))
    done <<< "$names"

    if (( dropped > 0 && ! dry_run )); then
        # FORCE terminates leftover sessions; without it a single stale connection blocks
        # the drop indefinitely, which is how the stock built up in the first place.
        if printf '%s' "$statements" | psql "$url" -X -q -f - >/dev/null 2>&1; then
            _axon_cleanup_log "db-sweep dropped=$dropped skipped=$skipped"
        else
            _axon_cleanup_log "db-sweep PARTIAL dropped<=$dropped skipped=$skipped"
        fi
    else
        _axon_cleanup_log "db-sweep dry_run=$dry_run candidates=$dropped skipped=$skipped"
    fi

    (( quiet )) || printf 'axon-cleanup-db: dropped=%d skipped=%d dry_run=%d\n' \
        "$dropped" "$skipped" "$dry_run"
    return 0
}

# When sourced for hook usage, expose a non-fatal wrapper that NEVER blocks start.
axon_cleanup_tmp_fixtures_safe() {
    # REQ-AXO-902272 — the DB sweep rides the same hook as the file sweep: it is the
    # startup path, and startup is the one moment guaranteed to happen after a killed run.
    axon_cleanup_orphan_test_databases --quiet 2>/dev/null || true
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
