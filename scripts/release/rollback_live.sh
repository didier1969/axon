#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=scripts/lib/axon-version.sh
source "$ROOT_DIR/scripts/lib/axon-version.sh"
# shellcheck source=scripts/lib/axon-instance.sh
source "$ROOT_DIR/scripts/lib/axon-instance.sh"
AXON_INSTANCE_KIND=live
axon_resolve_instance "$ROOT_DIR" "$(basename "$ROOT_DIR")"

MANIFEST_PATH=""
RESTART_LIVE=0
SKIP_POSTCHECK=0
DRY_RUN=0

assert_live_stopped() {
  if ! bash "$ROOT_DIR/scripts/stop.sh" --verify >/dev/null 2>&1; then
    echo "Live hard-stop verification failed; refusing restart." >&2
    return 1
  fi
}

usage() {
  cat <<'EOF'
Usage: bash scripts/release/rollback_live.sh --manifest <manifest.json> [--restart-live] [--skip-postcheck] [--dry-run]
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --manifest) MANIFEST_PATH="${2:-}"; shift 2 ;;
    --restart-live) RESTART_LIVE=1; shift ;;
    --skip-postcheck) SKIP_POSTCHECK=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage; exit 1 ;;
  esac
done

[[ -n "$MANIFEST_PATH" ]] || { echo "--manifest is required" >&2; exit 1; }

MANIFEST_PATH="$(realpath "$MANIFEST_PATH")"
export RELEASE_MANIFEST="$MANIFEST_PATH"

if [[ -f "$ROOT_DIR/.axon/live-release/pending.json" ]]; then
  echo "Stale pending live release exists; clear .axon/live-release/pending.json before rollback." >&2
  exit 1
fi

bash "$ROOT_DIR/scripts/release/preflight.sh" --artifact "$ROOT_DIR/bin/axon-core" --build-info "$ROOT_DIR/bin/axon-core.build-info" --check-pending --skip-build-match

python3 - <<'PY'
import hashlib, json, os, pathlib
manifest = json.loads(pathlib.Path(os.environ["RELEASE_MANIFEST"]).read_text())
artifact = pathlib.Path(manifest["artifact"]["path"])
if not artifact.exists():
    raise SystemExit(f"Artifact not found: {artifact}")
h = hashlib.sha256()
with artifact.open("rb") as handle:
    for chunk in iter(lambda: handle.read(1024 * 1024), b""):
        h.update(chunk)
if h.hexdigest() != manifest["artifact"]["sha256"]:
    raise SystemExit("Artifact checksum mismatch")
if manifest.get("state") != "promoted":
    raise SystemExit("Rollback requires a previously promoted live manifest")
PY

read_manifest_field() {
  python3 - <<'PY'
import json, os
manifest = json.load(open(os.environ["RELEASE_MANIFEST"]))
field = os.environ["MANIFEST_FIELD"]
cursor = manifest
for part in field.split("."):
    cursor = cursor[part]
print(cursor)
PY
}

export MANIFEST_FIELD="runtime_version.release_version"; release_version="$(read_manifest_field)"
export MANIFEST_FIELD="runtime_version.package_version"; package_version="$(read_manifest_field)"
export MANIFEST_FIELD="runtime_version.build_id"; build_id="$(read_manifest_field)"
export MANIFEST_FIELD="artifact.path"; artifact_path="$(read_manifest_field)"

install_generation="rollback-$(date -u +%Y%m%dT%H%M%SZ)"
release_root="$ROOT_DIR/.axon/live-release"
history_root="$release_root/history"
current_manifest="$release_root/current.json"
pending_manifest="$release_root/pending.json"
mkdir -p "$history_root"
export INSTALL_GENERATION="$install_generation"
export HISTORY_ROOT="$history_root"
export CURRENT_MANIFEST="$current_manifest"
export PENDING_MANIFEST="$pending_manifest"

if [[ "$DRY_RUN" -eq 1 ]]; then
  echo "DRY RUN: would roll back live using manifest $MANIFEST_PATH"
  echo "DRY RUN: release_version=$release_version build_id=$build_id install_generation=$install_generation"
  exit 0
fi

