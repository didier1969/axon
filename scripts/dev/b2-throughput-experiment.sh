#!/usr/bin/env bash
# REQ-AXO-901975 experiment — B2 throughput vs batch size + worker count.
# Runs in devenv shell. Widens the TRT max profile to batch=128, clears the
# engine cache once, then sweeps configs. Each bench is short (sweep mode).
set -uo pipefail

export ORT_STRATEGY=system
export ORT_DYLIB_PATH=$(jq -r .core_lib .axon/ort-artifacts/onnxruntime-tensorrt-cudaPackages/current.json)
export LD_LIBRARY_PATH=/usr/lib/wsl/lib:$(dirname "$ORT_DYLIB_PATH"):${LD_LIBRARY_PATH:-}
export AXON_DEV_DATABASE_URL=postgres://axon@127.0.0.1:44144/axon_dev

# Widen TRT profile so batch>64 is legal. opt stays at the common 64x256 point;
# max bumped to 128x512 so a single engine covers 1..128.
export AXON_TRT_PROFILE_MIN_SHAPES="input_ids:1x1,attention_mask:1x1,token_type_ids:1x1"
export AXON_TRT_PROFILE_OPT_SHAPES="input_ids:64x256,attention_mask:64x256,token_type_ids:64x256"
export AXON_TRT_PROFILE_MAX_SHAPES="input_ids:128x512,attention_mask:128x512,token_type_ids:128x512"

CACHE=/home/dstadel/.cache/axon/fastembed/tensorrt/engine-cache
echo "### clearing TRT engine cache (rebuild on widened profile) : $CACHE"
rm -rf "$CACHE" && echo "cleared"

BIN=".axon/cargo-target/release/axon-bench-pipeline-v2"
SRC="src/axon-core/src"          # bigger source than embedder/, more A chunks
DUR=60
WARM=25

run_cfg() {
  local label="$1" batch="$2" workers="$3"
  echo ""
  echo "############################################################"
  echo "### CONFIG $label : AXON_B2_BATCH_SIZE=$batch AXON_B2_WORKERS=$workers"
  echo "############################################################"
  AXON_B2_BATCH_SIZE="$batch" AXON_B2_WORKERS="$workers" \
    "$BIN" --source "$SRC" --max-files 60 --duration-secs "$DUR" --warmup-secs "$WARM" --gpu --human 2>&1 \
    | grep -E "sustained|Goldratt|B2 |warmup snapshot|wall|error|setInputShape" | sed "s/^/[$label] /"
}

run_cfg "b64_w1"  64 1
run_cfg "b96_w1"  96 1
run_cfg "b128_w1" 128 1
run_cfg "b64_w2"  64 2
echo ""
echo "### experiment done"
