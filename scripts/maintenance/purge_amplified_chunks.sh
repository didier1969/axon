#!/usr/bin/env bash
# scripts/maintenance/purge_amplified_chunks.sh — REQ-AXO-902335 tranche 2
#
# Purge the chunks of files the pre-fix chunker amplified, so the fixed chunker
# rebuilds them. REPORTS by default; deletes only with --execute.
#
# Why this exists
# ---------------
# Before commit cf2762f3 the per-chunk prefix was unbounded: `TextParser` stores
# the WHOLE FILE in `Symbol.docstring` (parser/text.rs) and `format_chunk_content`
# prepends the docstring to EVERY chunk. Two shapes came out of that, both
# measured on the live index on 2026-08-15:
#
#   * budget collapse — `5-galaxy38.txt`: 7 411 bytes ⇒ 945 chunks of 7 517 bytes
#     (the whole file, 945 times). `384.saturating_sub(2506).max(8)` floored
#     `body_budget` at 8, so the fan-out became the LINE COUNT.
#   * coarse path — `model_b_6.txt` (KKI): 1 945 033 bytes ⇒ 256 chunks of
#     1 952 771 bytes each. **500 MB stored for a 1.9 MB file.**
#
# Corpus-wide: 453 files (0.9 %) held 149 608 chunks — 23 % of the index — and
# 999 M of 1 178 M tokens, i.e. **85 % of all embedding work**. Beyond the waste,
# hundreds of near-identical vectors per file dominate every ANN result.
#
# Why the fix alone does not repair it
# ------------------------------------
# `IndexedFileCache::should_index` compares the FILE's content hash and nothing
# else — no chunker-version component (indexed_file_cache.rs:128). Changing the
# chunker changes no hash, so pipeline A skips every one of these files forever.
# The bad chunks have to be deleted for the fixed chunker to see the files again.
#
# What one DELETE does
# --------------------
# `ist.indexedfile` is the FK parent of `ist.chunk`, which is the FK parent of
# `ist.chunkembedding`, both ON DELETE CASCADE (verified against
# information_schema). Deleting the IndexedFile row therefore removes the chunks
# AND their embeddings in one statement — no manual ordering to get wrong.
#
# The RAM cache is the other half (REQ-AXO-902262)
# ------------------------------------------------
# The map that DECIDES whether a file is re-read lives in the indexer's RAM,
# hydrated once at boot. Wiping PG alone leaves it untouched and A1 keeps
# answering "unchanged, skip" for files whose chunks were just deleted — measured
# on LLL: 434/434 chunked files fell to 2/438 with NO automatic recovery. So the
# purge also emits `NOTIFY ist_cache_invalidate, '<prefix>'`, which purges the
# in-RAM entries under that prefix and wakes the reconciliation walk immediately
# instead of waiting out its 900 s period (REQ-AXO-902268).
#
# Usage
#   bash scripts/maintenance/purge_amplified_chunks.sh                 # report
#   bash scripts/maintenance/purge_amplified_chunks.sh --execute       # purge
#   bash scripts/maintenance/purge_amplified_chunks.sh --toast-scan    # REQ-AXO-902336
#
# Exit: 0 ok · 1 guard refused · 2 usage
set -euo pipefail

DB_URL="${AXON_LIVE_DATABASE_URL:-postgres://axon@127.0.0.1:44144/axon_live}"
FIX_COMMIT="${AXON_CHUNKER_FIX_COMMIT:-cf2762f3}"
# Amplification above which a file's chunks are considered prefix-duplicated.
# 100× is far above anything legitimate: a healthy file sits near 1× (its chunks
# partition it, they do not each contain it). The measured population is
# bimodal — 47 879 files under 10×, 453 files above 100× — so nothing real sits
# near this line.
AMPLIFICATION_FLOOR="${AXON_AMPLIFICATION_FLOOR:-100}"
# Runaway backstop. The measured set is 0.9 % of files; a designation that
# suddenly claims a third of the corpus means the criterion or the data changed,
# and the answer then is to look, not to delete.
MAX_PURGE_PCT="${AXON_MAX_PURGE_PCT:-10}"

