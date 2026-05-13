#!/usr/bin/env bash
# ensure-runtime.sh — idempotent bootstrap helpers used by scripts/start.sh.
#
# Reason of existence: the canonical start path (`./scripts/axon-{live,dev}
# start ...`) used to assume Postgres was already up, the `axon` role
# already existed, and the target database was already populated. Any
# fresh WSL / lost devenv up / wiped state forced the operator into a
# ~5-step manual recovery (devenv up postgres, CREATE ROLE, restore
# from pg_dump, then start). This library closes that gap so `axon
# start --brain-only` works from any sane disk state.
#
# All functions are idempotent and safe to call on every start. They
# exit non-zero only on real environmental failure (Docker container
# squatting the canonical port, no usable backup, etc.).
#
# Entry points:
#   ensure_runtime_ready <instance_kind>
#       Composite: refuse competing PG → start devenv PG → ensure role
#       → ensure target DB seeded. Pass "live" or "dev".

set -u

axon_canonical_pg_port="${PGPORT:-44144}"
axon_backup_dir="${AXON_SOLL_BACKUP_DIR:-${HOME}/backups/soll}"
# Minimum SOLL nodes required to consider a DB "seeded". 50 is comfortably
# below any real project (axon itself has 849); a fresh empty DB has 0.
axon_seeded_min_soll_nodes="${AXON_SEEDED_MIN_SOLL_NODES:-50}"

# Resolve PG client binaries directly from /nix/store so this lib can run
# from a non-devenv shell without paying a `devenv shell` entry (~5-15s
# on this machine). Falls back to PATH for operators outside the devenv.
axon_resolve_pg_bin() {
    local name="$1"
    local found
    found="$(ls -1d /nix/store/*-postgresql-and-plugins-17.*/bin/"$name" 2>/dev/null | sort -V | tail -1 || true)"
    if [[ -z "$found" ]]; then
        found="$(ls -1d /nix/store/*-postgresql-and-plugins-*/bin/"$name" 2>/dev/null | sort -V | tail -1 || true)"
    fi
    if [[ -z "$found" ]]; then
        found="$(command -v "$name" 2>/dev/null || true)"
    fi
    [[ -n "$found" ]] || return 1
    echo "$found"
}

PSQL_BIN="${PSQL_BIN:-$(axon_resolve_pg_bin psql || true)}"
PG_ISREADY_BIN="${PG_ISREADY_BIN:-$(axon_resolve_pg_bin pg_isready || true)}"
DEVENV_BIN="${DEVENV_BIN:-$(command -v devenv 2>/dev/null || true)}"

axon_pg_port_listener_pid() {
    ss -tnlp 2>/dev/null \
        | awk -v p="$axon_canonical_pg_port" '
            $1 == "LISTEN" {
                split($4, addr_parts, ":")
                if (addr_parts[length(addr_parts)] != p) next
                match($0, /pid=([0-9]+)/, m)
                if (m[1] != "") { print m[1]; exit }
            }'
}

axon_pg_listener_is_devenv() {
    local pid="${1:-}"
    [[ -n "$pid" ]] || return 1
    local exe
    exe="$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)"
    [[ "$exe" == /nix/store/*postgresql*/bin/postgres ]] \
        || [[ "$exe" == *.postgres-wrapped ]] \
        || [[ "$exe" == *postgres-wrapp* ]]
}

ensure_no_competing_pg_listener() {
    local pid exe
    pid="$(axon_pg_port_listener_pid)"
    if [[ -z "$pid" ]]; then
        return 0
    fi
    if axon_pg_listener_is_devenv "$pid"; then
        return 0
    fi
    exe="$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)"
    cat >&2 <<EOF
❌ Non-devenv process holds the canonical Postgres port (${axon_canonical_pg_port}).
   pid:  ${pid}
   exe:  ${exe:-<unknown>}
   This is typically a stale Docker container left over from a smoke or
   bench run. Stop it before retrying:
       docker ps --format '{{.Names}}\t{{.Ports}}' | grep ${axon_canonical_pg_port}
       docker rm -f <container-name>
EOF
    return 1
}

ensure_devenv_pg_running() {
    if axon_pg_port_listener_pid >/dev/null; then
        return 0
    fi
    echo "🐘 Postgres not running on :${axon_canonical_pg_port} — booting devenv-Nix service..."
    local proj
    proj="${PROJECT_ROOT:-${PWD}}"
    if [[ -z "$DEVENV_BIN" ]]; then
        echo "❌ devenv binary not found in PATH (required to boot postgres)." >&2
        return 1
    fi
    (cd "$proj" && "$DEVENV_BIN" up postgres -d >/dev/null 2>&1)
    local rc=$?
    if [[ $rc -ne 0 ]]; then
        echo "❌ devenv up postgres -d failed (rc=$rc)." >&2
        return 1
    fi
    local deadline=$(( SECONDS + 60 ))
    while (( SECONDS < deadline )); do
        if axon_pg_port_listener_pid >/dev/null \
           && "$PG_ISREADY_BIN" -h 127.0.0.1 -p "$axon_canonical_pg_port" -q 2>/dev/null; then
            echo "✅ Postgres ready on :${axon_canonical_pg_port}"
            return 0
        fi
        sleep 1
    done
    echo "❌ Postgres did not become ready within 60s." >&2
    return 1
}

