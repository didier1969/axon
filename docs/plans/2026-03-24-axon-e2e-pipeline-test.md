# Axon End-to-End Pipeline Validation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Establish an infallible End-to-End (E2E) test that mathematically proves the entire Axon architecture (Elixir Watcher -> UDS Bridge -> Rust Parser -> KuzuDB -> MCP Query) functions correctly for a given language, eliminating silent parsing failures.

**Architecture:** A standalone Python script that acts as both the filesystem trigger and the AI client. It creates a dummy Elixir file, triggers ingestion via the telemetry socket, awaits the success response, and then connects to the MCP socket to execute a Cypher query verifying the exact number of extracted symbols in the graph.

**Tech Stack:** Python 3 (socket, json, time, os)

---

### Task 1: Create the E2E Test Script Structure

**Files:**
- Create: `tests/e2e_pipeline_test.py`

**Step 1: Write the failing test**

```python
# We are writing the test script itself, so the script IS the test.
# We will create a skeleton that will fail if the pipeline doesn't work.
import socket
import json
import time
import os
import sys

def test_pipeline():
    print("Testing pipeline...")
    # This will fail because we haven't implemented the logic yet
    assert False, "Pipeline test not implemented"

if __name__ == "__main__":
    test_pipeline()
```

**Step 2: Run test to verify it fails**

Run: `python3 tests/e2e_pipeline_test.py`
Expected: AssertionError: Pipeline test not implemented

**Step 3: Write minimal implementation (The Payload Trigger)**

```python
import socket
import json
import time
import os
import sys

def send_parse_request(path):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect('/tmp/axon-telemetry.sock')
    # Read the welcome message
    s.recv(1024)
    # The new Titan protocol requires the 'lane' parameter
    req = 'PARSE_FILE ' + json.dumps({'path': path, 'lane': 'fast'}) + '\n'
    s.sendall(req.encode())
    
    # Wait for the response
    s.settimeout(10.0)
    response = s.recv(4096).decode()
    s.close()
    return response

def test_pipeline():
    test_file_path = "/tmp/axon_test_dummy.ex"
    with open(test_file_path, "w") as f:
        f.write("defmodule DummyTest do\n  def hello(), do: :world\nend")
        
    print("1. Sending file to Telemetry Socket...")
    response = send_parse_request(test_file_path)
    print(f"Response: {response}")
    assert "FileIndexed" in response, "Failed to get indexing confirmation"
    
if __name__ == "__main__":
    test_pipeline()
```

**Step 4: Run test to verify it passes (Partial)**

Run: `python3 tests/e2e_pipeline_test.py`
Expected: PASS (Prints FileIndexed response)

**Step 5: Commit**

```bash
git add tests/e2e_pipeline_test.py
git commit -m "test(e2e): create initial telemetry trigger test"
```

---

### Task 2: Implement MCP Cypher Validation

**Files:**
- Modify: `tests/e2e_pipeline_test.py`

**Step 1: Write the failing test (Add Cypher check)**

```python
# Add to test_pipeline():
    print("2. Querying MCP Socket for AST Symbols...")
    mcp_s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    mcp_s.connect('/tmp/axon-mcp.sock')
    
    query = f"MATCH (f:File)-[:CONTAINS]->(s:Symbol) WHERE f.path = '{test_file_path}' RETURN count(s)"
    req = {
        'jsonrpc': '2.0', 
        'method': 'tools/call', 
        'params': {'name': 'axon_cypher', 'arguments': {'cypher': query}}, 
        'id': 1
    }
    mcp_s.sendall((json.dumps(req) + '\n').encode())
    
    mcp_response_raw = mcp_s.recv(4096).decode()
    mcp_s.close()
    
    mcp_response = json.loads(mcp_response_raw)
    result_text = mcp_response['result']['content'][0]['text']
    print(f"MCP Cypher Result: {result_text}")
    
    # We expect 2 symbols: the module and the function
    assert 'Int64(2)' in result_text, f"Expected 2 symbols, got {result_text}"
```

**Step 2: Run test to verify it fails (if graph insertion is broken)**

Run: `python3 tests/e2e_pipeline_test.py`
Expected: Might fail if the previous graph insertion bugs are still lingering, or PASS if the architecture is truly sound.

**Step 3: Write minimal implementation (Cleanup & Robustness)**

```python
# Add cleanup to the end of the test
    os.remove(test_file_path)
    print("✅ E2E Pipeline Validation Successful.")
```

**Step 4: Run test to verify it passes**

Run: `python3 tests/e2e_pipeline_test.py`
Expected: PASS and prints Success message.

**Step 5: Commit**

```bash
git add tests/e2e_pipeline_test.py
git commit -m "test(e2e): implement mcp cypher validation for AST extraction"
```
