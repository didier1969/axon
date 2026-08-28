#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$ROOT_DIR/scripts/lib/axon-instance.sh"
# shellcheck source=scripts/lib/axon-gpu-detect.sh
# REQ-AXO-902285 — `gpu_wedged_pids` for the fail-fast GPU-wedge gate below.
source "$ROOT_DIR/scripts/lib/axon-gpu-detect.sh"
# REQ-AXO-902350 — THE canonical PG port. Sourced rather than defaulted inline: an
# inline `:-44144` is exactly the duplicated literal that let the 2026-08-20 drift
# survive across reboots.
# shellcheck source=scripts/lib/axon-pg-port.sh
source "$ROOT_DIR/scripts/lib/axon-pg-port.sh"
# shellcheck source=scripts/lib/axon-promote-lease.sh
# REQ-AXO-902526 — one kernel-owned live promotion at a time, with a durable
# attempt journal that survives SIGKILL and makes recovery distinguishable.
source "$ROOT_DIR/scripts/lib/axon-promote-lease.sh"
# shellcheck source=scripts/lib/axon-time.sh
source "$ROOT_DIR/scripts/lib/axon-time.sh"
AXON_INSTANCE_KIND=live
axon_resolve_instance "$ROOT_DIR" "$(basename "$ROOT_DIR")"

PROJECT_CODE="AXO"
SKIP_BUILD=0
SKIP_QUALIFY=0
DRY_RUN=0
SKIP_DEV_VALIDATION=0
# REQ-AXO-902391 — 0 = build from a FROZEN worktree at the resolved SHA.
BUILD_FROM_TREE=0
BREAK_GLASS_REASON=""
BREAK_GLASS_ACTOR=""
# REQ-AXO-902543 — a cold, optimized build of the monolithic Rust crate can
# legitimately exceed the former hard-coded 20-minute budget. Keep a bounded
# deadline, but make it operator-configurable and long enough to converge on a
# cold worktree. The effective value is recorded by run_step's journal event.
PROMOTE_LIVE_BUILD_TIMEOUT_S="${PROMOTE_LIVE_BUILD_TIMEOUT_S:-3600}"
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

Le build (étape 1) part d'un WORKTREE DÉTACHÉ au SHA de HEAD, jamais de l'arbre
de travail : `cargo` lit les sources au fil de la compilation, donc une écriture
concurrente produirait un binaire composite sous une étiquette qu'il ne mérite
pas (REQ-AXO-902391). Un SHA non poussé est refusé — un binaire live doit
correspondre à un commit qu'un tiers peut retrouver.

Flags:
  --dirty                Construit depuis l'arbre de travail (dev uniquement).
                         Le binaire cesse d'être une fonction du SHA seul et
                         `git describe` le marque `-dirty`. JAMAIS le défaut.
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

Environment:
  PROMOTE_LIVE_BUILD_TIMEOUT_S
                         Build-step deadline in seconds (default: 3600, minimum: 60).
                         This changes only the deadline; it never skips the build.

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
    --break-glass-reason) BREAK_GLASS_REASON="${2:-}"; shift 2 ;;
    --break-glass-actor) BREAK_GLASS_ACTOR="${2:-}"; shift 2 ;;
    # REQ-AXO-902391 — build from the WORKING TREE instead of a frozen worktree.
    # Never the default: the whole point is that the binary's identity must be a
    # function of the SHA alone.
    --dirty) BUILD_FROM_TREE=1; shift ;;
    # REQ-AXO-902256 — no-op: the cutover is the only step-5 path now.
    --cutover) shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage; exit 1 ;;
  esac
done

[[ -n "$PROJECT_CODE" ]] || { echo "--project is required" >&2; exit 1; }
if [[ ! "$PROMOTE_LIVE_BUILD_TIMEOUT_S" =~ ^[0-9]+$ ]] || \
   (( PROMOTE_LIVE_BUILD_TIMEOUT_S < 60 )); then
  echo "❌ PROMOTE_LIVE_BUILD_TIMEOUT_S must be an integer >= 60 seconds (got: ${PROMOTE_LIVE_BUILD_TIMEOUT_S@Q})" >&2
  exit 2
fi

# --- REQ-AXO-902526: exclusive lease BEFORE build/install/stage -----------------------
#
# `flock` is authoritative: process death releases it in the kernel. The adjacent owner
# JSON is diagnostic and deliberately survives SIGKILL, so the next lease holder can
# record an interrupted attempt and reconcile pending/current/runtime before proceeding.
# A contender returns 75 before it creates a log/journal or touches release artifacts.
PROMOTE_TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
LOG_DIR="$ROOT_DIR/.axon/live-release"
start_head="$(git -C "$ROOT_DIR" rev-parse HEAD)"
RELEASE_ATTEMPT_ID="${PROMOTE_TIMESTAMP}-$$-${start_head:0:12}"
export AXON_RELEASE_ATTEMPT_ID="$RELEASE_ATTEMPT_ID"
lease_rc=0
axon_promote_lease_acquire \
  "$LOG_DIR" "$AXON_INSTANCE_KIND" "$PROJECT_CODE" "$start_head" \
  "$RELEASE_ATTEMPT_ID" "${PROMOTE_LEASE_TTL_SECONDS:-14400}" || lease_rc=$?
if [[ "$lease_rc" -ne 0 ]]; then
  exit "$lease_rc"
fi

# --- REQ-AXO-901758: logging + step tracking + error trap ---
PROMOTE_LOG="$LOG_DIR/promote-${RELEASE_ATTEMPT_ID}.log"

CURRENT_STEP=0
CURRENT_STEP_NAME="init"

promote_log() {
  local ts
  ts="$(date -u +%H:%M:%S)"
  echo "[$ts] $*" >> "$PROMOTE_LOG"
  echo "$*"
}

promotion_gate_summary() {
  python3 - "$LOG_DIR/pending.json" "$LOG_DIR/current.json" <<'PY' 2>/dev/null || printf 'unavailable'
import json, pathlib, sys
for raw in sys.argv[1:]:
    path = pathlib.Path(raw)
    if path.exists():
        data = json.loads(path.read_text())
        gates = data.get("promotion_gates", {})
        print(json.dumps(gates, sort_keys=True, separators=(",", ":")))
        break
else:
    print("{}")
PY
}

