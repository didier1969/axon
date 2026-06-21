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
# REQ-AXO-902064 — opt-in in-place restart (atomic-swap binaries + SIGTERM, let
# process-compose auto-restart) for ~6s MCP downtime vs ~82s full stop+start.
# Default 0 keeps the proven full path; falls back to it on any in-place failure.
IN_PLACE=0
SKIP_POSTCHECK=0
DRY_RUN=0
FINALIZE_ONLY=0
RESUME=0
# REQ-AXO-901638 + DEC-AXO-901598 : caller-side polling discipline.
# All durations in seconds (sub-second fractions accepted by bash sleep).
ASSERT_STOPPED_TIMEOUT_S="${PROMOTE_LIVE_ASSERT_STOPPED_TIMEOUT_S:-5}"
ASSERT_STOPPED_INTERVAL_S="${PROMOTE_LIVE_ASSERT_STOPPED_INTERVAL_S:-0.1}"
# REQ-AXO-901857 : budget par défaut relevé 150→300s. Sous cold-reindex /
# backlog-embed (~500% CPU, REQ-AXO-155 cold-start : BGE-Large load + Phoenix
# + indexer rise), 150s expirait alors que readyz=ready + build_id correct ⇒
# promote marqué FAILED à tort + manifeste désynchronisé (viole PIL-AXO-005).
POSTCHECK_TIMEOUT_S="${PROMOTE_LIVE_POSTCHECK_TIMEOUT_S:-300}"
POSTCHECK_INTERVAL_S="${PROMOTE_LIVE_POSTCHECK_INTERVAL_S:-2}"

# poll_until <description> <timeout_seconds> <interval_seconds> <command...>
# Returns 0 as soon as <command> succeeds, 1 after <timeout> seconds elapse.
# Caller-side discipline replacing fixed sleeps : we wait on condition truth,
# not on arbitrary delays. Aligns with DEC-AXO-901598 + REQ-AXO-901638.
poll_until() {
  local desc="$1" timeout_s="$2" interval_s="$3"; shift 3
  local now_ms end_ms
  end_ms=$(( $(date +%s%N) / 1000000 + ${timeout_s%.*} * 1000 ))
  while true; do
    if "$@"; then
      return 0
    fi
    now_ms=$(( $(date +%s%N) / 1000000 ))
    if (( now_ms >= end_ms )); then
      [[ -n "${POLL_DEBUG:-}" ]] && echo "poll_until: timeout after ${timeout_s}s waiting for: $desc" >&2
      return 1
    fi
    sleep "$interval_s"
  done
}

assert_live_stopped() {
  # DEC-AXO-901598 + REQ-AXO-901638 : caller-side polling absorbs the
  # OS-level cleanup window after `axonctl stop` (flock release, port
  # unbind, AF_UNIX socket unlink) that briefly survives the synchronous
  # stop return. scripts/stop.sh --verify itself stays atomic (single
  # snapshot, no internal retry).
  local last_log
  last_log="$(mktemp)"
  if poll_until "live canonical fully stopped" \
       "$ASSERT_STOPPED_TIMEOUT_S" "$ASSERT_STOPPED_INTERVAL_S" \
       bash -c "bash '$ROOT_DIR/scripts/stop.sh' --verify > '$last_log' 2>&1"; then
    rm -f "$last_log"
    return 0
  fi
  echo "Live hard-stop verification failed after ${ASSERT_STOPPED_TIMEOUT_S}s of polling (interval ${ASSERT_STOPPED_INTERVAL_S}s); refusing restart." >&2
  echo "--- last stop.sh --verify output (canonical scope PIL-AXO-008) ---" >&2
  cat "$last_log" >&2 || true
  echo "--- end stop.sh --verify output ---" >&2
  rm -f "$last_log"
  return 1
}

