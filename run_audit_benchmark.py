import socket
import json
import time

SOCK_PATH = "/tmp/axon-v2.sock"

def test_audit(project_name):
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.connect(SOCK_PATH)
    
    # Read SystemReady message
    client.recv(1024)

    request = {
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_audit",
            "arguments": {"project": project_name}
        },
        "id": 1
    }
    
    start_time = time.time()
    client.send((json.dumps(request) + "\n").encode())
    
    response_data = b""
    while True:
        chunk = client.recv(8192)
        if not chunk: break
        response_data += chunk
        if b"\n" in chunk: break
        
    end_time = time.time()
    duration_ms = (end_time - start_time) * 1000
    
    try:
        resp = json.loads(response_data.decode())
        if resp.get("result"):
            result_text = resp.get("result", {}).get("content", [{}])[0].get("text", "")
            print(f"Audit for '{project_name}' completed in {duration_ms:.2f} ms")
            
            # Extract score if possible
            score_line = [line for line in result_text.split('\n') if "Score" in line]
            if score_line:
                print(f"Result: {score_line[0]}")
            else:
                print(f"Result (truncated): {result_text[:100]}")
        else:
            print(f"Error: {resp}")
    except Exception as e:
        print(f"Error parsing response for {project_name}: {e}")
        print(f"Raw response: {response_data}")
        
    client.close()

if __name__ == "__main__":
    print("Starting Audit Benchmarks...")
    test_audit("SwarmEx")
    test_audit("axon")
    test_audit("MetaGPT")
