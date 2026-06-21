#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$ROOT_DIR/scripts/lib/axon-instance.sh"
AXON_INSTANCE_KIND=live
axon_resolve_instance "$ROOT_DIR" "$(basename "$ROOT_DIR")"

PROJECT_CODE="AXO"
SKIP_BUILD=0
SKIP_QUALIFY=0
DRY_RUN=0
SKIP_DEV_VALIDATION=0

usage() {
  cat <<'EOF'
Usage: bash scripts/release/promote_live_safe.sh [--project <code>] [--skip-build] [--skip-qualify] [--skip-dev-validation] [--dry-run]

One-shot promotion flow:
  1. Build canonical release artifact
  2. Restart dev with candidate binary + validate dev healthy
  3. Run release preflight
  4. Create qualified release manifest
  5. Promote live (copy + restart)
  6. Run core MCP qualification
  7. Finalize (SOLL export + status)

Live promotion always builds the brain MCP + indexer authority contract.

Flags:
  --skip-dev-validation  EMERGENCY ONLY. Bypasses dev pre-flight. Use
                         only when dev environment is intentionally
                         unavailable (e.g. fresh-clone bootstrap before
                         dev has ever been started). Logs the bypass.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --project) PROJECT_CODE="${2:-}"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --skip-qualify) SKIP_QUALIFY=1; shift ;;
    --skip-dev-validation) SKIP_DEV_VALIDATION=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage; exit 1 ;;
  esac
done

[[ -n "$PROJECT_CODE" ]] || { echo "--project is required" >&2; exit 1; }

# --- REQ-AXO-901758: logging + step tracking + error trap ---
PROMOTE_TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
LOG_DIR="$ROOT_DIR/.axon/live-release"
mkdir -p "$LOG_DIR"
PROMOTE_LOG="$LOG_DIR/promote-${PROMOTE_TIMESTAMP}.log"

CURRENT_STEP=0
CURRENT_STEP_NAME="init"

promote_log() {
  local ts
  ts="$(date -u +%H:%M:%S)"
  echo "[$ts] $*" >> "$PROMOTE_LOG"
  echo "$*"
}

on_promote_failure() {
  local exit_code=$?
  promote_log ""
  promote_log "❌ PROMOTE FAILED at step ${CURRENT_STEP}: ${CURRENT_STEP_NAME}"
  promote_log "   Exit code: ${exit_code}"
  promote_log "   Log: ${PROMOTE_LOG}"
  promote_log "   Recovery: fix the issue and re-run the command."
  echo "" >&2
  echo "❌ PROMOTE FAILED at step ${CURRENT_STEP}: ${CURRENT_STEP_NAME} — see ${PROMOTE_LOG}" >&2
}
trap on_promote_failure ERR

run_step() {
  local step_num="$1"
  local step_name="$2"
  shift 2
  CURRENT_STEP="$step_num"
  CURRENT_STEP_NAME="$step_name"
  promote_log ""
  promote_log "== step ${step_num}: ${step_name} =="
  local _step_t0=$SECONDS
  local step_tmp
  step_tmp="$(mktemp)"
  set +e
  "$@" > "$step_tmp" 2>&1
  local rc=$?
  set -e
  cat "$step_tmp" | tee -a "$PROMOTE_LOG"
  rm -f "$step_tmp"
  if [[ "$rc" -ne 0 ]]; then
    promote_log "   step ${step_num} (${step_name}) returned exit code ${rc} after $((SECONDS - _step_t0))s"
    promote_log ""
    promote_log "❌ PROMOTE FAILED at step ${step_num}: ${step_name}"
    promote_log "   Exit code: ${rc}"
    promote_log "   Log: ${PROMOTE_LOG}"
    echo "" >&2
    echo "❌ PROMOTE FAILED at step ${step_num}: ${step_name} — see ${PROMOTE_LOG}" >&2
    exit "$rc"
  fi
  promote_log "   ✅ step ${step_num} (${step_name}) done in $((SECONDS - _step_t0))s"
}

start_head="$(git -C "$ROOT_DIR" rev-parse HEAD)"
promote_log "promote_live_safe.sh started at ${PROMOTE_TIMESTAMP}"
promote_log "project=${PROJECT_CODE} head=${start_head} skip_build=${SKIP_BUILD} skip_qualify=${SKIP_QUALIFY} skip_dev=${SKIP_DEV_VALIDATION}"