rollback_bin_to_current() {
  # REQ-AXO-901638 : on promote-live failure, restore bin/* from the
  # canonical artifact paths referenced by current.json. Restores bin/*
  # ↔ current.json coherence so the next live start serves the manifest
  # that is actually labelled as current.
  local current_manifest_path="${1:-$ROOT_DIR/.axon/live-release/current.json}"
  if [[ ! -f "$current_manifest_path" ]]; then
    echo "rollback_bin_to_current: no current.json at $current_manifest_path ; skipping." >&2
    return 0
  fi
  CURRENT_MANIFEST="$current_manifest_path" ROOT_DIR="$ROOT_DIR" python3 - <<'PY' >&2
import json, os, pathlib, shutil, sys
root = pathlib.Path(os.environ["ROOT_DIR"])
current = pathlib.Path(os.environ["CURRENT_MANIFEST"])
try:
    manifest = json.loads(current.read_text())
except Exception as exc:
    print(f"rollback: cannot parse current.json: {exc}")
    sys.exit(1)
artifacts = manifest.get("artifacts") or {"axon-core": manifest["artifact"]}
restored = []
missing = []
for name, entry in artifacts.items():
    source = pathlib.Path(entry["path"])
    target = root / "bin" / name
    if not source.exists():
        missing.append(f"{name}={source}")
        continue
    shutil.copy2(source, target)
    restored.append(name)
print(f"rollback_bin_to_current: restored bin/* = {restored}")
if missing:
    print(f"rollback_bin_to_current: missing artifact source paths: {missing}")
PY
}

# REQ-AXO-902064 — in-place restart: ~6s MCP downtime vs ~82s full stop+start.
# Atomically swaps bin/* (os.replace works even on a running executable — the
# live process keeps the old inode, the next exec opens the new one), then
# SIGTERMs brain[+indexer] and lets process-compose's availability:restart bring
# them back on the new binary. The brain re-applies its idempotent DDL at boot
# (graph_bootstrap) and re-acquires the SOLL writer lock. Returns 1 on any
# failure so the caller falls back to the proven full stop+copy+start path.
# Empirically validated: SIGTERM→readyz recovery = 6.4s (session 88).
inplace_restart_live() {
  local pc_port=8080  # live process-compose management API (axon-supervisor.sh)
  if ! curl -sf -m 2 "http://127.0.0.1:${pc_port}/live" >/dev/null 2>&1; then
    echo "in-place: live process-compose daemon not healthy on :${pc_port}; full restart." >&2
    return 1
  fi
  # Atomic binary swap (no stop needed — os.replace renames over the running file).
  if ! RELEASE_MANIFEST="$MANIFEST_PATH" ROOT_DIR="$ROOT_DIR" \
       RELEASE_VERSION="$release_version" BUILD_ID="$build_id" \
       PACKAGE_VERSION="$package_version" INSTALL_GENERATION="$install_generation" \
       python3 - <<'PY'
import json, os, pathlib, shlex, shutil
root = pathlib.Path(os.environ["ROOT_DIR"])
manifest = json.loads(pathlib.Path(os.environ["RELEASE_MANIFEST"]).read_text())
artifacts = manifest.get("artifacts") if isinstance(manifest.get("artifacts"), dict) else {}
if not artifacts:
    artifacts = {"axon-core": manifest["artifact"]}
for name, entry in artifacts.items():
    source = pathlib.Path(entry["path"])
    target = root / "bin" / name
    target.parent.mkdir(parents=True, exist_ok=True)
    tmp = root / "bin" / f"{name}.inplace.new"
    shutil.copy2(source, tmp)
    os.replace(tmp, target)  # atomic; safe while the old binary runs
    bi = root / "bin" / f"{name}.build-info"
    src_bi = entry.get("build_info_path")
    if isinstance(src_bi, str) and pathlib.Path(src_bi).exists():
        shutil.copy2(src_bi, bi)
    else:
        with bi.open("w") as h:
            for k in ("RELEASE_VERSION", "BUILD_ID", "PACKAGE_VERSION", "INSTALL_GENERATION"):
                h.write(f"AXON_{k}={shlex.quote(os.environ[k])}\n")
PY
  then
    echo "in-place: binary swap failed; full restart." >&2
    return 1
  fi
  # SIGTERM brain (+ indexer when present); process-compose auto-restarts them.
  local pids; pids="$(pgrep -f 'bin/axon-brain' || true)"
  pids="$pids $(pgrep -f 'bin/axon-indexer' || true)"
  local p
  for p in $pids; do kill -TERM "$p" 2>/dev/null || true; done
  # Wait for the brain to come back on /readyz (auto-restart). Bounded.
  if poll_until "brain readyz after in-place restart" 60 0.2 \
       bash -c "curl -sf -m 2 http://127.0.0.1:44129/readyz >/dev/null 2>&1"; then
    echo "✅ in-place restart: brain back on /readyz (process-compose auto-restart)." >&2
    return 0
  fi
  echo "in-place: brain did not return on /readyz within 60s; full restart." >&2
  return 1
}

