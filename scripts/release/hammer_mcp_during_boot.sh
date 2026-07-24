#!/usr/bin/env bash
# REQ-AXO-902233 — hammer-test : martèle l'MCP d'un brain pendant son boot/cutover
# et mesure la fenêtre d'indisponibilité CLIENT (vrais appels JSON-RPC, pas /readyz).
#
# Usage: hammer_mcp_during_boot.sh <brain_port> <out_log> [duration_s]
# Chaque itération (~1s) : /readyz (code) + vrai tools/call status (Content-Type json,
# latence, PASS si result.jsonrpc / FAIL sinon). Trace le 1er readyz-green, le 1er
# MCP-pass, et le GAP = la vraie coupure client.
set -uo pipefail
PORT="${1:?brain_port required}"
OUT="${2:?out_log required}"
DUR="${3:-360}"
URL="http://127.0.0.1:${PORT}"
: > "$OUT"
readyz_green=""; mcp_first_pass=""; mcp_fail_count=0; mcp_pass_count=0
start=$(date +%s)
i=0
while :; do
  now=$(date +%s); el=$((now-start))
  [ "$el" -ge "$DUR" ] && break
  i=$((i+1))
  rz=$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "$URL/readyz" 2>/dev/null || echo "refused")
  # vrai appel MCP client : mesure latence + verdict
  t0=$(date +%s.%N)
  body=$(curl -s --max-time 5 -H 'Content-Type: application/json' "$URL/mcp" \
    -d '{"jsonrpc":"2.0","method":"tools/call","id":1,"params":{"name":"status","arguments":{"mode":"brief"}}}' 2>/dev/null)
  t1=$(date +%s.%N)
  lat=$(awk "BEGIN{printf \"%.2f\", $t1-$t0}")
  if printf '%s' "$body" | grep -q '"result"'; then
    verdict="PASS"; mcp_pass_count=$((mcp_pass_count+1))
    [ -z "$mcp_first_pass" ] && { mcp_first_pass=$el; }
  else
    verdict="FAIL"; mcp_fail_count=$((mcp_fail_count+1))
  fi
  [ -z "$readyz_green" ] && [ "$rz" = "200" ] && readyz_green=$el
  printf 't+%03ds readyz=%s mcp=%s lat=%ss\n' "$el" "$rz" "$verdict" "$lat" >> "$OUT"
  # arrêt anticipé : 5 PASS consécutifs après le premier pass = brain stable
  if [ -n "$mcp_first_pass" ] && [ "$mcp_pass_count" -ge 5 ]; then
    tail_fails=$(tail -5 "$OUT" | grep -c 'mcp=FAIL')
    [ "$tail_fails" -eq 0 ] && break
  fi
  sleep 1
done
gap="n/a"
[ -n "$readyz_green" ] && [ -n "$mcp_first_pass" ] && gap=$((mcp_first_pass-readyz_green))
{
  echo "=== RÉSUMÉ hammer (port $PORT) ==="
  echo "readyz_green   = t+${readyz_green:-never}s"
  echo "mcp_first_pass = t+${mcp_first_pass:-never}s"
  echo "GAP readyz-vert -> mcp-servant = ${gap}s  (= fenêtre où /readyz ment)"
  echo "mcp FAIL total = ${mcp_fail_count}   mcp PASS total = ${mcp_pass_count}"
} >> "$OUT"
