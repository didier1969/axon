import socket
import json
import sys
import time

def verify_mcp():
    sock_path = "/tmp/axon-v2.sock"
    print(f"🔍 Connecting to Axon Bridge at {sock_path}...")
    
    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.connect(sock_path)
        print("✅ Connected.")
    except Exception as e:
        print(f"❌ Error: {e}")
        return False

    client.settimeout(5.0)
    
    try:
        # 1. Wait for "Axon Bridge Ready"
        while True:
            line = client.recv(1024).decode()
            if "Axon Bridge Ready" in line:
                print("📥 Received: Axon Bridge Ready")
                break
            if not line: break

        # 2. Send MCP tools/list request
        request = {
            "jsonrpc": "2.0",
            "method": "tools/list",
            "params": {},
            "id": 1
        }
        
        print(f"📤 Sending MCP Request: tools/list")
        client.sendall((json.dumps(request) + "\n").encode())

        # 3. Receive tools/list response
        response_data = b""
        while True:
            chunk = client.recv(4096)
            if not chunk: break
            response_data += chunk
            if b"\n" in response_data: break
        
        response_str = response_data.decode().strip()
        response = json.loads(response_str)
        if "result" in response and "tools" in response["result"]:
            tools = [t["name"] for t in response["result"]["tools"]]
            print(f"✅ Found {len(tools)} MCP Tools: {', '.join(tools)}")
        else:
            print(f"❌ Invalid MCP response format")
            return False
            
        # 4. Perform a real audit via MCP
        audit_request = {
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {
                "name": "axon_audit",
                "arguments": {
                    "project": "axon"
                }
            },
            "id": 2
        }
        
        print(f"\n📤 Sending MCP Request: axon_audit(project='axon')")
        client.sendall((json.dumps(audit_request) + "\n").encode())
        
        response_data = b""
        while True:
            chunk = client.recv(8192)
            if not chunk: break
            response_data += chunk
            if b"\n" in response_data: break
        
        response_str = response_data.decode().strip()
        print(f"📥 Received MCP Audit Response: {response_str[:300]}...")
        
        response = json.loads(response_str)
        if response.get("error") is None and "result" in response:
            print("✅ Audit Tool successfully executed.")
            return True
        else:
            print("❌ Audit Tool execution failed.")
            return False

    except Exception as e:
        print(f"❌ Error during verification: {e}")
        return False
    finally:
        client.close()

if __name__ == "__main__":
    success = verify_mcp()
    sys.exit(0 if success else 1)