usage() {
  cat <<'EOF'
Usage: bash scripts/release/promote_live.sh --manifest <manifest.json> [--restart-live] [--in-place] [--skip-postcheck] [--dry-run] [--finalize-only] [--resume]

  --restart-live    Stop the live canonical, copy bin/* from the manifest, then
                    bring the FULL live profile up (brain + indexer + dashboard
                    via `start full`). REQ-AXO-901782 : the post-check enforces
                    indexer_ready=true (runtime_authority_contract("brain"))
                    so brain_only would always time out. Use --finalize-only
                    instead when the operator has already pre-staged the brain
                    via AXON_LIVE_RELEASE_MANIFEST env-override.

  --finalize-only   REQ-AXO-286: assume the live brain already serves the target
                    manifest (started via env-override AXON_LIVE_RELEASE_MANIFEST=
                    <pending>). Verify build_id via MCP, then archive current and
                    promote pending → current without any service restart. Skips
                    staging copy, indexer rise, and the strict authority contract
                    (which requires indexer_ready=true and breaks brain_only ops).

  --resume          REQ-AXO-901638 : reuse the existing pending.json from a
                    previous partial-failed promotion (state=staged) and retry
                    the restart + post-check + manifest-swap phases. bin/* must
                    already match the pending manifest sha256 (the previous run
                    copied them) ; the script verifies coherence before proceeding.

Tunable polling envelopes (env, all in seconds) :
  PROMOTE_LIVE_ASSERT_STOPPED_TIMEOUT_S    default 5    (caller-side wait for canonical down)
  PROMOTE_LIVE_ASSERT_STOPPED_INTERVAL_S   default 0.1
  PROMOTE_LIVE_POSTCHECK_TIMEOUT_S         default 150  (live MCP build_id match)
  PROMOTE_LIVE_POSTCHECK_INTERVAL_S        default 2
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --manifest) MANIFEST_PATH="${2:-}"; shift 2 ;;
    --restart-live) RESTART_LIVE=1; shift ;;
    --in-place) IN_PLACE=1; shift ;;
    --skip-postcheck) SKIP_POSTCHECK=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --finalize-only) FINALIZE_ONLY=1; shift ;;
    --resume) RESUME=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage; exit 1 ;;
  esac
done

[[ -n "$MANIFEST_PATH" ]] || { echo "--manifest is required" >&2; exit 1; }

if [[ "$FINALIZE_ONLY" -eq 1 && "$RESTART_LIVE" -eq 1 ]]; then
  echo "--finalize-only and --restart-live are mutually exclusive" >&2
  exit 1
fi
if [[ "$RESUME" -eq 1 && "$FINALIZE_ONLY" -eq 1 ]]; then
  echo "--resume and --finalize-only are mutually exclusive" >&2
  exit 1
fi

MANIFEST_PATH="$(realpath "$MANIFEST_PATH")"
export RELEASE_MANIFEST="$MANIFEST_PATH"

# REQ-AXO-901638 : --resume reuses an existing pending.json (state=staged).
# Without --resume, the presence of pending.json is a hard error to prevent
# accidentally overwriting a partial-failed staging.
if [[ -f "$ROOT_DIR/.axon/live-release/pending.json" && "$FINALIZE_ONLY" -ne 1 && "$RESUME" -ne 1 ]]; then
  echo "Stale pending live release exists; clear .axon/live-release/pending.json or rerun with --resume." >&2
  exit 1
fi

if [[ "$RESUME" -eq 1 && ! -f "$ROOT_DIR/.axon/live-release/pending.json" ]]; then
  echo "--resume requires an existing pending.json from a previous partial-failed promotion." >&2
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

if [[ "$FINALIZE_ONLY" -ne 1 && "$RESUME" -ne 1 ]]; then
  bash "$ROOT_DIR/scripts/release/preflight.sh" \
    --check-pending
fi
# REQ-AXO-901638 --resume: pending.json existence is expected (and required) ;
# preflight --check-pending would reject it. We already validated bin/* sha256
# matches the staged pending manifest before reaching this point, so the
# preflight bin/integrity guarantee is preserved by a different path.

# REQ-AXO-286 + REQ-AXO-901638 : both --finalize-only and --resume must reuse
# the existing pending manifest's install_generation. Brain started with
# AXON_LIVE_RELEASE_MANIFEST=<pending> reports the install_generation embedded
# in that manifest ; the post-check (check_live_runtime_version.py) compares
# brain's reported value to the script's $install_generation. Generating a
# fresh timestamp here in --resume mode would cause a perpetual post-check
# mismatch — visible as a 150s polling timeout despite a healthy brain.
if [[ "$FINALIZE_ONLY" -eq 1 || "$RESUME" -eq 1 ]]; then
  if [[ -f "$ROOT_DIR/.axon/live-release/pending.json" ]]; then
    MANIFEST_FIELD="runtime_version.install_generation" RELEASE_MANIFEST="$ROOT_DIR/.axon/live-release/pending.json" \
      install_generation="$(read_manifest_field)"
  else
    export MANIFEST_FIELD="runtime_version.install_generation"
    install_generation="$(read_manifest_field)"
  fi
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
    echo "Bring the live brain up first (e.g. AXON_LIVE_RELEASE_MANIFEST=$MANIFEST_PATH AXON_SKIP_BIN_SYNC=1 ./scripts/axon-live start brain) and retry." >&2
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

if [[ "$RESUME" -ne 1 ]]; then
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
else
  # REQ-AXO-901638 --resume : reuse existing pending.json. Verify bin/* sha256
  # already matches pending manifest so we know setup --artifact-only succeeded
  # on the previous attempt. Aborts loudly if bin/* drift detected.
  ROOT_DIR="$ROOT_DIR" PENDING_MANIFEST="$pending_manifest" python3 - <<'PY'
import hashlib, json, os, pathlib, sys
root = pathlib.Path(os.environ["ROOT_DIR"])
pending = pathlib.Path(os.environ["PENDING_MANIFEST"])
manifest = json.loads(pending.read_text())
artifacts = manifest.get("artifacts") or {"axon-core": manifest["artifact"]}
mismatches = []
for name, entry in artifacts.items():
    target = root / "bin" / name
    expected = entry.get("sha256")
    if not target.exists():
        mismatches.append(f"bin/{name} missing")
        continue
    h = hashlib.sha256()
    with target.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            h.update(chunk)
    if h.hexdigest() != expected:
        mismatches.append(f"bin/{name} sha256 {h.hexdigest()[:16]} != pending {expected[:16]}")
if mismatches:
    print("--resume: bin/* drift vs pending.json :")
    for m in mismatches:
        print(f"  {m}")
    print("Rerun without --resume to rebuild and re-stage, or restore bin/* manually.")
    sys.exit(1)
print(f"--resume: bin/* coherent with pending.json (state={manifest.get('state')} build_id={manifest.get('runtime_version',{}).get('build_id')})")
PY
  echo "Resuming promote-live from existing pending.json (state=staged)."
fi

verified=0
restart_failed=0
postcheck_failed=0
if [[ "$RESTART_LIVE" -eq 1 ]]; then
  # REQ-AXO-902064 — try the fast in-place restart first (atomic bin swap +
  # SIGTERM + process-compose auto-restart, ~6s MCP downtime). On any failure it
  # returns non-zero and we fall through to the proven full stop+copy+start.
  inplace_done=0
  if [[ "$IN_PLACE" -eq 1 ]]; then
    if inplace_restart_live; then
      inplace_done=1
    else
      echo "in-place restart did not complete; falling back to full stop+copy+start." >&2
    fi
  fi

  # REQ-AXO-286 Bug 1 fix: stop services BEFORE copying binaries (full path only).
  # Previously the copy ran first and failed with `OSError: [Errno 26] Text
  # file busy` whenever the live brain held bin/axon-brain open. The stop
  # then never ran, leaving the script aborted mid-promotion.
  if [[ "$inplace_done" -ne 1 ]]; then
    if ! "$ROOT_DIR/scripts/axon" --instance live stop; then
      restart_failed=1
    elif ! assert_live_stopped; then
      restart_failed=1
    fi
  fi

  if [[ "$restart_failed" -ne 1 && "$inplace_done" -ne 1 ]]; then
    # REQ-AXO-286 Bug 1 follow-up: AXON_SKIP_BIN_SYNC=1 short-circuit.
    # When the operator has already pre-staged the binary (canonical recovery
    # pattern via AXON_LIVE_RELEASE_MANIFEST + AXON_SKIP_BIN_SYNC, or via
    # promote_live.sh --resume) and bin/<artifact> sha256 already matches
    # the manifest, skip the copy entirely. Reduces I/O + avoids the EBUSY
    # race when the script is re-run after a partial failure.
    if [[ "$RESUME" -eq 1 ]]; then
      AXON_SKIP_BIN_SYNC=1
    fi
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

  # Start services on the staged manifest (only if stop+copy succeeded).
  # REQ-AXO-901782 : the post-check (check_live_runtime_version.py) enforces
  # `indexer_ready=true` as part of runtime_authority_contract("brain"), so
  # spawning `start brain --fast` (brain_only) makes the gate impossible to
  # pass. The canonical live profile is `start full` (brain + indexer +
  # dashboard) — matches what the operator gets via `./scripts/axon-live
  # start full` and what the rest of the qualified-release lineage assumes.
  if [[ "$restart_failed" -ne 1 && "$inplace_done" -ne 1 ]]; then
    if ! AXON_INSTANCE_KIND=live AXON_LIVE_RELEASE_MANIFEST="$pending_manifest" AXON_SKIP_BIN_SYNC=1 bash "$ROOT_DIR/scripts/axon" --instance live start full; then
      restart_failed=1
    fi
  fi

  # REQ-AXO-902064 — post-check (identity verify) runs for BOTH the full-restart
  # and in-place paths (the in-place path skipped `start full` above).
  if [[ "$restart_failed" -ne 1 && "$SKIP_POSTCHECK" -ne 1 ]]; then
      # REQ-AXO-901638 : poll_until replaces the legacy 24*5s fixed-sleep
      # loop. Default 150s timeout (covers brain cold-start: BGE-Large model
      # load + Phoenix dashboard, REQ-AXO-155 cold-start budget). Polling
      # interval 2s = sub-5s-cache-TTL window. Tunable via env.
      # REQ-AXO-901857 : gate léger /readyz (SELECT 1, cheap même à 500% CPU)
      # AVANT toute composition. Puis identité manifeste via
      # check_live_runtime_version.py, qui valide DÉJÀ build_id + package_version
      # + install_generation + instance_kind + brain_ready + indexer_ready +
      # authority. Le `status` lourd (requêtes PG + assemblage) était redondant
      # ET le maillon lent qui expirait le budget sous cold-reindex → retiré.
      _readyz_url="http://127.0.0.1:${AXON_BRAIN_PORT}/readyz"
      _postcheck_predicate() {
        curl -fsS --connect-timeout 3 --max-time 5 "$_readyz_url" >/dev/null 2>&1 \
        && python3 "$ROOT_DIR/scripts/release/check_live_runtime_version.py" \
          --manifest "$MANIFEST_PATH" \
          --url "$AXON_MCP_URL" \
          --install-generation "$install_generation" >/dev/null 2>&1
      }
      export -f _postcheck_predicate 2>/dev/null || true
      if poll_until "live MCP build_id + status indexer + status brain" \
           "$POSTCHECK_TIMEOUT_S" "$POSTCHECK_INTERVAL_S" \
           _postcheck_predicate; then
        verified=1
      else
        postcheck_failed=1
        echo "Post-check timed out after ${POSTCHECK_TIMEOUT_S}s (interval ${POSTCHECK_INTERVAL_S}s). Last diagnostics :" >&2
        python3 "$ROOT_DIR/scripts/release/check_live_runtime_version.py" \
          --manifest "$MANIFEST_PATH" --url "$AXON_MCP_URL" \
          --install-generation "$install_generation" >&2 || true
      fi
  fi
fi

if [[ "$restart_failed" -eq 1 ]]; then
  echo "" >&2
  echo "Live restart failed after staging the promotion artifact." >&2
  echo "  - pending manifest preserved : $pending_manifest" >&2
  echo "  - current manifest unchanged : $current_manifest" >&2
  echo "  - bin/* coherent with pending (sha256 matches)" >&2
  echo "" >&2
  echo "Next actions (REQ-AXO-901638 recovery menu) :" >&2
  echo "  1. Retry the failed restart phase only (preserves the build) :" >&2
  echo "       ./scripts/axon promote-live --manifest $MANIFEST_PATH --restart-live --resume" >&2
  echo "  2. Revert to the previous live manifest entirely :" >&2
  echo "       ./scripts/axon rollback-live    # picks the most-recent .axon/live-release/history/*.json" >&2
  echo "  3. Force-rollback bin/* to current.json artifacts (keeps pending for later --resume) :" >&2
  echo "       (functions sourced from this script ; from devenv shell)" >&2
  exit 1
fi

if [[ "$postcheck_failed" -eq 1 ]]; then
  echo "" >&2
  echo "Live restarted on the staged artifact, but MCP runtime_version post-check failed (${POSTCHECK_TIMEOUT_S}s polling exhausted)." >&2
  echo "  - pending manifest preserved : $pending_manifest" >&2
  echo "  - current manifest unchanged : $current_manifest" >&2
  echo "  - canonical processes alive on staged binaries (verify with ./scripts/axon-live status)" >&2
  echo "" >&2
  echo "Next actions :" >&2
  echo "  1. Inspect live runtime version / freshness :" >&2
  echo "       ./scripts/axon-live status" >&2
  echo "       curl -s http://127.0.0.1:44129/mcp -d '{\"jsonrpc\":\"2.0\",\"method\":\"tools/call\",\"id\":1,\"params\":{\"name\":\"status\",\"arguments\":{\"mode\":\"brief\"}}}'" >&2
  echo "  2. Retry promote (preserves build + restart) :" >&2
  echo "       ./scripts/axon promote-live --manifest $MANIFEST_PATH --restart-live --resume" >&2
  echo "  3. Or roll back to previous manifest :" >&2
  echo "       ./scripts/axon rollback-live" >&2
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