# REQ-AXO-902064 — fail-fast tracked-dirty gate BEFORE the (~2 min) build. The
# authoritative gate is step 3 release-preflight, but it runs AFTER the build, so
# a dirty tree used to waste the whole compile (observed session 88). This light
# pre-check (tracked changes only, <1s) fails fast; step 3 stays the full gate.
if [[ "$SKIP_BUILD" -ne 1 ]] && ! git -C "$ROOT_DIR" diff --quiet HEAD 2>/dev/null; then
  promote_log ""
  promote_log "❌ Tracked git state is dirty — failing fast BEFORE the build (step 3 preflight is the full gate)."
  git -C "$ROOT_DIR" status --short 2>/dev/null | tee -a "$PROMOTE_LOG" >&2 || true
  echo "❌ PROMOTE aborted: commit or stash tracked changes first (fast pre-gate, saved a full build)." >&2
  exit 1
fi

ensure_head_stable() {
  local current_head
  current_head="$(git -C "$ROOT_DIR" rev-parse HEAD)"
  if [[ "$current_head" != "$start_head" ]]; then
    promote_log "HEAD changed during promotion: start=$start_head current=$current_head"
    return 1
  fi
}

if [[ "$DRY_RUN" -eq 1 ]]; then
  echo "DRY RUN: would promote current HEAD via safe one-shot flow"
  echo "DRY RUN: project=$PROJECT_CODE runtime_contract=brain_mcp_indexer_ist head=$start_head skip_build=$SKIP_BUILD skip_qualify=$SKIP_QUALIFY skip_dev_validation=$SKIP_DEV_VALIDATION"
  exit 0
fi

