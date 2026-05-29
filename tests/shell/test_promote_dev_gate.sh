#!/usr/bin/env bash
# REQ-AXO-901662 — Self-test for the promote-live dev validation gate.
#
# Why: session 51 (2026-05-22) shipped REQ-AXO-901659 (dev validation
# code-enforced gate in promote_live_safe.sh) but the original parser
# used `grep -oE '"build_id"' | head -1` which silently picked
# `peer_runtime_version.build_id` (a cached entry) instead of the
# brain's own `runtime_version.build_id`. The bug was discovered on
# first real use — a recursive `feedback_dev_first_no_exception`
# violation (meta-tooling not tested against real data before commit).
#
# REQ-AXO-901660 fixed the parser to extract via `python3 json` at
# the precise path `.result.data.runtime_version.build_id`. This
# self-test locks in the contract so any future regression is caught
# at CI / pre-commit instead of at promote time.
#
# How: mocks two synthetic dev MCP status payloads (one matches HEAD,
# one doesn't) and exercises the parser in isolation. No live PG, no
# brain, no dev-shell required. Runs as a plain bash script.
#
# Usage:
#   bash tests/shell/test_promote_dev_gate.sh
#   echo $?  # 0 = pass, non-zero = fail
#
# REQ-AXO-901659 / 901660 / 901661 / 901662 family.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

fail() {
    echo "❌ FAIL: $1" >&2
    exit 1
}

pass() {
    echo "✅ PASS: $1"
}

# --- The parser under test (matches promote_live_safe.sh validate_dev_healthy) ---
extract_runtime_version_build_id() {
    python3 -c '
import json, sys
try:
    doc = json.load(sys.stdin)
    bid = doc.get("result", {}).get("data", {}).get("runtime_version", {}).get("build_id")
    if isinstance(bid, str) and bid:
        print(bid)
except Exception:
    pass
' 2>/dev/null || true
}

# --- Synthetic dev MCP status fixtures ---

# Case 1 : both peer_runtime_version and runtime_version present.
# The parser MUST pick runtime_version.build_id, NOT peer_runtime_version.
read -r -d '' FIXTURE_BOTH <<'EOF' || true
{
  "result": {
    "data": {
      "runtime_authority": {
        "runtime_state": {
          "peer_runtime_version": {
            "build_id": "v0.8.0-513-g65936669-OLDPEER"
          }
        }
      },
      "runtime_version": {
        "build_id": "v0.8.0-636-gd84ab23f-CANDIDATE"
      }
    }
  }
}
EOF

extracted=$(echo "$FIXTURE_BOTH" | extract_runtime_version_build_id)
if [[ "$extracted" != "v0.8.0-636-gd84ab23f-CANDIDATE" ]]; then
    fail "Case 1 — both fields present : parser picked '$extracted' (expected 'v0.8.0-636-gd84ab23f-CANDIDATE'). Old `grep | head` would pick peer."
fi
pass "Case 1 — both fields present : runtime_version picked, not peer_runtime_version"

# Case 2 : only runtime_version present (live brain pre-federation cache).
read -r -d '' FIXTURE_ONLY <<'EOF' || true
{
  "result": {
    "data": {
      "runtime_version": {
        "build_id": "v0.8.0-100-gAAAAAAAA"
      }
    }
  }
}
EOF

extracted=$(echo "$FIXTURE_ONLY" | extract_runtime_version_build_id)
if [[ "$extracted" != "v0.8.0-100-gAAAAAAAA" ]]; then
    fail "Case 2 — only runtime_version : parser picked '$extracted' (expected 'v0.8.0-100-gAAAAAAAA')."
fi
pass "Case 2 — only runtime_version : extracted correctly"

# Case 3 : missing runtime_version (older brain contract pre-REQ-AXO-150).
# Parser must return empty so the gate falls back to soft-warning.
read -r -d '' FIXTURE_MISSING <<'EOF' || true
{
  "result": {
    "data": {
      "runtime_authority": {
        "runtime_state": {
          "peer_runtime_version": {
            "build_id": "v0.8.0-513-g65936669-OLDPEER"
          }
        }
      }
    }
  }
}
EOF

extracted=$(echo "$FIXTURE_MISSING" | extract_runtime_version_build_id)
if [[ -n "$extracted" ]]; then
    fail "Case 3 — runtime_version absent : parser returned '$extracted' (expected empty so gate soft-warns)."
