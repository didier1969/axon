#!/usr/bin/env bash
# =============================================================================
# scripts/governance/vacuum_bloat_audit.sh
#
# REQ-AXO-902611 — Gouvernance du stockage PostgreSQL Axon sous réserve Nexus
#
# Diagnostique l'empreinte disque des tables et index (FTS GIN, pgvector HNSW, btree),
# mesure le bloat (dead tuples), surveille la réserve disque minimale et fournit
# les commandes de maintenance sécurisée sans indisponibilité (REINDEX CONCURRENTLY,
# VACUUM ANALYZE) SANS JAMAIS supprimer d'index à l'aveugle.
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
LIB_DIR="$PROJECT_ROOT/scripts/lib"

# shellcheck source=scripts/lib/axon-pg-port.sh
if [[ -f "$LIB_DIR/axon-pg-port.sh" ]]; then
  source "$LIB_DIR/axon-pg-port.sh"
fi

DB_URL="${AXON_LIVE_DATABASE_URL:-postgres://axon@127.0.0.1:${AXON_CANONICAL_PG_PORT:-44144}/axon_live}"
DISK_WARN_THRESHOLD_GIB="${AXON_DISK_WARN_THRESHOLD_GIB:-10}"
DISK_CRIT_THRESHOLD_GIB="${AXON_DISK_CRIT_THRESHOLD_GIB:-5}"

MODE="report"
OUTPUT_FORMAT="text"

usage() {
  cat <<'EOF'
Usage: vacuum_bloat_audit.sh [OPTIONS]

Options:
  --check               Auditer la taille, le bloat et les seuils disque (défaut).
  --json                Rendre le rapport au format JSON structuré.
  --vacuum              Exécuter un VACUUM ANALYZE contrôlé sur les tables principales.
  --reindex-table TBL   Exécuter REINDEX TABLE CONCURRENTLY sur la table indiquée (ex: ist.edge).
  --help                Afficher cette aide.

Politique d'architecture (REQ-AXO-902611):
  - Ne JAMAIS supprimer un index à l'aveugle pour contourner un manque d'espace disque.
  - Préférer la réindexation concurrente (REINDEX CONCURRENTLY) et VACUUM ANALYZE.
EOF
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check)
      MODE="report"
      shift
      ;;
    --json)
      OUTPUT_FORMAT="json"
      shift
      ;;
    --vacuum)
      MODE="vacuum"
      shift
      ;;
    --reindex-table)
      MODE="reindex"
      TARGET_TABLE="${2:-}"
      if [[ -z "$TARGET_TABLE" ]]; then
        echo "Erreur: --reindex-table exige un nom de table (ex: ist.edge)" >&2
        exit 1
      fi
      shift 2
      ;;
    --help|-h)
      usage
      ;;
    *)
      echo "Option inconnue: $1" >&2
      usage
      ;;
  esac
done

if ! command -v psql >/dev/null 2>&1; then
  echo "Erreur: psql introuvable. Exécutez ce script sous devenv shell." >&2
  exit 1
fi

psql_cmd() {
  psql "$DB_URL" -v ON_ERROR_STOP=1 "$@"
}

psql_val() {
  psql "$DB_URL" -At -v ON_ERROR_STOP=1 -c "$1"
}

# 1. Vérification de connectivité
if ! psql_val "SELECT 1" >/dev/null 2>&1; then
  echo "Erreur: Impossible de joindre PostgreSQL sur $DB_URL" >&2
  exit 2
fi

# 2. Mesure de l'espace disque sur le volume de données
AVAIL_KB=$(df -P "$PROJECT_ROOT" | awk 'NR==2 {print $4}')
AVAIL_GIB=$(( AVAIL_KB / 1024 / 1024 ))

DB_SIZE_BYTES=$(psql_val "SELECT pg_database_size(current_database());")
DB_SIZE_PRETTY=$(psql_val "SELECT pg_size_pretty(pg_database_size(current_database()));")

if [[ "$MODE" == "vacuum" ]]; then
  echo "==> Exécution de VACUUM (ANALYZE, VERBOSE) sur les tables volumineuses..."
  for tbl in "ist.chunk" "ist.chunkembedding" "ist.edge" "ist.symbol" "pgmq.a_tsv_pending"; do
    if psql_val "SELECT 1 FROM pg_tables WHERE schemaname = split_part('$tbl', '.', 1) AND tablename = split_part('$tbl', '.', 2);" | grep -q 1; then
      echo "  -> VACUUM ANALYZE $tbl..."
      psql_cmd -c "VACUUM (ANALYZE, VERBOSE) $tbl;"
    fi
  done
  echo "==> Maintenance VACUUM terminée."
  exit 0
fi

if [[ "$MODE" == "reindex" ]]; then
  echo "==> Exécution de REINDEX TABLE CONCURRENTLY sur $TARGET_TABLE..."
  psql_cmd -c "REINDEX TABLE CONCURRENTLY $TARGET_TABLE;"
  echo "==> Réindexation concurrente terminée."
  exit 0
fi

# Mode rapport (text ou json)
if [[ "$OUTPUT_FORMAT" == "json" ]]; then
  python3 -c "
import json, subprocess, sys