python3 - <<'PY'
import json, os, pathlib, datetime as dt
manifest = json.loads(pathlib.Path(os.environ["RELEASE_MANIFEST"]).read_text())
manifest["rolled_back_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
manifest["state"] = "staged"
manifest["runtime_version"]["install_generation"] = os.environ["INSTALL_GENERATION"]
pending = pathlib.Path(os.environ["PENDING_MANIFEST"])
pending.parent.mkdir(parents=True, exist_ok=True)
pending.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
PY

verified=0
restart_failed=0
postcheck_failed=0
if [[ "$RESTART_LIVE" -eq 1 ]]; then
  install -m 755 "$artifact_path" "$ROOT_DIR/bin/axon-core"
  axon_write_export_file "$ROOT_DIR/bin/axon-core.build-info" \
    AXON_RELEASE_VERSION "$release_version" \
    AXON_BUILD_ID "$build_id" \
    AXON_PACKAGE_VERSION "$package_version" \
    AXON_INSTALL_GENERATION "$install_generation"
  runtime_state="$ROOT_DIR/.axon/live-run/runtime.env"
  start_args=()
  if [[ -f "$runtime_state" ]]; then
    # shellcheck disable=SC1090
    source "$runtime_state"
    case "${AXON_RUNTIME_MODE:-full}" in
      graph_only) start_args+=(--graph-only) ;;
      read_only) start_args+=(--read-only) ;;
      mcp_only) start_args+=(--mcp-only) ;;
      *) start_args+=(--full) ;;
    esac
    if [[ "${AXON_DASHBOARD_ENABLED:-1}" != "1" && "${AXON_RUNTIME_MODE:-}" != "mcp_only" ]]; then
      start_args+=(--no-dashboard)
    fi
  else
    start_args+=(--full)
  fi
  if ! "$ROOT_DIR/scripts/axon" --instance live stop; then
    restart_failed=1
  elif ! assert_live_stopped; then
    restart_failed=1
  elif ! AXON_LIVE_RELEASE_MANIFEST="$pending_manifest" AXON_SKIP_BIN_SYNC=1 "$ROOT_DIR/scripts/axon" --instance live start "${start_args[@]}"; then
    restart_failed=1
  elif [[ "$SKIP_POSTCHECK" -ne 1 ]]; then
    if python3 "$ROOT_DIR/scripts/release/check_live_runtime_version.py" \
      --manifest "$MANIFEST_PATH" \
      --url "$AXON_MCP_URL" \
      --install-generation "$install_generation"; then
      verified=1
    else
      postcheck_failed=1
    fi
  fi
fi

if [[ "$restart_failed" -eq 1 ]]; then
  echo "Live restart failed after staging the rollback artifact."
  echo "Pending manifest remains at $pending_manifest and current manifest stays unchanged."
  echo "Inspect live status, fix the restart issue, then rerun rollback with restart or promote a known-good release explicitly."
  exit 1
fi

if [[ "$postcheck_failed" -eq 1 ]]; then
  echo "Live restarted on the staged rollback artifact, but MCP runtime_version post-check failed."
  echo "Pending manifest remains at $pending_manifest and current manifest stays unchanged."
  echo "Investigate live status, then rerun rollback with restart or promote a known-good release explicitly."
  exit 1
fi

if [[ "$verified" -eq 1 ]]; then
  if [[ -f "$current_manifest" ]]; then
    export CURRENT_MANIFEST="$current_manifest"
    previous_generation="$(python3 - <<'PY'
import json, os, pathlib
manifest = json.loads(pathlib.Path(os.environ["CURRENT_MANIFEST"]).read_text())
print(manifest["runtime_version"].get("install_generation", "previous"))
PY
)"
    cp "$current_manifest" "$history_root/${previous_generation}.json"
  fi
  python3 - <<'PY'
import json, os, pathlib
pending = pathlib.Path(os.environ["PENDING_MANIFEST"])
manifest = json.loads(pending.read_text())
manifest["state"] = "promoted"
payload = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
pathlib.Path(os.environ["CURRENT_MANIFEST"]).write_text(payload)
pathlib.Path(os.environ["HISTORY_ROOT"]).mkdir(parents=True, exist_ok=True)
pathlib.Path(os.environ["HISTORY_ROOT"]).joinpath(f"{os.environ['INSTALL_GENERATION']}.json").write_text(payload)
pending.unlink(missing_ok=True)
PY
  echo "Rolled back live to $release_version ($build_id) generation=$install_generation"
else
  if [[ "$RESTART_LIVE" -eq 1 && "$SKIP_POSTCHECK" -eq 1 ]]; then
    echo "Live restarted on staged rollback artifact $release_version ($build_id) generation=$install_generation without MCP post-check."
    echo "The rollback remains staged and unverified until rollback is rerun with post-check."
  else
    echo "Staged live rollback artifact $release_version ($build_id) generation=$install_generation; restart/post-check required before it becomes active."
  fi
fi