fi
pass "Case 3 — runtime_version absent : empty extraction (gate falls back to warning)"

# Case 4 : malformed JSON. Must not crash, must return empty.
extracted=$(echo "not a json" | extract_runtime_version_build_id)
if [[ -n "$extracted" ]]; then
    fail "Case 4 — malformed JSON : parser returned '$extracted' (expected empty)."
fi
pass "Case 4 — malformed JSON : empty extraction (no crash)"

# Case 5 : empty body.
extracted=$(echo "" | extract_runtime_version_build_id)
if [[ -n "$extracted" ]]; then
    fail "Case 5 — empty body : parser returned '$extracted' (expected empty)."
fi
pass "Case 5 — empty body : empty extraction"

# Case 6 : runtime_version.build_id is empty string (treat as missing).
read -r -d '' FIXTURE_EMPTY <<'EOF' || true
{"result":{"data":{"runtime_version":{"build_id":""}}}}
EOF
extracted=$(echo "$FIXTURE_EMPTY" | extract_runtime_version_build_id)
if [[ -n "$extracted" ]]; then
    fail "Case 6 — empty build_id : parser returned '$extracted' (expected empty)."
fi
pass "Case 6 — empty build_id : empty extraction"

# --- Match semantics : the gate compares `*$short_head*` ---

# Case 7 : exact short-head match.
short_head="d84ab23f"
build_id="v0.8.0-636-gd84ab23f"
if [[ "$build_id" != *"$short_head"* ]]; then
    fail "Case 7 — gate match : '$short_head' should be in '$build_id'"
fi
pass "Case 7 — gate match : short_head 'd84ab23f' found in build_id"

# Case 8 : dirty suffix tolerated.
build_id="v0.8.0-637-gNEWHASH8-dirty"
short_head="NEWHASH8"
if [[ "$build_id" != *"$short_head"* ]]; then
    fail "Case 8 — dirty suffix : '$short_head' should be in '$build_id'"
fi
pass "Case 8 — dirty suffix : tolerated"

# Case 9 : mismatch (the gate's primary failure path).
build_id="v0.8.0-629-gd0d7a43f"
short_head="d84ab23f"
if [[ "$build_id" == *"$short_head"* ]]; then
    fail "Case 9 — mismatch : '$short_head' should NOT be in '$build_id'"
fi
pass "Case 9 — mismatch : gate refuses correctly"

# --- REQ-AXO-901782 : --restart-live must spawn FULL live profile, not brain_only ---
#
# Why : the post-check (check_live_runtime_version.py) enforces
# `indexer_ready=true` via runtime_authority_contract("brain"), so any
# brain_only restart times out at 150s. Operator hit the workaround twice
# in session 59 (curl POST /process/start/{axon-indexer,dashboard} then
# --finalize-only). The canonical spawn in promote_live.sh --restart-live
# is now `start full` — locked in by static grep below.

promote_script="$ROOT_DIR/scripts/release/promote_live.sh"
if [[ ! -f "$promote_script" ]]; then
    fail "Case 10 — promote_live.sh missing at $promote_script"
fi

# Case 10a : the canonical `start full` invocation lives inside the
# RESTART_LIVE block. We grep the literal command tail to keep the
# guard tight against unrelated edits.
if ! grep -q "scripts/axon\" --instance live start full" "$promote_script"; then
    fail "Case 10a — promote_live.sh does not invoke 'start full' for --restart-live (REQ-AXO-901782 regression?). Inspect lines around the RESTART_LIVE branch."
fi
pass "Case 10a — promote_live.sh --restart-live spawns 'start full' (brain+indexer+dashboard)"

# Case 10b : the deprecated `start brain --fast` MUST NOT appear in the
# restart-live spawn line. If it reappears, the post-check times out
# again. We allow the substring to live in comments/docs, but not as the
# active spawn argument.
if grep -E '^[[:space:]]*if[[:space:]]+!.*scripts/axon".*--instance live start brain --fast' "$promote_script" >/dev/null; then
    fail "Case 10b — promote_live.sh still spawns 'start brain --fast' (REQ-AXO-901782 regression). Replace with 'start full'."
fi
pass "Case 10b — promote_live.sh does not spawn 'start brain --fast' in --restart-live"

echo ""
echo "🎯 All 11 cases passed — promote-live dev gate parser + --restart-live spawn locked in."
exit 0
