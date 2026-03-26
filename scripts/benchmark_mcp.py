#!/usr/bin/env python3
import json
import time
import sys
import urllib.request
import urllib.error

HTTP_ENDPOINT = "http://127.0.0.1:44129/mcp"

def send_mcp_request(method_id, tool_name, arguments):
    request_data = {
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments
        },
        "id": method_id
    }
    
    data = json.dumps(request_data).encode('utf-8')
    req = urllib.request.Request(HTTP_ENDPOINT, data=data, headers={'Content-Type': 'application/json'})
    
    start_time = time.time()
    try:
        with urllib.request.urlopen(req, timeout=30.0) as response:
            result_data = response.read()
            end_time = time.time()
            parsed_response = json.loads(result_data.decode('utf-8'))
            return parsed_response, (end_time - start_time) * 1000  # Return in ms
    except urllib.error.URLError as e:
        end_time = time.time()
        print(f"❌ Error during HTTP request for {tool_name}: {e}")
        return None, (end_time - start_time) * 1000
    except TimeoutError:
        end_time = time.time()
        print(f"❌ Timeout during HTTP request for {tool_name}")
        return None, (end_time - start_time) * 1000
    except json.JSONDecodeError:
        end_time = time.time()
        print(f"❌ Error decoding JSON response for {tool_name}")
        return None, (end_time - start_time) * 1000

def benchmark_all():
    print("=================================================")
    print("🚀 AXON CORE V2 - MCP BENCHMARK SUITE")
    print("=================================================")
    
    # 1. Test Tools
    tools_to_test = [
        ("axon_refine_lattice", {}),
        ("axon_fs_read", {"uri": "src/axon-core/Cargo.toml"}),
        ("axon_query", {"query": "Elixir Supervisor", "project": "axon"}),
        ("axon_inspect", {"symbol": "axon", "project": "axon"}),
        ("axon_audit", {"project": "axon"}),
        ("axon_impact", {"symbol": "axon", "depth": 1}),
        ("axon_health", {"project": "axon"}),
        ("axon_diff", {"diff_content": "--- a/src/main.rs\n+++ b/src/main.rs\n+fn new_func() {}"}),
        ("axon_batch", {"calls": [{"tool": "axon_query", "args": {"query": "Elixir Supervisor", "project": "axon"}}]}),
        ("axon_cypher", {"cypher": "MATCH (n) RETURN count(n) as count"}),
        ("axon_semantic_clones", {"symbol": "axon"}),
        ("axon_architectural_drift", {"source_layer": "ui", "target_layer": "db"}),
        ("axon_bidi_trace", {"symbol": "axon", "depth": 1}),
        ("axon_api_break_check", {"symbol": "axon"}),
        ("axon_simulate_mutation", {"symbol": "axon", "depth": 1}),
        ("axon_debug", {}),
    ]

    success_count = 0
    total_time = 0
    
    print(f"{'Tool Name':<25} | {'Status':<10} | {'Latency (ms)':<15}")
    print("-" * 55)

    for i, (tool, args) in enumerate(tools_to_test, 1):
        response, latency = send_mcp_request(i, tool, args)
        
        status = "❌ FAIL"
        if response:
            if "error" in response:
                status = "⚠️ ERROR"
            elif "result" in response:
                status = "✅ OK"
                success_count += 1
                total_time += latency
        
        print(f"{tool:<25} | {status:<10} | {latency:.2f} ms")

    print("=================================================")
    print(f"🏁 Benchmark Complete: {success_count}/{len(tools_to_test)} commands succeeded.")
    if success_count > 0:
        print(f"⏱️ Average Latency (Successful): {total_time / success_count:.2f} ms")
    print("=================================================")
    
    return success_count == len(tools_to_test)

if __name__ == "__main__":
    success = benchmark_all()
    sys.exit(0 if success else 1)
