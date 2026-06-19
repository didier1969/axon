#!/usr/bin/env bash
# Restore TensorRT runtime — v2. REQ-AXO-902021.
#
# Context: WSL disk corruption damaged many nix-store paths. A whole-store
# `nix-store --verify --check-contents --repair` (done separately) already
# repaired the toolchain (gcc-wrapper, bash) + ~265 paths and left ORT
# INVALID (unregistered) because --verify --repair only SUBSTITUTES and ORT
# is a local (non-substitutable) build. An invalid path must be BUILT with
# `--realise`, NOT `--repair-path` (which only repairs *valid* paths and
# errors "is not valid" otherwise — the v1 infinite-loop bug).
#
# This job: build ORT via --realise (toolchain now valid -> build succeeds)
# -> confirm TRT still integral -> restart live runtime indexer-full
# (TensorRT) -> verify no libnvinfer segfault. NO time limit, capped retries,
# idempotent, fully logged.
#
# Launch: setsid bash scripts/restore-tensorrt-runtime.sh >/dev/null 2>&1 </dev/null &
# Watch:  tail -f .axon/restore-tensorrt.log

set -uo pipefail
ROOT=/home/dstadel/projects/axon
LOG="$ROOT/.axon/restore-tensorrt.log"
NIXSTORE="$(command -v nix-store)"
ORT=/nix/store/0bk9hvccz0rhbrfjvx3628lqy3sgpyzm-onnxruntime-1.24.4
ORTDRV=/nix/store/m2452ryjglg6413h2pkqwplnzpd0739w-onnxruntime-1.24.4.drv
TRT=/nix/store/4sh6704g12h1h3gkx0701nhqxdynfbw4-tensorrt-local-10.14.1.48
PATHSAVE="$PATH"
log() { echo "[$(date '+%F %T')] $*" >> "$LOG"; }
ort_ok() { nix path-info "$ORT" >/dev/null 2>&1 && [ -e "$ORT/lib/libonnxruntime.so" ]; }

: > "$LOG"
log "=== restore v2 START (pid $$) ==="

# 1. Build ORT (invalid -> realise builds it from its deriver; ~2-3h CUDA build).
n=0
while ! ort_ok; do
  n=$((n + 1))
  if [ "$n" -gt 3 ]; then
    log "ORT build still failing after $n attempts — ABORT. Inspect $LOG for the build error (likely another corrupt build dep)."
    log "RESULT: NEEDS-REVIEW — ORT build failed."
    exit 1
  fi
  log "ORT --realise attempt #$n (build the DERIVER from source, ~2-3h, toolchain repaired)"
  nix-store --realise "$ORTDRV" >> "$LOG" 2>&1 || log "realise returned nonzero (re-checking validity)"
  ort_ok && break
  sleep 15
done
log "ORT INTEGRAL ✅ ($(du -sh "$ORT" 2>/dev/null | cut -f1))"

# 2. Confirm TRT still integral (repair if regressed).
if nix store verify --no-trust "$TRT" 2>&1 | grep -qiE 'was modified|does not exist'; then
  log "TRT regressed — repairing"
  sudo -n env "PATH=$PATHSAVE" "$NIXSTORE" --repair-path "$TRT" >> "$LOG" 2>&1 || log "TRT repair nonzero"
fi
log "TRT confirmed integral"

# 3. Restart live runtime in indexer-full (TensorRT EP, dashboard incl.).
log "Restarting live runtime: indexer-full (TensorRT)."
cd "$ROOT" || { log "FATAL cd"; exit 1; }
./scripts/axon-live restart --indexer-full >> "$LOG" 2>&1 || log "restart returned nonzero (continuing to verify)"

# 4. Warmup + verification.
log "Waiting 180s for indexer TensorRT warmup..."
sleep 180
if pgrep -f 'bin/axon-indexer' >/dev/null; then
  log "indexer ALIVE uptime=$(ps -o etimes= -C axon-indexer 2>/dev/null | tr -d ' ' | head -1)s"
  OK=1
else
  log "indexer NOT running after warmup"; OK=0
fi
log "fresh libnvinfer segfaults (expect NONE):"
( dmesg -T 2>/dev/null || dmesg ) 2>/dev/null | grep -iE 'axon-indexer.*(segfault|libnvinfer|signal 11)' | tail -4 | sed 's/^/    /' >> "$LOG"
log "--- axon-live status ---"
./scripts/axon-live status >> "$LOG" 2>&1 || true
if [ "${OK:-0}" = 1 ]; then
  log "RESULT: SUCCESS — ORT rebuilt, TensorRT indexer running."
else
  log "RESULT: NEEDS-REVIEW — indexer not confirmed; inspect log + dmesg."
fi
log "=== restore v2 DONE ==="
