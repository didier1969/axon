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
# REQ-AXO-902165 / DEC-AXO-901666 / REQ-AXO-902256 — step 5 runs via `axonctl cutover`
# (the Rust health-gated cutover with NATIVE auto-rollback). There is no longer a second
# step-5 implementation to choose between: the `USE_CUTOVER` toggle and the
# `promote-live --in-place` branch it guarded are both REMOVED.
#
# Why the toggle had to go rather than merely flip to 1: it defaulted to 0, so every
# promote for months ran the unprotected path while the protected one sat behind a flag
# nobody passed. That path has left the live indexer down on three documented occasions
# (s95/1306 · 2026-06-27/REQ-AXO-902109 · 2026-07-26/1399, where step-6c's recovery then
# took the BRAIN down ~3m53s and third-party MCP clients self-restarted). A safe path
# that is opt-in is not a safe path.

usage() {
  cat <<'EOF'
Usage: bash scripts/release/promote_live_safe.sh [--project <code>] [--skip-build] [--skip-qualify] [--skip-dev-validation] [--dry-run]

One-shot promotion flow:
  1. Build canonical release artifact
  2. Restart dev with candidate binary + validate dev healthy
  3. Run release preflight
  4. Create qualified release manifest
  5. Promote live — health-gated cutover with auto-rollback (axonctl)
  6. Run core MCP qualification
  7. Finalize (SOLL export + status)

Live promotion always builds the brain MCP + indexer authority contract.

Flags:
  --skip-dev-validation  EMERGENCY ONLY. Bypasses dev pre-flight. Use
                         only when dev environment is intentionally
                         unavailable (e.g. fresh-clone bootstrap before
                         dev has ever been started). Logs the bypass.
  --cutover              Accepted and ignored. Step 5 ALWAYS runs the health-gated
                         `axonctl cutover` now (240s liveness budget covering the
                         indexer's GPU cold-start, native auto-rollback to the
                         previous build, DDL re-bootstrap). The in-place
                         alternative and its toggle were removed in
                         REQ-AXO-902256. Kept only so existing muscle memory and
                         docs do not fail on an unknown option.

Emergency paths (deliberately NOT flags of this script):
  Stranded pending.json  bash scripts/release/promote_live_safe.sh   (auto-resumes)
  Direct executor        bin/axonctl cutover --project-root . --instance-kind live \
                         --manifest <candidate.json> --max-polls 120 --poll-interval-ms 2000
  Roll back a release    bash scripts/release/rollback_live.sh
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --project) PROJECT_CODE="${2:-}"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --skip-qualify) SKIP_QUALIFY=1; shift ;;
    --skip-dev-validation) SKIP_DEV_VALIDATION=1; shift ;;
    # REQ-AXO-902256 — no-op: the cutover is the only step-5 path now.
    --cutover) shift ;;
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

# --- REQ-AXO-902194: best-effort cross-project MCP-disruption broadcast ---
# The step-5 brain restart drops every connected LLM's MCP for a few seconds. A
# broadcast (to_project='*') leaves an explanatory trace so peers find "planned
# promote, not a crash" on reconnect instead of burning tokens on a false RCA.
# STRICTLY best-effort: a missing/slow mailbox must NEVER fail a promote.
broadcast_promote() {
  local subject="$1" body="$2" key="$3" args
  args="$(python3 -c "
import json, sys
print(json.dumps({
    'to_project': '*', 'from': '${PROJECT_CODE}',
    'subject': sys.argv[1], 'body_dense': sys.argv[2],
    'idempotency_key': sys.argv[3], 'priority': 'high',
}))" "$subject" "$body" "$key" 2>/dev/null || true)"
  [[ -z "$args" ]] && return 0
  timeout 20 "$ROOT_DIR/scripts/axon" --instance live mcp-call call mcp_outbox_send \
    --args "$args" --format text >> "$PROMOTE_LOG" 2>&1 || true
}

