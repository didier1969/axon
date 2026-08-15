#!/usr/bin/env bash
# tests/shell/test_promote_error_path.sh — REQ-AXO-902327
#
# The FAILURE path of promote_live_safe.sh, which is the one part of the script that
# is almost never exercised.
#
# Why this file exists
# --------------------
# The nominal path runs on every successful promote and is therefore continuously
# tested. The failure path runs maybe once in fifty — and on 2026-08-15 it turned out
# to have rotted, in two separate ways, with nothing signalling it:
#
#   1. `on_promote_exit` (the EXIT trap, registered near the top) CALLED
#      `_report_mcp_outage`, which was DEFINED ~400 lines further down. Any failure
#      before that definition exited on `_report_mcp_outage: command not found`, so
#      the MCP-outage measurement that REQ-AXO-902256 makes mandatory was missing
#      exactly on the runs where it matters most. Observed twice the same day: a
#      step-2d failure (before the definition) lost the report; a step-5b failure
#      (after it) printed it correctly.
#
#   2. `broadcast_promote` built its JSON with `$(python3 -c "…")`. Inside a command
#      substitution bash still expands backticks — including inside double quotes —
#      so a python COMMENT quoting `mailbox_sweep()` was parsed as a command
#      substitution and executed on every promote. It errored on `()`, which is why
#      the JSON survived and the defect stayed cosmetic; a comment holding a valid
#      command would have RUN it.
#
# `bash -n` catches NEITHER: it does not resolve function names, and it does not
# evaluate command substitutions. That is precisely why these assertions are here.
#
# Run: bash tests/shell/test_promote_error_path.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
PROMOTE="$ROOT_DIR/scripts/release/promote_live_safe.sh"

PASS=0
FAIL=0
pass() { printf '  PASS  %s\n' "$1"; PASS=$(( PASS + 1 )); }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=$(( FAIL + 1 )); }

[[ -f "$PROMOTE" ]] || { echo "missing $PROMOTE" >&2; exit 1; }

# --- 1. static syntax (necessary, and demonstrably not sufficient) -------------------
if bash -n "$PROMOTE" 2>/dev/null; then
  pass "promote_live_safe.sh parses"
else
  fail "promote_live_safe.sh does not parse"
fi

# --- 2. every function the EXIT trap calls is defined BEFORE the trap ----------------
# Positional invariant, made explicit: a trap registered at line N may only call
# functions defined before line N. Checking it structurally beats remembering it.
trap_line="$(grep -n '^trap on_promote_exit EXIT' "$PROMOTE" | cut -d: -f1 || true)"
if [[ -z "$trap_line" ]]; then
  fail "cannot locate the EXIT trap registration"
else
  # Functions invoked from inside on_promote_exit's body.
  body_start="$(grep -n '^on_promote_exit() {' "$PROMOTE" | cut -d: -f1)"
  missing=""
  for fn in _report_mcp_outage broadcast_promote; do
    def_line="$(grep -n "^${fn}() {" "$PROMOTE" | cut -d: -f1 || true)"
    if [[ -z "$def_line" ]]; then
      missing="${missing} ${fn}(undefined)"
    elif (( def_line > trap_line )); then
      missing="${missing} ${fn}(line ${def_line} > trap ${trap_line})"
    fi
  done
  # The sampler pid the trap's callee reads must also exist by then.
  pid_line="$(grep -n '^MCP_SAMPLER_PID=""' "$PROMOTE" | head -1 | cut -d: -f1 || true)"
  if [[ -z "$pid_line" || "$pid_line" -gt "$trap_line" ]]; then
    missing="${missing} MCP_SAMPLER_PID(after trap)"
  fi
  if [[ -z "$missing" ]]; then
    pass "EXIT trap (line ${trap_line}, body ${body_start}) only calls callees defined above it"
  else
    fail "EXIT trap calls callees defined AFTER it —${missing}"
  fi
fi

# --- 3. the broadcast JSON builder is expansion-free ---------------------------------
# Extract the python source variable + the function, feed hostile input, and require
# BOTH a valid JSON object AND a silent stderr. The stderr assertion is the load-bearing
# one: the old form produced correct JSON *and* a shell error on every single promote.
py_start="$(grep -n "^read -r -d '' _BROADCAST_PY" "$PROMOTE" | cut -d: -f1 || true)"
fn_start="$(grep -n '^broadcast_promote() {' "$PROMOTE" | cut -d: -f1 || true)"
if [[ -z "$py_start" || -z "$fn_start" ]]; then
  fail "cannot locate _BROADCAST_PY / broadcast_promote"
else
  harness="$(mktemp)"
  out="$(mktemp)"
  err="$(mktemp)"
  {
    echo '#!/usr/bin/env bash'
    echo 'set -uo pipefail'
    echo 'PROJECT_CODE=AXO'
    # Take the python heredoc plus the two lines of the function that build `args`,
    # then stop before the network call.
    sed -n "${py_start},$(( fn_start + 5 ))p" "$PROMOTE" \
      | sed 's|^  timeout 20.*|  printf "%s" "$args"; return 0|; s|^    --args .*||'
    echo '}'
    echo 'broadcast_promote "$1" "$2" "$3"'
  } > "$harness"
  # Hostile-but-plausible operator text: accents, an apostrophe, a dollar and a
  # backtick — all of which the previous form would have re-parsed.
  bash "$harness" 'Promote AXO — coupure à venir' 'l’indexeur $HOME `id`' 'k-1' \
    >"$out" 2>"$err" || true

  if python3 -c "import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get('priority')=='low' and d.get('ttl_hours')==24 and d.get('to_project')=='*' else 1)" "$out" 2>/dev/null; then
    pass "broadcast JSON is well-formed and carries priority=low + ttl_hours=24"
  else
    fail "broadcast JSON malformed or missing the retention contract: $(cat "$out")"
  fi

  if [[ ! -s "$err" ]]; then
    pass "broadcast builder emits NOTHING on stderr (no shell re-parse of its own source)"
  else
    fail "broadcast builder still re-parses its source: $(cat "$err")"
  fi
  rm -f "$harness" "$out" "$err"
fi

# --- 4. no backtick survives inside a command substitution in this script ------------
# The class, not the instance: a backtick inside `$( … )` is text that bash EXECUTES.
#
# Shell comments are excluded, and that exclusion is itself load-bearing: the first
# version of this guard flagged the very comment that documents the defect it forbids.
# A guard that fires on its own prose is a guard someone deletes — the same self-match
# that REQ-AXO-902260's anti-recidive test hit an hour earlier.
offenders="$(grep -nE '\$\(.*`' "$PROMOTE" | grep -vE '^[0-9]+:[[:space:]]*#' || true)"
if [[ -n "$offenders" ]]; then
  fail "a backtick lives inside a command substitution: $(echo "$offenders" | head -3)"
else
  pass "no backtick inside any command substitution (comments excluded)"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
