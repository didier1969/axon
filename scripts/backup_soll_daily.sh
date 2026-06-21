#!/usr/bin/env bash
# Daily backup of the live SOLL + runtime intent schemas (axon_live).
#
# SCOPE (GUI-AXO-1025): dumps only the irreplaceable intent schemas
# (soll ~7 MB, axon ~3 MB — runtime telemetry consolidated into `axon` per
# REQ-AXO-901854) — NOT the `ist` (12 GB) or `pgmq`
# (300 MB+) schemas, which are rebuildable from source by the indexer.
# Before this scoping the script dumped the entire 12 GB DB through gzip -9,
# which never finished inside the window between brain restarts: the final
# `mv` (and the daily marker write) never ran, so every devenv enterShell
# re-triggered a fresh full dump. That piled multi-GB .partial files AND held
# an AccessShareLock on every table for ~10 min, serializing the boot DDL
# (ALTER TABLE axon_runtime.EmbedderLifecycleHeartbeat) behind it. Scoping to
# the ~10 MB intent schemas makes the dump complete in seconds, sets the
# marker, and frees the lock immediately.
#
# Idempotent: at most one dump per UTC day. Safe to invoke from cron, the
# Windows Task Scheduler, devenv shell entry, or by hand. Pass --force to
# bypass the daily marker.
#
# Retention (REQ-AXO-311) — 3-tier, supersedes the flat RETAIN_DAYS window:
#   tier 1 rolling daily : keep EVERY dump from the last ROLLING_DAYS days
#   tier 2 monthly anchor: older than that, keep the NEWEST dump per calendar
#                          month for the last MONTHLY_MONTHS months
#   tier 3 keeper        : dumps named *.keeper.sql.gz NEVER expire. Produce one
#                          with `--keeper` (called post-promote-live so each
#                          qualified release has a permanent SOLL snapshot).
# AXON_SOLL_BACKUP_RETAIN_DAYS stays as a back-compat floor: any dump within
# RETAIN_DAYS is retained even if its month already has a newer anchor.
#
# Env overrides:
#   AXON_LIVE_DATABASE_URL           (default postgres://axon@127.0.0.1:44144/axon_live)
#   AXON_SOLL_BACKUP_DIR             (default $HOME/backups/soll)
#   AXON_SOLL_BACKUP_RETAIN_DAYS     (default 30)  — back-compat floor
#   AXON_SOLL_BACKUP_ROLLING_DAYS    (default 7)
#   AXON_SOLL_BACKUP_MONTHLY_MONTHS  (default 12)

set -euo pipefail

DB_URL="${AXON_LIVE_DATABASE_URL:-postgres://axon@127.0.0.1:44144/axon_live}"
BACKUP_DIR="${AXON_SOLL_BACKUP_DIR:-${HOME}/backups/soll}"
RETAIN_DAYS="${AXON_SOLL_BACKUP_RETAIN_DAYS:-30}"
ROLLING_DAYS="${AXON_SOLL_BACKUP_ROLLING_DAYS:-7}"
MONTHLY_MONTHS="${AXON_SOLL_BACKUP_MONTHLY_MONTHS:-12}"
MARKER="${BACKUP_DIR}/.last-daily-backup"
LOCK="${BACKUP_DIR}/.daily-backup.lock"

# --force bypasses the daily marker; --keeper produces a never-expiring,
# release-anchored dump (also implies bypassing the daily marker).
force=0
keeper=0
for arg in "$@"; do
  case "${arg}" in
    --force) force=1 ;;
    --keeper) keeper=1; force=1 ;;
  esac
done

mkdir -p "${BACKUP_DIR}"

# Serialize concurrent invocations (e.g. devenv enterShell firing twice).
# fd 9 holds the lock until the script exits.
exec 9>"${LOCK}"
if ! flock -n 9; then
  echo "[backup_soll] another instance is running; skip"
  exit 0
fi

today="$(date -u +%Y%m%d)"
if [[ "${force}" -eq 0 && -f "${MARKER}" && "$(cat "${MARKER}")" == "${today}" ]]; then
  echo "[backup_soll] already ran today (${today}); skip"
  exit 0
fi

# pg_dump must match the live server major version. Multiple postgresql-and-plugins-*
# directories may exist in /nix/store from previous devenv builds.
server_major=""
psql_bin="$(command -v psql || true)"
if [[ -z "${psql_bin}" ]]; then
  psql_bin="$(ls -1d /nix/store/*-postgresql-and-plugins-*/bin/psql 2>/dev/null | sort -V | tail -1 || true)"
fi
if [[ -n "${psql_bin}" ]]; then
  server_major="$("${psql_bin}" -tAX "${DB_URL}" -c 'SHOW server_version_num' 2>/dev/null | head -1 | cut -c1-2 || true)"
fi
if [[ -z "${server_major}" ]]; then
  echo "[backup_soll] could not probe server version via psql; defaulting to highest installed pg_dump" >&2
fi

pg_dump_bin=""
if [[ -n "${server_major}" ]]; then
  pg_dump_bin="$(ls -1d /nix/store/*-postgresql-and-plugins-${server_major}.*/bin/pg_dump 2>/dev/null | sort -V | tail -1 || true)"
fi
if [[ -z "${pg_dump_bin}" ]]; then
  pg_dump_bin="$(ls -1d /nix/store/*-postgresql-and-plugins-*/bin/pg_dump 2>/dev/null | sort -V | tail -1 || true)"
fi
if [[ -z "${pg_dump_bin}" ]]; then
  pg_dump_bin="$(command -v pg_dump || true)"
