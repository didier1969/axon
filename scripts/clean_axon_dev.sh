#!/usr/bin/env bash
# clean_axon_dev.sh — REQ-AXO-901609 reproducibility wrapper.
#
# Truncates the canonical pipeline_v2 tables on axon_dev so benches can run
# from a known empty state. Safety-checks the target DB name to refuse any
# accidental run against axon_live.
#
# Tables truncated (CASCADE) on PG public schema:
#   public.chunkembedding
#   public.chunk
#   public.symbol
#   public.edge
#   public.indexedfile
#
# Env:
#   AXON_DEV_DATABASE_URL   default postgres://axon@127.0.0.1:44144/axon_dev
#
# Usage:
#   ./scripts/clean_axon_dev.sh           # interactive prompt
#   ./scripts/clean_axon_dev.sh --yes     # non-interactive (CI/sweep)
#   ./scripts/clean_axon_dev.sh --dry-run # just print what WOULD happen

set -euo pipefail

DB_URL="${AXON_DEV_DATABASE_URL:-postgres://axon@127.0.0.1:44144/axon_dev}"

yes=0
dry_run=0
for arg in "$@"; do
  case "$arg" in
    --yes|-y) yes=1 ;;
    --dry-run|-n) dry_run=1 ;;
    *)
      echo "Unknown arg: $arg" >&2
      echo "Usage: $0 [--yes] [--dry-run]" >&2
      exit 1
      ;;
  esac
done

# --- Safety check : refuse any DB name containing "live" or "prod". ---
db_name=$(psql "$DB_URL" -tAc 'SELECT current_database();' 2>/dev/null || true)
if [[ -z "$db_name" ]]; then
  echo "[clean_axon_dev] ERROR: could not query current_database() from $DB_URL" >&2
  exit 2
fi
case "$db_name" in
  *live*|*prod*|*production*)
    echo "[clean_axon_dev] REFUSED: target DB '$db_name' looks like a live/prod database." >&2
    echo "[clean_axon_dev]          This script only truncates axon_dev or equivalent sandboxes." >&2
    exit 3
    ;;
esac

if [[ "$db_name" != "axon_dev" ]]; then
  echo "[clean_axon_dev] WARN: target DB '$db_name' is not 'axon_dev'. Continue at your own risk." >&2
fi

# --- Pre-state ---
echo "[clean_axon_dev] target DB: $db_name"
echo "[clean_axon_dev] pre-state:"
psql "$DB_URL" -c "SELECT pg_size_pretty(pg_database_size(current_database())) AS size,
  (SELECT count(*) FROM public.chunkembedding) AS embeddings,
  (SELECT count(*) FROM public.chunk) AS chunks,
  (SELECT count(*) FROM public.symbol) AS symbols,
  (SELECT count(*) FROM public.edge) AS edges,
  (SELECT count(*) FROM public.indexedfile) AS files;"

if [[ $dry_run -eq 1 ]]; then
  echo "[clean_axon_dev] --dry-run set, would run:"
  echo "  TRUNCATE TABLE public.chunkembedding, public.chunk, public.symbol, public.edge, public.indexedfile CASCADE;"
  exit 0
fi

if [[ $yes -ne 1 ]]; then
  read -r -p "[clean_axon_dev] confirm TRUNCATE on '$db_name' [y/N]? " ans
  case "$ans" in
    y|Y|yes|YES) ;;
    *) echo "[clean_axon_dev] aborted." ; exit 0 ;;
  esac
fi

# --- TRUNCATE ---
echo "[clean_axon_dev] truncating pipeline_v2 tables…"
psql "$DB_URL" -c "TRUNCATE TABLE public.chunkembedding, public.chunk, public.symbol, public.edge, public.indexedfile CASCADE;"

# --- Post-state ---
echo "[clean_axon_dev] post-state:"
psql "$DB_URL" -c "SELECT pg_size_pretty(pg_database_size(current_database())) AS size,
  (SELECT count(*) FROM public.chunkembedding) AS embeddings,
  (SELECT count(*) FROM public.chunk) AS chunks,
  (SELECT count(*) FROM public.symbol) AS symbols,
  (SELECT count(*) FROM public.edge) AS edges,
  (SELECT count(*) FROM public.indexedfile) AS files;"

echo "[clean_axon_dev] done."
