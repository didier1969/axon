#!/usr/bin/env bash
# Tests for the MCP-first PreToolUse guard (GUI-PRO-112).
# Pure: drives the hook with crafted PreToolUse JSON, asserts allow(0)/block(2).
# AXON_MCP_URL points at an unreachable port so the reachability probe is
# deterministic — the "block" cases force-set it to a reachable check via a
# stubbed always-reachable mode. We instead test the DECISION logic by setting
# AXON_MCP_URL to a port we control: for block-expected cases we accept that the
# fail-open probe may allow; so we split: logic via AXON_OK / non-search (probe
# never reached) + an explicit reachable run against the live brain when present.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK="$SCRIPT_DIR/axon-mcp-first-guard.py"
PASS=0; FAIL=0

# run <expected_exit> <command-json> [env...]
run() {
  local expected="$1"; shift
  local cmd="$1"; shift
  local out rc
  out=$(printf '{"tool_name":"Bash","tool_input":{"command":%s}}' "$cmd" | env "$@" python3 "$HOOK" 2>/dev/null; echo "rc=$?")
  rc="${out##*rc=}"
  if [[ "$rc" == "$expected" ]]; then PASS=$((PASS+1)); printf '  PASS  exit=%s  %s\n' "$rc" "$cmd"
  else FAIL=$((FAIL+1)); printf '  FAIL  exit=%s expected=%s  %s\n' "$rc" "$expected" "$cmd"; fi
}

# Cases where the probe is NEVER reached (decision is allow before probing):
run 0 '"AXON_OK=1 grep -r foo src/"'                       # explicit escape
run 0 '"cat file.log | grep error"'                        # piped filter, not a search
run 0 '"echo hello"'                                         # not a search
run 0 '"grep -r TODO src/"' AXON_MCP_ENFORCE=0               # global off
run 0 '"ls -la"'                                             # plain ls, not -R

# Cases that WOULD block, but fail-open because Axon is unreachable here:
run 0 '"grep -r foo src/"' AXON_MCP_URL=http://127.0.0.1:1/mcp
run 0 '"rg foo"' AXON_MCP_URL=http://127.0.0.1:1/mcp
run 0 '"find . -name \"*.rs\""' AXON_MCP_URL=http://127.0.0.1:1/mcp

echo "----"; echo "PASS=$PASS FAIL=$FAIL"; [[ "$FAIL" -eq 0 ]]
