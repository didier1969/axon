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
"$PIP" install -q -U "optimum[onnxruntime]" "transformers>=4.48" onnx accelerate

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
