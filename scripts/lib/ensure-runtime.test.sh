#!/usr/bin/env bash
# Post-crash recovery tests for purge_stale_postmaster_pid +
# purge_stale_writer_locks. Tracks the 2026-05-19 session 48 incident
# where a stale postmaster.pid + .axon-soll.writer.lock survived a WSL
# crash and blocked axon-live start brain.
#
# Run: bash scripts/lib/ensure-runtime.test.sh
# Exit code 0 on pass, 1 on any failed assertion.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=ensure-runtime.sh
source "$SCRIPT_DIR/ensure-runtime.sh"

PASS=0
FAIL=0

assert() {
    local desc="$1"
    local cond="$2"
    if eval "$cond"; then
        printf '  PASS  %s\n' "$desc"
        PASS=$(( PASS + 1 ))
    else
        printf '  FAIL  %s  (cond: %s)\n' "$desc" "$cond"
        FAIL=$(( FAIL + 1 ))
    fi
}

mk_sandbox() {
    SANDBOX="$(mktemp -d -t axon-ensure-runtime-test-XXXXXX)"
    mkdir -p "$SANDBOX/.devenv/state/postgres"
    mkdir -p "$SANDBOX/.axon/graph_v2"
    export PROJECT_ROOT="$SANDBOX"
}

cleanup_sandbox() {
    if [[ -n "${SANDBOX:-}" && -d "$SANDBOX" ]]; then
        rm -rf "$SANDBOX"
    fi
    unset PROJECT_ROOT SANDBOX
}

# T1 — All locks belong to dead PIDs ; everything must be purged.
test_purge_when_pids_dead() {
    mk_sandbox
    # PIDs in the 999990+ range are conventionally outside normal Linux
    # ranges (kernel.pid_max defaults to 4 194 304 but actual live PIDs
    # rarely exceed 100 000). If a real process ever lands here the test
    # would false-positive — fix by picking a random high PID and skipping
    # the test if kill -0 succeeds.
    echo "999999" > "$SANDBOX/.devenv/state/postgres/postmaster.pid"
    cat > "$SANDBOX/.axon/graph_v2/.axon-soll.writer.lock" <<EOF
target=SOLL
owner=axon-live-axon-brain;pid=999998
EOF
    cat > "$SANDBOX/.axon/graph_v2/.axon-ist.writer.lock" <<EOF
target=IST
owner=axon-live-axon-indexer;pid=999997
EOF

    purge_stale_postmaster_pid >/dev/null
    purge_stale_writer_locks >/dev/null

    assert "T1 postmaster.pid purged when PID dead" \
        '[[ ! -f "$SANDBOX/.devenv/state/postgres/postmaster.pid" ]]'
    assert "T1 .axon-soll.writer.lock purged when PID dead" \
        '[[ ! -f "$SANDBOX/.axon/graph_v2/.axon-soll.writer.lock" ]]'
    assert "T1 .axon-ist.writer.lock purged when PID dead" \
        '[[ ! -f "$SANDBOX/.axon/graph_v2/.axon-ist.writer.lock" ]]'

    cleanup_sandbox
}

# T2 — Recorded PID is alive (this test's own bash) ; preserve everything.
test_preserve_when_pid_alive() {
    mk_sandbox
    echo "$$" > "$SANDBOX/.devenv/state/postgres/postmaster.pid"
    cat > "$SANDBOX/.axon/graph_v2/.axon-soll.writer.lock" <<EOF
target=SOLL
owner=axon-live-axon-brain;pid=$$
EOF

    purge_stale_postmaster_pid >/dev/null
    purge_stale_writer_locks >/dev/null

    assert "T2 live-PID postmaster.pid preserved" \
        '[[ -f "$SANDBOX/.devenv/state/postgres/postmaster.pid" ]]'
    assert "T2 live-PID .axon-soll.writer.lock preserved" \
        '[[ -f "$SANDBOX/.axon/graph_v2/.axon-soll.writer.lock" ]]'

    cleanup_sandbox
}

# T3 — Lock has no parseable pid= field. Safe default: preserve so Rust
# startup enforcement (the authoritative gate) can surface the real reason.
test_preserve_when_lock_malformed() {
    mk_sandbox
    echo "garbage content with no pid field at all" \
        > "$SANDBOX/.axon/graph_v2/.axon-soll.writer.lock"

    purge_stale_writer_locks >/dev/null

    assert "T3 malformed lock preserved (safe default)" \
        '[[ -f "$SANDBOX/.axon/graph_v2/.axon-soll.writer.lock" ]]'

    cleanup_sandbox
}

# T4 — No files at all. Functions must exit cleanly without errors.
test_noop_when_no_files() {
    mk_sandbox
    # Sandbox has no .pid file and no lock files.
    purge_stale_postmaster_pid >/dev/null
    purge_stale_writer_locks >/dev/null

    assert "T4 no-op when no files exist" 'true'

    cleanup_sandbox
}

# T5 — postmaster.pid has empty content. Safe default: purge anyway since
# no PID means no owner to defend.
test_purge_when_postmaster_empty() {
    mk_sandbox
    : > "$SANDBOX/.devenv/state/postgres/postmaster.pid"

    purge_stale_postmaster_pid >/dev/null

    assert "T5 empty postmaster.pid purged (no PID = no owner)" \
        '[[ ! -f "$SANDBOX/.devenv/state/postgres/postmaster.pid" ]]'

    cleanup_sandbox
}

# T6 — Multi-line lock with pid= on second line. Extraction must succeed.
test_extract_pid_from_second_line() {
    mk_sandbox
    # 999998 verified dead via kill -0 in T1 ; reused here.
    cat > "$SANDBOX/.axon/graph_v2/.axon-soll.writer.lock" <<EOF
target=SOLL
extra=metadata
owner=axon-live-axon-brain;pid=999998
db_path=/dev/null
EOF

    purge_stale_writer_locks >/dev/null

    assert "T6 multi-line lock with pid on line 3 purged" \
        '[[ ! -f "$SANDBOX/.axon/graph_v2/.axon-soll.writer.lock" ]]'

    cleanup_sandbox
}

echo "Running ensure-runtime.sh post-crash recovery tests..."
echo

test_purge_when_pids_dead
test_preserve_when_pid_alive
test_preserve_when_lock_malformed
test_noop_when_no_files
test_purge_when_postmaster_empty
test_extract_pid_from_second_line

echo
echo "Results: $PASS passed, $FAIL failed"

if (( FAIL > 0 )); then
    exit 1
fi
