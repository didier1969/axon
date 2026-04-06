#!/usr/bin/env python3
import sys
import os
import json
import urllib.request
import urllib.error

def main():
    while True:
        line = sys.stdin.readline()
        if not line:
            break
        line = line.strip()
        if not line:
            continue
            
        try:
            req_data = json.loads(line)
        except json.JSONDecodeError:
            continue
            
        req_id = req_data.get("id", 1)
        
        try:
            req = urllib.request.Request(
                "http://127.0.0.1:44129/mcp", 
                data=line.encode('utf-8'),
                headers={
                    'Content-Type': 'application/json',
                    'X-Workspace-Path': os.getcwd()
                },
                method='POST'
            )
            with urllib.request.urlopen(req, timeout=10) as response:
                result = response.read().decode('utf-8')
                if result.strip():
                    sys.stdout.write(result.strip() + "\n")
                    sys.stdout.flush()
        except urllib.error.URLError as e:
            err_msg = json.dumps({
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {
                    "code": -32000,
                    "message": f"Axon Backend is unavailable or timed out: {e}"
                }
            })
            sys.stdout.write(err_msg + "\n")
            sys.stdout.flush()
        except Exception as e:
            err_msg = json.dumps({
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {
                    "code": -32603,
                    "message": f"Internal proxy error: {e}"
                }
            })
            sys.stdout.write(err_msg + "\n")
            sys.stdout.flush()

if __name__ == "__main__":
    main()