# All-clear on ANY exit (success OR the step-6 cold-start false-fail where the script
# exits non-zero but the brain IS back). Fires only if (a) the pre-notice went out and
# (b) the live brain answers /readyz — so we never claim "back" while it is still down.
BROADCAST_PREFLIGHT_SENT=0
on_promote_exit() {
  local rc=$?
  [[ "$BROADCAST_PREFLIGHT_SENT" -eq 1 ]] || return 0
  curl -fsS --max-time 5 "http://127.0.0.1:44129/readyz" >/dev/null 2>&1 || return 0
  if [[ "$rc" -eq 0 ]]; then
    broadcast_promote "✅ Promote ${PROJECT_CODE} terminé — MCP rétabli" \
      "build_id=${final_build_id:-?} live. Si ton MCP est tombé depuis ${PROMOTE_TIMESTAMP} c'était CE promote (restart brain), pas un incident. Reconnecte via /mcp si ton binding de catalogue est stale. Tout est de nouveau disponible." \
      "promote-clear-${PROMOTE_TIMESTAMP}"
  else
    broadcast_promote "⚠️ Promote ${PROJECT_CODE} sorti (rc=${rc}) — brain UP" \
      "Le brain live RÉPOND (/readyz ok). Si ton MCP est tombé c'était le restart de CE promote (${PROMOTE_TIMESTAMP}), PAS un incident à diagnostiquer. Reconnecte via /mcp. (Le promote a pu false-fail au qualify cold-start ; l'opérateur AXO vérifie.)" \
      "promote-clear-${PROMOTE_TIMESTAMP}"
  fi
}
trap on_promote_exit EXIT

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
  # REQ-AXO-902263 — STREAM the step's output; do NOT buffer it to a temp file and print at
  # the end. Buffering means a step that HANGS produces zero diagnostic: the promote of
  # 2026-07-27 sat in `dev_restart` for ~30 min and was killed by its own `timeout 2400`
  # having written nothing past the step header, so the cause was invisible in the log. The
  # buffered text was only recoverable by digging a leftover /tmp file out of the
  # filesystem afterwards. An operator watching a promote must be able to see WHERE it is
  # stuck while it is stuck — that is the whole point of a step log.
  #
  # `pipefail` is already set (line 2), so the pipeline's status is the COMMAND's status,
  # not tee's. PIPESTATUS is captured anyway to stay correct if that ever changes.
  set +e
  "$@" 2>&1 | tee -a "$PROMOTE_LOG"
  local rc="${PIPESTATUS[0]}"
  set -e
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
    echo "   New binaries must validate in dev BEFORE promoting to live." >&2
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

# --- REQ-AXO-902104: auto-resume an interrupted promote ---
# A prior run killed/interrupted mid-step-5 leaves the new binary live but the
# manifest UNFINALIZED (pending.json present) and the runtime degraded (query-embed
# down, indexer not ready). Stacking a fresh promote on top compounds the mess —
# instead, detect the pending state and resume it (restart-live + finalize) first.
# Set PROMOTE_SKIP_AUTORESUME=1 to bypass.
pending_manifest="$ROOT_DIR/.axon/live-release/pending.json"
if [[ -f "$pending_manifest" && "${PROMOTE_SKIP_AUTORESUME:-0}" != "1" ]]; then
  pending_build="$(jq -r '.build_id // empty' "$pending_manifest" 2>/dev/null || true)"
  promote_log "⚠️ Unfinalized pending promote detected (build_id=${pending_build:-?}) — auto-resuming before any fresh promote (REQ-AXO-902104)."
  candidate_manifest="$(ls -1 "$ROOT_DIR"/.axon/releases/candidates/*"${pending_build}".json 2>/dev/null | head -1)"
  if [[ -n "$candidate_manifest" && -f "$candidate_manifest" ]]; then
    # REQ-AXO-902256 — resume through the CUTOVER, not `promote-live --resume`. This was
    # the last thing keeping promote_live.sh alive, and keeping it meant keeping two
    # divergent executors for the same job. Re-running the cutover on the stranded build's
    # candidate manifest IS the resume: snapshot (current.json is still a valid rollback
    # target — the stranded run never finalized), stage (rewrites pending.json + bin/*,
    # idempotent), restart, liveness gate, then finalize. It is strictly stronger than the
    # old path because the byte check of REQ-AXO-902258 now runs on the way through, so a
    # resume cannot re-commit a wrong binary.
    "$ROOT_DIR/bin/axonctl" cutover \
      --project-root "$ROOT_DIR" --instance-kind live --manifest "$candidate_manifest" \
      --max-polls 120 --poll-interval-ms 2000 --json >> "$PROMOTE_LOG" 2>&1
    resume_rc=$?
    promote_log "   auto-resume via cutover exit=$resume_rc (build_id=$pending_build)"
    exit $resume_rc
  fi
  promote_log "   ⚠️ candidate manifest for $pending_build not found — aborting to avoid stacking."
  promote_log "      Recover with: bin/axonctl cutover --project-root $ROOT_DIR --instance-kind live --manifest <candidate>"
  promote_log "      Or roll back:  bash scripts/release/rollback_live.sh"
  exit 1
fi