MODE="report"
case "${1:-}" in
    "")           MODE="report" ;;
    --execute)    MODE="execute" ;;
    --toast-scan) MODE="toast" ;;
    -h|--help)    sed -n '2,45p' "$0"; exit 0 ;;
    *)            printf 'unknown argument: %s\n' "$1" >&2; exit 2 ;;
esac

# REQ-AXO-902340 — the designation criterion is selectable via AXON_PURGE_CRITERION.
# `amplification` (default) catches prefix-DUPLICATED chunks (REQ-AXO-902335): each
# chunk re-contains the file, amplification ≫ 1. `oversized` catches chunks LARGER
# than the model window (max token_count > WINDOW): they partition the file fine
# (amplification ~1.4×) but the embedder TRUNCATES each to 512 tokens, so ~90 % of a
# big file is never vectorised — indexed, shown 100 % covered, invisible to search.
# Same delete + RAM-invalidate machinery; only the WHERE changes, kept DRY by
# parameterising the single designation CTE.
CRITERION="${AXON_PURGE_CRITERION:-amplification}"
WINDOW="${AXON_CHUNK_WINDOW:-512}"
case "$CRITERION" in
    amplification) SQL_WHERE="amplification >= $AMPLIFICATION_FLOOR"
                   CRIT_DESC="Σ(token_count) × 4 / file size  >=  ${AMPLIFICATION_FLOOR}×"
                   METRIC_ORDER="amplification"; METRIC_UNIT="×" ;;
    oversized)     SQL_WHERE="max_tok > $WINDOW"
                   CRIT_DESC="max(token_count per file)  >  ${WINDOW} tokens (embedder truncation)"
                   METRIC_ORDER="max_tok"; METRIC_UNIT=" tok" ;;
    *)             printf 'unknown AXON_PURGE_CRITERION: %s (want amplification|oversized)\n' "$CRITERION" >&2; exit 2 ;;
esac

psql_q() { psql "$DB_URL" -At -F'|' -v ON_ERROR_STOP=1 -c "$1"; }

# --- The designation, defined ONCE and reused by report / count / delete ------
# GUI-PRO-013: two copies of this predicate is how the report and the delete
# drift apart, and the delete is the one you cannot take back.
read -r -d '' DOOMED_CTE <<'SQL' || true
WITH per_file AS (
    SELECT file_path, count(*) AS chunks, sum(token_count) AS tokens, max(token_count) AS max_tok
    FROM ist.chunk
    GROUP BY file_path
),
judged AS (
    SELECT p.file_path,
           f.project_code,
           f.size_bytes,
           p.chunks,
           p.tokens,
           p.max_tok,
           (p.tokens * 4.0) / f.size_bytes AS amplification
    FROM per_file p
    JOIN ist.indexedfile f ON f.path = p.file_path
    WHERE f.size_bytes > 0
),
doomed AS (
    SELECT * FROM judged WHERE :where
)
SQL