historical_promote_estimate() {
  python3 - "$LOG_DIR/attempts" <<'PY' 2>/dev/null || printf 'historique indisponible'
import json, pathlib, statistics, sys
durations=[]
for path in sorted(pathlib.Path(sys.argv[1]).glob("*.jsonl"))[-20:]:
    try:
        rows=[json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    except Exception:
        continue
    if not rows or not any(r.get("event")=="lease_released" and r.get("status")=="completed" for r in rows):
        continue
    start=rows[0].get("monotonic_ms")
    cut=next((r.get("monotonic_ms") for r in rows if r.get("event")=="step_started" and r.get("phase") in {"cutover","cutover_prepare"}), None)
    if isinstance(start, int) and isinstance(cut, int) and cut >= start:
        durations.append((cut-start)//1000)
if durations:
    print(f"médiane historique jusqu'au cutover={int(statistics.median(durations))}s sur {len(durations)} tentative(s) réussie(s)")
else:
    print("historique insuffisant; aucune durée promise")
PY
}

# REQ-AXO-902285 — refuse the promote FAIL-FAST (0s of MCP outage) when the WSL2 GPU
# virtualisation channel is WEDGED. A cutover's stop_instance tears the indexer's
# TensorRT/CUDA session down through the dxg channel; if a process is already stuck in
# uninterruptible D-state on it, the NEW indexer hangs the same way, the health-gate never
# goes green, and axonctl auto-rolls-back — paying a full MCP outage (measured 104s pic /
# 209s on 2026-08-09) for ZERO gain. Detect the wedge with a pure `ps` scan (never touches
# the GPU, so the check itself cannot wedge) and refuse before we stop anything.
#
# CONFIRM-TWICE: a healthy live indexer doing continuous GPU work samples `D` transiently,
# so a single sample would refuse a perfectly promotable host. Only a pid stuck in D across
# a 1s gap is a genuine wedge. First sample empty → return immediately (0 added latency on a
# clean host).
require_gpu_channel_free() {
  local phase="$1" first second persistent p q
  # `|| true`: gpu_wedged_pids ends in a pipe; under `set -o pipefail` a stray ps failure
  # must never itself fail the promote — an unreadable process table is "not wedged".
  first="$(gpu_wedged_pids || true)"
  [[ -z "$first" ]] && return 0
  sleep 1
  second="$(gpu_wedged_pids || true)"
  persistent=""
  # Explicit `if` (not `[[ ]] && x`): under `set -e` a false `&&` list as the loop-body
  # statement can trip the errexit trap; the if-test form never does.
  for p in $first; do
    for q in $second; do
      if [[ "$p" == "$q" ]]; then persistent+="$p "; fi
    done
  done
  persistent="${persistent% }"
  [[ -z "$persistent" ]] && return 0
  promote_log "❌ PROMOTE REFUSED ($phase): GPU virtualisation channel WEDGED — pid(s) persistently in uninterruptible D-state (2 samples, 1s apart) touching the GPU: ${persistent}."
  promote_log "   Proceeding would strand the new indexer: it cannot finish a TensorRT teardown through a stuck dxg channel (REQ-AXO-902271), the health-gate fails ~200s later, and the cutover auto-rolls-back — a full MCP outage for nothing."
  promote_log "   Most frequent cause: agent-deck's Footer GPU widget shelling \`nvidia-smi\` every 5s. Cure: remove \"gpu\" from [system_stats].show in ~/.agent-deck/config.toml (hot-reloads)."
  promote_log "   Recover: wait for \`ps -eo stat | grep '^D'\` to clear (frees itself ~15 min) or \`wsl --shutdown\` (operator), then re-run this promote."
  exit 3
}

# --- REQ-AXO-902194: best-effort cross-project MCP-disruption broadcast ---
# The step-5 brain restart drops every connected LLM's MCP for a few seconds. A
# broadcast (to_project='*') leaves an explanatory trace so peers find "planned
# promote, not a crash" on reconnect instead of burning tokens on a false RCA.
# STRICTLY best-effort: a missing/slow mailbox must NEVER fail a promote.
# REQ-AXO-902327 — la source python vit dans une VARIABLE, remplie par un heredoc à
# délimiteur QUOTÉ (`<<'PY'`), et n'est plus écrite littéralement dans un `$( ... )`.
#
# Motif : à l'intérieur d'une substitution de commande, bash développe encore les
# backticks et les `$` — y compris entre guillemets doubles. Le commentaire python qui
# citait `mailbox_sweep()` entre backticks était donc lu comme une substitution de
# commande et EXÉCUTÉ à chaque promote. Il échouait sur `()`, d'où le
# `syntax error: unexpected end of file` en fin de chaque promote ; le JSON survivait
# parce qu'un commentaire amputé reste un commentaire. Le défaut n'était pas le message
# d'erreur : c'était qu'un COMMENTAIRE pilotait une exécution. Un jour où le texte cité
# aurait été une commande valide, elle aurait tourné.
#
# `<<'PY'` désactive toute expansion. `${PROJECT_CODE}` passe donc par argv, comme les
# trois autres valeurs — plus aucune interpolation shell dans du code python.
read -r -d '' _BROADCAST_PY <<'PY' || true
import json, sys
project, subject, body, key = sys.argv[1:5]
print(json.dumps({
    'to_project': '*', 'from': project,
    'subject': subject, 'body_dense': body,
    # REQ-AXO-902306 — 'low' et non 'high'. Ces avis sont URGENTS à l'instant où ils
    # arrivent, ils ne sont pas IMPORTANTS à conserver : deux notions que la priorité
    # confondait. Depuis 902306 un message 'high' n'est JAMAIS archivé automatiquement
    # (ni lecture ni TTL) ; les laisser en 'high' les rendrait immortels et recréerait
    # exactement l'accumulation que REQ-AXO-902304 vient de résorber (8217 messages).
    'idempotency_key': key, 'priority': 'low',
    # REQ-AXO-902304 — ces avis sont périssables : « coupure dans 3 minutes » n'a
    # aucune valeur le lendemain. Sans TTL ils s'empilaient à jamais (8217 messages
    # depuis juillet, 118 par projet, 100% de l'inbox pour quatre d'entre eux) alors
    # que `mailbox_sweep()` n'attendait que cette colonne pour les archiver.
    'ttl_hours': 24,
}))
PY

broadcast_promote() {
  local subject="$1" body="$2" key="$3" args hook_rc
  args="$(python3 -c "$_BROADCAST_PY" "$PROJECT_CODE" "$subject" "$body" "$key" 2>/dev/null || true)"
  if [[ -z "$args" ]]; then
    python3 "$ROOT_DIR/scripts/release/durable_hook.py" \
      --state-root "$LOG_DIR/hooks" --attempt-id "$RELEASE_ATTEMPT_ID" \
      --hook-name "broadcast-${key}" --defer-reason "broadcast arguments could not be encoded" || true
    return 0
  fi
  hook_rc=0
  python3 "$ROOT_DIR/scripts/release/durable_hook.py" \
    --state-root "$LOG_DIR/hooks" --attempt-id "$RELEASE_ATTEMPT_ID" \
    --hook-name "broadcast-${key}" --max-attempts 3 --timeout-seconds 20 \
    --retry-delay-seconds 1 -- \
    "$ROOT_DIR/scripts/axon" --instance live mcp-call call mcp_outbox_send \
    --args "$args" --format text >> "$PROMOTE_LOG" 2>&1 || hook_rc=$?
  axon_promote_journal_event hook_result notification \
    "$([[ "$hook_rc" -eq 0 ]] && printf completed || printf failed)" \
    "hook=broadcast-${key} exit_code=${hook_rc}" || true
  return 0
}

# REQ-AXO-902327 — ce bloc était DÉFINI ~400 lignes plus bas, alors que le trap EXIT
# ci-dessous l'APPELLE. Tout échec antérieur à l'ancienne définition sortait sur
# `_report_mcp_outage: command not found` — donc la mesure de coupure MCP que
# REQ-AXO-902256 rend obligatoire manquait précisément sur les échecs précoces.
# Vécu deux fois le 2026-08-15 : échec step 2d (avant la définition) → message perdu ;
# échec step 5b (après) → rapport correct. Définir AVANT le trap rend l'invariant
# structurel au lieu de positionnel.
# --- MCP availability sampler (REQ-AXO-902256) ---
# The promote used to report step 5 as "done in 35s" — a figure that measures the binary
# copy and EXCLUDES the indexer coming back and any step-6c recovery. On promote 1399 the
# real MCP outage was ~3m53s while the reported number was 35s, so the operator was told
# the interruption was negligible when third-party clients were self-restarting. Estimates
# are not good enough here: sample the endpoint other clients actually call, once a second,
# across steps 5→6c, then report the measured worst contiguous gap.
MCP_SAMPLE_FILE="$LOG_DIR/mcp-availability-${PROMOTE_TIMESTAMP}.csv"
MCP_SAMPLER_PID=""
PROMOTE_FROZEN_WORKTREE=""
CANDIDATE_BIN_DIR=""
CUTOVER_PREPARED=0

_cleanup_frozen_worktree() {
  local promote_rc="${1:-0}"
  [[ -n "$PROMOTE_FROZEN_WORKTREE" ]] || return 0
  # Un gate pré-cutover peut échouer pour un état runtime transitoire (indexer en
  # récupération, pression hôte) après un build de plusieurs minutes. Conserver ce
  # checkpoint ne qualifie rien : le retry rejoue tous les gates et le preflight de
  # contenu. Il évite seulement de transformer une protection fail-closed en nouvelle
  # tempête mémoire. Un succès nettoie toujours le worktree.
  if [[ "$promote_rc" -ne 0 ]] && \
     [[ "$(git -C "$PROMOTE_FROZEN_WORKTREE" rev-parse HEAD 2>/dev/null || true)" == "$PROMOTE_SHA" ]] && \
     git -C "$PROMOTE_FROZEN_WORKTREE" diff --quiet HEAD -- && \
     git -C "$PROMOTE_FROZEN_WORKTREE" diff --cached --quiet HEAD --; then
    promote_log "   ↻ checkpoint figé conservé pour retry: $PROMOTE_FROZEN_WORKTREE"
    PROMOTE_FROZEN_WORKTREE=""
    return 0
  fi
  git -C "$ROOT_DIR" worktree remove --force "$PROMOTE_FROZEN_WORKTREE" >/dev/null 2>&1 || true
  PROMOTE_FROZEN_WORKTREE=""
}

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
      local_body="$LOG_DIR/.mcp-sample-body.$$"
      curl_meta="$(curl -sS -m 2 -o "$local_body" -w '%{http_code}|%{exitcode}' \
        "http://127.0.0.1:${AXON_BRAIN_PORT:-44129}/mcp" \
        -H 'content-type: application/json' \
        -d '{"jsonrpc":"2.0","method":"tools/list","id":1}' 2>/dev/null)" || true
      code="${curl_meta%%|*}"; curl_rc="${curl_meta##*|}"
      state="up"; diagnosis="ready"
      if [[ "$curl_rc" == "7" ]]; then state="down"; diagnosis="connection_refused"
      elif [[ "$curl_rc" == "28" || -z "$curl_meta" ]]; then state="down"; diagnosis="timeout"
      elif [[ "$code" == 5* ]]; then state="down"; diagnosis="http_5xx"
      elif ! python3 -m json.tool "$local_body" >/dev/null 2>&1; then state="down"; diagnosis="invalid_json"
      elif ! python3 - "$local_body" <<'PY' >/dev/null 2>&1
import json, sys
d=json.load(open(sys.argv[1], encoding="utf-8"))
assert isinstance(d.get("result",{}).get("tools"), list)
PY
      then state="down"; diagnosis="mcp_nonready"
      elif ! grep -q '"promote_status"' "$local_body"; then state="down"; diagnosis="functional_failure"
      fi
      printf '%s,%s,%s\n' "$(date -u +%s)" "$state" "$diagnosis"
      rm -f "$local_body"
      sleep 1
    done
  ) >> "$MCP_SAMPLE_FILE" 2>/dev/null &
  MCP_SAMPLER_PID=$!
}
_report_mcp_outage() {
  [[ -n "$MCP_SAMPLER_PID" ]] || return 0
  kill "$MCP_SAMPLER_PID" 2>/dev/null || true
  wait "$MCP_SAMPLER_PID" 2>/dev/null || true
  MCP_SAMPLER_PID=""
  local worst total n span res
  read -r worst total n span res < <(python3 "$ROOT_DIR/scripts/release/mcp_outage_report.py" "$MCP_SAMPLE_FILE" 2>/dev/null || echo "0 0 0 0 0")
  promote_log ""
  promote_log "== MCP availability (measured across steps 5→6c) =="
  # A silent instrument reads exactly like a green result: publish the sample count and the
  # covered span so "0 outage" can never be confused with "measured nothing".
  if [[ "$n" -lt 5 ]]; then
    promote_log "   ⚠️ NOT MEASURED — only ${n} sample(s) collected. Treat the figures below as UNKNOWN, not as zero."
  fi
  promote_log "   worst contiguous outage: ${worst}s · total unreachable: ${total}s"
  # The resolution is MEASURED from the samples, not asserted. It is the number that says
  # how finely this instrument may be quoted — a sub-second claim from a ~3s instrument is
  # not a measurement.
  promote_log "   ${n} samples over ${span}s · measured resolution ${res}s (a blip shorter than that can fall between two samples)"
  promote_log "   samples: $MCP_SAMPLE_FILE"
  promote_log "   classifications: $(cut -d, -f3 "$MCP_SAMPLE_FILE" | sort | uniq -c | tr '\n' ';' || true)"
  if [[ "${worst:-0}" -gt 60 ]]; then
    promote_log "   ⚠️ outage > 60s — third-party MCP clients may have declared a crash and self-restarted (REQ-AXO-902256 acceptance breach)."
  fi
}

