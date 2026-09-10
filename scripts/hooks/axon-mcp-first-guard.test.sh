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

# ---------------------------------------------------------------------------
# REQ-AXO-902624 (friction MRG 430) — le garde ne doit bloquer que si Axon peut
# REELLEMENT repondre a une recherche de CODE sur ce projet.
#
# Un serveur jetable rend la reponse de `status` avec le drapeau voulu. Sans les
# deux cas, le correctif ne prouverait rien : le premier (visible=true) est le
# CONTROLE NEGATIF — s'il n'aboutissait pas a un blocage, le second ne dirait pas
# que c'est le drapeau qui decide.
# ---------------------------------------------------------------------------
faux_serveur() {  # $1 = true|false  -> imprime le port sur stdout
  python3 - "$1" <<'PYEOF' &
import http.server, json, socket, sys, threading
visible = sys.argv[1] == "true"
corps = json.dumps({"jsonrpc":"2.0","id":1,"result":{"structuredContent":{
    "availability":{"advanced_indexed_surfaces_visible": visible}}}}).encode()
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0) or 0))
        self.send_response(200); self.send_header("Content-Type","application/json")
        self.send_header("Content-Length", str(len(corps))); self.end_headers()
        self.wfile.write(corps)
    def log_message(self, *a): pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
threading.Thread(target=srv.serve_forever, daemon=True).start()
import time; time.sleep(20)
PYEOF
  :
}

for drapeau in true false; do
  # attendu : visible=true -> BLOQUE (2) ; visible=false -> laisse passer (0)
  [[ "$drapeau" == "true" ]] && attendu=2 || attendu=0
  tmp=$(mktemp)
  faux_serveur "$drapeau" > "$tmp"
  pid_srv=$!
  port=""
  for _ in $(seq 1 40); do port=$(head -1 "$tmp" 2>/dev/null); [[ -n "$port" ]] && break; sleep 0.1; done
  if [[ -n "$port" ]]; then
    run "$attendu" '"grep -r foo src/"' "AXON_MCP_URL=http://127.0.0.1:$port/mcp"
  else
    FAIL=$((FAIL+1)); printf '  FAIL  le faux serveur (visible=%s) n a pas demarre\n' "$drapeau"
  fi
  kill "$pid_srv" 2>/dev/null || true
  rm -f "$tmp"
done

# ---------------------------------------------------------------------------
# REQ-AXO-902406 — Le garde MCP-first fail-open quand le code-intel du projet
# est VIDE (aucun symbole) ou non mesuré, évitant d'interdire l'outil qui répond
# au profit d'outils aveugles sur le projet.
# ---------------------------------------------------------------------------
faux_serveur_payload() {
  python3 - "$1" <<'PYEOF' &
import http.server, json, sys, threading
payload = json.loads(sys.argv[1])
corps = json.dumps({"jsonrpc":"2.0","id":1,"result":payload}).encode()
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0) or 0))
        self.send_response(200); self.send_header("Content-Type","application/json")
        self.send_header("Content-Length", str(len(corps))); self.end_headers()
        self.wfile.write(corps)
    def log_message(self, *a): pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
threading.Thread(target=srv.serve_forever, daemon=True).start()
import time; time.sleep(20)
PYEOF
  :
}

run_with_payload() {
  local expected="$1"
  local payload="$2"
  local desc="$3"
  local tmp
  tmp=$(mktemp)
  faux_serveur_payload "$payload" > "$tmp"
  local pid_srv=$!
  local port=""
  for _ in $(seq 1 40); do port=$(head -1 "$tmp" 2>/dev/null); [[ -n "$port" ]] && break; sleep 0.1; done
  if [[ -n "$port" ]]; then
    run "$expected" '"grep -r foo src/"' "AXON_MCP_URL=http://127.0.0.1:$port/mcp"
  else
    FAIL=$((FAIL+1)); printf '  FAIL  serveur (%s) n a pas demarre\n' "$desc"
  fi
  kill "$pid_srv" 2>/dev/null || true
  rm -f "$tmp"
}

# 1. visible=true MAIS code_intel VIDE dans le texte markdown -> laisse passer (0)
run_with_payload 0 '{"structuredContent":{"availability":{"advanced_indexed_surfaces_visible":true}},"content":[{"type":"text","text":"**Code-intel:** VIDE — DVM ne compte AUCUN fichier porteur de symboles."}]}' "code-intel texte vide"

# 2. visible=true MAIS structured code_intel status=empty -> laisse passer (0)
run_with_payload 0 '{"structuredContent":{"availability":{"advanced_indexed_surfaces_visible":true},"code_intel":{"status":"empty","completed_files":0,"total_files":0}}}' "code-intel structured empty"

# 3. visible=true ET code-intel LIVE -> BLOQUE (2)
run_with_payload 2 '{"structuredContent":{"availability":{"advanced_indexed_surfaces_visible":true},"code_intel":{"status":"live","completed_files":850,"total_files":900}},"content":[{"type":"text","text":"**Code-intel:** LIVE — AXO 850/900 fichiers"}]}' "code-intel live"

echo "----"; echo "PASS=$PASS FAIL=$FAIL"; [[ "$FAIL" -eq 0 ]]

