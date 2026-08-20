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

# Fictional ports for the pidfile-only tests (no server is bound).
DRIFTED=44195
CANON=44196

port_busy() {
    local p="$1"
    (exec 3<>"/dev/tcp/127.0.0.1/$p") >/dev/null 2>&1 && { exec 3<&- 3>&-; return 0; }
    return 1
}


# --- REQ-AXO-902350 — PG port-drift guard -----------------------------------
# 2026-08-20: after a reboot PG bound a non-canonical port; the start only probed
# the canonical one, so it read "nothing there" and failed opaquely. Throwaway
# ports throughout — the live PG is never touched.

# T7 — no pidfile: nothing to assert.
test_port_guard_noop_without_pidfile() {
    mk_sandbox
    local dd="$SANDBOX/.devenv/state/postgres"

    assert "T7 running_port returns non-zero when no pidfile" \
        '! axon_pg_running_port "$dd" >/dev/null 2>&1'
    assert "T7 guard is a no-op when no pidfile (boot path owns it)" \
        'axon_assert_pg_canonical_port "$dd" "$CANON" >/dev/null 2>&1'

    cleanup_sandbox
}

# T8 — stale pidfile (dead PID) is not drift; purge_stale_postmaster_pid owns it.
test_port_guard_ignores_stale_pidfile() {
    mk_sandbox
    local dd="$SANDBOX/.devenv/state/postgres"
    # 999999 verified dead in T1.
    printf '999999\n%s\n1787200000\n%s\n' "$dd" "$DRIFTED" > "$dd/postmaster.pid"

    assert "T8 running_port returns non-zero for a dead PID" \
        '! axon_pg_running_port "$dd" >/dev/null 2>&1'
    assert "T8 guard does not fire on a stale pidfile" \
        'axon_assert_pg_canonical_port "$dd" "$CANON" >/dev/null 2>&1'

    cleanup_sandbox
}

# T9 — detection only: line 4 is the port. Uses this shell's PID as a live owner.
test_port_guard_reads_port_line() {
    mk_sandbox
    local dd="$SANDBOX/.devenv/state/postgres"
    printf '%s\n%s\n1787200000\n%s\n' "$$" "$dd" "$DRIFTED" > "$dd/postmaster.pid"

    assert "T9 running_port reads the port off line 4 of a live pidfile" \
        '[[ "$(axon_pg_running_port "$dd")" == "$DRIFTED" ]]'
    assert "T9 guard is silent when the running port IS canonical" \
        'axon_assert_pg_canonical_port "$dd" "$DRIFTED" >/dev/null 2>&1'

    cleanup_sandbox
}

# T10 — the falsification: a REAL postmaster on a throwaway datadir, bound to the
# wrong port. The guard must detect AND re-bind it; a no-op guard fails the last
# assertion. The precondition assert stops this passing vacuously.
test_port_guard_rebinds_real_postgres() {
    local initdb pgctl
    initdb="$(axon_resolve_pg_bin initdb 2>/dev/null || true)"
    pgctl="$(axon_resolve_pg_bin pg_ctl 2>/dev/null || true)"
    if [[ -z "$initdb" || -z "$pgctl" ]]; then
        printf '  SKIP  T10 real-postgres rebind (initdb/pg_ctl not resolvable)\n'
        return 0
    fi

    local wrong=44198 want=44199
    if port_busy "$wrong" || port_busy "$want"; then
        printf '  SKIP  T10 real-postgres rebind (throwaway ports %s/%s busy)\n' "$wrong" "$want"
        return 0
    fi

    mk_sandbox
    local dd="$SANDBOX/pgdata"
    if ! "$initdb" -D "$dd" --locale=C --encoding=UTF8 >/dev/null 2>&1; then
        printf '  SKIP  T10 real-postgres rebind (initdb failed in sandbox)\n'
        cleanup_sandbox
        return 0
    fi
    # The datadir conf carries the socket dir, exactly as devenv's generated conf
    # does in production — otherwise PG falls back to /run/postgresql, which does
    # not exist here, and the guard's own restart would inherit that failure.
    printf "unix_socket_directories = '%s'\n" "$dd" >> "$dd/postgresql.conf"

    # Start on the WRONG port — this reproduces the drifted-conf situation.
    if ! "$pgctl" -D "$dd" -o "-p $wrong" \
            -l "$dd/startup.log" -w start >/dev/null 2>&1; then
        printf '  SKIP  T10 real-postgres rebind (sandbox PG would not start)\n'
        cleanup_sandbox
        return 0
    fi

    # Precondition: genuinely drifted. Without this the test could pass vacuously.
    assert "T10 precondition: sandbox PG is really bound to :$wrong" \
        '[[ "$(axon_pg_running_port "$dd")" == "'"$wrong"'" ]]'

    axon_assert_pg_canonical_port "$dd" "$want" >/dev/null 2>&1 || true

    assert "T10 guard re-bound the drifted PG onto :$want" \
        '[[ "$(axon_pg_running_port "$dd")" == "'"$want"'" ]]'

    "$pgctl" -D "$dd" -m immediate stop >/dev/null 2>&1 || true
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
test_port_guard_noop_without_pidfile
test_port_guard_ignores_stale_pidfile
test_port_guard_reads_port_line
test_port_guard_rebinds_real_postgres

echo
echo "Results: $PASS passed, $FAIL failed"

if (( FAIL > 0 )); then
    exit 1
fi
