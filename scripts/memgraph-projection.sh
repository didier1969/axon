#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
COMPOSE_FILE="$ROOT_DIR/docker-compose.memgraph.yml"

usage() {
  cat <<'USAGE'
Usage: scripts/memgraph-projection.sh <command> [options]

Commands:
  start                         Start Memgraph + Lab with Docker Compose
  stop                          Stop Memgraph + Lab
  status                        Show Docker Compose status
  build-query-pack [--out FILE]  Build the standalone Lab-visible PreparedQuery bootstrap file
  build-import --publication-dir DIR [--out FILE] [--batch-size N]
  validate --publication-dir DIR [--require-import-file]
  load --publication-dir DIR     Load generated memgraph_import.cypherl through mgconsole container
  query-pack-status              Show installed PreparedQuery pack count from active Memgraph
  smoke-queries [--query-dir DIR] [--mode explain|execute]
                                Validate the prepared human query pack; default is compact EXPLAIN

This is a human-only visualization path. LLM clients must use Axon MCP.
USAGE
}

need_docker() {
  if ! docker info >/dev/null 2>&1; then
    echo "Docker daemon is unavailable. Start Docker, then retry." >&2
    exit 2
  fi
}

cmd="${1:-}"
shift || true

case "$cmd" in
  start)
    need_docker
    python3 "$SCRIPT_DIR/memgraph_build_query_pack.py" \
      --out "$ROOT_DIR/queries/memgraph/bootstrap/axon_query_pack.cypherl" >/dev/null
    exec docker compose -f "$COMPOSE_FILE" up -d
    ;;
  stop)
    need_docker
    exec docker compose -f "$COMPOSE_FILE" down
    ;;
  status)
    need_docker
    exec docker compose -f "$COMPOSE_FILE" ps
    ;;
  build-import)
    exec python3 "$SCRIPT_DIR/memgraph_build_cypherl.py" "$@"
    ;;
  validate)
    exec python3 "$SCRIPT_DIR/memgraph_validate_publication.py" "$@"
    ;;
  build-query-pack)
    exec python3 "$SCRIPT_DIR/memgraph_build_query_pack.py" "$@"
    ;;
  load)
    need_docker
    publication_dir=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --publication-dir)
          publication_dir="${2:-}"
          shift 2
          ;;
        --publication-dir=*)
          publication_dir="${1#*=}"
          shift
          ;;
        *)
          echo "unknown option for load: $1" >&2
          usage
          exit 1
          ;;
      esac
    done
    if [[ -z "$publication_dir" ]]; then
      echo "load requires --publication-dir DIR" >&2
      exit 1
    fi
    import_file="$publication_dir/memgraph_import.cypherl"
    if [[ ! -f "$import_file" ]]; then
      python3 "$SCRIPT_DIR/memgraph_build_cypherl.py" --publication-dir "$publication_dir" --out "$import_file"
    fi
    load_out="$(mktemp /tmp/axon_memgraph_load.XXXXXX.out)"
    trap 'rm -f "$load_out"' EXIT
    max_attempts="${AXON_MEMGRAPH_LOAD_ATTEMPTS:-3}"
    attempt=1
    while true; do
      if docker run --rm -i --network container:axon-memgraph "${AXON_MGCONSOLE_IMAGE:-memgraph/mgconsole:1.5.0}" < "$import_file" >"$load_out" 2>&1; then
        cat "$load_out"
        break
      fi
      if grep -q "storage.access_timeout" "$load_out" && [[ "$attempt" -lt "$max_attempts" ]]; then
        echo "Memgraph storage access timeout during load; retrying ($attempt/$max_attempts)..." >&2
        sleep 5
        attempt=$((attempt + 1))
        continue
      fi
      cat "$load_out" >&2
      exit 1
    done
    ;;
  query-pack-status)
    need_docker
    docker run --rm -i --network container:axon-memgraph "${AXON_MGCONSOLE_IMAGE:-memgraph/mgconsole:1.5.0}" <<'CYPHER'
MATCH (p:PreparedQueryPack)
OPTIONAL MATCH (p)-[:HAS_PREPARED_QUERY]->(q:PreparedQuery)
RETURN p.id AS pack_id, p.publication_id AS publication_id, count(q) AS installed_prepared_queries;
CYPHER
    ;;
  smoke-queries)
    need_docker
    query_dir="$ROOT_DIR/queries/memgraph"
    smoke_mode="explain"
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --query-dir)
          query_dir="${2:-}"
          shift 2
          ;;
        --query-dir=*)
          query_dir="${1#*=}"
          shift
          ;;
        --mode)
          smoke_mode="${2:-}"
          shift 2
          ;;
        --mode=*)
          smoke_mode="${1#*=}"
          shift
          ;;
        *)
          echo "unknown option for smoke-queries: $1" >&2
          usage
          exit 1
          ;;
      esac
    done
    if [[ ! -d "$query_dir" ]]; then
      echo "query directory does not exist: $query_dir" >&2
      exit 1
    fi
    if [[ "$smoke_mode" != "explain" && "$smoke_mode" != "execute" ]]; then
      echo "smoke-queries --mode must be explain or execute" >&2
      exit 1
    fi
    shopt -s nullglob
    query_files=("$query_dir"/*.cypher "$query_dir"/catalog/*.cypher)
    if [[ ${#query_files[@]} -eq 0 ]]; then
      echo "no .cypher query files found in $query_dir" >&2
      exit 1
    fi
    tmp_query="$(mktemp /tmp/axon_memgraph_query_smoke.XXXXXX.cypher)"
    tmp_out="$(mktemp /tmp/axon_memgraph_query_smoke.XXXXXX.out)"
    trap 'rm -f "$tmp_query" "$tmp_out"' EXIT
    for query_file in "${query_files[@]}"; do
      rel_path="${query_file#$query_dir/}"
      python3 - "$query_file" "$tmp_query" "$smoke_mode" <<'PY'
from pathlib import Path
import sys

source = Path(sys.argv[1])
target = Path(sys.argv[2])
mode = sys.argv[3]
query = "\n".join(
    line for line in source.read_text(encoding="utf-8").splitlines()
    if not line.lstrip().startswith("//")
).strip()
query = (
    query
    .replace("$project_code", '""')
    .replace("$target", '"Axon"')
    .replace("$min_degree", "25")
    .replace("$limit", "100")
)
if mode == "explain":
    query = "EXPLAIN " + query
target.write_text(query + "\n", encoding="utf-8")
PY
      docker run --rm -i --network container:axon-memgraph "${AXON_MGCONSOLE_IMAGE:-memgraph/mgconsole:1.5.0}" < "$tmp_query" >"$tmp_out"
      echo "ok $rel_path"
    done
    echo "memgraph query pack smoke passed (${#query_files[@]} queries, mode=$smoke_mode)"
    ;;
  -h|--help|help|"")
    usage
    ;;
  *)
    echo "unknown command: $cmd" >&2
    usage
    exit 1
    ;;
esac
