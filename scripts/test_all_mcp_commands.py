import socket
import json
import sys

SOCK_PATH = "/tmp/axon-v2.sock"

def read_response(client):
    response_data = b""
    while True:
        chunk = client.recv(8192)
        if not chunk: break
        response_data += chunk
        if b"\n" in chunk: break
    
    responses = response_data.decode().strip().split("\n")
    # Return the last valid JSON response (ignoring intermediate logging or ready states)
    for resp in reversed(responses):
        try:
            parsed = json.loads(resp)
            if "jsonrpc" in parsed:
                return parsed
        except json.JSONDecodeError:
            continue
    return None

def test_tool(client, method_id, tool_name, arguments):
    request = {
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments
        },
        "id": method_id
    }
    
    print(f"\n[{method_id}] 📤 Testing {tool_name}...")
    client.sendall((json.dumps(request) + "\n").encode())
    
    response = read_response(client)
    
    if not response:
        print(f"❌ Failed to parse response.")
        return False
        
    if "error" in response and response["error"] is not None:
        print(f"⚠️ Tool returned an error state: {response['error']}")
        return False
        
    if "result" in response:
        content = str(response["result"])
        preview = content[:150] + "..." if len(content) > 150 else content
        print(f"✅ Success. Output preview: {preview}")
        return True
        
    print(f"❌ Unexpected response format: {response}")
    return False

def verify_all_mcp_commands():
    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.connect(SOCK_PATH)
        client.settimeout(10.0)
    except Exception as e:
        print(f"❌ Cannot connect to socket: {e}")
        return False

    # Wait for ready signal
    while True:
        line = client.recv(1024).decode()
        if "Axon Bridge Ready" in line:
            break
            
    print("🔌 Socket connected. Commencing exhaustive test of all 13 tools.\n")

    tools_to_test = [
        ("axon_query", {"query": "Elixir Supervisor", "project": "axon"}),
        ("axon_inspect", {"symbol": "axon", "project": "axon"}),
        ("axon_audit", {"project": "axon"}),
        ("axon_impact", {"symbol": "axon", "depth": 1}),
        ("axon_health", {"project": "axon"}),
        ("axon_diff", {"diff_content": "+ def new_func() do end"}),
        ("axon_batch", {"calls": [{"tool": "axon_query", "args": {"query": "test"}}]}),
        ("axon_semantic_clones", {"symbol": "axon"}),
        ("axon_architectural_drift", {"source_layer": "ui", "target_layer": "db"}),
        ("axon_bidi_trace", {"symbol": "axon", "depth": 1}),
        ("axon_api_break_check", {"symbol": "axon"}),
        ("axon_simulate_mutation", {"symbol": "axon", "depth": 1}),
        ("axon_cypher", {"cypher": "MATCH (n) RETURN count(n) as count"})
    ]

    success_count = 0
    
    for i, (tool, args) in enumerate(tools_to_test, 1):
        if test_tool(client, i, tool, args):
            success_count += 1

    client.close()
    
    print(f"\n========================================")
    print(f"🏁 Test Complete: {success_count}/{len(tools_to_test)} commands succeeded.")
    print(f"========================================")
    
    return success_count == len(tools_to_test)

if __name__ == "__main__":
    success = verify_all_mcp_commands()
    sys.exit(0 if success else 1)