# All-clear on ANY exit (success OR the step-6 cold-start false-fail where the script
# exits non-zero but the brain IS back). Fires only if (a) the pre-notice went out and
# (b) the live brain answers /readyz — so we never claim "back" while it is still down.
BROADCAST_PREFLIGHT_SENT=0
on_promote_exit() {
  local rc=$?
  # Cleanup must run even if an observability/broadcast helper fails while the process is
  # already exiting. The kernel is the final safety net; this path closes the journal and
  # removes the human-readable owner record on every trappable exit.
  set +e
  if [[ "$rc" -ne 0 && "$CUTOVER_PREPARED" -eq 1 ]]; then
    promote_log "   promotion_gates=$(promotion_gate_summary)"
    promote_log "   ↩ prepared transaction failed after activation; invoking LKG rollback"
    "$ROOT_DIR/bin/axonctl" cutover --project-root "$ROOT_DIR" --instance-kind live \
      --phase rollback --max-polls 120 --poll-interval-ms 2000 --json >> "$PROMOTE_LOG" 2>&1
    rollback_rc=$?
    axon_promote_journal_event rollback_result rollback \
      "$([[ "$rollback_rc" -eq 0 ]] && printf completed || printf rollback_failed)" \
      "exit_code=${rollback_rc}" || true
  fi
  # REQ-AXO-902233 — report the measured outage on EVERY exit, including the failure path.
  # `run_step` calls `exit "$rc"` when a step fails, which skipped the nominal call to
  # `_report_mcp_outage` at the end of the script — so the availability number was missing
  # exactly on the runs where it matters most, and the sampler subshell was left running.
  # It is a no-op when the sampler never started or has already been reported (the function
  # clears MCP_SAMPLER_PID).
  _report_mcp_outage || true
  if [[ "$BROADCAST_PREFLIGHT_SENT" -eq 1 ]] && \
     curl -fsS --max-time 5 "http://127.0.0.1:44129/readyz" >/dev/null 2>&1; then
    if [[ "$rc" -eq 0 ]]; then
      broadcast_promote "✅ Promote ${PROJECT_CODE} terminé — MCP rétabli" \
        "build_id=${final_build_id:-?} live. Si ton MCP est tombé depuis ${PROMOTE_TIMESTAMP} c'était CE promote (restart brain), pas un incident. Reconnecte via /mcp si ton binding de catalogue est stale. Tout est de nouveau disponible." \
        "promote-clear-${PROMOTE_TIMESTAMP}"
    else
      broadcast_promote "⚠️ Promote ${PROJECT_CODE} sorti (rc=${rc}) — brain UP" \
        "Le brain live RÉPOND (/readyz ok). Si ton MCP est tombé c'était le restart de CE promote (${PROMOTE_TIMESTAMP}), PAS un incident à diagnostiquer. Reconnecte via /mcp. (Le promote a pu false-fail au qualify cold-start ; l'opérateur AXO vérifie.)" \
        "promote-clear-${PROMOTE_TIMESTAMP}"
    fi
    # REQ-AXO-902304 — celui qui pollue nettoie. Le TTL ne sert à rien sans balayage
    # périodique, et `mailbox_sweep` était documenté « on demand (operator/cron) »
    # sans qu'aucun cron ne l'appelle : d'où 8217 avis accumulés. Le promote est le
    # producteur de ces messages, c'est donc le bon endroit pour les faire expirer.
    # Best-effort strict : un balayage qui échoue ne doit JAMAIS peser sur un promote.
    timeout 20 "$ROOT_DIR/scripts/axon" --instance live mcp-call call mailbox_sweep \
      --args '{}' --format text >> "$PROMOTE_LOG" 2>&1 || true
  fi
  _cleanup_frozen_worktree "$rc"
  if [[ "$rc" -eq 0 ]]; then
    axon_promote_lease_release completed "promotion process exited with rc=0"
  else
    axon_promote_lease_release failed "promotion process exited with rc=${rc}; reconcile before retry"
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
  local timeout_seconds deadline_monotonic_ms
  timeout_seconds="$(step_timeout_seconds "$step_name")"
  deadline_monotonic_ms="$(axon_deadline_after_seconds "$timeout_seconds")"
  axon_promote_journal_event step_started "$step_name" running \
    "step=${step_num} timeout_seconds=${timeout_seconds} deadline_monotonic_ms=${deadline_monotonic_ms}"
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
  "$@" > >(tee -a "$PROMOTE_LOG") 2>&1 &
  local command_pid=$! timed_out=0 now_monotonic_ms rc
  while kill -0 "$command_pid" 2>/dev/null; do
    now_monotonic_ms="$(axon_monotonic_ms)"
    if (( now_monotonic_ms >= deadline_monotonic_ms )); then
      timed_out=1
      promote_log "   deadline_exceeded step=${step_num} name=${step_name} timeout_seconds=${timeout_seconds}"
      pkill -TERM -P "$command_pid" 2>/dev/null || true
      kill -TERM "$command_pid" 2>/dev/null || true
      break
    fi
    sleep 1
  done
  wait "$command_pid"
  rc=$?
  [[ "$timed_out" -eq 1 ]] && rc=124
  set -e
  if [[ "$rc" -ne 0 ]]; then
    axon_promote_journal_event step_failed "$step_name" failed \
      "step=${step_num} exit_code=${rc}" || true
    promote_log "   step ${step_num} (${step_name}) returned exit code ${rc} after $((SECONDS - _step_t0))s"
    promote_log ""
    promote_log "❌ PROMOTE FAILED at step ${step_num}: ${step_name}"
    promote_log "   Exit code: ${rc}"
    promote_log "   Log: ${PROMOTE_LOG}"
    echo "" >&2
    echo "❌ PROMOTE FAILED at step ${step_num}: ${step_name} — see ${PROMOTE_LOG}" >&2
    exit "$rc"
  fi
  axon_promote_journal_event step_completed "$step_name" passed \
    "step=${step_num} elapsed_seconds=$((SECONDS - _step_t0))"
  promote_log "   ✅ step ${step_num} (${step_name}) done in $((SECONDS - _step_t0))s"
}

