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
FINALIZE_ONLY=0

assert_live_stopped() {
  if ! bash "$ROOT_DIR/scripts/stop.sh" --verify >/dev/null 2>&1; then
    echo "Live hard-stop verification failed; refusing restart." >&2
    return 1
  fi
}

usage() {
  cat <<'EOF'
Usage: bash scripts/release/promote_live.sh --manifest <manifest.json> [--restart-live] [--skip-postcheck] [--dry-run] [--finalize-only]

  --finalize-only   REQ-AXO-286: assume the live brain already serves the target
                    manifest (started via env-override AXON_LIVE_RELEASE_MANIFEST=
                    <pending>). Verify build_id via MCP, then archive current and
                    promote pending → current without any service restart. Skips
                    staging copy, indexer rise, and the strict authority contract
                    (which requires indexer_ready=true and breaks brain_only ops).
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --manifest) MANIFEST_PATH="${2:-}"; shift 2 ;;
    --restart-live) RESTART_LIVE=1; shift ;;
    --skip-postcheck) SKIP_POSTCHECK=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --finalize-only) FINALIZE_ONLY=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage; exit 1 ;;
  esac
done

[[ -n "$MANIFEST_PATH" ]] || { echo "--manifest is required" >&2; exit 1; }

if [[ "$FINALIZE_ONLY" -eq 1 && "$RESTART_LIVE" -eq 1 ]]; then
  echo "--finalize-only and --restart-live are mutually exclusive" >&2
  exit 1
fi

MANIFEST_PATH="$(realpath "$MANIFEST_PATH")"
export RELEASE_MANIFEST="$MANIFEST_PATH"

if [[ -f "$ROOT_DIR/.axon/live-release/pending.json" && "$FINALIZE_ONLY" -ne 1 ]]; then
  echo "Stale pending live release exists; clear .axon/live-release/pending.json before promoting." >&2
  exit 1
fi

export FINALIZE_ONLY
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
finalize_only = os.environ.get("FINALIZE_ONLY", "0") == "1"
allowed_states = {"qualified", "staged"} if finalize_only else {"qualified"}
if manifest.get("state") not in allowed_states:
    raise SystemExit(
        "Only qualified manifests may be promoted"
        if not finalize_only
        else "Finalize-only accepts state in {qualified, staged}; got " + repr(manifest.get("state"))
    )
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
export MANIFEST_FIELD="artifact.sha256"; artifact_digest="$(read_manifest_field)"
export ROOT_DIR RELEASE_VERSION="$release_version" PACKAGE_VERSION="$package_version" BUILD_ID="$build_id"

if [[ "$FINALIZE_ONLY" -ne 1 ]]; then
  bash "$ROOT_DIR/scripts/release/preflight.sh" \
    --check-pending
fi

# REQ-AXO-286: under --finalize-only the manifest already carries its own
# install_generation (set during the original staging attempt). Reuse it so
# the live brain (started with AXON_LIVE_RELEASE_MANIFEST pointing at this
# manifest) keeps reporting a generation that matches current.json.
if [[ "$FINALIZE_ONLY" -eq 1 ]]; then
  export MANIFEST_FIELD="runtime_version.install_generation"
  install_generation="$(read_manifest_field)"
  if [[ -z "$install_generation" || "$install_generation" == "None" ]]; then
    install_generation="live-$(date -u +%Y%m%dT%H%M%SZ)"
  fi
else
  install_generation="live-$(date -u +%Y%m%dT%H%M%SZ)"
fi
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
  echo "DRY RUN: would promote manifest $MANIFEST_PATH"
  echo "DRY RUN: runtime_contract=brain_mcp_indexer_ist release_version=$release_version build_id=$build_id install_generation=$install_generation"
  echo "DRY RUN: artifact=$artifact_path sha256=$artifact_digest"
  [[ "$FINALIZE_ONLY" -eq 1 ]] && echo "DRY RUN: finalize-only — no staging, no restart, file labels only"
  exit 0
fi

# REQ-AXO-286 --finalize-only fast path: brain is already serving the
# manifest via AXON_LIVE_RELEASE_MANIFEST env-override; verify MCP reports
# the expected build_id (lightweight check, doesn't require indexer_ready),
# then perform file-label transition + build-info refresh without restart.
if [[ "$FINALIZE_ONLY" -eq 1 ]]; then
  # Light MCP check: build_id match. Probes the configured live MCP URL.
  live_mcp_url="${AXON_MCP_URL:-http://127.0.0.1:44129/mcp}"
  observed_build_id="$(python3 - <<'PY' 2>/dev/null || true
import json, os, urllib.request
url = os.environ.get("LIVE_MCP_URL", "http://127.0.0.1:44129/mcp")
body = json.dumps({"jsonrpc":"2.0","method":"tools/call","id":1,
    "params":{"name":"status","arguments":{"mode":"brief"}}}).encode()
