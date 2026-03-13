import socket
import json
import time
import sys

def run_demo():
    sock_path = "/tmp/axon-v2.sock"
    
    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.connect(sock_path)
    except Exception as e:
        print(f"Error connecting to UDS {sock_path}: {e}")
        sys.exit(1)

    # Read the initial welcome message from the server
    try:
        welcome = client.recv(4096).decode('utf-8')
        print(f"[Core] Welcome Payload:\n{welcome.strip()}")
    except Exception as e:
        print(f"Error reading welcome: {e}")

    # 1. Test tools/list
    list_req = {
        "jsonrpc": "2.0",
        "method": "tools/list",
        "id": 1
    }
    client.sendall((json.dumps(list_req) + "\n").encode('utf-8'))
    
    time.sleep(0.5)
    list_res = client.recv(4096).decode('utf-8')
    print("\n[MCP] tools/list Response:")
    try:
        parsed = json.loads(list_res)
        tools = parsed.get("result", {}).get("tools", [])
        for t in tools:
            print(f"  - {t['name']}: {t['description']}")
    except:
        print(list_res)

    # 2. Test tools/call (axon_health)
    call_req = {
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_health",
            "arguments": {
                "project": "demo_project"
            }
        },
        "id": 2
    }
    client.sendall((json.dumps(call_req) + "\n").encode('utf-8'))
    
    time.sleep(0.5)
    call_res = client.recv(4096).decode('utf-8')
    print("\n[MCP] tools/call (axon_health) Response:")
    try:
        parsed = json.loads(call_res)
        content = parsed.get("result", {}).get("content", [])[0].get("text", "")
        print(f"  {content}")
    except:
        print(call_res)

    client.close()

if __name__ == "__main__":
    run_demo()