step_timeout_seconds() {
  case "$1" in
    build) echo "$PROMOTE_LIVE_BUILD_TIMEOUT_S" ;; dev_restart) echo 480 ;; test_targets_compile) echo 600 ;;
    lifecycle_gate) echo 240 ;; cutover_prepare) echo 420 ;;
    qualify_mcp|qualify_indexer_truth) echo 240 ;;
    preflight|candidate_recheck|manifest|apply_ddl_live) echo 180 ;;
    cutover_finalize) echo 60 ;; *) echo 300 ;;
  esac
}

promote_log "promote_live_safe.sh started at ${PROMOTE_TIMESTAMP}"
promote_log "project=${PROJECT_CODE} head=${start_head} skip_build=${SKIP_BUILD} skip_qualify=${SKIP_QUALIFY} skip_dev=${SKIP_DEV_VALIDATION}"
promote_log "release_attempt_id=${RELEASE_ATTEMPT_ID} lease_deadline_unix_ms=${AXON_PROMOTE_DEADLINE_UNIX_MS} journal=${AXON_PROMOTE_JOURNAL_PATH}"
axon_promote_journal_event script_started preflight running \
  "log=${PROMOTE_LOG}; release state reconciliation begins before mutation"

# Canonical promotion never turns a bypass into a qualified release. Emergency recovery
# is a separate operator workflow; here we only audit the request and refuse before build.
if [[ "$SKIP_QUALIFY" -eq 1 || "$SKIP_DEV_VALIDATION" -eq 1 ]]; then
  if [[ -z "$BREAK_GLASS_REASON" || -z "$BREAK_GLASS_ACTOR" ]]; then
    axon_promote_journal_event break_glass_refused preflight failed \
      "skip requested without --break-glass-reason and --break-glass-actor"
    echo "❌ skip flags require --break-glass-reason and --break-glass-actor; canonical promotion remains unqualified" >&2
    exit 78
  fi
  axon_promote_journal_event break_glass_refused preflight failed \
    "actor=${BREAK_GLASS_ACTOR}; reason=${BREAK_GLASS_REASON}; canonical promote cannot qualify bypassed gates"
  echo "❌ break-glass request audited but refused by canonical promotion; use the explicit recovery workflow" >&2
  exit 78
fi

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
    echo "❌ could not extract .result.data.runtime_version.build_id from dev status" >&2
    echo "   Candidate identity is unproven; refusing before any live mutation." >&2
    return 1
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
  pending_build="$(jq -r '.runtime_version.build_id // empty' "$pending_manifest" 2>/dev/null || true)"
  pending_state="$(jq -r '.state // "unknown"' "$pending_manifest" 2>/dev/null || true)"
  promote_log "⚠️ Unfinalized transaction detected (state=${pending_state}, build_id=${pending_build:-?}) — fail-safe rollback before any fresh promote."
  # Never auto-finalize a stranded candidate: after `prepare`, the process may have died
  # before DDL/core/indexer gates. Replaying the legacy full cutover would erase that fact.
  "$ROOT_DIR/bin/axonctl" cutover --project-root "$ROOT_DIR" --instance-kind live \
    --phase rollback --max-polls 120 --poll-interval-ms 2000 --json >> "$PROMOTE_LOG" 2>&1
  resume_rc=$?
  promote_log "   stranded transaction rollback exit=$resume_rc; rerun to start a fresh attempt"
  exit "$resume_rc"
