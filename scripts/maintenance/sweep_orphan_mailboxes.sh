#!/usr/bin/env bash
# REQ-AXO-902386 — inventory mailboxes addressed to a project ABSENT from
# ProjectCodeRegistry, and re-route their messages to a canonical recipient.
#
# WHY THIS EXISTS: `mcp_outbox_send` accepted any `to_project` and answered
# `delivered`. APS addressed two messages to "AXON" (a typo for the canonical
# "AXO"); both were stored in a mailbox for a project that does not exist, and
# nothing told the sender. Those two were their most serious findings of the day —
# recovered only because APS noticed and re-sent (inbox 11934).
#
# The code fix (tools_mailbox.rs, reject_unknown_recipient) stops NEW messages
# landing in a ghost mailbox. It cannot move the ones already there — hence this.
#
# Measured on axon_live 2026-08-21: 146 messages across 5 ghost mailboxes.
#   AXON             2   <- the APS pair, ids 11929-11930
#   OTH PJA PJB PRJ  36 each
# The latter four are OUR OWN promote broadcasts: `to_project='*'` fans out to
# every code in the registry AT THAT MOMENT, so a code retired later leaves its
# messages behind. Not an active bug (the fan-out only ever uses registered
# codes), but the residue is real and nothing could surface it.
#
# NEVER DELETES. A message is evidence of an exchange; it is re-routed or left in
# place, never dropped. Mirrors the SOLL data policy.
#
#   bash scripts/maintenance/sweep_orphan_mailboxes.sh                    # report
#   bash scripts/maintenance/sweep_orphan_mailboxes.sh --route AXON=AXO   # re-route
set -euo pipefail

_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../lib" && pwd)"
# shellcheck source=scripts/lib/axon-pg-port.sh
source "$_LIB_DIR/axon-pg-port.sh"

DB_URL="${AXON_LIVE_DATABASE_URL:-postgres://axon@127.0.0.1:${AXON_CANONICAL_PG_PORT:?axon-pg-port.sh not sourced}/axon_live}"
ROUTE=""

while (( $# > 0 )); do
    case "$1" in
        --route) ROUTE="${2:-}"; shift 2 ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) printf 'unknown argument: %s\n' "$1" >&2; exit 2 ;;
    esac
done

psql_q() { psql "$DB_URL" -At -v ON_ERROR_STOP=1 -c "$1"; }
esc() { printf '%s' "$1" | sed "s/'/''/g"; }

printf '== orphan mailboxes (recipient absent from ProjectCodeRegistry) ==\n\n'
psql "$DB_URL" -At -F' | ' -v ON_ERROR_STOP=1 -c "
SELECT m.to_project, count(*), min(m.id), max(m.id)
  FROM axon.mailbox_message m
 WHERE NOT EXISTS (SELECT 1 FROM soll.ProjectCodeRegistry r
                    WHERE r.project_code = m.to_project)
 GROUP BY 1 ORDER BY 2 DESC"

if [[ -z "$ROUTE" ]]; then
    printf '\n(report only — re-route with --route GHOST=CANONICAL; nothing is ever deleted)\n'
    exit 0
fi

ghost="${ROUTE%%=*}"
canonical="${ROUTE#*=}"
if [[ -z "$ghost" || -z "$canonical" || "$ghost" == "$ROUTE" ]]; then
    printf 'malformed --route (expected GHOST=CANONICAL): %s\n' "$ROUTE" >&2
    exit 2
fi

# The destination must be canonical — re-routing into a second ghost mailbox
# would move the problem rather than fix it.
known="$(psql_q "SELECT count(*) FROM soll.ProjectCodeRegistry WHERE project_code = '$(esc "$canonical")'")"
if [[ "$known" == "0" ]]; then
    printf 'refusing: `%s` is not in ProjectCodeRegistry either.\n' "$canonical" >&2
    exit 1
fi

# The recipient's read cursor is per-project, so a re-routed message must land
# BEFORE the cursor is consulted again; it simply appears as unread, which is the
# intent — the recipient never saw it.
moved="$(psql_q "
    UPDATE axon.mailbox_message
       SET to_project = '$(esc "$canonical")'
     WHERE to_project = '$(esc "$ghost")'
    RETURNING 1" | wc -l)"

printf '\nre-routed %s message(s): %s -> %s (0 deleted)\n' "$moved" "$ghost" "$canonical"
printf 'They now surface as UNREAD for %s — which is accurate: nobody ever read them.\n' "$canonical"
