#!/usr/bin/env bash
# REQ-AXO-902365 — requeue chunks stuck in embed_status='failed', IN PACED BATCHES.
#
# WHY THIS EXISTS: ~233k chunks sat in embed_status='failed' with no vector.
#
# THE POISON (measured 2026-08-20): BGE-Large has max_position_embeddings=512. A chunk
# above that is OUT OF CONTRACT, not merely expensive — the ORT run fails on
# `/embeddings/word_embeddings/Gather` (out-of-range index), and stage_b2 fails the
# WHOLE batch. So ONE oversized chunk kills its 64 healthy neighbours. The arithmetic
# closes: 3640 oversized x 64 = 232960 vs 232879 failed measured (one partial batch).
# Confirmed the other way round: no chunk >512 has EVER embedded (embedded max = 512).
#
# Hence the requeue EXCLUDES oversized chunks and SAYS how many it skipped. Requeueing
# them re-poisons the queue and re-creates the incident. They stay failed until the
# chunker stops emitting them (REQ-AXO-902364) — that REQ is the CAUSE of this one.
#
# An earlier, DIFFERENT failure mode on the same day (REQ-AXO-902365): the mass reindex
# triggered an autovacuum storm and the embedder's own status UPDATE blew its
# statement_timeout (57014). Real, but not what keeps chunks failed now. Both guards
# below are kept — the autovacuum precondition was manual in the REQ, here it is WIRED
# (GUI-PRO-118: a hand gesture is not delivered).
#
# Reports by default; mutates only with --execute.
#
#   bash scripts/maintenance/reset_failed_embeddings.sh              # report
#   bash scripts/maintenance/reset_failed_embeddings.sh --execute    # requeue, paced
#
# Knobs: AXON_RESET_BATCH_SIZE (default 20000), AXON_RESET_DRAIN_FLOOR (2000),
#        AXON_RESET_DRAIN_TIMEOUT seconds per batch (900),
#        AXON_EMBED_MAX_TOKENS (512 = BGE-Large max_position_embeddings).
set -euo pipefail

_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../lib" && pwd)"
# shellcheck source=scripts/lib/axon-pg-port.sh
source "$_LIB_DIR/axon-pg-port.sh"

DB_URL="${AXON_LIVE_DATABASE_URL:-postgres://axon@127.0.0.1:${AXON_CANONICAL_PG_PORT:?axon-pg-port.sh not sourced}/axon_live}"
BATCH="${AXON_RESET_BATCH_SIZE:-20000}"
DRAIN_FLOOR="${AXON_RESET_DRAIN_FLOOR:-2000}"
DRAIN_TIMEOUT="${AXON_RESET_DRAIN_TIMEOUT:-900}"
# Model context window. Chunks above it cannot embed and poison their whole batch.
MAX_TOKENS="${AXON_EMBED_MAX_TOKENS:-512}"
MODE="report"

while (( $# > 0 )); do
    case "$1" in
        --execute) MODE="execute" ;;
        --report)  MODE="report" ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) printf 'unknown argument: %s\n' "$1" >&2; exit 2 ;;
    esac
    shift
done

psql_q() { psql "$DB_URL" -At -v ON_ERROR_STOP=1 -c "$1"; }

# All THREE terminal states, never just the active one. The 2026-08-20 monitor watched
# `pending` alone, saw it hit zero and declared DRAINED while 237k sat in `failed`.
counts() {
    psql "$DB_URL" -At -F'=' -v ON_ERROR_STOP=1 -c \
        "SELECT embed_status, count(*) FROM ist.chunk GROUP BY embed_status ORDER BY embed_status" \
        | paste -sd' ' -
}

count_of() { psql_q "SELECT count(*) FROM ist.chunk WHERE embed_status = '$1'"; }

# Failed chunks the model can actually accept. The loop MUST count these, not every
# failed row: the oversized ones are never requeued, so counting them would spin forever.
count_eligible() {
    psql_q "SELECT count(*) FROM ist.chunk
             WHERE embed_status = 'failed' AND token_count <= ${MAX_TOKENS}"
}
count_oversized_failed() {
    psql_q "SELECT count(*) FROM ist.chunk
             WHERE embed_status = 'failed' AND token_count > ${MAX_TOKENS}"
}