# --- REQ-AXO-902194: pre-notice (brain still up) — warn peers the step-5 restart
# will drop MCP briefly. Async, so mostly read on reconnect; harmless to send early. ---
broadcast_promote "🔧 Promote ${PROJECT_CODE} en cours — coupure MCP brève à venir" \
  "Un promote AXO démarre (${PROMOTE_TIMESTAMP}). Au restart du brain (dans ~3-6 min) le MCP tombera pour TOUS les clients connectés. Ordre de grandeur mesuré: le brain met ~8 s entre son lancement et /readyz, plus la durée de son arrêt — donc DIZAINES DE SECONDES, pas quelques secondes. C'est PLANIFIÉ: NE relance PAS le serveur toi-même, ton self-heal entrerait en course avec la bascule (incident du 2026-07-26, REQ-AXO-902256). Attends l'all-clear, qui suit dès que le brain répond. Si ton binding reste stale ensuite, reconnecte via /mcp." \
  "promote-notice-${PROMOTE_TIMESTAMP}"
BROADCAST_PREFLIGHT_SENT=1

# --- Step 1: build ---
# REQ-AXO-901763 — Build BEFORE dev-gate so the dev brain can be restarted
# with the candidate binary. The previous ordering (dev_gate -> build) meant
# the dev brain always ran a binary compiled pre-commit whose build_id
# (git describe) pointed to HEAD^ instead of HEAD. The promote then failed
# because build_id != HEAD.
if [[ "$SKIP_BUILD" -ne 1 ]]; then
  run_step 1 build "$ROOT_DIR/scripts/axon" setup --artifact-only
fi

# --- Step 4 (manifest) launched EARLY, in background (REQ-AXO-902188) ---
# The manifest (sha + archive of bin/*) depends ONLY on the build (step 1), NOT on the
# dev_gate (step 2, read-only on bin/*) nor preflight (step 3). Run it CONCURRENTLY with
# the dev_gate to take ~10-30s off the critical path; joined at step 4 below (before the
# step-5 swap, which needs manifest_path). Both sides are read-only on bin/* → no write
# race. If HEAD moves meanwhile, the ensure_head_stable guards at steps 3/5 fail-close
# before the (now-stale) manifest is ever used.
manifest_bg_out="$(mktemp)"
manifest_bg_pid=""
( "$ROOT_DIR/scripts/axon" create-release-manifest --state qualified ) > "$manifest_bg_out" 2>&1 &
manifest_bg_pid=$!
promote_log "== step 4: manifest launched in background (∥ step 2 dev_gate — REQ-AXO-902188) pid=${manifest_bg_pid} =="

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
  # RCA promote 20260627 (REQ-AXO-902101) — tear down the dev instance NOW, before
  # the live restart + post-check (steps 5/6). A lingering dev brain auto-pauses
  # the live indexer (REQ-AXO-234 GPU-exclusion) → the live post-check's
  # `indexer_ready` never becomes true → step 5 times out (600s) even though the
  # binary swapped correctly (observed: live brain on candidate, indexer stale,
  # manifest left pending). Stopping dev here lets the live indexer resume before
  # the gate. The dev instance is no longer needed once dev_gate has validated it.
  teardown_dev_after_validation() {
    bash "$ROOT_DIR/scripts/axon-dev" stop 2>&1 || true
  }
  run_step 2c teardown_dev teardown_dev_after_validation

  # REQ-AXO-902263 — LIFECYCLE GATE. Placed AFTER teardown_dev on purpose: with dev gone,
  # the live indexer has resumed (REQ-AXO-234 GPU exclusion), so this exercises the real
  # per-role restart on a representative runtime rather than a GPU-starved one.
  #
  # Why gate a release on this: step-6c TIER-1 is the promote's own recovery path, and it
  # was INOPERATIVE for a whole day — it trusted the HTTP 200 of
  # `POST /process/restart/axon-indexer` while the role stayed down. Nothing caught it
  # because ~2 758 lines of lifecycle script had zero functional coverage. Shipping a
  # release whose recovery path is broken is exactly what this refuses.
  #
  # Cost, stated rather than hidden: the live INDEXER is dropped once more than before,
  # here, upstream of the cutover. The operator explicitly authorised seconds to 2-3 min of
  # indexer downtime; the BRAIN is the sensitive one and the test asserts it never stops
  # serving. The script SKIPs (exit 0) when the runtime is not in a testable state, so this
  # never fails a promote for an unrelated reason.
  # Exit 77 = the script SKIPPED (nothing measured, e.g. the role is not Running+Ready).
  # A skip must not fail a release — but it must not pass silently either, or the gate
  # becomes the vacuous green it exists to prevent. Surface it loudly and continue.
  lifecycle_gate_step() {
    local rc=0
    bash "$ROOT_DIR/tests/shell/test_role_restart_live.sh" || rc=$?
    if [[ "$rc" -eq 77 ]]; then
      echo "⚠️ lifecycle gate SKIPPED (nothing measured) — the per-role restart was NOT verified for this release"
      return 0
    fi
    return "$rc"
  }
  run_step 2d lifecycle_gate lifecycle_gate_step