req = urllib.request.Request(url, data=body,
    headers={"Content-Type":"application/json"})
with urllib.request.urlopen(req, timeout=5) as r:
    payload = json.loads(r.read())
data = payload.get("result", {}).get("data") or {}
rv = data.get("runtime_version") or {}
print(rv.get("build_id", ""))
PY
)"
  LIVE_MCP_URL="$live_mcp_url" observed_build_id="${observed_build_id:-}"
  if [[ -z "$observed_build_id" ]]; then
    echo "--finalize-only: could not probe live MCP at $live_mcp_url for runtime_version.build_id" >&2
    echo "Bring the live brain up first (e.g. AXON_LIVE_RELEASE_MANIFEST=$MANIFEST_PATH AXON_SKIP_BIN_SYNC=1 ./scripts/axon-live start --brain-only) and retry." >&2
    exit 1
  fi
  if [[ "$observed_build_id" != "$build_id" ]]; then
    echo "--finalize-only: live MCP build_id mismatch — expected $build_id, observed $observed_build_id" >&2
    exit 1
  fi
  echo "--finalize-only: live MCP reports build_id=$build_id ✓"

  # Refresh bin/<artifact>.build-info AXON_INSTALL_GENERATION to match
  python3 - <<'PY'
import json, os, pathlib, shlex
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
    build_info_target = root / "bin" / f"{name}.build-info"
    if not build_info_target.parent.exists():
        continue
    payload = {
        "AXON_RELEASE_VERSION": release_version,
        "AXON_BUILD_ID": build_id,
        "AXON_PACKAGE_VERSION": package_version,
        "AXON_INSTALL_GENERATION": install_generation,
    }
    extra = entry.get("build_info_path")
    if isinstance(extra, str):
        payload["AXON_ARTIFACT_BUILD_INFO_PATH"] = extra
    sha = entry.get("sha256")
    if isinstance(sha, str):
        payload["AXON_ARTIFACT_SHA256"] = sha
    artifact_source = entry.get("path")
    if isinstance(artifact_source, str):
        payload["AXON_ARTIFACT_SOURCE"] = artifact_source
    with build_info_target.open("w") as handle:
        for key, value in payload.items():
            handle.write(f"{key}={shlex.quote(value)}\n")
PY

  # Archive existing current.json then write new current.json with state=promoted
  python3 - <<'PY'
import json, os, pathlib
current = pathlib.Path(os.environ["CURRENT_MANIFEST"])
history_root = pathlib.Path(os.environ["HISTORY_ROOT"])
history_root.mkdir(parents=True, exist_ok=True)
if current.exists():
    prev = json.loads(current.read_text())
    prev_gen = prev.get("runtime_version", {}).get("install_generation", "previous")
    (history_root / f"{prev_gen}.json").write_text(json.dumps(prev, indent=2, sort_keys=True) + "\n")

manifest = json.loads(pathlib.Path(os.environ["RELEASE_MANIFEST"]).read_text())
manifest["state"] = "promoted"
manifest["runtime_version"]["install_generation"] = os.environ["INSTALL_GENERATION"]
payload = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
current.write_text(payload)
(history_root / f"{os.environ['INSTALL_GENERATION']}.json").write_text(payload)

pending = pathlib.Path(os.environ["PENDING_MANIFEST"])
if pending.exists():
    pending.unlink()
PY
  echo "Finalized live promotion: $RELEASE_VERSION ($build_id) generation=$install_generation"
  exit 0
fi

