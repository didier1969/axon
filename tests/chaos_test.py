import json
import subprocess
import time

def chaos_test():
    print("--- 🌪️ Starting Chaos Test on Pod B (Parser Slave) ---")
    cmd = ["uv", "run", "python", "src/axon/bridge/worker.py"]
    slave = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

    # Cas 1 : Commande JSON invalide
    print("Test 1: Malformed JSON...")
    slave.stdin.write("INVALID JSON\n")
    slave.stdin.flush()
    # Le worker doit loguer une erreur sur stderr mais RESTER ALERTE.
    
    # Cas 2 : Commande inconnue
    print("Test 2: Unknown command...")
    slave.stdin.write(json.dumps({"command": "DELETE_ALL_DATABASE"}) + "\n")
    slave.stdin.flush()
    res = json.loads(slave.stdout.readline())
    assert res["status"] == "error"
    print(f"  Result: {res['status']} (Message: {res['message']})")

    # Cas 3 : Parsing de code "poubelle"
    print("Test 3: Garbage code parsing...")
    slave.stdin.write(json.dumps({"command": "parse", "path": "garbage.py", "content": "1 %$# @! INVALID"}) + "\n")
    slave.stdin.flush()
    res = json.loads(slave.stdout.readline())
    # Tree-sitter est robuste, il renvoie souvent une liste vide ou partielle mais ne crashe pas.
    assert res["status"] == "ok"
    print(f"  Result: {res['status']} (Symbols found: {len(res['data']['symbols'])})")

    slave.terminate()
    print("--- ✅ Chaos Test Passed: Slave is indestructible ---")

if __name__ == "__main__":
    chaos_test()