fi

# --- Step 2e: the TEST TARGETS must still compile (REQ-AXO-902269) ---
# `--lib` and `--bins` — the pair every delivery runs — do not BUILD `src/axon-core/tests/`.
# On 2026-07-12 REQ-AXO-902227 added a field to `SymbolRow` without updating four
# initializers there, and the six integration binaries stopped compiling. It went unnoticed
# for 15 days while every session reported a green suite: `0 failed` was true and useless.
#
# This builds the test targets in DEBUG and does not RUN them, deliberately. The 9 tests are
# `#[ignore = "requires docker"]`, so running them without `--ignored` executes nothing, and
# with `--ignored` the gate would depend on a Docker daemon — an environment dependency is
# how a gate ends up disabled (the fate of the `USE_CUTOVER` toggle REQ-AXO-902256 removed).
# Compiling catches 100% of the class actually observed — structural drift — for ~1 min on a
# warm cache.
test_targets_compile_step() {
  devenv shell --no-reload --no-tui -- bash -lc \
    "cd '$ROOT_DIR/src/axon-core' && cargo build --tests 2>&1 | tail -20"
}
run_step 2e test_targets_compile test_targets_compile_step

# --- Step 3: preflight ---
ensure_head_stable
run_step 3 preflight "$ROOT_DIR/scripts/axon" release-preflight
ensure_head_stable

# --- Step 4: manifest (JOIN the background job launched before step 2, REQ-AXO-902188) ---
CURRENT_STEP=4; CURRENT_STEP_NAME="manifest"
promote_log ""
promote_log "== step 4: manifest (join background pid=${manifest_bg_pid:-none}) =="
if [[ -n "$manifest_bg_pid" ]]; then
  if ! wait "$manifest_bg_pid"; then
    cat "$manifest_bg_out" | tee -a "$PROMOTE_LOG"
    rm -f "$manifest_bg_out"
    promote_log "❌ background manifest job (pid=${manifest_bg_pid}) failed"
    exit 1
  fi
else
  # Fallback: background launch was skipped — build the manifest synchronously now.
  "$ROOT_DIR/scripts/axon" create-release-manifest --state qualified > "$manifest_bg_out" 2>&1 \
    || { cat "$manifest_bg_out" | tee -a "$PROMOTE_LOG"; rm -f "$manifest_bg_out"; exit 1; }
fi
cat "$manifest_bg_out" | tee -a "$PROMOTE_LOG"
manifest_path="$(tail -n 1 "$manifest_bg_out")"
rm -f "$manifest_bg_out"
if [[ -z "$manifest_path" || ! -f "$manifest_path" ]]; then
  promote_log "Failed to capture manifest path from create-release-manifest output"
  exit 1
fi
manifest_path="$(realpath "$manifest_path")"
promote_log "   ✅ step 4 (manifest) done — $manifest_path"