python3 - <<'PY'
import json, os, pathlib, datetime as dt
manifest = json.loads(pathlib.Path(os.environ["RELEASE_MANIFEST"]).read_text())
manifest["promoted_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
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
  # REQ-AXO-286 Bug 1 fix: stop services BEFORE copying binaries.
  # Previously the copy ran first and failed with `OSError: [Errno 26] Text
  # file busy` whenever the live brain held bin/axon-brain open. The stop
  # then never ran, leaving the script aborted mid-promotion.
  if ! "$ROOT_DIR/scripts/axon" --instance live stop; then
    restart_failed=1
  elif ! assert_live_stopped; then
    restart_failed=1
  fi

  if [[ "$restart_failed" -ne 1 ]]; then
    # REQ-AXO-286 Bug 1 follow-up: AXON_SKIP_BIN_SYNC=1 short-circuit.
    # When the operator has already pre-staged the binary (canonical recovery
    # pattern via AXON_LIVE_RELEASE_MANIFEST + AXON_SKIP_BIN_SYNC) and the
    # bin/<artifact> sha256 already matches the manifest, skip the copy
    # entirely. Reduces I/O + avoids the EBUSY race when the script is
    # re-run after a partial failure.
    AXON_SKIP_BIN_SYNC="${AXON_SKIP_BIN_SYNC:-0}" python3 - <<'PY'
import hashlib, json, os, pathlib, shutil, shlex
root = pathlib.Path(os.environ["ROOT_DIR"])
manifest = json.loads(pathlib.Path(os.environ["RELEASE_MANIFEST"]).read_text())
release_version = os.environ["RELEASE_VERSION"]
build_id = os.environ["BUILD_ID"]
package_version = os.environ["PACKAGE_VERSION"]
install_generation = os.environ["INSTALL_GENERATION"]
skip_bin_sync = os.environ.get("AXON_SKIP_BIN_SYNC", "0") == "1"
artifacts = manifest.get("artifacts") if isinstance(manifest.get("artifacts"), dict) else {}
if not artifacts:
    artifacts = {"axon-core": manifest["artifact"]}
for name, entry in artifacts.items():
    source = pathlib.Path(entry["path"])
    target = root / "bin" / name
    target.parent.mkdir(parents=True, exist_ok=True)
    expected_sha = entry.get("sha256")
    skip_this = False
    if skip_bin_sync and target.exists() and isinstance(expected_sha, str):
        h = hashlib.sha256()
        with target.open("rb") as f:
            for chunk in iter(lambda: f.read(1024 * 1024), b""):
                h.update(chunk)
        if h.hexdigest() == expected_sha:
            skip_this = True
            print(f"  skip-bin-sync: bin/{name} sha256 already matches manifest")
    if not skip_this:
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
  fi

  # Start services on the staged manifest (only if stop+copy succeeded)
  if [[ "$restart_failed" -ne 1 ]]; then
    if ! AXON_INSTANCE_KIND=live AXON_LIVE_RELEASE_MANIFEST="$pending_manifest" AXON_SKIP_BIN_SYNC=1 bash "$ROOT_DIR/scripts/lib/start-indexer.sh"; then
      restart_failed=1
    elif ! AXON_INSTANCE_KIND=live AXON_LIVE_RELEASE_MANIFEST="$pending_manifest" AXON_SKIP_BIN_SYNC=1 bash "$ROOT_DIR/scripts/lib/start-brain.sh"; then
      restart_failed=1
    elif [[ "$SKIP_POSTCHECK" -ne 1 ]]; then
      # REQ-AXO-155 — brain cold-start (BGE-Large model load + Phoenix
      # dashboard) typically takes 60-90s; the previous 12*5s=60s window
      # timed out before the post-check could observe the new
      # runtime_version even when the live was actually fine. Widened to
      # 24*5s=120s to fit the standard cold-start budget.
      POSTCHECK_ATTEMPTS=24
      for ((attempt = 1; attempt <= POSTCHECK_ATTEMPTS; attempt++)); do
        if python3 "$ROOT_DIR/scripts/release/check_live_runtime_version.py" \
          --manifest "$MANIFEST_PATH" \
          --url "$AXON_MCP_URL" \
          --install-generation "$install_generation" \
          && AXON_INSTANCE_KIND=live bash "$ROOT_DIR/scripts/lib/status-indexer.sh" >/dev/null \
          && AXON_INSTANCE_KIND=live bash "$ROOT_DIR/scripts/lib/status-brain.sh" >/dev/null; then
          verified=1
          break
        fi
        echo "Live MCP runtime post-check not ready yet (attempt $attempt/$POSTCHECK_ATTEMPTS); retrying..." >&2
        sleep 5
      done
      if [[ "$verified" -ne 1 ]]; then
        postcheck_failed=1
      fi
    fi
  fi
fi

if [[ "$restart_failed" -eq 1 ]]; then
  echo "Live restart failed after staging the promotion artifact."
  echo "Pending manifest remains at $pending_manifest and current manifest stays unchanged."
  echo "Inspect live status, fix the restart issue, then rerun promotion with restart or roll back explicitly."
  exit 1
fi

if [[ "$postcheck_failed" -eq 1 ]]; then
  echo "Live restarted on the staged artifact, but MCP runtime_version post-check failed."
  echo "Pending manifest remains at $pending_manifest and current manifest stays unchanged."
  echo "Investigate live status, then rerun promotion with restart or roll back explicitly."
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
  echo "Promoted live to $release_version ($build_id) generation=$install_generation"
else
  if [[ "$RESTART_LIVE" -eq 1 && "$SKIP_POSTCHECK" -eq 1 ]]; then
    echo "Live restarted on staged artifact $release_version ($build_id) generation=$install_generation without MCP post-check."
    echo "The release remains staged and unverified until promotion is rerun with post-check."
  else
    echo "Staged live artifact $release_version ($build_id) generation=$install_generation; restart/post-check required before it becomes promoted."
  fi
fi
