import subprocess
import json
import sys
import time

def test():
    proxy = subprocess.Popen(
        ["python3", "/home/dstadel/projects/axon/scripts/mcp-stdio-proxy.py"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1
    )
    
    init_req = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "test-client", "version": "1.0.0"}
        }
    }
    
    print(f"Sending: {json.dumps(init_req)}")
    proxy.stdin.write(json.dumps(init_req) + "\n")
    proxy.stdin.flush()
    
    print("Waiting for response...")
    start = time.time()
    
    while time.time() - start < 5:
        line = proxy.stdout.readline()
        if line:
            print(f"STDOUT: {line.strip()}")
            return
        
        ret = proxy.poll()
        if ret is not None:
             print(f"Proxy exited with {ret}")
             print(f"STDERR: {proxy.stderr.read()}")
             return
            
        time.sleep(0.1)
        
    print("TIMEOUT")
    proxy.terminate()

test()