fi

# REQ-AXO-902285 — fresh-promote fail-fast gate. Placed AFTER the auto-resume block (so a
# wedged host never BLOCKS a recovery: the stranded resume-cutover above runs and fails at
# its own health-gate) and BEFORE the pre-notice broadcast (so a refusal here sends no
# "MCP will drop" message that would then need an all-clear). A wedge appearing LATER, during
# the ~9-min build, is caught by the second gate just before step 5.
require_gpu_channel_free "pre-flight (fresh promote)"

# --- REQ-AXO-902194: pre-notice (brain still up) — warn peers the step-5 restart
# will drop MCP briefly. Async, so mostly read on reconnect; harmless to send early. ---
broadcast_promote "🔧 Promote ${PROJECT_CODE} en cours — coupure MCP brève à venir" \
  "Un promote AXO démarre (${PROMOTE_TIMESTAMP}); $(historical_promote_estimate). Au restart du brain le MCP tombera pour TOUS les clients connectés. La coupure est mesurée en direct et chaque phase possède une deadline monotone. C'est PLANIFIÉ: NE relance PAS le serveur toi-même, ton self-heal entrerait en course avec la bascule. Attends l'all-clear, qui suit dès que le brain répond." \
  "promote-notice-${PROMOTE_TIMESTAMP}"
BROADCAST_PREFLIGHT_SENT=1

# --- Step 1: build ---
# REQ-AXO-901763 — Build BEFORE dev-gate so the dev brain can be restarted
# with the candidate binary. The previous ordering (dev_gate -> build) meant
# the dev brain always ran a binary compiled pre-commit whose build_id
# (git describe) pointed to HEAD^ instead of HEAD. The promote then failed
# because build_id != HEAD.
# REQ-AXO-902391 — construire depuis un arbre FIGÉ, pas depuis l'arbre de travail.
#
# `cargo` lit les sources AU FIL de la compilation. Un build de ~140 s prend donc
# le disque tel qu'il est à chaque lecture de fichier, pas tel qu'il était au
# départ : une écriture concurrente produit un binaire composite, étiqueté d'un
# SHA qu'il ne contient pas. Ce n'est pas un risque d'usage, c'est une propriété
# du script — et elle contredit PIL-AXO-005 (« Diff(empreinte du binaire tournant
# vs artefact du manifeste) = 0 ») autant que GUI-PRO-006 (builds déterministes).
#
# Le 2026-08-20, trois promotes ont été interrompus par écriture concurrente dans
# la même soirée, dont un après un gel annoncé des DEUX côtés : deux agents
# compétents qui se préviennent par messages n'ont pas tenu une fenêtre de 140 s.
# C'est la démonstration de GUI-PRO-118 — une propriété mécanique ne peut pas
# reposer sur une discipline. Le contournement manuel (worktree détaché) était
# déjà validé par VPC ; le câbler ici le rend systématique.
#
# Le worktree déplace les sources ET la cible de compilation, et c'est voulu.
#
# Ce commentaire affirmait le contraire — « `CARGO_TARGET_DIR` reste la cible
# canonique » — et le code posait effectivement cette variable. Elle n'a jamais
# redirigé le build : cargo tourne dans `devenv shell`, où `devenv.nix` réassigne
# `CARGO_TARGET_DIR = DEVENV_ROOT/.axon/cargo-target`, donc le target DU WORKTREE.
# Elle ne redirigeait que l'INSTALLATION, restée hors du shell — d'où un promote qui
# compile ici et publie le binaire de là. Le 2026-08-23 : 8 min 24 de compilation
# jetées, l'ancien binaire servi à 75 tenants sous la nouvelle étiquette, quatre
# gardes vertes (REQ-AXO-902464).
#
# Désormais : une seule cible, celle du worktree, et `setup.sh` installe depuis la
# cible que le build lui RAPPORTE. `AXON_BUILD_ID` est posé pour que `build.rs`
# grave le SHA promu dans le binaire — c'est ce que `preflight.sh` vérifie ensuite
# en lisant le CONTENU, seul contrôle qui ne soit pas auto-référentiel.
build_from_frozen_worktree() {
  local sha="$1"
  # Le target Cargo d'un candidat release+tests dépasse 20 Gio. Sur cette machine,
  # /tmp est un tmpfs de 31 Gio : y placer le worktree transforme les artefacts de
  # compilation en mémoire engagée et finit en ENOSPC/OOM pendant `cargo build
  # --tests`. Les checkpoints de promotion sont donc disk-backed sous .axon.
  PROMOTE_FROZEN_WORKTREE="$ROOT_DIR/.axon/promote-worktrees/${sha:0:12}"
  local worktree="$PROMOTE_FROZEN_WORKTREE"

  # Un SIGKILL/OOM ne peut exécuter aucun trap. S'il survient après le build, le
  # worktree figé et son target Cargo constituent un checkpoint coûteux mais sûr à
  # réutiliser. On ne le reprend que si Git prouve l'identité exacte du SHA ET
  # l'absence de toute modification de source; le preflight de contenu recertifie
  # ensuite chaque binaire. Toute ambiguïté retombe sur une création neuve.
  local reuse_checkpoint=0
  if [[ -d "$worktree/.git" || -f "$worktree/.git" ]]; then
    if [[ "$(git -C "$worktree" rev-parse HEAD 2>/dev/null || true)" == "$sha" ]] && \
       git -C "$worktree" diff --quiet HEAD -- && \
       git -C "$worktree" diff --cached --quiet HEAD -- && \
       [[ -z "$(git -C "$worktree" status --porcelain --untracked-files=normal 2>/dev/null)" ]]; then
      reuse_checkpoint=1
    fi
  fi
  if [[ "$reuse_checkpoint" -eq 1 ]]; then
    echo "  ↻ reprise du checkpoint figé $worktree (SHA exact, sources propres)"
  else
    git -C "$ROOT_DIR" worktree remove --force "$worktree" >/dev/null 2>&1 || true
    git -C "$ROOT_DIR" worktree add --detach "$worktree" "$sha" >/dev/null
  fi
  # Retained through DEV validation and test-target compilation; the process EXIT trap
  # removes it on success and every trappable failure.

  # REQ-AXO-902543 — sources are isolated per SHA, compiler cache is not. A
  # per-worktree target forced every promote to rebuild ~20 GiB of unchanged
  # dependencies and made the optimized monolithic axon-core exceed even a
  # one-hour deadline. The promotion lease serializes writers to this dedicated
  # cache; Cargo fingerprints still invalidate changed sources, Cargo.lock,
  # toolchain flags, build.rs inputs, and AXON_BUILD_ID. setup.sh continues to
  # install through the exact target path reported by this build, and release
  # preflight still verifies the identity embedded in every candidate binary.
  local shared_cargo_target="$ROOT_DIR/.axon/promote-cargo-target"
  local worktree_cargo_target="$worktree/.axon/cargo-target"
  mkdir -p "$worktree/.axon"
  if [[ -d "$worktree_cargo_target" && ! -L "$worktree_cargo_target" ]]; then
    if [[ ! -e "$shared_cargo_target" ]]; then
      # One-time migration of a checkpoint created before REQ-AXO-902543. A
      # rename on the same disk preserves the expensive partial cache without
      # copying it or accepting any prebuilt binary as qualified.
      mv "$worktree_cargo_target" "$shared_cargo_target"
    else
      echo "❌ both local and shared promotion Cargo targets exist; refusing ambiguous cache selection" >&2
      return 1
    fi
  fi
  mkdir -p "$shared_cargo_target"
  if [[ ! -L "$worktree_cargo_target" ]]; then
    ln -s "$shared_cargo_target" "$worktree_cargo_target"
  fi

  # `AXON_BUILD_ID` = l'identité du SHA promu, lue DANS le worktree détaché : c'est
  # elle que setup estampille dans chaque binaire après le link, et que
  # `preflight.sh` va chercher dans le contenu publié. Sans elle, le binaire ne
  # pourrait pas dire d'où il sort.
  local frozen_build_id
  frozen_build_id="$(git -C "$worktree" describe --tags --always --dirty)"
  if ! AXON_REQUIRE_NEXUS_ADMISSION=1 AXON_BUILD_ID="$frozen_build_id" \
      "$worktree/scripts/axon" setup --artifact-only; then
    echo "❌ frozen-worktree setup failed; refusing to inspect or publish partial artifacts" >&2
    return 1
  fi

  # Les artefacts candidats restent dans le worktree jusqu'à l'activation. `bin/axonctl`
  # demeure ainsi le contrôleur LKG et aucun gate ne peut exécuter le candidat par accident.
  local installed=0 artifact
  local -a expected_artifacts=(axon-core axon-brain axon-indexer axonctl axon-query-embed-worker)
  for artifact in "${expected_artifacts[@]}"; do
    if [[ ! -x "$worktree/bin/$artifact" ]]; then
      echo "❌ build depuis le worktree figé : exécutable attendu absent: $worktree/bin/$artifact" >&2
      return 1
    fi
    installed=$((installed + 1))
  done
  CANDIDATE_BIN_DIR="$worktree/bin"
  echo "  ✅ $installed artefact(s) construits depuis le SHA figé $sha (worktree détaché, arbre de travail non lu)"
}

