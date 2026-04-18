#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$ROOT_DIR/scripts/lib/axon-instance.sh"

ALLOW_LIVE_RUNNING=0
NO_BACKUP=0

usage() {
  cat <<'EOF'
Usage: ./scripts/seed-dev-from-live.sh [--allow-live-running] [--no-backup]

Default behavior:
  - requires live to be stopped for a coherent local DuckDB file snapshot
  - requires dev to be stopped
  - backs up the current dev graph root before replacement

Options:
  --allow-live-running   Copy live DB files plus WAL while live is running (best-effort only)
  --no-backup            Replace dev root without creating a timestamped backup first
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --allow-live-running)
      ALLOW_LIVE_RUNNING=1
      shift
      ;;
    --no-backup)
      NO_BACKUP=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

resolve_for() {
  local kind="$1"
  AXON_INSTANCE_KIND="$kind" axon_resolve_instance "$ROOT_DIR" "$(basename "$ROOT_DIR")"
  printf '%s\n' "$AXON_DB_ROOT|$AXON_PID_FILE|$HYDRA_HTTP_PORT"
}

instance_running() {
  local pid_file="$1"
  local port="$2"
  local pid=""
  if [[ -f "$pid_file" ]]; then
    pid="$(cat "$pid_file" 2>/dev/null || true)"
    if [[ -n "$pid" && -d "/proc/$pid" ]]; then
      return 0
    fi
  fi
  ss -ltn "( sport = :$port )" 2>/dev/null | tail -n +2 | grep -q .
}

copy_db_family() {
  local src_root="$1"
  local dst_root="$2"
  local rel="$3"
  local src="$src_root/$rel"
  local wal="$src_root/$rel.wal"
  local dst="$dst_root/$rel"

  if [[ ! -f "$src" ]]; then
    echo "⚠️ Missing source file: $src"
    return 0
  fi

  mkdir -p "$(dirname "$dst")"
  install -m 644 "$src" "$dst"
  if [[ -f "$wal" ]]; then
    install -m 644 "$wal" "$dst.wal"
  else
    rm -f "$dst.wal"
  fi
}

live_meta="$(resolve_for live)"
dev_meta="$(resolve_for dev)"

IFS='|' read -r LIVE_DB_ROOT LIVE_PID_FILE LIVE_PORT <<<"$live_meta"
IFS='|' read -r DEV_DB_ROOT DEV_PID_FILE DEV_PORT <<<"$dev_meta"

if instance_running "$DEV_PID_FILE" "$DEV_PORT"; then
  echo "❌ Dev instance is running. Stop it before seeding." >&2
  exit 2
fi

if instance_running "$LIVE_PID_FILE" "$LIVE_PORT" && [[ "$ALLOW_LIVE_RUNNING" -ne 1 ]]; then
  echo "❌ Live instance is running. Stop it for a coherent snapshot, or rerun with --allow-live-running." >&2
  exit 2
fi

if [[ "$ALLOW_LIVE_RUNNING" -eq 1 ]]; then
  echo "⚠️ Proceeding with live running. This copies DB + WAL as a best-effort snapshot."
fi

if [[ ! -d "$LIVE_DB_ROOT" ]]; then
  echo "❌ Live DB root not found: $LIVE_DB_ROOT" >&2
  exit 2
fi

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
backup_root="$ROOT_DIR/.axon-dev/backups/$timestamp"

if [[ -d "$DEV_DB_ROOT" && "$NO_BACKUP" -ne 1 ]]; then
  mkdir -p "$(dirname "$backup_root")"
  echo "📦 Backing up current dev graph root to $backup_root"
  rm -rf "$backup_root"
  cp -a "$DEV_DB_ROOT" "$backup_root"
fi

echo "🧹 Replacing dev graph root: $DEV_DB_ROOT"
rm -rf "$DEV_DB_ROOT"
mkdir -p "$DEV_DB_ROOT/sanctuary"

copy_db_family "$LIVE_DB_ROOT" "$DEV_DB_ROOT" "ist.db"
copy_db_family "$LIVE_DB_ROOT" "$DEV_DB_ROOT" "sanctuary/soll.db"

for rel in meta.json capabilities.toml; do
  if [[ -f "$ROOT_DIR/.axon/$rel" ]]; then
    mkdir -p "$ROOT_DIR/.axon-dev"
    install -m 644 "$ROOT_DIR/.axon/$rel" "$ROOT_DIR/.axon-dev/$rel"
  fi
done

echo "✅ Dev graph root seeded from live."
echo "   live: $LIVE_DB_ROOT"
echo "   dev:  $DEV_DB_ROOT"
if [[ "$NO_BACKUP" -ne 1 && -d "$backup_root" ]]; then
  echo "   backup: $backup_root"
fi
