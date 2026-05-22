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
  0. Validate dev instance healthy (`feedback_dev_first_no_exception`)
  1. Build canonical release artifact
  2. Run release preflight
  3. Create qualified release manifest
  4. Promote live with restart and MCP runtime post-check
  5. Run core MCP qualification and final live status

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

start_head="$(git -C "$ROOT_DIR" rev-parse HEAD)"

ensure_head_stable() {
  local current_head
  current_head="$(git -C "$ROOT_DIR" rev-parse HEAD)"
  if [[ "$current_head" != "$start_head" ]]; then
    echo "HEAD changed during promotion flow: start=$start_head current=$current_head" >&2
    return 1
  fi
}

run_step() {
  local label="$1"
  shift
  echo "== $label =="
  "$@"
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
    echo "     ./scripts/axon-dev start --brain-only        # or --indexer-full" >&2
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
  local candidate_head="$(git -C "$ROOT_DIR" rev-parse HEAD)"
  local dev_build_id
  dev_build_id=$(curl -fsS --max-time 5 -X POST "http://127.0.0.1:${dev_mcp_port}/mcp" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"tools/call","params":{"name":"status","arguments":{"mode":"verbose"}},"id":1}' 2>&1 \
    | grep -oE '"build_id":"[^"]+"' | head -1 | sed 's/"build_id":"//;s/"$//' || true)

  if [[ -z "$dev_build_id" ]]; then
    # Brain `status` may not surface build_id in JSON output — fall back
    # to a soft warning rather than block (don't penalize an environment
    # where introspection isn't wired). Operator can still override via
    # --skip-dev-validation if they accept the risk.
    echo "  ⚠️ could not extract dev build_id from MCP status ; binary-match check skipped"
    return 0
  fi

  # Match : dev build_id must contain the short HEAD sha. Format ex :
  # v0.8.0-629-gd0d7a43f → contains `d0d7a43f`.
  local short_head="${candidate_head:0:8}"
  if [[ "$dev_build_id" == *"$short_head"* ]]; then
    echo "  ✅ dev brain runs candidate binary (build_id=$dev_build_id matches HEAD $short_head)"
  else
    echo "❌ Dev brain runs a DIFFERENT binary than the promotion candidate." >&2
    echo "   dev build_id      : $dev_build_id" >&2
    echo "   candidate HEAD    : $candidate_head ($short_head)" >&2
    echo "   You are about to promote untested code to live." >&2
    echo "" >&2
    echo "   Recovery:" >&2
    echo "     # 1. Rebuild dev with current HEAD" >&2
    echo "     ./scripts/axon-dev stop --hard" >&2
    echo "     ./scripts/axon-dev start --indexer-full" >&2
    echo "     # 2. Functional test in dev (e.g. create file, query MCP)" >&2
    echo "     # 3. Re-run this command" >&2
    echo "" >&2
    echo "   Bypass (EMERGENCY ONLY) :" >&2
    echo "     bash scripts/release/promote_live_safe.sh --skip-dev-validation ..." >&2
    return 1
  fi
}

if [[ "$SKIP_DEV_VALIDATION" -eq 1 ]]; then
  echo "== dev validation =="
  echo "  ⚠️ BYPASSED via --skip-dev-validation (violation of feedback_dev_first_no_exception)"
else
  run_step "dev validation gate (feedback_dev_first_no_exception)" validate_dev_healthy
fi

if [[ "$SKIP_BUILD" -ne 1 ]]; then
  run_step "build canonical release artifact" "$ROOT_DIR/scripts/axon" setup --artifact-only
fi

ensure_head_stable
run_step "release preflight" "$ROOT_DIR/scripts/axon" release-preflight
ensure_head_stable

manifest_path="$(run_step "create qualified release manifest" "$ROOT_DIR/scripts/axon" create-release-manifest --state qualified | tail -n 1)"
[[ -n "$manifest_path" ]] || { echo "Failed to capture manifest path" >&2; exit 1; }
manifest_path="$(realpath "$manifest_path")"

ensure_head_stable
run_step "promote live and verify runtime truth" "$ROOT_DIR/scripts/axon" promote-live --manifest "$manifest_path" --restart-live

if [[ "$SKIP_QUALIFY" -ne 1 ]]; then
  ensure_head_stable
  run_step "qualify live MCP core surface" "$ROOT_DIR/scripts/axon" --instance live qualify-mcp --surface core --checks quality,latency --project "$PROJECT_CODE"
fi

# REQ-AXO-126 — snapshot the SOLL graph at the moment of promotion so
# the artifact is part of the qualified-release lineage (PIL-AXO-005).
# Best-effort: if the export call fails, log a warning but do not roll
# back the promotion — the manifest is the authoritative artifact.
ensure_head_stable
echo "== snapshot SOLL for release lineage =="
soll_export_args=$(printf '{"project_code":"%s"}' "$PROJECT_CODE")
if ! "$ROOT_DIR/scripts/axon" --instance live mcp-call call soll_export --args "$soll_export_args" --format text; then
  echo "WARN: soll_export call failed; promotion is still complete (manifest is authoritative)" >&2
fi

ensure_head_stable
run_step "final live status" bash "$ROOT_DIR/scripts/axon-live" status

echo "SAFE PROMOTION COMPLETE"
echo "manifest=$manifest_path"