def run_query(sql):
    cmd = ['psql', '$DB_URL', '-At', '-F', '\t', '-c', sql]
    res = subprocess.run(cmd, capture_output=True, text=True, check=True)
    return [line.split('\t') for line in res.stdout.strip().split('\n') if line]

try:
    tables_raw = run_query('''
        SELECT schemaname, relname, pg_total_relation_size(relid), pg_relation_size(relid), pg_indexes_size(relid), n_live_tup, n_dead_tup
        FROM pg_stat_user_tables
        ORDER BY pg_total_relation_size(relid) DESC LIMIT 10;
    ''')
    tables = [
        {
            'schema': r[0],
            'name': r[1],
            'total_bytes': int(r[2]),
            'table_bytes': int(r[3]),
            'indexes_bytes': int(r[4]),
            'live_tuples': int(r[5] or 0),
            'dead_tuples': int(r[6] or 0)
        }
        for r in tables_raw
    ]

    indexes_raw = run_query('''
        SELECT schemaname, tablename, indexname, pg_relation_size(quote_ident(schemaname) || '.' || quote_ident(indexname))::bigint
        FROM pg_indexes
        WHERE schemaname IN ('ist', 'soll', 'pgmq')
        ORDER BY pg_relation_size(quote_ident(schemaname) || '.' || quote_ident(indexname))::bigint DESC LIMIT 15;
    ''')
    indexes = [
        {
            'schema': r[0],
            'table': r[1],
            'index': r[2],
            'size_bytes': int(r[3])
        }
        for r in indexes_raw
    ]

    report = {
        'database_size_bytes': int('$DB_SIZE_BYTES'),
        'disk_available_gib': int('$AVAIL_GIB'),
        'disk_warning_threshold_gib': int('$DISK_WARN_THRESHOLD_GIB'),
        'disk_critical_threshold_gib': int('$DISK_CRIT_THRESHOLD_GIB'),
        'status': 'critical' if int('$AVAIL_GIB') < int('$DISK_CRIT_THRESHOLD_GIB') else ('warning' if int('$AVAIL_GIB') < int('$DISK_WARN_THRESHOLD_GIB') else 'ok'),
        'tables': tables,
        'indexes': indexes
    }
    print(json.dumps(report, indent=2))
except Exception as e:
    sys.stderr.write(f'Erreur audit JSON: {e}\n')
    sys.exit(1)
"
  exit 0
fi

# Rapport texte standard
echo "============================================================================="
echo "🏛️  Axon PostgreSQL Storage Governance & Bloat Audit (REQ-AXO-902611)"
echo "============================================================================="
echo "Base:               $DB_URL"
echo "Taille globale:     $DB_SIZE_PRETTY ($DB_SIZE_BYTES octets)"
echo "Disque disponible:  ${AVAIL_GIB} GiB (Seuil Warning: ${DISK_WARN_THRESHOLD_GIB} GiB, Seuil Critique: ${DISK_CRIT_THRESHOLD_GIB} GiB)"

if (( AVAIL_GIB < DISK_CRIT_THRESHOLD_GIB )); then
  echo "❌ ALERTE CRITIQUE : Espace disque ($AVAIL_GIB GiB) sous la réserve critique Nexus ($DISK_CRIT_THRESHOLD_GIB GiB) !"
elif (( AVAIL_GIB < DISK_WARN_THRESHOLD_GIB )); then
  echo "⚠️  AVERTISSEMENT : Espace disque ($AVAIL_GIB GiB) sous le seuil de vigilance ($DISK_WARN_THRESHOLD_GIB GiB)."
else
  echo "✅ Espace disque conforme aux exigences de réserve Nexus."
fi

echo ""
echo "--- Top 10 Tables par volume total (Table + Index + Toast) ---"
psql_cmd -c "
SELECT schemaname || '.' || relname AS relation,
       pg_size_pretty(pg_total_relation_size(relid)) AS total,
       pg_size_pretty(pg_relation_size(relid)) AS table,
       pg_size_pretty(pg_indexes_size(relid)) AS indexes,
       n_live_tup AS live_tuples,
       n_dead_tup AS dead_tuples,
       round(100.0 * n_dead_tup / nullif(n_live_tup + n_dead_tup, 0), 1) AS dead_pct
FROM pg_stat_user_tables
ORDER BY pg_total_relation_size(relid) DESC
LIMIT 10;
"

echo ""
echo "--- Top 15 Index par taille physique ---"
psql_cmd -c "
SELECT schemaname || '.' || indexname AS index_full_name,
       tablename,
       pg_size_pretty(pg_relation_size(quote_ident(schemaname) || '.' || quote_ident(indexname))::bigint) AS size
FROM pg_indexes
WHERE schemaname IN ('ist', 'soll', 'pgmq')
ORDER BY pg_relation_size(quote_ident(schemaname) || '.' || quote_ident(indexname))::bigint DESC
LIMIT 15;
"

echo ""
echo "Rappel d'Architecture : Interdiction stricte de DROP INDEX à l'aveugle."
echo "Pour éliminer le bloat d'un index sans coupure : vacuum_bloat_audit.sh --reindex-table <nom_table>"
echo "Pour recycler les dead tuples : vacuum_bloat_audit.sh --vacuum"