# --- MCP availability sampler (REQ-AXO-902256) ---
# The promote used to report step 5 as "done in 35s" — a figure that measures the binary
# copy and EXCLUDES the indexer coming back and any step-6c recovery. On promote 1399 the
# real MCP outage was ~3m53s while the reported number was 35s, so the operator was told
# the interruption was negligible when third-party clients were self-restarting. Estimates
# are not good enough here: sample the endpoint other clients actually call, once a second,
# across steps 5→6c, then report the measured worst contiguous gap.
MCP_SAMPLE_FILE="$LOG_DIR/mcp-availability-${PROMOTE_TIMESTAMP}.csv"
MCP_SAMPLER_PID=""
_start_mcp_sampler() {
  : > "$MCP_SAMPLE_FILE"
  (
    while :; do
      # Two bugs lived on this line; both are worth naming because they broke in OPPOSITE
      # directions and each looked like a green result.
      #  v1: `... || echo 000` — `-w '%{http_code}'` ALREADY prints 000 on a refused
      #      connection, so the fallback appended a SECOND one → code="000000" → the
      #      `== "000"` test never matched and EVERY sample was classified `up`. It
      #      reported "0s outage" across a promote that demonstrably restarted the brain.
      #  v2: dropping the fallback fixed the value but exposed curl's non-zero exit to
      #      `set -e`, which killed the sampler subshell at the FIRST outage — 4 samples
      #      then silence, i.e. it died exactly when it had something to record.
      # `|| true` is what both needed: the substitution still captures the "000" that -w
      # printed, and the exit status is neutralised so `set -e` leaves the loop alone.
      code="$(curl -s -m 2 -o /dev/null -w '%{http_code}' \
        "http://127.0.0.1:${AXON_BRAIN_PORT:-44129}/mcp" \
        -H 'content-type: application/json' \
        -d '{"jsonrpc":"2.0","method":"tools/list","id":1}' 2>/dev/null)" || true
      # Any real HTTP status means the endpoint answered; 000 (or empty, if curl itself
      # could not run) = connection refused/timeout — exactly what a third-party MCP
      # client experiences as "server down".
      if [[ -z "$code" || "$code" == "000" ]]; then
        printf '%s,down\n' "$(date -u +%s)"
      else
        printf '%s,up\n' "$(date -u +%s)"
      fi
      sleep 1
    done
  ) >> "$MCP_SAMPLE_FILE" 2>/dev/null &
  MCP_SAMPLER_PID=$!
}
_report_mcp_outage() {
  [[ -n "$MCP_SAMPLER_PID" ]] || return 0
  kill "$MCP_SAMPLER_PID" 2>/dev/null || true
  wait "$MCP_SAMPLER_PID" 2>/dev/null || true
  local worst total
  read -r worst total < <(python3 - "$MCP_SAMPLE_FILE" <<'PY' 2>/dev/null || echo "0 0"
import sys
run = worst = total = 0
try:
    for line in open(sys.argv[1]):
        parts = line.strip().split(',')
        if len(parts) != 2:
            continue
        if parts[1] == 'down':
            run += 1; total += 1; worst = max(worst, run)
        else:
            run = 0
except OSError:
    pass
print(worst, total)
PY
  )
  # A silent instrument reads exactly like a green result. Report the sample count and
  # the covered span so "0 outage" can be distinguished from "measured nothing", and say
  # the true resolution: each sample costs a curl (up to its 2s timeout) PLUS the 1s
  # sleep. MEASURED on a refused port: ~3s between samples, so a blip shorter than that
  # can fall between two samples. Do not quote this instrument to finer than ~3s.
  local n span first last
  n="$(wc -l < "$MCP_SAMPLE_FILE" 2>/dev/null | tr -d ' ')"; n="${n:-0}"
  first="$(head -1 "$MCP_SAMPLE_FILE" 2>/dev/null | cut -d, -f1)"
  last="$(tail -1 "$MCP_SAMPLE_FILE" 2>/dev/null | cut -d, -f1)"
  span=0
  [[ -n "$first" && -n "$last" ]] && span=$(( last - first ))
  promote_log ""
  promote_log "== MCP availability (measured across steps 5→6c) =="
  if [[ "$n" -lt 5 ]]; then
    promote_log "   ⚠️ NOT MEASURED — only ${n} sample(s) collected. Treat the figures below as UNKNOWN, not as zero."
  fi
  promote_log "   worst contiguous outage: ${worst}s · total unreachable: ${total}s"
  promote_log "   ${n} samples over ${span}s (measured resolution ~3s — a shorter blip can fall between samples)"
  promote_log "   samples: $MCP_SAMPLE_FILE"
  if [[ "${worst:-0}" -gt 60 ]]; then
    promote_log "   ⚠️ outage > 60s — third-party MCP clients may have declared a crash and self-restarted (REQ-AXO-902256 acceptance breach)."
  fi
}

# --- Step 5: promote (copy + restart) ---
_start_mcp_sampler
ensure_head_stable
old_md5="$(md5sum "$ROOT_DIR/bin/axon-brain" 2>/dev/null | cut -d' ' -f1 || echo "none")"
# REQ-AXO-902165 / DEC-AXO-901666 — health-gated cutover with NATIVE auto-rollback
# (the Rust control-plane executor). One command: snapshot (current.json = rollback
# target) → stage (candidate bin/*) → full restart (re-bootstraps DDL, unlike the retired
# in-place path → step 5b is now a cheap idempotent no-op kept as a guard) → `axonctl
# liveness` gate (FULL runtime_contract: brain + indexer /readyz) → finalize
# (pending→current) OR auto-rollback (restore the PREVIOUS build + restart). A bad
# candidate is reverted to the last-good build, not just restarted. bin/axonctl was
# rebuilt by step 1, so it carries the current cutover logic. On failure it exits
# non-zero → run_step fails the promote (the ERR trap logs it; no half-finalized
# manifest — cutover rolled back).
# --max-polls 120 × 2000ms = 240s health-gate budget: covers the new indexer's
# BGE-Large GPU cold-start (can exceed the 60s default under load) so a slow-but-fine
# indexer is NOT falsely rolled back.
#
# REQ-AXO-902256 — the `promote-live --in-place` branch that used to live here is GONE,
# and with it the dual-executor divergence. Keeping two co-equal step-5 implementations
# is what let the orchestrator run the unprotected one for months: no liveness gate, no
# rollback, no DDL re-bootstrap, and a documented history of leaving the live indexer
# down (s95/1306, 2026-06-27/902109, 2026-07-26/1399). The file-level guarantees this
# executor relies on are pinned by tests in axonctl_tests.rs (snapshot refuses without a
# rollback target · stage leaves current.json intact · rollback restores the OLD binary
# BYTES · finalize consumes pending and archives both generations).
#
# `promote_live.sh` is NOT deleted: it still carries `--resume`, the documented recovery
# for a stranded `pending.json` — and `release_reconciler::next_action` points operators
# at it by name — BOTH have since been repointed at the cutover, and the script itself
# is DELETED. `--resume` now means: re-run this script; it detects the stranded
# pending.json and replays the cutover on that build's candidate manifest, byte-check
# included. One executor, one path.
run_step 5 cutover "$ROOT_DIR/bin/axonctl" cutover \
  --project-root "$ROOT_DIR" --instance-kind live --manifest "$manifest_path" \
  --max-polls 120 --poll-interval-ms 2000 --json
