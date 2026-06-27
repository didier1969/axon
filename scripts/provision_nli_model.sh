#!/usr/bin/env bash
# REQ-AXO-902096 — provision the NLI cross-encoder (tasksource/ModernBERT-base-nli)
# as ONNX for contradiction_check. Run from repo root, AFTER any promote completes
# (heavy pip+download must not contend with a live promote/qualify).
set -euo pipefail

MODEL="tasksource/ModernBERT-base-nli"
OUT=".axon/models/nli-modernbert-base"
PY=.venv/bin/python
PIP=.venv/bin/pip

echo "== step 1: install ONNX export tooling =="
# The ONNX export runs on CPU — install CPU-only torch FIRST to avoid optimum
# pulling the multi-GB CUDA wheels (nvidia-cublas/cudnn/torch ~3-4GB) that
# timed out the network. Then optimum sees torch satisfied and skips them.
export UV_HTTP_TIMEOUT=600
PKGS=("optimum[onnxruntime]" "transformers>=4.48" onnx accelerate)
if command -v uv >/dev/null 2>&1; then
  uv pip install --python "$PY" torch --index-url https://download.pytorch.org/whl/cpu
  uv pip install --python "$PY" "${PKGS[@]}"
else
  "$PIP" install --retries 8 --timeout 240 torch --index-url https://download.pytorch.org/whl/cpu
  "$PIP" install --retries 8 --timeout 240 -U "${PKGS[@]}"
fi

echo "== step 2: export $MODEL -> $OUT (ONNX) =="
mkdir -p "$OUT"
.venv/bin/optimum-cli export onnx --model "$MODEL" --task text-classification "$OUT"

echo "== step 3: list artifacts =="
ls -la "$OUT"

echo "== step 4: smoke-test ORT load + id2label =="
"$PY" - <<'PY'
import json, onnxruntime as ort
from pathlib import Path
out = Path(".axon/models/nli-modernbert-base")
sess = ort.InferenceSession(str(out / "model.onnx"), providers=["CPUExecutionProvider"])
print("ORT inputs :", [(i.name, i.shape) for i in sess.get_inputs()])
print("ORT outputs:", [(o.name, o.shape) for o in sess.get_outputs()])
cfg = json.loads((out / "config.json").read_text())
print("id2label   :", cfg.get("id2label"))
PY
echo "== provisioning done =="
