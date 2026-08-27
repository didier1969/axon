#!/usr/bin/env bash
# REQ-AXO-902535 — un hook long survit au shell qui l'a distribué.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
STATE_ROOT="$(mktemp -d)"
trap 'rm -rf "$STATE_ROOT"' EXIT

# Le shell distributeur se termine immédiatement. `setsid --fork` doit laisser le
# runner terminer et publier atomiquement son état après cette sortie.
bash -c '
  setsid --fork python3 "$1/scripts/release/durable_hook.py" \
    --state-root "$2" --attempt-id detached --hook-name survivor \
    --max-attempts 1 --timeout-seconds 10 -- \
    bash -c "sleep 1; exit 0" </dev/null >/dev/null 2>&1
' _ "$ROOT_DIR" "$STATE_ROOT"

state="$STATE_ROOT/detached/survivor.json"
for _ in $(seq 1 50); do
  [[ -s "$state" ]] && [[ "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["status"])' "$state")" == completed ]] && break
  sleep 0.1
done

python3 - "$state" <<'PY'
import json, sys
state = json.load(open(sys.argv[1], encoding="utf-8"))
assert state["status"] == "completed", state
assert state["attempts_made"] == 1, state
assert state["runner_pid"] > 1, state
PY
printf '  PASS  detached durable hook survives its distributor and completes\n'