# REQ-AXO-901656 — Step 0 : pre-flight dev validation gate. Refuses to
# promote live if dev MCP is not responding. Catches start.sh regressions
# and binary startup bugs in dev BEFORE they hit live (session 51 lesson :
# tmux send-keys 2KB truncation broke live for 1h because dev was never
# tested first ; `feedback_dev_first_no_exception` mandates this gate).
validate_dev_healthy() {
  local dev_mcp_port="44139"
  local probe_status
  probe_status=$(curl -fsS --max-time 5 -X POST "http://127.0.0.1:${dev_mcp_port}/mcp" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"tools/list","id":1}' 2>&1 | head -c 80 || true)
  if [[ "$probe_status" != *'"jsonrpc"'* ]]; then
    echo "❌ Dev MCP not responding on port ${dev_mcp_port} (feedback_dev_first_no_exception)." >&2
    echo "   New binaries must validate in dev BEFORE promote-live." >&2
    echo "   Recovery:" >&2
    echo "     ./scripts/axon-dev start brain        # or full" >&2
    echo "     # Verify dev MCP responds, run for >5 min." >&2
    echo "     # Re-run this command." >&2
    echo "" >&2
    echo "   Bypass (EMERGENCY ONLY, logs the violation):" >&2
    echo "     bash scripts/release/promote_live_safe.sh --skip-dev-validation ..." >&2
    return 1
  fi
  echo "  ✅ dev MCP responsive on ${dev_mcp_port}"

  # REQ-AXO-901659 — STRONGER gate : dev brain MUST run the candidate
  # binary (same git HEAD). Without this, "dev validation" was just a
  # ping ; an unchanged dev passes the ping while live receives an
  # untested new binary. Session 51 reinforcement (operator critique
  # after 3 violations of `feedback_dev_first_no_exception`).
  #
  # REQ-AXO-901660 (session 51 marathon fix) — extraction targets the
  # canonical JSON path `.result.data.runtime_version.build_id` (the
  # brain's OWN build_id) instead of the previous naive `grep build_id`
  # which incidentally captured `peer_runtime_version.build_id` (a
  # cached / federated entry that lags reality by N commits). The
  # naive parser would silently let mismatched dev brains pass when
  # they happened to share peer metadata with the candidate.
  local candidate_head="$(git -C "$ROOT_DIR" rev-parse HEAD)"
  local dev_status_json
  dev_status_json=$(curl -fsS --max-time 5 -X POST "http://127.0.0.1:${dev_mcp_port}/mcp" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"tools/call","params":{"name":"status","arguments":{"mode":"verbose"}},"id":1}' 2>&1 || true)
  local dev_build_id
  dev_build_id=$(printf '%s' "$dev_status_json" | python3 -c '
import json, sys
try:
    doc = json.load(sys.stdin)
    bid = doc.get("result", {}).get("data", {}).get("runtime_version", {}).get("build_id")
    if isinstance(bid, str) and bid:
        print(bid)
except Exception:
    pass
' 2>/dev/null || true)

  if [[ -z "$dev_build_id" ]]; then
    # Brain `status` may not surface `runtime_version.build_id` (older
    # binary contracts pre-REQ-AXO-150). Fall back to soft warning to
    # avoid blocking environments where introspection isn't wired ;
    # operator can still override via --skip-dev-validation if they
    # accept the risk.
    echo "  ⚠️ could not extract .result.data.runtime_version.build_id from dev status ; binary-match check skipped"
    return 0
  fi

  # Match : dev build_id must contain the short HEAD sha. Format ex :
  # `v0.8.0-635-g5e61cdd1` → contains `5e61cdd1`.
  local short_head="${candidate_head:0:8}"
  if [[ "$dev_build_id" == *"$short_head"* ]]; then
    echo "  ✅ dev brain runs candidate binary (build_id=$dev_build_id matches HEAD $short_head)"
  else
    echo "❌ Dev brain runs a DIFFERENT binary than the promotion candidate." >&2
    echo "   dev runtime_version.build_id : $dev_build_id" >&2
    echo "   candidate HEAD               : $candidate_head ($short_head)" >&2
    echo "   You are about to promote untested code to live." >&2
    echo "" >&2
    echo "   Recovery:" >&2
    echo "     # 1. Rebuild dev with current HEAD (force build.rs re-eval if cached)" >&2
    echo "     ./scripts/axon-dev stop --hard" >&2
    echo "     touch src/axon-core/build.rs 2>/dev/null  # force git-info rebuild" >&2
    echo "     devenv shell --no-reload --no-tui -- bash -lc 'cargo build --manifest-path src/axon-core/Cargo.toml --bin axon-brain --bin axon-indexer'" >&2
    echo "     ./scripts/axon-dev start full   # or brain" >&2
    echo "     # 2. Functional test in dev (e.g. create file, query MCP, observe effect)" >&2
    echo "     # 3. Re-run this command" >&2
    echo "" >&2
    echo "   Bypass (EMERGENCY ONLY) :" >&2
    echo "     bash scripts/release/promote_live_safe.sh --skip-dev-validation ..." >&2
    return 1
  fi
}

# --- Step 1: build ---
# REQ-AXO-901763 — Build BEFORE dev-gate so the dev brain can be restarted
# with the candidate binary. The previous ordering (dev_gate -> build) meant
# the dev brain always ran a binary compiled pre-commit whose build_id
# (git describe) pointed to HEAD^ instead of HEAD. The promote then failed
# because build_id != HEAD.
if [[ "$SKIP_BUILD" -ne 1 ]]; then
  run_step 1 build "$ROOT_DIR/scripts/axon" setup --artifact-only
fi

# --- Step 2: dev gate ---
# After building, restart dev with the new binary so validate_dev_healthy
# can verify the correct build_id. The restart is cheap (~5s) and ensures
# dev always validates the exact binary that will be promoted.
if [[ "$SKIP_DEV_VALIDATION" -eq 1 ]]; then
  promote_log "== step 2: dev_gate =="
  promote_log "  ⚠️ BYPASSED via --skip-dev-validation (violation of feedback_dev_first_no_exception)"
else
  restart_dev_with_candidate() {
    local dev_build_id_pre=""
    dev_build_id_pre=$(curl -fsS --max-time 5 -X POST "http://127.0.0.1:44139/mcp" \
      -H "Content-Type: application/json" \
      -d '{"jsonrpc":"2.0","method":"tools/call","params":{"name":"status","arguments":{"mode":"brief"}},"id":1}' 2>/dev/null \
      | python3 -c 'import json,sys; print(json.load(sys.stdin).get("result",{}).get("data",{}).get("runtime_version",{}).get("build_id",""))' 2>/dev/null || true)
    local short_head="${start_head:0:8}"
    if [[ -n "$dev_build_id_pre" && "$dev_build_id_pre" == *"$short_head"* ]]; then
      echo "  dev brain already runs candidate (build_id=$dev_build_id_pre)"
      return 0
    fi
    echo "  dev brain build_id ($dev_build_id_pre) != HEAD ($short_head), restarting dev..."
    bash "$ROOT_DIR/scripts/axon-dev" stop 2>&1 || true
    bash "$ROOT_DIR/scripts/axon-dev" start brain --fast 2>&1
  }
  run_step 2 dev_restart restart_dev_with_candidate
  run_step 2b dev_gate validate_dev_healthy
fi

# --- Step 3: preflight ---
ensure_head_stable
run_step 3 preflight "$ROOT_DIR/scripts/axon" release-preflight
ensure_head_stable

# --- Step 4: manifest ---
CURRENT_STEP=4; CURRENT_STEP_NAME="manifest"
promote_log ""
promote_log "== step 4: manifest =="
manifest_output="$("$ROOT_DIR/scripts/axon" create-release-manifest --state qualified 2>&1 | tee -a "$PROMOTE_LOG")"
manifest_path="$(echo "$manifest_output" | tail -n 1)"
if [[ -z "$manifest_path" || ! -f "$manifest_path" ]]; then
  promote_log "Failed to capture manifest path from create-release-manifest output"
  exit 1
fi
manifest_path="$(realpath "$manifest_path")"
promote_log "   ✅ step 4 (manifest) done — $manifest_path"

# --- Step 5: promote (copy + restart) ---
ensure_head_stable
old_md5="$(md5sum "$ROOT_DIR/bin/axon-brain" 2>/dev/null | cut -d' ' -f1 || echo "none")"
run_step 5 promote_copy_restart "$ROOT_DIR/scripts/axon" promote-live --manifest "$manifest_path" --restart-live --in-place
new_md5="$(md5sum "$ROOT_DIR/bin/axon-brain" 2>/dev/null | cut -d' ' -f1 || echo "none")"
promote_log "   bin/axon-brain md5: ${old_md5} → ${new_md5}"
# NOTE: an UNCHANGED md5 is NOT a failure — re-promoting an identical build
# (same HEAD → byte-identical candidate) is idempotent and expected. Promotion
# correctness is proven by promote-live's internal runtime-identity match +
# step-6 qualify-mcp, not by an old-vs-new binary diff. (clean-win: removed the
# false "md5 unchanged → copy may have failed" warning.)

# --- Step 6: qualify ---
if [[ "$SKIP_QUALIFY" -ne 1 ]]; then
  ensure_head_stable
  run_step 6 qualify_mcp "$ROOT_DIR/scripts/axon" --instance live qualify-mcp --surface core --checks quality,latency --project "$PROJECT_CODE"
fi

# --- Step 7: finalize (SOLL export + status) ---
CURRENT_STEP=7; CURRENT_STEP_NAME="finalize"
promote_log ""
promote_log "== step 7: finalize =="

# REQ-AXO-126 — SOLL snapshot for release lineage (best-effort)
soll_export_args=$(printf '{"project_code":"%s"}' "$PROJECT_CODE")
if ! "$ROOT_DIR/scripts/axon" --instance live mcp-call call soll_export --args "$soll_export_args" --format text >> "$PROMOTE_LOG" 2>&1; then
  promote_log "   ⚠️ soll_export failed (non-blocking — manifest is authoritative)"
fi

ensure_head_stable
# REQ-AXO-901879 — step 7 is finalize (SOLL export + status DISPLAY).
# Promotion correctness is already gated by step 5 (atomic binary swap +
# runtime-identity match) and step 6 (qualify-mcp verdict=ok against the live
# brain). The legacy pid-file `axon-live status` surface mis-reports OVERALL
# DOWN on a healthy process-compose runtime — it reads stale
# `.axon/live-run/*.pid` that the process-compose supervisor no longer writes —
# so its exit code must NOT fire the ERR trap and spuriously roll back a
# successful promote. Display-only; `|| true` neutralises the pipefail exit.
bash "$ROOT_DIR/scripts/axon-live" status 2>&1 | tee -a "$PROMOTE_LOG" || true
promote_log "   ✅ step 7 (finalize) done"

# REQ-AXO-902052 #6-B — fire-and-forget Memgraph publication refresh. Runs
# OUTSIDE `run_step` (which aborts on rc≠0) and can NEVER fail the promote: the
# wrapper is graceful (clean skip + marker, exit 0, when Docker/tools are
# unavailable — the current WSL state), and it is backgrounded so the promote
# never waits on the ~200 MB export/load. PIL-AXO-005 fail-closed is untouched.
( nohup bash "$ROOT_DIR/scripts/publish-memgraph.sh" >>"$PROMOTE_LOG" 2>&1 & ) || true
promote_log "   ▶ Memgraph publication refresh dispatched (background, best-effort)"

# --- Final summary ---
final_md5="$(md5sum "$ROOT_DIR/bin/axon-brain" 2>/dev/null | cut -d' ' -f1 || echo "unknown")"
final_build_id="$(python3 -c "
import json, sys
try:
    d = json.load(open('$ROOT_DIR/.axon/live-release/current.json'))
    print(d.get('source',{}).get('build_id','') or d.get('runtime_version',{}).get('build_id','unknown'))
except: print('unknown')
" 2>/dev/null || echo "unknown")"

promote_log ""
promote_log "✅ PROMOTE COMPLETE"
promote_log "   build_id=${final_build_id}"
promote_log "   sha=${start_head:0:12}"
promote_log "   bin/axon-brain md5=${final_md5}"
promote_log "   manifest=${manifest_path}"
promote_log "   log=${PROMOTE_LOG}"

# Disable the ERR trap — we succeeded
trap - ERR