# --- Guard 1: the installed indexer must carry the fix -----------------------
# Purging before the fixed binary is live makes the LIVE indexer rebuild the same
# bad chunks within one reconciliation sweep: strictly wasted work plus a
# needless re-embed storm. This is the guard that makes the ordering enforced
# rather than remembered.
#
# Reads the release manifest and hashes the installed binary. Two paths were
# tried first and rejected, both worth naming so nobody re-tries them:
#
#   * `tools/call embedding_status` over raw HTTP returns only `content[0].text`
#     (markdown) — `structuredContent.indexer_build_id` is visible to an MCP
#     client and NOT to curl. The first version of this guard read nothing and
#     blamed "live MCP down?" while the live was answering `readyz` normally: a
#     guard that misnames its own failure, the exact defect class this whole REQ
#     is about.
#   * `bin/axon-indexer --version` — there is no such flag, so the binary treats
#     it as no-op args and BOOTS THE RUNTIME. Running it contended for
#     `.axon/graph_v2/.axon-ist.writer.lock` and restarted the LIVE indexer
#     (observed 2026-08-15 21:46). Never invoke a role binary to ask it a
#     question. Tracked as REQ-AXO-902338.
guard_indexer_has_fix() {
    local manifest build_id sha declared actual
    manifest=".axon/live-release/current.json"
    if [[ ! -r "$manifest" ]]; then
        printf '  ❌ GUARD: %s unreadable — cannot tell which build is installed.\n' "$manifest"
        return 1
    fi

    build_id="$(python3 -c "
import json,sys
d=json.load(open('$manifest'))
print((d.get('runtime_version') or {}).get('build_id') or '')" 2>/dev/null)"
    declared="$(python3 -c "
import json,sys
d=json.load(open('$manifest'))
print(((d.get('artifacts') or {}).get('axon-indexer') or {}).get('sha256') or '')" 2>/dev/null)"

    if [[ -z "$build_id" || -z "$declared" ]]; then
        printf '  ❌ GUARD: manifest has no runtime_version.build_id / axon-indexer sha256.\n'
        return 1
    fi

    # The manifest describes what a promote INSTALLED. Hash the binary that is
    # actually in bin/ so a hand-copied one (which PIL-AXO-005 forbids and which
    # is precisely how this guard could be fooled) is caught rather than trusted.
    actual="$(sha256sum bin/axon-indexer 2>/dev/null | cut -d' ' -f1)"
    if [[ "$actual" != "$declared" ]]; then
        printf '  ❌ GUARD: bin/axon-indexer does not match the manifest.\n'
        printf '     manifest : %s\n     on disk  : %s\n' "$declared" "${actual:-<unreadable>}"
        printf '     The manifest describes a build that is not the one installed, so its\n'
        printf '     build_id says nothing about the code that would rebuild these chunks.\n'
        return 1
    fi

    # `v0.8.0-1482-gff3b99cf` → `ff3b99cf`
    sha="${build_id##*-g}"
    printf '  installed indexer  : %s (commit %s, sha256 verified)\n' "$build_id" "$sha"

    if ! git cat-file -e "${sha}^{commit}" 2>/dev/null; then
        printf '  ❌ GUARD: commit %s is unknown to this repository.\n' "$sha"
        return 1
    fi
    if git merge-base --is-ancestor "$FIX_COMMIT" "$sha" 2>/dev/null; then
        printf '  chunker fix %s : ✅ present in the installed indexer\n' "$FIX_COMMIT"
        return 0
    fi
    printf '  ❌ GUARD: the installed indexer does NOT contain %s.\n' "$FIX_COMMIT"
    printf '     Purging now would have THIS binary rebuild the same amplified chunks.\n'
    printf '     Promote first:  bash scripts/release/promote_live_safe.sh --project AXO\n'
    return 1
}