ensure_axon_role_exists() {
    local owner_user
    owner_user="$(id -un)"
    local exists
    exists="$("$PSQL_BIN" -h 127.0.0.1 -p "$axon_canonical_pg_port" -U "$owner_user" \
        -d postgres -tAXc "SELECT 1 FROM pg_roles WHERE rolname='axon'" 2>/dev/null || true)"
    if [[ "$exists" == "1" ]]; then
        return 0
    fi
    echo "🔑 Creating Postgres role 'axon' (SUPERUSER, LOGIN)..."
    "$PSQL_BIN" -h 127.0.0.1 -p "$axon_canonical_pg_port" -U "$owner_user" -d postgres \
        -c "CREATE ROLE axon LOGIN SUPERUSER" >/dev/null
}

axon_database_for_instance() {
    case "${1:-live}" in
        live) echo "axon_live" ;;
        dev)  echo "axon_dev" ;;
        *)    echo "axon_${1}" ;;
    esac
}

axon_latest_backup_for() {
    local dbname="$1"
    ls -1 "$axon_backup_dir"/"${dbname}"-*.sql.gz 2>/dev/null | sort -V | tail -1
}

axon_db_soll_node_count() {
    local dbname="$1"
    "$PSQL_BIN" -h 127.0.0.1 -p "$axon_canonical_pg_port" -U axon -d "$dbname" -tAXc \
        "SELECT count(*) FROM soll.node" 2>/dev/null || echo 0
}

ensure_database_seeded() {
    local instance="${1:-live}"
    local dbname
    dbname="$(axon_database_for_instance "$instance")"

    if ! "$PSQL_BIN" -h 127.0.0.1 -p "$axon_canonical_pg_port" -U axon -d postgres -tAXc \
        "SELECT 1 FROM pg_database WHERE datname='${dbname}'" 2>/dev/null | grep -q 1; then
        echo "📦 Creating database ${dbname}..."
        "$PSQL_BIN" -h 127.0.0.1 -p "$axon_canonical_pg_port" -U axon -d postgres \
            -c "CREATE DATABASE ${dbname}" >/dev/null
    fi

    local node_count
    node_count="$(axon_db_soll_node_count "$dbname")"
    if [[ "$node_count" -ge "$axon_seeded_min_soll_nodes" ]]; then
        return 0
    fi

    local backup
    backup="$(axon_latest_backup_for "$dbname")"
    if [[ -z "$backup" ]]; then
        if [[ "$instance" == "live" ]]; then
            echo "❌ ${dbname} is empty (soll.node=${node_count}) and no backup found in ${axon_backup_dir}." >&2
            echo "   Cannot auto-recover live SOLL — run a manual restore or seed before retrying." >&2
            return 1
        fi
        # Dev: try to seed from latest live backup so pipeline work has a
        # realistic IST/SOLL surface to test against.
        backup="$(axon_latest_backup_for "axon_live")"
        if [[ -z "$backup" ]]; then
            echo "ℹ️ ${dbname} is empty; no axon_live backup to seed from. Continuing with empty dev DB."
            return 0
        fi
        echo "🌱 Seeding ${dbname} from latest live backup: $(basename "$backup")"
    else
        echo "🗄️ ${dbname} is empty (soll.node=${node_count}). Restoring from $(basename "$backup")..."
    fi

    if ! zcat "$backup" | "$PSQL_BIN" -h 127.0.0.1 -p "$axon_canonical_pg_port" -U axon \
            -d "$dbname" -v ON_ERROR_STOP=1 >/dev/null 2>&1; then
        echo "❌ Restore of ${dbname} from $(basename "$backup") failed." >&2
        return 1
    fi
    node_count="$(axon_db_soll_node_count "$dbname")"
    echo "✅ ${dbname} restored (soll.node=${node_count})"
}

apply_canonical_ddl() {
    # REQ-AXO-90004 — Auto-install of DEC-AXO-082 canonical DDL files.
    # Applies db/ddl/[0-9][0-9]_*.sql in lexical order to the target DB.
    # Each file is idempotent (CREATE TABLE/INDEX/FUNCTION IF NOT EXISTS
    # or CREATE OR REPLACE for functions), so re-running is a no-op.
    # Replaces the previous manual-`psql -f` step that the operator
    # had to remember after every promote-live.
    local instance="$1"  # "live" or "dev"
    local dbname="axon_${instance}"
    local repo_root="${AXON_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
    local ddl_root="${repo_root}/db/ddl"
    if [[ ! -d "$ddl_root" ]]; then
        echo "ℹ️ ${dbname}: no db/ddl directory at ${ddl_root}; skip canonical DDL apply"
        return 0
    fi
    local file applied=0
    for file in "$ddl_root"/[0-9][0-9]_*.sql; do
        [[ -f "$file" ]] || continue
        if ! "$PSQL_BIN" -h 127.0.0.1 -p "$axon_canonical_pg_port" -U axon \
                -d "$dbname" -v ON_ERROR_STOP=1 -f "$file" >/dev/null 2>&1; then
            echo "❌ ${dbname}: applying canonical DDL $(basename "$file") failed." >&2
            return 1
        fi
        applied=$((applied + 1))
    done
    if [[ "$applied" -gt 0 ]]; then
        echo "✅ ${dbname}: applied ${applied} canonical DDL file(s) from db/ddl/"
    fi
}

ensure_runtime_ready() {
    local instance="${1:-${AXON_INSTANCE_KIND:-live}}"
    if [[ -z "$PSQL_BIN" || -z "$PG_ISREADY_BIN" ]]; then
        echo "❌ psql/pg_isready not resolvable from /nix/store or PATH." >&2
        return 1
    fi
    ensure_no_competing_pg_listener || return 1
    ensure_devenv_pg_running || return 1
    ensure_axon_role_exists || return 1
    ensure_database_seeded "$instance" || return 1
    apply_canonical_ddl "$instance" || return 1
}
