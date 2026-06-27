#!/usr/bin/env bash
# Clean pipeline-v2 throughput bench (REQ-AXO-289 / canonical procedure GUI-AXO-1028).
#
# WHY this script exists: a bench launched without a clean IST + an exclusive GPU
# gives a FALSE number (dedup contamination + TensorRT contention). Session 82's
# first attempt skipped both and produced an 18-min churn on a 14k-file stale dev
# DB instead of a ~5-min run on 875 files. This encodes the mandatory pre-flight
# so it cannot happen again. See GUI-AXO-1028 for the rationale.
#
# Usage:
#   scripts/run-clean-pipeline-bench.sh [--source PATH] [--max-files N] [--cpu] [--build]
# Defaults: --source repo root, --max-files 3000, GPU mode.
#   --build  : cargo build --release the bench binary first (compile != bench time).
#
# It must run inside (or self-enter) the devenv shell for ORT/CUDA dlopen.
set -uo pipefail

ROOT="/home/dstadel/projects/axon"
cd "$ROOT"

SOURCE="$ROOT"
MAX_FILES=3000
MODE="--gpu"
DO_BUILD=0
while [ $# -gt 0 ]; do
  case "$1" in
    --source)    SOURCE="$2"; shift 2 ;;
    --max-files) MAX_FILES="$2"; shift 2 ;;
    --cpu)       MODE="--cpu"; shift ;;
    --build)     DO_BUILD=1; shift ;;
    *) echo "unknown arg: $1"; exit 2 ;;
  esac
done

PG_HOST=127.0.0.1; PG_PORT=44144; PG_USER=axon; PG_DB=axon_dev
psql_dev() { psql -h "$PG_HOST" -p "$PG_PORT" -U "$PG_USER" -d "$PG_DB" "$@"; }

echo "### PRE-FLIGHT 1 — live indexer must be stopped (exclusive GPU)"
if pgrep -f "bin/axon-indexer" >/dev/null 2>&1; then
  echo "ABORT: a live/dev axon-indexer is running. Stop it first:"
  echo "       ./scripts/axon-live stop --role indexer   # keeps the brain for MCP"
  exit 5
fi

echo "### PRE-FLIGHT 2 — GPU must be near-idle (NVML telemetry, REQ-AXO-902085)"
# In-process NVML probe via scripts/lib/gpu_nvml.py (no nvidia-smi subprocess /
# CLI parsing — feedback_nvml_not_nvidia_smi). Gate on AXON_OPT_MAX_VRAM_USED_MB
# (default 800MiB). If NVML is unavailable we WARN and continue (a truly busy
# GPU still surfaces downstream as a TensorRT contention failure).
if [ "$MODE" = "--gpu" ]; then
  MAX_VRAM_USED_MB="${AXON_OPT_MAX_VRAM_USED_MB:-800}"
  python3 - "$ROOT" "$MAX_VRAM_USED_MB" <<'PY'
import os, sys
root, threshold = sys.argv[1], int(sys.argv[2])
sys.path.insert(0, os.path.join(root, "scripts", "lib"))
try:
    from gpu_nvml import gpu_status
except Exception as exc:  # noqa: BLE001
    print("    gpu telemetry import failed (%s) — skipping idle gate" % type(exc).__name__)
    sys.exit(2)
st = gpu_status()
if not st.get("available"):
    print("    gpu telemetry unavailable (%s) — skipping idle gate" % st.get("error", "unknown"))
    sys.exit(2)
used = int(st.get("memory_used_mb") or 0)
print("    gpu_mem_used=%dMiB threshold=%dMiB" % (used, threshold))
sys.exit(4 if used > threshold else 0)
PY
  gpu_gate_rc=$?
  if [ "$gpu_gate_rc" -eq 4 ]; then
    echo "ABORT: GPU not idle (> ${MAX_VRAM_USED_MB}MiB used). Stop competing TensorRT engines."
    exit 4
  fi
fi

echo "### PRE-FLIGHT 3 — clean the dev IST and VERIFY (atomic multi-table TRUNCATE"
echo "    silently fails if one table is absent, e.g. public.edge in dev — truncate"
echo "    table-by-table, then assert count=0)."
for t in ist.chunkembedding ist.chunk ist.symbol ist.indexedfile; do
  psql_dev -q -c "TRUNCATE $t CASCADE;" 2>/dev/null || echo "    (note: $t truncate skipped/absent)"
done
files=$(psql_dev -tAc "SELECT count(*) FROM ist.indexedfile;" | tr -d '[:space:]')
echo "    dev ist.indexedfile after clean = $files"
if [ "$files" != "0" ]; then echo "ABORT: dev IST not clean ($files files)"; exit 3; fi

echo "### ENV (GPU TensorRT — required on WSL2 or CUDA error 35)"
export ORT_STRATEGY=system
export ORT_DYLIB_PATH="$(jq -r .core_lib "$ROOT/.axon/ort-artifacts/onnxruntime-tensorrt-cudaPackages/current.json")"
export LD_LIBRARY_PATH="/usr/lib/wsl/lib:$(dirname "$ORT_DYLIB_PATH"):${LD_LIBRARY_PATH:-}"
export AXON_DEV_DATABASE_URL="postgres://${PG_USER}@${PG_HOST}:${PG_PORT}/${PG_DB}"
# Sustained-throughput tuning (session 55): A2=8 optimal on Ryzen 7 5800H.
export AXON_BULK_WRITER_ENABLED=1 AXON_A3_BATCH_SIZE=64 AXON_A3_BATCH_TIMEOUT_MS=50 \
       AXON_A3_WORKERS=4 AXON_A2_WORKERS=8

BIN="$ROOT/.axon/cargo-target/release/axon-bench-pipeline-v2"
if [ "$DO_BUILD" = "1" ] || [ ! -x "$BIN" ]; then
  echo "### BUILD release bench binary (compile time is NOT bench time)"
  cargo build --manifest-path src/axon-core/Cargo.toml --release --bin axon-bench-pipeline-v2
fi

echo "### RUN — source=$SOURCE max_files=$MAX_FILES mode=$MODE"
"$BIN" --source "$SOURCE" --max-files "$MAX_FILES" $MODE --human
echo "### BENCH DONE rc=$?"
echo "### REMINDER: restart the live indexer when done — ./scripts/axon-live start full"
