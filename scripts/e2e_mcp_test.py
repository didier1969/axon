import subprocess
import json
import sys
import time
import os

def run_e2e_test():
    proxy_script = os.path.join(os.path.dirname(__file__), "mcp-stdio-proxy.py")
    
    print(f"🔍 Running End-to-End MCP Verification on: {proxy_script}")
    
    if not os.path.exists(proxy_script):
        print(f"❌ Error: Proxy script not found at {proxy_script}")
        return False

    try:
        # Spawn the proxy exactly as the AI client (Claude/Gemini) would
        process = subprocess.Popen(
            [sys.executable, proxy_script],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1 # Line buffered
        )
        
        request = {
            "jsonrpc": "2.0",
            "method": "tools/list",
            "params": {},
            "id": 1
        }
        
        # Send request
        process.stdin.write(json.dumps(request) + "\n")
        process.stdin.flush()
        
        # Read response
        start_time = time.time()
        while True:
            if time.time() - start_time > 5:
                print("❌ Error: Timeout waiting for MCP proxy response")
                process.terminate()
                return False
                
            line = process.stdout.readline()
            if not line:
                # Check if process crashed
                ret = process.poll()
                if ret is not None:
                    stderr_output = process.stderr.read()
                    print(f"❌ Error: MCP proxy crashed with exit code {ret}")
                    print(f"Stderr: {stderr_output}")
                    return False
                time.sleep(0.1)
                continue
                
            try:
                response = json.loads(line.strip())
                # Ignore Axon Bridge specific telemetry events
                if "SystemReady" in response or "ScanComplete" in response or "FileIndexed" in response or "ProjectScanStarted" in response:
                    continue
                
                # Ignore MCP notifications (no ID)
                if "jsonrpc" in response and "id" not in response:
                    print(f"ℹ️ Received notification: {response.get('method')}")
                    continue
                    
                if "result" in response and "tools" in response["result"]:
                    tools = [t["name"] for t in response["result"]["tools"]]
                    print(f"✅ E2E Verification Success! Proxy returned {len(tools)} tools: {', '.join(tools)}")
                    process.terminate()
                    return True
                elif "error" in response:
                    print(f"❌ Error: Proxy returned MCP error: {response}")
                    process.terminate()
                    return False
                else:
                    print(f"❌ Error: Unexpected response format: {line}")
                    process.terminate()
                    return False
            except json.JSONDecodeError:
                # Might be a log message mixed in stdout, ignore and continue
                continue

    except Exception as e:
        print(f"❌ Exception during E2E verification: {e}")
        return False

if __name__ == "__main__":
    success = run_e2e_test()
    sys.exit(0 if success else 1)
