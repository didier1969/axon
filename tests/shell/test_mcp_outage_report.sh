#!/usr/bin/env bash
# tests/shell/test_mcp_outage_report.sh — REQ-AXO-902233
#
# Fixture tests for the MCP-availability scorer. No runtime, no promote: every case is a
# synthetic sample file.
#
# What this protects
# ------------------
# This scorer decides whether a change to the promote made the outage better or worse. Its
# predecessor counted SAMPLES and printed them as seconds, under-stating every real outage
# by ~3x — a promote that cut MCP for 187s reported "63s", and one that cut it for 191s
# reported "1s". Three earlier instruments on this REQ failed for three different reasons.
# A scorer that cannot be shown to fail is not a measurement, so each case below is a
# distinct way of being wrong.
#
# Run: bash tests/shell/test_mcp_outage_report.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
SCORER="$ROOT_DIR/scripts/release/mcp_outage_report.py"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

PASS=0
FAIL=0
pass() { printf '  PASS  %s\n' "$1"; PASS=$(( PASS + 1 )); }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=$(( FAIL + 1 )); }

# assert_score <desc> "<expected: worst total n span res>" <<< csv-on-stdin
assert_score() {
    local desc="$1" expected="$2" got
    cat > "$TMP/s.csv"
    got="$(python3 "$SCORER" "$TMP/s.csv")"
    if [[ "$got" == "$expected" ]]; then
        pass "$desc"
    else
        fail "$desc (attendu «$expected», obtenu «$got»)"
    fi
}

printf 'mcp_outage_report — REQ-AXO-902233\n'

# THE regression. Ten `down` samples spaced 3s apart is a 28-second outage, not a
# 10-second one. This is the exact shape of the 2026-07-27 promote that was reported as
# "10s" while third-party MCP clients had 28 seconds of refused connections.
assert_score 'des échantillons espacés de 3s comptent en SECONDES, pas en échantillons' \
    '28 28 14 32 3' <<'CSV'
1000,up
1002,up
1005,down
1008,down
1011,down
1014,down
1017,down
1020,down
1023,down
1026,down
1029,down
1030,up
1031,up
1032,up
CSV

# The window opens at the last KNOWN-good sample, not at the first `down`: a client calling
# one second after the last `up` is already refused. Measuring first-down→last-down would
# hide one sampling interval at each edge — in the flattering direction, again.
assert_score 'la fenêtre s ouvre au dernier up, pas au premier down' \
    '20 20 3 20 10' <<'CSV'
100,up
110,down
120,up
CSV

# A healthy promote must read zero, and must be distinguishable from "measured nothing"
# by the sample count — which is why n is published alongside.
assert_score 'aucune coupure rend 0, avec le compte d échantillons' \
    '0 0 4 3 1' <<'CSV'
50,up
51,up
52,up
53,up
CSV

# Two separate outages: `worst` is the longest one, `total` their sum. Reporting only the
# total would hide a single long cut inside a lot of small ones, and vice versa.
# Both windows open at their preceding `up`: 0→20 (20s) and 20→60 (40s). Measuring
# first-down→last-down would have given 10s and 10s — the understatement this file exists
# to prevent.
assert_score 'deux coupures : worst = la plus longue, total = la somme' \
    '40 60 6 60 10' <<'CSV'
0,up
10,down
20,up
30,down
40,down
60,up
CSV

# Never recovered inside the window: count to the last sample and NOT one second further.
# Inventing a recovery time would be the same class of lie the scorer exists to remove.
assert_score 'coupure jamais refermée : comptée jusqu au dernier échantillon' \
    '15 15 3 15 10' <<'CSV'
0,up
5,down
15,down
CSV

# Resolution ties break UPWARD (deltas [5,10] -> 10, not 5). An instrument must never
# advertise itself as finer than it is: the coarser half is the honest claim.
assert_score 'la résolution arrondit vers le HAUT en cas d égalité' \
    '0 0 3 15 10' <<'CSV'
0,up
5,up
15,up
CSV

# The file starts mid-outage: there is no known-good instant to open from, so the earliest
# evidence is the first sample. Anything else would be extrapolation.
assert_score 'fichier commençant en pleine coupure : borné par la 1re preuve' \
    '10 10 3 10 5' <<'CSV'
0,down
5,down
10,up
CSV

# Robustness: a truncated or malformed line must be skipped, never crash the promote's
# reporting path and never be silently counted as `up`.
assert_score 'les lignes malformées sont ignorées, pas comptées comme up' \
    '0 0 2 1 1' <<'CSV'
10,up
oops
11,up
12,
,down
99,sideways
CSV

# An empty file yields zeros AND n=0 — the caller prints "NOT MEASURED" on n<5, so a
# vacuous zero can never read as a green result.
assert_score 'fichier vide : zéros et n=0, pour que le vide ne passe pas pour du vert' \
    '0 0 0 0 0' </dev/null

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
