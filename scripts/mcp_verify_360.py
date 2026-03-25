#!/usr/bin/env python3
import subprocess
import json
import time
import sys

def test_360():
    try:
        proc = subprocess.Popen(
            ["/home/dstadel/projects/axon/bin/axon-mcp-tunnel"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True
        )
    except Exception as e:
        print(f"❌ ÉCHEC FATAL : Impossible de lancer le tunnel MCP ({e})")
        return False
        
    init_notif = {
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    }
    proc.stdin.write(json.dumps(init_notif) + "\n")
    proc.stdin.flush()
    time.sleep(0.2)
    
    if proc.poll() is not None:
        print("❌ ÉCHEC FATAL : Le tunnel a crashé suite à la notification d'initialisation.")
        return False
        
    req_debug = {
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_debug",
            "arguments": {}
        },
        "id": 42
    }
    
    proc.stdin.write(json.dumps(req_debug) + "\n")
    proc.stdin.flush()
    
    import select
    ready, _, _ = select.select([proc.stdout], [], [], 2.0)
    
    if not ready:
        print("❌ ÉCHEC FATAL : Timeout (2s). Le serveur ne répond pas.")
        proc.terminate()
        return False
        
    response_str = proc.stdout.readline().strip()
    try:
        response = json.loads(response_str)
        if "result" in response and "content" in response["result"]:
            print("✅ AUDIT 360° RÉUSSI : Le système est stable de bout en bout.")
            proc.terminate()
            return True
        else:
            print(f"❌ ÉCHEC FATAL : Format de réponse invalide : {response_str}")
            proc.terminate()
            return False
    except json.JSONDecodeError:
        print(f"❌ ÉCHEC FATAL : Réponse non-JSON : {response_str}")
        proc.terminate()
        return False

if __name__ == "__main__":
    success = test_360()
    sys.exit(0 if success else 1)