if [[ "$SKIP_BUILD" -ne 1 ]]; then
  if [[ "$BUILD_FROM_TREE" -eq 1 ]]; then
    promote_log "⚠️  --dirty : build depuis l'ARBRE DE TRAVAIL. Le binaire n'est PAS une fonction du SHA seul (REQ-AXO-902391) — réservé au dev."
    run_step 1 build env AXON_REQUIRE_NEXUS_ADMISSION=1 "$ROOT_DIR/scripts/axon" setup --artifact-only
  else
    PROMOTE_SHA="$(git -C "$ROOT_DIR" rev-parse HEAD)"
    # `run_step` streams through tee, so its command executes in a pipeline subshell.
    # Publish these paths in the parent first; otherwise DEV silently falls back to a
    # mutable workspace rebuild after the child assignment disappears.
    PROMOTE_FROZEN_WORKTREE="$ROOT_DIR/.axon/promote-worktrees/${PROMOTE_SHA:0:12}"
    CANDIDATE_BIN_DIR="$PROMOTE_FROZEN_WORKTREE/bin"
    # Un binaire live doit correspondre à un commit que quelqu'un d'autre peut
    # retrouver. Sans ça, « quelle version tourne en production » n'a pas de
    # réponse vérifiable par un tiers.
    if [[ -z "$(git -C "$ROOT_DIR" branch -r --contains "$PROMOTE_SHA" 2>/dev/null)" ]]; then
      echo "❌ $PROMOTE_SHA n'est sur aucune branche distante — pousse-le avant de promouvoir," >&2
      echo "   ou passe --dirty en connaissance de cause (build non reproductible)." >&2
      exit 2
    fi
    promote_log "   source figée : $PROMOTE_SHA (worktree détaché — l'arbre de travail n'est pas lu, REQ-AXO-902391)"
    run_step 1 build build_from_frozen_worktree "$PROMOTE_SHA"
  fi
fi

# --- Step 1b: preflight — la porte d'intégrité d'artefact, au seul moment où elle
# peut dire la vérité (REQ-AXO-902454).
#
# Elle vérifie trois choses : `bin/<rôle>` a bien l'empreinte que son `.build-info`
# déclare · `AXON_BUILD_ID` == `git describe` · et `bin/` correspond encore à
# l'artefact canonique du workspace. La troisième n'est vraie qu'AVANT que quoi que
# ce soit d'autre ne recompile dans le target partagé — c'est-à-dire ici, et nulle
# part plus loin dans la séquence. Aucune vérification n'est perdue : elles sont
# toutes faites, plus tôt, sur l'artefact fraîchement installé.
ensure_head_stable
run_step 1b preflight "$ROOT_DIR/scripts/axon" release-preflight --bin-dir "${CANDIDATE_BIN_DIR:-$ROOT_DIR/bin}"
ensure_head_stable

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
    if [[ -n "$PROMOTE_FROZEN_WORKTREE" ]]; then
      bash "$ROOT_DIR/scripts/axon-dev" start brain --fast \
        --candidate-bin-dir "$PROMOTE_FROZEN_WORKTREE/bin" 2>&1
    else
      bash "$ROOT_DIR/scripts/axon-dev" start brain --fast 2>&1
    fi
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
  # serving.
  # Exit 77 = the script SKIPPED (nothing measured, e.g. the role is not Running+Ready).
  # Admission can deliberately pause the indexer immediately before this gate. In that
  # case the test's safety cleanup restores it, so retry ONCE while that recovery is hot.
  # A second skip remains non-passing: no release is qualified without a measurement.
  lifecycle_gate_step() {
    local rc=0
    bash "$ROOT_DIR/tests/shell/test_role_restart_live.sh" || rc=$?
    [[ "$rc" -ne 77 ]] && return "$rc"

    echo "⚠️ lifecycle gate SKIPPED once — cleanup may have restored the admission-paused indexer; measuring once more"
    rc=0
    bash "$ROOT_DIR/tests/shell/test_role_restart_live.sh" || rc=$?
    if [[ "$rc" -eq 77 ]]; then
      echo "❌ lifecycle gate SKIPPED twice (nothing measured) — the per-role restart was NOT verified for this release"
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
  local source_root="$ROOT_DIR"
  [[ -n "$PROMOTE_FROZEN_WORKTREE" ]] && source_root="$PROMOTE_FROZEN_WORKTREE"
  (
    cd "$source_root"
    devenv shell --no-reload --no-tui -- bash -lc \
      "cd '$source_root/src/axon-core' && CARGO_BUILD_JOBS=1 cargo build --tests -j 1 2>&1 | tail -20"
  )
}
run_step 2e test_targets_compile test_targets_compile_step

# --- Step 3: post-DEV candidate recheck ---------------------------------------------
ensure_head_stable

# REQ-AXO-902529 — re-certify digests + embedded build identity after every DEV
# lifecycle action and frozen test compilation, immediately before the manifest.
run_step 3 candidate_recheck "$ROOT_DIR/scripts/axon" release-preflight --bin-dir "${CANDIDATE_BIN_DIR:-$ROOT_DIR/bin}"
ensure_head_stable