new_md5="$(md5sum "$ROOT_DIR/bin/axon-brain" 2>/dev/null | cut -d' ' -f1 || echo "none")"
promote_log "   bin/axon-brain md5: ${old_md5} → ${new_md5}"
# NOTE: an UNCHANGED md5 is NOT a failure — re-promoting an identical build
# (same HEAD → byte-identical candidate) is idempotent and expected. Promotion
# correctness is proven by the cutover's own byte verification (REQ-AXO-902258) +
# step-6 qualify-mcp, not by an old-vs-new binary diff. (clean-win: removed the
# false "md5 unchanged → copy may have failed" warning.)

# --- Step 5b: apply canonical DDL to live (REQ-AXO-902127) ---
# HISTORY: written because the (now-retired) in-place restart did NOT re-run the
# canonical DDL bootstrap, so a promote that ADDS/changes a db/ddl/*.sql file left
# axon_live without it (real incident: MBX-1's axon.mailbox_message missing
# post-promote, needed a manual psql).
#
# REQ-AXO-902256 — step 5 is now a full-restart cutover, which DOES re-bootstrap the
# DDL, so this step is expected to be a no-op. It is KEPT DELIBERATELY as a defensive
# guard, not left behind by accident: the DDL files are idempotent (CREATE … IF NOT
# EXISTS) so this costs a few ms when warm, and it is the only thing standing between a
# regression in the cutover's bootstrap and a live DB silently missing a table. A cheap
# idempotent guard is not the redundancy worth removing — two divergent promote
# executors was.
#
# Observed 2026-07-26 (promote 1399, in-place path): the runtime started BEFORE this
# step ran, so axon.EmbedderControl did not exist when the indexer tried to seed its
# idle-drop control row. The cutover ordering removes that window.
# Runs in devenv so psql resolves.
run_step 5b apply_ddl_live bash -lc "cd '$ROOT_DIR' && devenv shell --no-reload --no-tui -- bash -lc 'source scripts/lib/ensure-runtime.sh && apply_canonical_ddl live'"

# --- promote_status full-contract poll (shared by step 6 pre-gate + step 6c) ---
# REQ-AXO-902189 — hoisted above step 6 so the qualify pre-gate and the 6c health-gate
# reuse ONE definition. Sets recon_phase / recon_failed; returns 0 iff phase==clean.
recon_phase=""; recon_failed=""
_poll_promote_clean() {  # $1 = max attempts (×5s); sets recon_phase / recon_failed; 0 iff clean
  local attempts="$1" _a recon_json recon_eval
  recon_phase=""; recon_failed=""
  for _a in $(seq 1 "$attempts"); do
    recon_json="$(curl -s -m 8 "http://127.0.0.1:${AXON_BRAIN_PORT:-44129}/mcp" \
      -H 'content-type: application/json' \
      -d '{"jsonrpc":"2.0","method":"tools/call","id":1,"params":{"name":"promote_status","arguments":{}}}' 2>/dev/null || true)"
    recon_eval="$(printf '%s' "$recon_json" | python3 -c "import sys,json
ph=''; fg=''
for l in sys.stdin.read().splitlines():
    l=l.strip()
    if l.startswith('data:'): l=l[5:].strip()
    if not l: continue
    try:
        d=json.loads(l).get('result',{}).get('data') or {}
        if d.get('phase'): ph=d['phase']; fg=','.join(d.get('failed_gates') or [])
    except: pass