# --- Report -------------------------------------------------------------------
report() {
    printf '\n== chunk purge — designation report (criterion: %s) ==\n\n' "$CRITERION"
    printf '  criterion          : %s\n' "$CRIT_DESC"
    printf '  database           : %s\n\n' "$DB_URL"

    printf -- '--- corpus totals ---\n'
    psql_q "
${DOOMED_CTE//:where/$SQL_WHERE}
SELECT 'files designated:  ' || count(*)
     || E'\n' || 'chunks to delete:  ' || coalesce(sum(chunks), 0)
     || E'\n' || 'tokens reclaimed:  ' || coalesce(sum(tokens), 0)
     || E'\n' || 'worst file:        ' || coalesce(round(max($METRIC_ORDER))::text, 'n/a') || '$METRIC_UNIT'
FROM doomed;"

    printf -- '\n--- by project ---\n'
    psql_q "
${DOOMED_CTE//:where/$SQL_WHERE}
SELECT rpad(project_code, 6) || lpad(count(*)::text, 6) || ' files' ||
       lpad(sum(chunks)::text, 10) || ' chunks' ||
       lpad(round(max($METRIC_ORDER))::text, 8) || '$METRIC_UNIT worst'
FROM doomed GROUP BY project_code ORDER BY sum(chunks) DESC;"

    printf -- '\n--- 10 worst files ---\n'
    psql_q "
${DOOMED_CTE//:where/$SQL_WHERE}
SELECT rpad(project_code, 5) || lpad(size_bytes::text, 10) || 'B ' ||
       lpad(chunks::text, 6) || ' chunks ' || lpad(round($METRIC_ORDER)::text, 7) || '$METRIC_UNIT  ' ||
       split_part(file_path, '/', -1)
FROM doomed ORDER BY $METRIC_ORDER DESC LIMIT 10;"

    printf -- '\n--- share of the index ---\n'
    psql_q "
${DOOMED_CTE//:where/$SQL_WHERE}
SELECT 'designated chunks are ' ||
       round(100.0 * coalesce((SELECT sum(chunks) FROM doomed), 0)
             / (SELECT count(*) FROM ist.chunk), 2) || '% of the index';"
    printf '\n'
}

# --- Execute ------------------------------------------------------------------
execute() {
    local pct affected prefixes
    pct="$(psql_q "
${DOOMED_CTE//:where/$SQL_WHERE}
SELECT round(100.0 * coalesce((SELECT sum(chunks) FROM doomed), 0)
             / (SELECT count(*) FROM ist.chunk), 2);")"

    printf -- '--- runaway backstop ---\n'
    printf '  designated share   : %s%%   (ceiling %s%%)\n' "$pct" "$MAX_PURGE_PCT"
    if awk -v p="$pct" -v c="$MAX_PURGE_PCT" 'BEGIN{exit !(p>c)}'; then
        printf '  ❌ REFUSED: the designation claims more of the index than the ceiling.\n'
        printf '     Measured population was 0.9 %% of files / 23 %% of chunks. A jump past\n'
        printf '     the ceiling means the criterion or the data moved — look, do not delete.\n'
        printf '     Override deliberately with AXON_MAX_PURGE_PCT=<n> once you have looked.\n'
        return 1
    fi

    # Directory prefixes to invalidate in RAM. Directories, not files: the
    # listener purges by prefix, and one NOTIFY per file would be thousands of
    # round-trips for the same effect.
    prefixes="$(psql_q "
${DOOMED_CTE//:where/$SQL_WHERE}
SELECT DISTINCT regexp_replace(file_path, '/[^/]+\$', '') FROM doomed;")"

    printf -- '\n--- deleting (cascades to ist.chunk and ist.chunkembedding) ---\n'
    affected="$(psql "$DB_URL" -At -v ON_ERROR_STOP=1 -c "
${DOOMED_CTE//:where/$SQL_WHERE}
, gone AS (
    DELETE FROM ist.indexedfile f
    USING doomed d
    WHERE f.path = d.file_path
    RETURNING f.path
)
SELECT count(*) FROM gone;")"
    printf '  IndexedFile rows deleted : %s\n' "$affected"

    printf -- '\n--- invalidating the in-RAM dedup cache (REQ-AXO-902262) ---\n'
    local n=0
    while IFS= read -r prefix; do
        [[ -z "$prefix" || "$prefix" == "/" ]] && continue
        psql "$DB_URL" -At -v ON_ERROR_STOP=1 \
            -c "SELECT pg_notify('ist_cache_invalidate', '$prefix');" >/dev/null
        n=$((n + 1))
    done <<< "$prefixes"
    printf '  prefixes notified        : %s\n' "$n"
    printf '  (the listener also wakes the reconciliation walk — re-read starts now)\n'

    printf -- '\n--- what to watch ---\n'
    printf '  1. embedding_status: coverage dips, then climbs back to ~100%%\n'
    printf '  2. re-run this script in report mode — the designation must be EMPTY\n'
    printf '  3. `du` on the database: ist.chunk was 21 GB, most of it this residue\n'
    printf '  4. the chunks are REBUILT, not restored: their ids change, so any\n'
    printf '     evidence pinned to a chunk id is stale by design\n\n'
}

# --- TOAST scan (REQ-AXO-902336) ---------------------------------------------
# Locate the rows whose `content` cannot be detoasted. Measured 2026-08-15: 24
# broken TOAST values, ALL ending at chunk_seq 44 (~88 KB originals), 22 of them
# retaining only that last segment. A systematic pattern, not scattered disk rot.
#
# Reads pg_toast metadata FIRST — that costs an index scan and cannot itself trip
# the corruption, unlike a `length(content)` sweep which dies on the first bad row
# and tells you nothing about the rest.
toast_scan() {
    printf '\n== TOAST integrity scan on ist.chunk (REQ-AXO-902336) ==\n\n'
    local toastrel
    toastrel="$(psql_q "SELECT reltoastrelid::regclass::text FROM pg_class WHERE oid = 'ist.chunk'::regclass;")"
    printf '  toast relation     : %s\n\n' "$toastrel"

    printf -- '--- broken values (missing leading segments) ---\n'
    psql_q "
SELECT 'broken toast values: ' || count(*)
FROM (
    SELECT chunk_id FROM ${toastrel}
    GROUP BY chunk_id
    HAVING min(chunk_seq) <> 0 OR count(*) <> max(chunk_seq) + 1
) t;"

    printf -- '\n--- per-project readability (which projects hold them) ---\n'
    printf '    a project that ERRORS holds at least one unreadable row.\n\n'
    local projects
    projects="$(psql_q "SELECT DISTINCT project_code FROM ist.chunk ORDER BY 1;")"
    while IFS= read -r p; do
        [[ -z "$p" ]] && continue
        if psql "$DB_URL" -At -v ON_ERROR_STOP=1 \
             -c "SELECT count(length(content)) FROM ist.chunk WHERE project_code = '$p';" \
             >/dev/null 2>&1; then
            printf '  %-6s ✅ readable\n' "$p"
        else
            printf '  %-6s ❌ HOLDS UNREADABLE ROWS\n' "$p"
            # Narrow to the offending files inside that project. One statement per
            # file is slow but bounded, and only runs for a project already known
            # to be affected.
            local files
            files="$(psql_q "SELECT DISTINCT file_path FROM ist.chunk WHERE project_code = '$p' AND token_count > 5000;")"
            while IFS= read -r fp; do
                [[ -z "$fp" ]] && continue
                if ! psql "$DB_URL" -At -v ON_ERROR_STOP=1 \
                       -c "SELECT count(length(content)) FROM ist.chunk WHERE file_path = '$fp';" \
                       >/dev/null 2>&1; then
                    printf '           ↳ %s\n' "$fp"
                fi
            done <<< "$files"
        fi
    done <<< "$projects"

    printf -- '\n--- remediation ---\n'
    printf '  DELETE, not pg_surgery. The heap tuple reads fine — every non-TOAST\n'
    printf '  column answers — so `heap_force_kill`, which exists for UNREADABLE\n'
    printf '  tuples, is the wrong instrument and a destructive one. A plain DELETE\n'
    printf '  never detoasts `content`, and chunks are DERIVABLE: the indexer\n'
    printf '  rebuilds them from source.\n\n'
    printf '  Delete the IndexedFile row for each file listed above (cascades), then\n'
    printf '  NOTIFY ist_cache_invalidate on its directory — the same two steps\n'
    printf '  --execute performs. Most of these files are large document_body\n'
    printf '  symbols, so the amplification purge likely takes them out already:\n'
    printf '  re-run this scan AFTER --execute before doing anything targeted.\n\n'
}

# --- main ---------------------------------------------------------------------
printf '\n== purge_amplified_chunks — mode: %s ==\n\n' "$MODE"

if ! command -v psql >/dev/null 2>&1; then
    printf '  ❌ psql not on PATH. Run inside `devenv shell`.\n' >&2
    exit 1
fi
if ! psql "$DB_URL" -At -c 'SELECT 1' >/dev/null 2>&1; then
    printf '  ❌ cannot reach %s\n' "$DB_URL" >&2
    exit 1
fi

case "$MODE" in
    toast)
        toast_scan
        ;;
    report)
        report
        printf -- '--- ordering guard (advisory in report mode) ---\n'
        guard_indexer_has_fix || true
        printf '\n  Nothing was deleted. Re-run with --execute to purge.\n\n'
        ;;
    execute)
        printf -- '--- ordering guard ---\n'
        guard_indexer_has_fix || exit 1
        report
        execute
        ;;
esac
