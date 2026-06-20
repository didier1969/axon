#!/usr/bin/env bash
# REQ-AXO-902052 #6-B — end-to-end Memgraph publication refresh.
#
# Rebuilds the missing orchestrator: export live PG → publication-dir
# (memgraph_export_publication.py) → build import cypherl (memgraph_build_cypherl.py)
# → validate (memgraph_validate_publication.py) → load into Memgraph
# (memgraph-projection.sh load, the ONLY Docker-dependent step).
#
# GRACEFUL by contract (PIL-AXO-009 + DEC-901640 trigger #3): any missing tool
# or a down Docker daemon => clean skip + a `last_publish.json` marker, exit 0.
# NEVER fails its caller — safe to fire-and-forget from promote_live_safe.sh.
# Single-flight (flock) + min-interval throttle so two close promotes don't run
# two 200 MB exports concurrently. Publications live in a dedicated reaped dir.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

DB_URL="${AXON_LIVE_DATABASE_URL:-postgres://axon@127.0.0.1:44144/axon_live}"
PUB_ROOT="${AXON_MEMGRAPH_PUB_DIR:-$ROOT_DIR/.axon/memgraph/publications}"
MARKER="$ROOT_DIR/.axon/memgraph/last_publish.json"
LOCKFILE="$ROOT_DIR/.axon/memgraph/.publish.lock"
KEEP=3                              # publications to retain
MIN_INTERVAL_SECONDS="${AXON_MEMGRAPH_MIN_INTERVAL_SECONDS:-600}"
SOURCE_COMMIT="$(git -C "$ROOT_DIR" rev-parse --short HEAD 2>/dev/null || echo unknown)"

mkdir -p "$PUB_ROOT" "$(dirname "$MARKER")"

# Stamp the marker and exit 0 (the never-fail contract). Args: status, detail.
write_marker() {
  local status="$1" detail="$2"
  printf '{"status":"%s","detail":"%s","at_unix":%s,"source_commit":"%s"}\n' \
    "$status" "$detail" "$(date +%s)" "$SOURCE_COMMIT" > "$MARKER" 2>/dev/null || true
  echo "publish-memgraph: $status — $detail"
}

# Single-flight: bail (success) if another publish holds the lock.
exec 9>"$LOCKFILE" 2>/dev/null || true
if command -v flock >/dev/null 2>&1; then
  if ! flock -n 9; then
    write_marker "skipped" "another publish in progress (flock held)"
    exit 0
  fi
fi

# Throttle: skip if a publish ran within the min interval.
if [[ -f "$MARKER" ]]; then
  last_at="$(grep -oE '"at_unix":[0-9]+' "$MARKER" 2>/dev/null | grep -oE '[0-9]+' || echo 0)"
  now="$(date +%s)"
  if [[ "$last_at" -gt 0 && $((now - last_at)) -lt "$MIN_INTERVAL_SECONDS" ]]; then
    write_marker "skipped" "throttled (<${MIN_INTERVAL_SECONDS}s since last publish)"
    exit 0
  fi
fi

# Tool guards — any missing tool is a clean skip (e.g. not inside the devenv
# shell, or Docker unavailable on this host).
for tool in psql python3 docker; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    write_marker "skipped" "missing tool: $tool"
    exit 0
  fi
done
if ! python3 -c "import pyarrow" >/dev/null 2>&1; then
  write_marker "skipped" "pyarrow unavailable (run inside devenv shell)"
  exit 0
fi
if ! docker info >/dev/null 2>&1; then
  write_marker "skipped" "docker daemon unavailable (load step deferred)"
  exit 0
fi

# Ensure the Memgraph container is up (best-effort).
bash "$SCRIPT_DIR/memgraph-projection.sh" start >/dev/null 2>&1 || true

PUB_ID="pub-$(date +%s)"
PUB_DIR="$PUB_ROOT/$PUB_ID"
mkdir -p "$PUB_DIR"

if ! python3 "$SCRIPT_DIR/memgraph_export_publication.py" \
      --db-url "$DB_URL" --out-dir "$PUB_DIR" \
      --publication-id "$PUB_ID" --source-commit "$SOURCE_COMMIT"; then
  write_marker "failed" "export step failed"
  exit 0
fi
if ! python3 "$SCRIPT_DIR/memgraph_build_cypherl.py" \
      --publication-dir "$PUB_DIR" --out "$PUB_DIR/memgraph_import.cypherl" >/dev/null; then
  write_marker "failed" "build_cypherl step failed"
  exit 0
fi
if ! python3 "$SCRIPT_DIR/memgraph_validate_publication.py" \
      --publication-dir "$PUB_DIR" --require-import-file >/dev/null; then
  write_marker "failed" "validation step failed"
  exit 0
fi
if ! bash "$SCRIPT_DIR/memgraph-projection.sh" load --publication-dir "$PUB_DIR" >/dev/null 2>&1; then
  write_marker "failed" "memgraph load step failed"
  exit 0
fi

# Reap old publications (keep the most recent $KEEP).
ls -1dt "$PUB_ROOT"/pub-* 2>/dev/null | tail -n +$((KEEP + 1)) | xargs -r rm -rf

write_marker "ok" "published $PUB_ID"
exit 0
