import socket
import json
import time
import subprocess
import os

def send_cypher(query):
    sock_path = "/tmp/axon-telemetry.sock"
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        client.connect(sock_path)
        client.recv(1024)
        client.sendall(f"EXECUTE_CYPHER {query}\n".encode())
        time.sleep(0.1) 
    except Exception as e:
        print(f"Error: {e}")
    finally:
        client.close()

def check_vision():
    # Use MCP to check vision count
    import requests
    try:
        resp = requests.post("http://localhost:44129/mcp", json={
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "axon_cypher",
                "arguments": {"cypher": "MATCH (v:Vision) RETURN count(v)"}
            }
        })
        data = resp.json()
        # Parse the string result "[["Int64(1)"]]"
        text = data["result"]["content"][0]["text"]
        return "Int64(1)" in text
    except Exception as e:
        print(f"Check failed: {e}")
        return False

print("Step 1: Populating SOLL...")
send_cypher("MERGE (v:Vision {title: 'SOLL Isolation Test'})")
time.sleep(1)

if not check_vision():
    print("FAILED: Vision not created.")
    exit(1)

print("Step 2: Resetting Axon...")
# Simulate 'y' input to reset
process = subprocess.Popen(["bin/axon", "reset"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
stdout, stderr = process.communicate(input="y\n")
print(stdout)

print("Step 3: Waiting for restart...")
time.sleep(15) # Wait for reboot

print("Step 4: Verifying SOLL loss (Current behavior)...")
if check_vision():
    print("WARNING: Vision still exists! (Unexpected if reset worked)")
else:
    print("SUCCESS: SOLL is lost as expected. SOLL Isolation is needed.")
