#!/usr/bin/env bash
# REQ-AXO-902390 — re-type evidence rows whose artifact_type contradicts the SHAPE
# of their artifact_ref.
#
# WHY THIS EXISTS: `normalize_evidence_artifact_type` used `Document` as its
# fallback bucket — any ref without a `/` or a `.md` suffix landed there. So commit
# hashes and SOLL ids were stored as `Document`, and `broken_file_evidence` then
# stat()ed them as filesystem paths and reported them missing.
#
# Measured on axon_live 2026-08-20, BEFORE the code fix:
#   Commit    / hash git      6058   correctly typed, never disk-checked
#   File      / absolute path 3935   correctly typed
#   Document  / hash git       493   MIS-TYPED -> reported broken
#   Document  / SOLL id        113   MIS-TYPED -> reported broken
#   1173 rows carried artifact_status='broken'
#
# APS hit the same defect and checked all 22 of their "broken" refs BY HAND: 21
# were valid (inbox 12093). The tool's own suggested remedy,
# `soll_remove_evidence(broken_only=true)`, would have DELETED them.
#
# The code fix stops NEW rows being mis-typed. It cannot repair rows already
# written — hence this script.
#
# NEVER DELETES. SOLL evidence is never removed, only re-typed (Data Policy), and
# `artifact_status` is cleared so the next verify re-evaluates from scratch
# instead of trusting a verdict computed under the old rule.
#
#   bash scripts/maintenance/retype_evidence_refs.sh              # report
#   bash scripts/maintenance/retype_evidence_refs.sh --execute    # re-type
set -euo pipefail

_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../lib" && pwd)"
# shellcheck source=scripts/lib/axon-pg-port.sh
source "$_LIB_DIR/axon-pg-port.sh"

DB_URL="${AXON_LIVE_DATABASE_URL:-postgres://axon@127.0.0.1:${AXON_CANONICAL_PG_PORT:?axon-pg-port.sh not sourced}/axon_live}"
MODE="report"

while (( $# > 0 )); do
    case "$1" in
        --execute) MODE="execute" ;;
        --report)  MODE="report" ;;
        -h|--help) sed -n '2,28p' "$0"; exit 0 ;;
        *) printf 'unknown argument: %s\n' "$1" >&2; exit 2 ;;
    esac
    shift
done

psql_q() { psql "$DB_URL" -At -v ON_ERROR_STOP=1 -c "$1"; }

# The SAME shape rules as `classify_artifact_ref` (mcp/tools_soll/shared.rs). Kept
# in lockstep with it: a hash is 7-40 lowercase hex and nothing else; a SOLL id is
# TYPE-PROJ-N (DEC-AXO-085) or a `SOLL:` reference.
HASH_RE="^(git:)?([0-9a-f]{7,40}|(ORIG_|FETCH_|MERGE_)?HEAD([~^][0-9]*)?)$"
SOLL_RE="^([A-Z]{3,}-[A-Z0-9]{3}-[0-9]+|SOLL:.*)"
# A ref containing whitespace is a provenance note or a shell command recorded in
# the ref column, not a path. Found among the rows STILL flagged broken after the
# first pass: `mix compile --warnings-as-errors`,
# `axon-dev-brain tmux 2026-05-23T04:38:43`. Real evidence, wrong column — the
# status is cleared so the sweep stops calling them missing files, and the row is
# retyped `Note` so its nature is recorded rather than guessed again next time.
NOTE_RE="[[:space:]]"
# `scheme:value` with the scheme BEFORE any separator: structured, never a path.
# Generalised rather than enumerated — `git:`, `commit:`, `live:`,
# `disposition:` were each found on a separate pass, so the vocabulary is open.
SCHEME_RE="^[A-Za-z][A-Za-z0-9_-]*:"
# No separator at all: a bare label (`live-verification`, `dogfood-live-axon`).
# Not a path either, so a disk check can say nothing about it.
LABEL_RE="^[^/\\.:[:space:]]+$"

printf '== re-type evidence refs (mode: %s) ==\n\n' "$MODE"

printf 'mis-typed rows, by target type:\n'
psql "$DB_URL" -At -F' -> ' -v ON_ERROR_STOP=1 -c "
SELECT CASE WHEN artifact_ref ~ '${HASH_RE}' THEN 'Commit' ELSE 'SollRef' END AS target,
       count(*)
  FROM soll.Traceability
 WHERE lower(artifact_type) IN ('file','document')
   AND (artifact_ref ~ '${HASH_RE}' OR artifact_ref ~ '${SOLL_RE}')
 GROUP BY 1 ORDER BY 2 DESC"

printf '\nrows currently marked broken: %s\n' \
    "$(psql_q "SELECT count(*) FROM soll.Traceability WHERE artifact_status = 'broken'")"

if [[ "$MODE" == "report" ]]; then
    printf '\n(report only — re-run with --execute to re-type; nothing is ever deleted)\n'
    exit 0
fi

# artifact_status is CLEARED, not recomputed here: the verify path owns that
# decision, and a stale 'broken' verdict computed under the old rule would
# otherwise survive the re-typing and keep the row in the offender list.
commits="$(psql_q "
    UPDATE soll.Traceability
       SET artifact_type = 'Commit', artifact_status = NULL, artifact_checked_at = NULL
     WHERE lower(artifact_type) IN ('file','document')
       AND artifact_ref ~ '${HASH_RE}'
    RETURNING 1" | wc -l)"

soll_refs="$(psql_q "
    UPDATE soll.Traceability
       SET artifact_type = 'SollRef', artifact_status = NULL, artifact_checked_at = NULL
     WHERE lower(artifact_type) IN ('file','document')
       AND artifact_ref ~ '${SOLL_RE}'
    RETURNING 1" | wc -l)"

urls="$(psql_q "
    UPDATE soll.Traceability
       SET artifact_type = 'Url', artifact_status = NULL, artifact_checked_at = NULL
     WHERE lower(artifact_type) IN ('file','document')
       AND artifact_ref ~ '^https?://'
    RETURNING 1" | wc -l)"

notes="$(psql_q "
    UPDATE soll.Traceability
       SET artifact_type = 'Note', artifact_status = NULL, artifact_checked_at = NULL
     WHERE lower(artifact_type) IN ('file','document')
       AND artifact_ref ~ '${NOTE_RE}'
    RETURNING 1" | wc -l)"

# Structured refs and bare labels: type recorded, status CLEARED. Their nature is
# now stored instead of being re-guessed — and re-guessed wrong — on every sweep.
structured="$(psql_q "
    UPDATE soll.Traceability
       SET artifact_type = 'Ref', artifact_status = NULL, artifact_checked_at = NULL
     WHERE lower(artifact_type) IN ('file','document')
       AND artifact_ref ~ '${SCHEME_RE}'
    RETURNING 1" | wc -l)"

labels="$(psql_q "
    UPDATE soll.Traceability
       SET artifact_type = 'Label', artifact_status = NULL, artifact_checked_at = NULL
     WHERE lower(artifact_type) IN ('file','document')
       AND artifact_ref ~ '${LABEL_RE}'
    RETURNING 1" | wc -l)"

printf '\nre-typed: %s -> Commit, %s -> SollRef, %s -> Url, %s -> Note, %s -> Ref, %s -> Label (0 deleted)\n' \
    "$commits" "$soll_refs" "$urls" "$notes" "$structured" "$labels"
printf 'rows still marked broken: %s\n' \
    "$(psql_q "SELECT count(*) FROM soll.Traceability WHERE artifact_status = 'broken'")"
printf 'Remaining broken rows are now REAL missing paths — verify before removing any.\n'
