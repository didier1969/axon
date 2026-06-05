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
# DEC-AXO-901598 + REQ-AXO-901638 polling discipline (shared with promote_live.sh).
ASSERT_STOPPED_TIMEOUT_S="${PROMOTE_LIVE_ASSERT_STOPPED_TIMEOUT_S:-5}"
ASSERT_STOPPED_INTERVAL_S="${PROMOTE_LIVE_ASSERT_STOPPED_INTERVAL_S:-0.1}"

poll_until() {
  local desc="$1" timeout_s="$2" interval_s="$3"; shift 3
  local now_ms end_ms
  end_ms=$(( $(date +%s%N) / 1000000 + ${timeout_s%.*} * 1000 ))
  while true; do
    if "$@"; then
      return 0
    fi
    now_ms=$(( $(date +%s%N) / 1000000 ))
    (( now_ms >= end_ms )) && return 1
    sleep "$interval_s"
  done
}

assert_live_stopped() {
  # DEC-AXO-901598 + REQ-AXO-901638 : caller-side polling absorbs OS
  # cleanup window after `axonctl stop`. scripts/stop.sh --verify stays
  # atomic. Diagnostic output preserved on failure for triage.
  local last_log
  last_log="$(mktemp)"
  if poll_until "live canonical fully stopped" \
       "$ASSERT_STOPPED_TIMEOUT_S" "$ASSERT_STOPPED_INTERVAL_S" \
       bash -c "bash '$ROOT_DIR/scripts/stop.sh' --verify > '$last_log' 2>&1"; then
    rm -f "$last_log"
    return 0
  fi
  echo "Live hard-stop verification failed after ${ASSERT_STOPPED_TIMEOUT_S}s of polling; refusing restart." >&2
  echo "--- last stop.sh --verify output ---" >&2
  cat "$last_log" >&2 || true
  echo "--- end stop.sh --verify output ---" >&2
  rm -f "$last_log"
  return 1
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
artifacts = manifest.get("artifacts")
if isinstance(artifacts, dict):
    for name, entry in artifacts.items():
        if not isinstance(entry, dict):
            raise SystemExit(f"Invalid artifact entry for {name}")
        candidate = pathlib.Path(entry["path"])
        if not candidate.exists():
            raise SystemExit(f"Artifact not found: {candidate}")
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
export ROOT_DIR RELEASE_VERSION="$release_version" PACKAGE_VERSION="$package_version" BUILD_ID="$build_id"

bash "$ROOT_DIR/scripts/release/preflight.sh" \
  --check-pending \
  --skip-build-match

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
  echo "DRY RUN: runtime_contract=brain_mcp_indexer_ist release_version=$release_version build_id=$build_id install_generation=$install_generation"
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
  python3 - <<'PY'
import json, os, pathlib, shutil, shlex
root = pathlib.Path(os.environ["ROOT_DIR"])
manifest = json.loads(pathlib.Path(os.environ["RELEASE_MANIFEST"]).read_text())
release_version = os.environ["RELEASE_VERSION"]
build_id = os.environ["BUILD_ID"]
package_version = os.environ["PACKAGE_VERSION"]
install_generation = os.environ["INSTALL_GENERATION"]
artifacts = manifest.get("artifacts") if isinstance(manifest.get("artifacts"), dict) else {}
if not artifacts:
    artifacts = {"axon-core": manifest["artifact"]}
for name, entry in artifacts.items():
    source = pathlib.Path(entry["path"])
    target = root / "bin" / name
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)
    build_info_target = root / "bin" / f"{name}.build-info"
    build_info_source = entry.get("build_info_path")
    if isinstance(build_info_source, str) and pathlib.Path(build_info_source).exists():
        shutil.copy2(build_info_source, build_info_target)
    else:
        payload = {
            "AXON_RELEASE_VERSION": release_version,
            "AXON_BUILD_ID": build_id,
            "AXON_PACKAGE_VERSION": package_version,
            "AXON_INSTALL_GENERATION": install_generation,
        }
        with build_info_target.open("w") as handle:
            for key, value in payload.items():
                handle.write(f"{key}={shlex.quote(value)}\n")
PY
  if ! "$ROOT_DIR/scripts/axon" --instance live stop; then
    restart_failed=1
  elif ! assert_live_stopped; then
    restart_failed=1
  else
    # REQ-AXO-901782 : the post-check (check_live_runtime_version.py) enforces
    # indexer_ready=true via runtime_authority_contract("brain"), so a
    # brain_only restart (`start brain --fast`) makes the post-check impossible
    # to pass (no indexer heartbeat → 150s timeout → false postcheck_failed).
    # Mirror promote_live.sh:497 — the canonical live profile is `start full`.
    if ! AXON_INSTANCE_KIND=live AXON_LIVE_RELEASE_MANIFEST="$pending_manifest" AXON_SKIP_BIN_SYNC=1 bash "$ROOT_DIR/scripts/axon" --instance live start full; then
      restart_failed=1
    elif [[ "$SKIP_POSTCHECK" -ne 1 ]]; then
      # REQ-AXO-901638 : poll_until replaces 12*5s=60s legacy fixed-sleep loop.
      POSTCHECK_TIMEOUT_S="${PROMOTE_LIVE_POSTCHECK_TIMEOUT_S:-150}"
      POSTCHECK_INTERVAL_S="${PROMOTE_LIVE_POSTCHECK_INTERVAL_S:-2}"
      _rollback_postcheck_predicate() {
        python3 "$ROOT_DIR/scripts/release/check_live_runtime_version.py" \
          --manifest "$MANIFEST_PATH" \
          --url "$AXON_MCP_URL" \
          --install-generation "$install_generation" >/dev/null 2>&1 \
        && bash "$ROOT_DIR/scripts/axon" --instance live status >/dev/null 2>&1
      }
      export -f _rollback_postcheck_predicate 2>/dev/null || true
      if poll_until "live MCP runtime post-check" "$POSTCHECK_TIMEOUT_S" "$POSTCHECK_INTERVAL_S" \
           _rollback_postcheck_predicate; then
        verified=1
      else
        postcheck_failed=1
        echo "Post-check timed out after ${POSTCHECK_TIMEOUT_S}s (interval ${POSTCHECK_INTERVAL_S}s)." >&2
      fi
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