# --- Step 4: manifest — synchronous, after all candidate gates -----------------------
manifest_out="$(mktemp)"
create_manifest_step() {
  "$ROOT_DIR/scripts/axon" create-release-manifest --state qualified \
    --release-attempt-id "$RELEASE_ATTEMPT_ID" \
    --bin-dir "${CANDIDATE_BIN_DIR:-$ROOT_DIR/bin}" > "$manifest_out" 2>&1 || {
    cat "$manifest_out"
    return 1
  }
  cat "$manifest_out"
}
run_step 4 manifest create_manifest_step
manifest_path="$(tail -n 1 "$manifest_out")"
rm -f "$manifest_out"
if [[ -z "$manifest_path" || ! -f "$manifest_path" ]]; then
  promote_log "Failed to capture manifest path from create-release-manifest output"
  exit 1
fi
manifest_path="$(realpath "$manifest_path")"
promote_log "   ✅ step 4 (manifest) done — $manifest_path"


# --- Step 5: promote (copy + restart) ---
# REQ-AXO-902285 — last GPU-wedge check, immediately before the destructive cutover and
# BEFORE the outage sampler starts. Catches a wedge that appeared DURING the ~9-min build
# (the pre-flight gate could not). On refusal the brain is still up → the EXIT trap's
# "brain UP, pas un incident" all-clear reaches the peers who saw the pre-notice broadcast.
require_gpu_channel_free "pre-cutover"
_start_mcp_sampler
ensure_head_stable
old_md5="$(md5sum "$ROOT_DIR/bin/axon-brain" 2>/dev/null | cut -d' ' -f1 || echo "none")"
# REQ-AXO-902165 / DEC-AXO-901666 — health-gated cutover with NATIVE auto-rollback
# (the Rust control-plane executor). One command: snapshot (current.json = rollback
# target) → stage (candidate bin/*) → full restart (re-bootstraps DDL, unlike the retired
# in-place path → step 5b is a cheap idempotent guard — TRUE only since REQ-AXO-902328
# closed the 9-file gap between the compiled list and db/ddl/) → `axonctl
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
run_step 5 cutover_prepare "$ROOT_DIR/bin/axonctl" cutover \
  --project-root "$ROOT_DIR" --instance-kind live --manifest "$manifest_path" \
  --phase prepare --max-polls 120 --poll-interval-ms 2000 --json
CUTOVER_PREPARED=1
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
# REQ-AXO-902328 (2026-08-25) — « expected to be a no-op » N'ÉTAIT PAS VRAI, et
# l'incident cité juste au-dessus en est la preuve. Le brain re-bootstrappait bien le
# DDL au restart, mais depuis une LISTE `include_str!` écrite à la main, à laquelle il
# manquait 9 des 25 fichiers — dont `15_mailbox.sql`. C'est pourquoi
# `axon.mailbox_message` manquait après un promote : pas parce que le restart
# in-place ne rejouait pas le DDL, mais parce que le DDL rejoué ne CONTENAIT pas cette
# table. Un restart complet ne l'aurait pas créée non plus.
#
# Pour ces 9 fichiers, ce step n'était donc pas un garde-fou redondant : il était le
# SEUL applicateur, et le retirer aurait cassé le schéma. La liste est désormais
# dérivée du répertoire par `build.rs`, et une garde
# (`postgres::ddl::tests::the_compiled_ddl_list_matches_the_directory`) refuse qu'elle
# redevienne un souvenir. À partir de maintenant, et pas avant, la phrase ci-dessus
# dit la vérité.
#
# Observed 2026-07-26 (promote 1399, in-place path): the runtime started BEFORE this
# step ran, so axon.EmbedderControl did not exist when the indexer tried to seed its
# idle-drop control row. The cutover ordering removes that window.
# Runs in devenv so psql resolves.
gate_with_attestation() {
  local gate="$1"
  shift
  local rc=0
  "$@" || rc=$?
  local gate_status="failed"
  case "$rc" in
    0) gate_status="passed" ;;
    124) gate_status="timeout" ;;
    77) gate_status="skipped" ;;
    65) gate_status="error" ;;
  esac
  "$ROOT_DIR/bin/axonctl" cutover --project-root "$ROOT_DIR" --instance-kind live \
    --phase record-gate --gate "$gate" \
    --gate-status "$gate_status" \
    --evidence "release_attempt_id=${RELEASE_ATTEMPT_ID}; exit_code=${rc}; log=${PROMOTE_LOG}" \
    --json >> "$PROMOTE_LOG" 2>&1 || true
  return "$rc"
}
run_step 5b apply_ddl_live gate_with_attestation ddl bash -lc "cd '$ROOT_DIR' && devenv shell --no-reload --no-tui -- bash -lc 'source scripts/lib/ensure-runtime.sh && apply_canonical_ddl live'"