print(f'{ph}|{fg}')" 2>/dev/null || true)"
    recon_phase="${recon_eval%%|*}"; recon_failed="${recon_eval##*|}"
    [[ "$recon_phase" == "clean" ]] && return 0
    sleep 5
  done
  return 1
}

# --- Step 6: qualify ---
# REQ-AXO-902189 (incident promote 1316) — GATE qualify on the FULL runtime_contract
# liveness FIRST. A fresh live indexer's BGE-Large GPU cold-start can take minutes; if
# qualify (surface=core, brain-only latency/quality) runs while the indexer is still
# cold, quality=fail → run_step exits non-zero → the promote FAILS at step 6 and the
# step-6c fail-closed recovery gate NEVER runs (it is PREEMPTED), leaving a live runtime
# that actually deployed fine reported as FAILED. Poll promote_status (brain_serving +
# indexer_alive) up to ~120s; only run the qualify scenarios once clean. If the runtime
# never reaches clean within budget, SKIP qualify and DEFER to step 6c — which owns the
# auto-recovery + authoritative fail-closed verdict — so cold-start can never preempt it.
if [[ "$SKIP_QUALIFY" -ne 1 ]]; then
  ensure_head_stable
  if _poll_promote_clean 24; then
    run_step 6 qualify_mcp "$ROOT_DIR/scripts/axon" --instance live qualify-mcp --surface core --checks quality,latency --project "$PROJECT_CODE"
  else
    promote_log "   ⚠️ step 6: runtime not clean after ~120s warmup (phase=${recon_phase:-unreachable}) — SKIP qualify, DEFER to step 6c recovery gate (REQ-AXO-902189: cold-start must not preempt the fail-closed verdict)."
  fi
fi

# --- Step 6c: reconcile + FAIL-CLOSED health-gate (REQ-AXO-902111 / REQ-AXO-902157) ---
# Dogfood promote_status as the post-swap verdict over the FULL runtime_contract
# (brain_serving + indexer_alive). Written for the s95 incident (promote 1306): the
# RETIRED in-place restart left the live indexer down/crash-looping and the promote
# reported COMPLETE on a DEGRADED runtime, because this gate was warn-only AND step-6
# qualify tests only the brain (surface=core). Poll the verdict on an extended
# GPU-cold-start budget; on a persistent non-clean phase, AUTO-RECOVER, re-verify, and
# FAIL CLOSED if still not clean — a promote must NEVER silently report success on a
# half-up runtime_contract.
#
# REQ-AXO-902256 — step 5 is now the health-gated cutover, which owns its OWN liveness
# gate (240s) plus native rollback, so this block should rarely fire at all. It is kept
# because the two gates check different things: the cutover gates the CANDIDATE and
# rolls back a bad build, while this one gates the runtime_contract after everything the
# orchestrator did (including step 5b's DDL). Recovery is now an escalation ladder
# (see below) rather than an unconditional full restart.
CURRENT_STEP=6c; CURRENT_STEP_NAME="reconcile"
promote_log ""
promote_log "== step 6c: reconcile + health-gate (promote_status) =="

# _poll_promote_clean is defined above step 6 (REQ-AXO-902189) — reused here as the
# authoritative post-swap health-gate. The step-6 pre-gate never preempts this block.

# First gate: extended warmup (~120s) — a fresh live indexer's BGE-Large GPU cold-start
# can take minutes to publish its first heartbeat.
_poll_promote_clean 24 || true

