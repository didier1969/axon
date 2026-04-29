#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
default_db="$repo_root/.axon/graph_v2/ist.db"
db_path="${AXON_IST_DB_PATH:-$default_db}"

if [[ $# -eq 0 ]]; then
  cat <<EOF
usage: scripts/ist-query.sh [--db PATH] [--format json|csv|tsv] [--preset NAME] <SQL>

defaults:
  db: $db_path

examples:
  scripts/ist-query.sh --format csv "select count(*) as files from File"
  AXON_INSTANCE_KIND=dev python3 scripts/benchmark-query.py --preset recent-vector-batches
EOF
  exit 1
fi

args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --db)
      shift
      db_path="${1:?missing path after --db}"
      ;;
    *)
      args+=("$1")
      ;;
  esac
  shift || true
done

cd "$repo_root"
exec cargo run --quiet --manifest-path src/axon-plugin-duckdb/Cargo.toml --bin ist-query -- --db "$db_path" "${args[@]}"
