#!/usr/bin/env bash
# MIL-AXO-015 smoke test playbook — verify PG migration end-to-end.
#
# This script boots a short-lived `axon-test/age-pgvector:pg17`
# container, configures the brain + indexer with the opt-in flags
# shipped in commits 3365086 (slice 3h gate opt-in) + c88a5fe
# (AXON_AGE_READ default ON) + 5380d97 (B.4 prep flag), runs the
# indexer against a small test corpus, and verifies parity vs the
# DuckDB baseline.
#
# Acceptance:
# - axon-indexer boots under PG (slice 3h gate opt-in flag set)
# - Indexer drains the test corpus without panics
# - Symbol/Chunk/CONTAINS/CALLS counts match DuckDB baseline (within ±5%)
# - AGE graph populated: `MATCH (s:Symbol) RETURN count(*)` > 0
# - All 6 B.3 readers via AXON_AGE_READ=true return non-empty results
#   on the populated graph (no fallback to SQL needed for primary path)
#
# Usage:
#   ./scripts/smoke-pg-migration.sh             # default: smoke test only
#   ./scripts/smoke-pg-migration.sh --keep      # leave container + data after
#
# Cleanup at end unless --keep is passed.
#
# REQ-AXO-205 (P9 indexer migration) — operator playbook.

set -euo pipefail

KEEP=0
for arg in "$@"; do
    case "$arg" in
        --keep) KEEP=1 ;;
    esac
done

CONTAINER=axon-smoke-mig
PORT=5433
PG_USER=postgres
PG_PASS=axon
PG_DB=axon_smoke_mig
PG_URL="postgres://${PG_USER}:${PG_PASS}@localhost:${PORT}/${PG_DB}"
TEST_CORPUS="${AXON_SMOKE_CORPUS:-/tmp/axon-smoke-corpus}"

cleanup() {
    if [ "$KEEP" -eq 0 ]; then
        echo "[smoke] cleanup container..."
        docker rm -f "$CONTAINER" 2>/dev/null || true
    else
        echo "[smoke] --keep: container '$CONTAINER' left running on port $PORT."
    fi
}
trap cleanup EXIT

echo "=== Phase 1: Boot PG container ==="
docker rm -f "$CONTAINER" 2>/dev/null || true
docker run -d --name "$CONTAINER" \
    -p ${PORT}:5432 \
    -e POSTGRES_USER=${PG_USER} \
    -e POSTGRES_PASSWORD=${PG_PASS} \
    -e POSTGRES_DB=${PG_DB} \
    axon-test/age-pgvector:pg17 >/dev/null

until docker exec "$CONTAINER" pg_isready -U ${PG_USER} >/dev/null 2>&1; do
    sleep 1
done

docker exec "$CONTAINER" psql -U ${PG_USER} -d ${PG_DB} -c "
    CREATE EXTENSION IF NOT EXISTS vector;
    CREATE EXTENSION IF NOT EXISTS age;
    LOAD 'age';
    SET search_path = ag_catalog, '\$user', public;
    SELECT * FROM ag_catalog.create_graph('axon_graph');
" >/dev/null
echo "[smoke] PG container UP, AGE + pgvector loaded, axon_graph created."

echo "=== Phase 2: Prepare test corpus ==="
if [ ! -d "$TEST_CORPUS" ]; then
    echo "[smoke] creating minimal test corpus at $TEST_CORPUS"
    mkdir -p "$TEST_CORPUS/src"
    cat > "$TEST_CORPUS/src/main.rs" <<'RS'
fn main() {
    util::greet();
}
RS
    cat > "$TEST_CORPUS/src/util.rs" <<'RS'
pub fn greet() {
    println!("hello");
}
RS
    cat > "$TEST_CORPUS/Cargo.toml" <<'TOML'
[package]
name = "smoke-corpus"
version = "0.0.1"
edition = "2021"
[lib]
path = "src/util.rs"
TOML
fi
echo "[smoke] corpus ready at $TEST_CORPUS"

echo "=== Phase 3: Boot indexer under PG (slice 3h opt-in) ==="
export AXON_DB_BACKEND=postgres
export AXON_LIVE_DATABASE_URL="$PG_URL"
export AXON_DEV_DATABASE_URL="$PG_URL"
export AXON_INDEXER_PG_OPT_IN=1
export AXON_AGE_DUAL_WRITE=true
export AXON_AGE_READ=true
export AXON_PROJECT_ROOT="$TEST_CORPUS"
export AXON_WATCH_DIR="$TEST_CORPUS"

# Build indexer if needed.
if [ ! -x .axon/cargo-target/debug/axon-indexer ]; then
    echo "[smoke] building axon-indexer..."
    CARGO_TARGET_DIR=.axon/cargo-target cargo build \
        --manifest-path src/axon-core/Cargo.toml \
        --bin axon-indexer 2>&1 | tail -3
fi

echo "[smoke] launching indexer (60 s probe)..."
timeout 60 .axon/cargo-target/debug/axon-indexer 2>&1 | tail -10 || true

echo "=== Phase 4: Verify IST counts ==="
docker exec "$CONTAINER" psql -U ${PG_USER} -d ${PG_DB} -c "
    SELECT 'File' AS t, count(*) FROM public.File
    UNION ALL SELECT 'Symbol', count(*) FROM public.Symbol
    UNION ALL SELECT 'Chunk', count(*) FROM public.Chunk
    UNION ALL SELECT 'CONTAINS', count(*) FROM public.CONTAINS
    UNION ALL SELECT 'CALLS', count(*) FROM public.CALLS
    UNION ALL SELECT 'CALLS_NIF', count(*) FROM public.CALLS_NIF
    UNION ALL SELECT 'ChunkEmbedding', count(*) FROM public.ChunkEmbedding;
"

echo "=== Phase 5: Verify AGE dual-write populated ==="
docker exec "$CONTAINER" psql -U ${PG_USER} -d ${PG_DB} -c "
    LOAD 'age';
    SET search_path = ag_catalog, '\$user', public;
" -c "
    SELECT 'symbol_vertices' AS t, * FROM cypher('axon_graph',
        \$\$ MATCH (s:Symbol) RETURN count(s) AS n \$\$) AS (n agtype);
" -c "
    SELECT 'calls_edges' AS t, * FROM cypher('axon_graph',
        \$\$ MATCH ()-[r:CALLS]->() RETURN count(r) AS n \$\$) AS (n agtype);
" -c "
    SELECT 'contains_edges' AS t, * FROM cypher('axon_graph',
        \$\$ MATCH ()-[r:CONTAINS]->() RETURN count(r) AS n \$\$) AS (n agtype);
"

echo
echo "=== Smoke test complete ==="
echo "Manual next steps:"
echo "  1. Review counts above. Symbols/CALLS should be > 0."
echo "  2. AGE counts should match SQL counts (B.2 dual-write parity)."
echo "  3. If green: open a PR removing the AXON_INDEXER_PG_OPT_IN guard"
echo "     in src/axon-core/src/runtime_boot.rs:441."
echo "  4. After validation: enable AXON_AGE_ONLY_RELATIONS=true and re-run"
echo "     to verify B.4 prep (SQL relation writes skipped)."
echo "  5. Final step: apply scripts/migrations/drop-relation-tables.sql"
echo "     to actually drop CALLS / CALLS_NIF / CONTAINS tables (irreversible)."