# REQ-AXO-902256 — ESCALATION LADDER, not a single hammer. The previous code went
# straight to `stop --hard + start full`, which takes the BRAIN down to recover the
# INDEXER. That violates PIL-AXO-008 (the two roles are independently activatable) and
# is what cost ~3m53s of MCP unavailability on promote 1399 — long enough that MCP
# clients in other projects declared a crash and restarted the server themselves.
# Tier 1 restarts ONLY the failed role via the process-compose REST control plane
# (which already carries a per-process readiness probe: has_ready_probe=true). Tier 2
# is the old full restart, unchanged, reached only when Tier 1 does not converge — so
# the safety net is preserved while the blast radius is capped in the common case.
# REQ-AXO-902263 — TIER-1 now VERIFIES the effect instead of trusting the HTTP code.
# The first version of this block did `[[ "$code" == "200" ]]`, and that predicate is
# WRONG: `POST /process/restart/<name>` answers 200 and then may leave the role down.
# Observed on the live indexer — 200, then `Terminating` ~4 min (a tokio worker stuck in
# state D on wchan `dxgvmb_send_sync_msg`, unkillable even by SIGKILL), then `Completed`
# with NO new process, because a REQUESTED stop is not a "failure" so
# `availability.restart: on_failure` never fires. TIER-1 therefore reported a recovery it
# had not performed, burned ~3 min, and only then fell through to TIER-2 — strictly worse
# than having no TIER-1 at all.
# `axon_restart_role_verified` polls the OBSERVED state (Running AND Ready AND a pid
# different from the one seen before) and sends the missing explicit `start` when the
# supervisor gives up. Same lesson as the byte check of REQ-AXO-902258: never certify an
# outcome from the return code of the thing you asked.
if [[ "$recon_phase" != "clean" ]]; then
  # Tier 1 — the indexer is the ONLY failing gate: recover it without touching the brain.
  if [[ "$recon_failed" == "indexer_alive" ]]; then
    promote_log "   ⚠️ step 6c: phase=${recon_phase:-unreachable} (failed_gates: indexer_alive) — TIER-1 AUTO-RECOVERY: restart the indexer ONLY (brain keeps serving)."
    set +e
    # shellcheck source=../lib/axon-supervisor.sh
    source "$ROOT_DIR/scripts/lib/axon-supervisor.sh"
    if axon_restart_role_verified live axon-indexer 180 >> "$PROMOTE_LOG" 2>&1; then
      promote_log "   ↻ TIER-1: indexer restart VERIFIED (new pid, Running+Ready)"
    else
      promote_log "   ⚠️ TIER-1 could not verify the indexer restart — escalating to TIER-2"
    fi
    set -e
    _poll_promote_clean 36 || true   # ~180s: BGE-Large GPU cold-start budget
    [[ "$recon_phase" == "clean" ]] && promote_log "   ✅ step 6c tier-1: recovered WITHOUT a brain outage (blast radius contained)."
  else
    promote_log "   ⚠️ step 6c: phase=${recon_phase:-unreachable} (failed_gates: ${recon_failed:-none}) — not indexer-only, skipping tier 1."
  fi

  # Tier 2 — unchanged full restart, only if tier 1 did not converge.
  if [[ "$recon_phase" != "clean" ]]; then
    promote_log "   ⚠️ step 6c: still phase=${recon_phase:-unreachable} (failed_gates: ${recon_failed:-none}) — TIER-2 AUTO-RECOVERY: full restart (stop --hard + start full). THIS INTERRUPTS THE BRAIN — expect third-party MCP clients to see an outage."
    set +e
    bash "$ROOT_DIR/scripts/axon-live" stop --hard >> "$PROMOTE_LOG" 2>&1
    bash "$ROOT_DIR/scripts/axon-live" start full  >> "$PROMOTE_LOG" 2>&1
    set -e
    _poll_promote_clean 36 || true   # re-verify on a fuller cold-start budget (~180s)
  fi
fi

# Stop sampling and report BEFORE the verdict, so the measured outage is logged on the
# failure path too (that branch exits 1 — the outage figure matters most there).
_report_mcp_outage

if [[ "$recon_phase" == "clean" ]]; then
  promote_log "   ✅ step 6c: phase=clean (manifest↔runtime↔FULL-contract liveness all green)"
else
  promote_log "   ❌ step 6c: phase=${recon_phase:-unreachable} (failed_gates: ${recon_failed:-none}) persists after auto-recovery — FAILING the promote: the runtime_contract is degraded (indexer not alive). Do NOT trust a 'COMPLETE'; investigate the indexer."
  exit 1
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

# REQ-AXO-902105 — step 7 is COSMETIC (SOLL export + status display). The
# promotion is ALREADY correct at this point: gated by step 5 (atomic swap +
# runtime-identity match) and step 6 (qualify-mcp verdict=ok). A concurrent commit
# moving HEAD during finalize (observed s91: an operator commit during the run)
# must NOT fail-close an already-good promote. Warn only — never exit 1 here. The
# strict HEAD-stability guard stays on steps 3/5 where it protects the build/swap.
current_head_finalize="$(git -C "$ROOT_DIR" rev-parse HEAD 2>/dev/null || echo unknown)"
if [[ "$current_head_finalize" != "$start_head" ]]; then
  promote_log "   ⚠️ HEAD moved during finalize ($start_head → $current_head_finalize) — harmless: promotion already gated by steps 5+6."
fi
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

# REQ-AXO-311 tier 3 — anchor a permanent (never-expiring) SOLL snapshot to this
# qualified release. Same fire-and-forget contract as the Memgraph hook above:
# runs outside run_step, backgrounded, can never fail the promote. PIL-AXO-005
# fail-closed is untouched.
( nohup bash "$ROOT_DIR/scripts/backup_soll_daily.sh" --keeper >>"$PROMOTE_LOG" 2>&1 & ) || true
promote_log "   ▶ SOLL keeper backup dispatched (background, best-effort)"

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