fi
if [[ -z "${pg_dump_bin}" || ! -x "${pg_dump_bin}" ]]; then
  echo "[backup_soll] pg_dump not found; enter devenv shell or set PATH" >&2
  exit 1
fi
echo "[backup_soll] using pg_dump: ${pg_dump_bin}"

ts="$(date -u +%Y%m%dT%H%M%SZ)"
# keeper dumps carry a `.keeper` infix so the retention sweep never expires them.
if [[ "${keeper}" -eq 1 ]]; then
  out="${BACKUP_DIR}/axon_live-${ts}.keeper.sql.gz"
else
  out="${BACKUP_DIR}/axon_live-${ts}.sql.gz"
fi
tmp="${out}.partial"

echo "[backup_soll] dumping ${DB_URL} -> ${out}"
# REQ-AXO-901740 — capture per-stage exit codes so a pg_dump failure
# masked by gzip success becomes visible. Previous form ran the pipeline
# without pipefail : a truncated dump was indistinguishable from a clean
# one, and the only safety net was the schema-presence grep below.
set -o pipefail
# Intent schemas only — see SCOPE note in the header. `ist` and `pgmq` are
# rebuildable and would balloon the dump to 12 GB / never finish.
"${pg_dump_bin}" --no-owner --no-privileges --format=plain \
  --schema=soll --schema=axon \
  "${DB_URL}" 2>"${tmp}.pgdump.err" | gzip -9 > "${tmp}"
pipe_rc=("${PIPESTATUS[@]}")
set +o pipefail
if [[ "${pipe_rc[0]}" -ne 0 || "${pipe_rc[1]}" -ne 0 ]]; then
  echo "[backup_soll] dump pipeline failed (pg_dump rc=${pipe_rc[0]}, gzip rc=${pipe_rc[1]})" >&2
  if [[ -s "${tmp}.pgdump.err" ]]; then
    echo "[backup_soll] pg_dump stderr:" >&2
    sed 's/^/  /' "${tmp}.pgdump.err" >&2
  fi
  rm -f "${tmp}" "${tmp}.pgdump.err"
  exit 4
fi
rm -f "${tmp}.pgdump.err"

# Size sanity : a SOLL dump is never < 10 KB even on a fresh DB. A
# smaller file means the pipeline silently produced a near-empty gzip.
dump_size_bytes="$(stat -c '%s' "${tmp}" 2>/dev/null || echo 0)"
if [[ "${dump_size_bytes}" -lt 10240 ]]; then
  echo "[backup_soll] dump size suspiciously small (${dump_size_bytes} bytes < 10240); aborting" >&2
  rm -f "${tmp}"
  exit 5
fi

# Sanity check: refuse to keep a dump that does not mention the SOLL schema.
# Subshell disables pipefail so SIGPIPE from `grep -q` does not poison the rc.
if ! ( set +o pipefail; gzip -dc "${tmp}" | grep -qE 'CREATE TABLE soll\.|SCHEMA soll' ); then
  echo "[backup_soll] dump does not contain SOLL schema; aborting without rotation" >&2
  rm -f "${tmp}"
  exit 2
fi

mv "${tmp}" "${out}"
# A keeper dump must NOT satisfy the daily gate — a normal daily should still run.
[[ "${keeper}" -eq 1 ]] || echo "${today}" > "${MARKER}"

# Retention (REQ-AXO-311) — 3-tier sweep. Glob excludes keepers via the loop
# guard below (they carry a `.keeper` infix and never expire).
now_epoch="$(date -u +%s)"
rolling_cutoff=$(( now_epoch - ROLLING_DAYS * 86400 ))
retain_cutoff=$(( now_epoch - RETAIN_DAYS * 86400 ))
# YYYYMM floor for the monthly-anchor window (≈30.44 days/month).
monthly_floor_ym="$(date -u -d "@$(( now_epoch - MONTHLY_MONTHS * 2629800 ))" +%Y%m 2>/dev/null || echo 000000)"
declare -A month_anchor_seen
# Newest-first so the first file seen in a month becomes that month's anchor.
mapfile -t all_dumps < <(ls -1t "${BACKUP_DIR}"/axon_live-*.sql.gz 2>/dev/null || true)
for f in "${all_dumps[@]:-}"; do
  [[ -n "${f}" ]] || continue
  base="$(basename "${f}")"
  # tier 3 keeper: never expire.
  [[ "${base}" == *.keeper.sql.gz ]] && continue
  ts_raw="${base#axon_live-}"; ts_raw="${ts_raw%.sql.gz}"
  ymd="${ts_raw%%T*}"           # YYYYMMDD
  ym="${ymd:0:6}"              # YYYYMM
  f_epoch="$(date -u -d "${ymd}" +%s 2>/dev/null || echo 0)"
  # tier 1 rolling daily (and the back-compat RETAIN_DAYS floor): keep all.
  if [[ "${f_epoch}" -ge "${rolling_cutoff}" || "${f_epoch}" -ge "${retain_cutoff}" ]]; then
    continue
  fi
  # tier 2 monthly anchor: within the window, keep the newest dump per month.
  if [[ ! "${ym}" < "${monthly_floor_ym}" && -z "${month_anchor_seen[${ym}]:-}" ]]; then
    month_anchor_seen[${ym}]=1
    continue
  fi
  echo "[backup_soll] retention(3-tier): removing ${base}"
  rm -f "${f}"
done

size="$(du -h "${out}" | cut -f1)"
echo "[backup_soll] done — ${out} (${size})"