# Autovacuum running on the chunk tables = do not touch anything yet.
autovacuum_busy() {
    local n
    n="$(psql_q "SELECT count(*) FROM pg_stat_activity
                  WHERE query LIKE 'autovacuum:%'
                    AND (query LIKE '%ist.chunk%' OR query LIKE '%chunkembedding%')")"
    [[ "$n" != "0" ]]
}

wait_for_quiet_autovacuum() {
    local deadline=$(( SECONDS + DRAIN_TIMEOUT ))
    while autovacuum_busy; do
        if (( SECONDS >= deadline )); then
            printf '  ! autovacuum still running on the chunk tables after %ss — stopping here.\n' "$DRAIN_TIMEOUT"
            printf '    Resetting under autovacuum re-times-out immediately (verified). Re-run later.\n'
            return 1
        fi
        printf '  . autovacuum busy on chunk tables — waiting\n'
        sleep 30
    done
    return 0
}

# Wait for the embedder to work the batch off before queueing the next one.
wait_for_drain() {
    local deadline=$(( SECONDS + DRAIN_TIMEOUT )) pending
    while true; do
        pending="$(count_of pending)"
        (( pending <= DRAIN_FLOOR )) && return 0
        if (( SECONDS >= deadline )); then
            printf '  ! drain timeout (%ss, pending=%s) — stopping to avoid stacking work.\n' \
                "$DRAIN_TIMEOUT" "$pending"
            return 1
        fi
        sleep 15
    done
}

printf '== reset failed embeddings (mode: %s, batch: %s) ==\n\n' "$MODE" "$BATCH"
printf 'states now: %s\n' "$(counts)"

failed_total="$(count_eligible)"
oversized="$(count_oversized_failed)"
printf 'failed and requeueable (<= %s tokens): %s\n' "$MAX_TOKENS" "$failed_total"
# Never silent: an excluded population that nobody prints reads as "all handled".
printf 'failed but OVERSIZED (> %s tokens), left alone: %s  <- cause: REQ-AXO-902364\n' \
    "$MAX_TOKENS" "$oversized"

if [[ "$failed_total" == "0" ]]; then
    printf '\nnothing requeueable.\n'; exit 0
fi

if autovacuum_busy; then
    printf '\nBLOCKED: autovacuum is running on the chunk tables. Resetting now would re-time-out.\n'
    [[ "$MODE" == "report" ]] || exit 1
fi

if [[ "$MODE" == "report" ]]; then
    printf '\n(report only — re-run with --execute to requeue in batches of %s)\n' "$BATCH"
    exit 0
fi

batch_no=0
while true; do
    remaining="$(count_eligible)"
    (( remaining == 0 )) && break
    batch_no=$(( batch_no + 1 ))

    wait_for_quiet_autovacuum || exit 1

    moved="$(psql "$DB_URL" -At -v ON_ERROR_STOP=1 -c "
        WITH batch AS (
            SELECT id FROM ist.chunk
             WHERE embed_status = 'failed'
               AND token_count <= ${MAX_TOKENS}
             ORDER BY id
             LIMIT ${BATCH}
             FOR UPDATE SKIP LOCKED
        )
        UPDATE ist.chunk c
           SET embed_status = 'pending', embed_attempts = 0
          FROM batch b
         WHERE c.id = b.id
        RETURNING 1;" | wc -l)"

    psql_q "SELECT pg_notify('chunk_pending_embed', 'recovery-batch-${batch_no}')" >/dev/null

    printf 'batch %s: requeued %s (requeueable left: %s) | %s\n' \
        "$batch_no" "$moved" "$(count_eligible)" "$(counts)"

    wait_for_drain || exit 1
done

printf '\ndone. states: %s\n' "$(counts)"
printf 'still failed because oversized (> %s tokens): %s — fix the chunker (REQ-AXO-902364)\n' \
    "$MAX_TOKENS" "$(count_oversized_failed)"