# --- promote_status full-contract poll (shared by step 6 pre-gate + step 6c) ---
# REQ-AXO-902189 — hoisted above step 6 so the qualify pre-gate and the 6c health-gate
# reuse ONE definition. Sets recon_phase / recon_failed; returns 0 iff phase==clean.
recon_phase=""; recon_failed=""
_poll_promote_clean() {  # $1 = monotonic budget seconds
  local budget_seconds="$1" deadline recon_json recon_eval jitter_seconds
  deadline="$(axon_deadline_after_seconds "$budget_seconds")"
  axon_promote_journal_event readiness_wait reconcile running \
    "source=mcp_promote_status; deadline_monotonic_ms=${deadline}; budget_seconds=${budget_seconds}; residual_polling_jitter_seconds=1..3; supervisor readiness events are consumed by axonctl prepare"
  recon_phase=""; recon_failed=""
  while (( $(axon_monotonic_ms) < deadline )); do
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
    jitter_seconds=$((1 + RANDOM % 3))
    sleep "$jitter_seconds"
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
# REQ-AXO-902383 — WHICH qualifications ran, and which did not.
#
# Seven `qualify_*.py` live in scripts/; this step ran ONE. A promote therefore went
# green having tested only the brain's MCP surface — the script says so itself, in the
# step-6c comment below: "step-6 qualify tests only the brain (surface=core)". The
# partial coverage was known, documented, and compensated by another gate rather than
# closed. `qualify_indexer_truth` — the one that would have covered the 2026-08-20
# embed incident — never ran on any promote that day.
#
# ONE is added here: `qualify_indexer_truth`, which verifies the index says true
# things. It runs AFTER the cutover, so a red result lands on a runtime that
# `axonctl cutover` can still roll back natively.
#
# It was BROKEN and that is almost certainly why it was never wired: every table it
# queried read `public.*`, gone since the IST moved to `ist.*`, so it crashed on its
# first query; and it counted files by `status='indexed'`, a value the vocabulary no
# longer contains (live rows are `parsed` or `discovered`, `indexed` matches zero).
# Two independent breakages, both silent, on the one gate that would have covered the
# 2026-08-20 embed incident. Repaired and verified green before wiring:
#   MIL-AXO-032 verrou PASS — AXO syms=12389 files=897 ratio=13.81
#
# The others stay OUT, each for a stated reason — an inventory of gates that cannot
# run is what created this problem:
#   qualify_runtime        targets the DEV instance (:44139). Verified: it fails with
#                          "MCP runtime not ready after 120s" when dev is down, which
#                          is the normal state during a live promote. Wiring it would
#                          add a gate that always reds — the same disease, inverted.
#   qualify_ingestion_run  needs a corpus to ingest.
#   qualify_mcp_*          overlap surface=core, already covered by step 6.
# Widening further is a decision, not a default that grows.
#
# QUALIFY_RAN / QUALIFY_SKIPPED are echoed in the final summary. A gate nobody crossed
# reads exactly like a gate that passed, which is how VPC read "6 passed, 0 failed"
# (the restart gate's assertion count) as "6 qualifies out of 7".
QUALIFY_RAN=()
QUALIFY_SKIPPED=()

if [[ "$SKIP_QUALIFY" -eq 1 ]]; then
  QUALIFY_SKIPPED+=("all (--skip-qualify)")
else
  ensure_head_stable
  # The prepare phase already proved full-contract liveness and persisted that gate.
  # `promote_status` deliberately reports `staged` while pending.json exists, so polling
  # for `clean` here would deadlock the two-phase transaction by definition.
  if true; then
    qualify_core_gate() {
      local output rc=0
      output="$(mktemp)"
      timeout 180 "$ROOT_DIR/scripts/axon" --instance live qualify-mcp --surface core --checks quality,latency --project "$PROJECT_CODE" 2>&1 | tee "$output" || rc=${PIPESTATUS[0]}
      if [[ "$rc" -eq 0 ]] && ! grep -q '^verdict=ok$' "$output"; then
        echo "qualify-mcp returned success without parseable verdict=ok" >&2
        rc=65
      fi
      rm -f "$output"
      return "$rc"
    }
    run_step 6 qualify_mcp gate_with_attestation core_qualification qualify_core_gate
    QUALIFY_RAN+=("qualify_mcp(surface=core)")

    # Advisory: a red result here must SURFACE, not abort a cutover that already
    # succeeded — step 6c owns the fail-closed verdict. Recording the outcome is the
    # point; swallowing it silently is what this REQ exists to stop.
    QUALIFY_SKIPPED+=("qualify_runtime (targets dev :44139, down during a live promote)")
    QUALIFY_SKIPPED+=("qualify_ingestion_run (needs a corpus)")
    QUALIFY_SKIPPED+=("qualify_mcp_guidance/robustness/retrieval_context (overlap surface=core)")
    [[ -f "$ROOT_DIR/scripts/qualify_indexer_truth.py" ]] || {
      "$ROOT_DIR/bin/axonctl" cutover --project-root "$ROOT_DIR" --instance-kind live \
        --phase record-gate --gate indexer_truth --gate-status failed \
        --evidence "qualification script absent" --json >> "$PROMOTE_LOG" 2>&1 || true
      echo "qualify_indexer_truth.py absent" >&2
      exit 69
    }
    run_step 6b qualify_indexer_truth gate_with_attestation indexer_truth timeout 180 env \
      AXON_DEV_DATABASE_URL="${AXON_LIVE_DATABASE_URL:-postgres://axon@127.0.0.1:${AXON_CANONICAL_PG_PORT:?axon-pg-port.sh not sourced}/axon_live}" \
      python3 "$ROOT_DIR/scripts/qualify_indexer_truth.py"
    QUALIFY_RAN+=("qualify_indexer_truth")
  else
    QUALIFY_SKIPPED+=("all (runtime not clean after ~120s warmup, phase=${recon_phase:-unreachable})")
    promote_log "   ⚠️ step 6: runtime not clean after ~120s warmup (phase=${recon_phase:-unreachable}) — SKIP qualify, DEFER to step 6c recovery gate (REQ-AXO-902189: cold-start must not preempt the fail-closed verdict)."
  fi
fi

# Commit point: axonctl refuses this transition unless liveness, DDL, core qualification,
# and indexer truth are all durably attested `passed` in pending.json.
run_step 6f cutover_finalize "$ROOT_DIR/bin/axonctl" cutover \
  --project-root "$ROOT_DIR" --instance-kind live --phase finalize --json
CUTOVER_PREPARED=0

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
_poll_promote_clean 120 || true

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
    _poll_promote_clean 180 || true
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
    _poll_promote_clean 180 || true
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

# REQ-AXO-902531 — non-cutover side effects are durable jobs. Their bounded retry
# outcome is projected separately and never confused with the already-qualified cutover.
HOOK_STATE_ROOT="$LOG_DIR/hooks"
dispatch_durable_hook() {
  local hook_name="$1" timeout_seconds="$2"
  shift 2
  # `nohup ... &` reste un descendant du job de promotion. Les runners d'exécution
  # et certains superviseurs nettoient tout ce groupe quand le shell principal sort :
  # le hook keeper observé le 2026-08-27 est mort avec attempts_made=0 et status=running.
  # Une nouvelle session forkée n'appartient plus au job parent et peut publier son
  # verdict terminal après la fin du promote.
  setsid --fork python3 "$ROOT_DIR/scripts/release/durable_hook.py" \
    --state-root "$HOOK_STATE_ROOT" --attempt-id "$RELEASE_ATTEMPT_ID" \
    --hook-name "$hook_name" --max-attempts 3 --timeout-seconds "$timeout_seconds" \
    --retry-delay-seconds 2 -- "$@" </dev/null >> "$PROMOTE_LOG" 2>&1
  axon_promote_journal_event hook_dispatched finalize deferred \
    "hook=${hook_name} detached_session=true; final hook verdict is independent from cutover verdict"
  promote_log "   ▶ durable hook ${hook_name} dispatched in detached session (bounded retry; status=${HOOK_STATE_ROOT}/${RELEASE_ATTEMPT_ID}/${hook_name}.json)"
}

soll_export_args=$(printf '{"project_code":"%s"}' "$PROJECT_CODE")
dispatch_durable_hook soll-export 120 \
  "$ROOT_DIR/scripts/axon" --instance live mcp-call call soll_export \
  --args "$soll_export_args" --format text

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
dispatch_durable_hook memgraph-publication 900 bash "$ROOT_DIR/scripts/publish-memgraph.sh"

# REQ-AXO-311 tier 3 — anchor a permanent (never-expiring) SOLL snapshot to this
# qualified release. Same fire-and-forget contract as the Memgraph hook above:
# runs outside run_step, backgrounded, can never fail the promote. PIL-AXO-005
# fail-closed is untouched.
dispatch_durable_hook soll-keeper-backup 900 bash "$ROOT_DIR/scripts/backup_soll_daily.sh" --keeper

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
# REQ-AXO-902383 — name what was verified AND what was not. A summary that lists only
# successes cannot be told apart from one where nothing ran.
promote_log "   qualify_ran=${QUALIFY_RAN[*]:-(none)}"
promote_log "   qualify_skipped=${QUALIFY_SKIPPED[*]:-(none)}"
promote_log "   note: 'N passed, M failed' lines above come from the RESTART GATE (pid/ready/availability assertions), NOT from qualify — do not read them as a qualify count."
promote_log "   build_id=${final_build_id}"
promote_log "   sha=${start_head:0:12}"
promote_log "   bin/axon-brain md5=${final_md5}"
promote_log "   manifest=${manifest_path}"
promote_log "   log=${PROMOTE_LOG}"
promote_log "   release_attempt_id=${RELEASE_ATTEMPT_ID}"
promote_log "   chronology=${AXON_PROMOTE_JOURNAL_PATH}"
promote_log "   hooks=${HOOK_STATE_ROOT}/${RELEASE_ATTEMPT_ID} (running/retrying/completed/failed/deferred; hook failure does not rewrite cutover verdict)"
promote_log "   promotion_gates=$(promotion_gate_summary)"

# Disable the ERR trap — we succeeded
trap - ERR
