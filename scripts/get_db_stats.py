import socket
import json
import re
from collections import defaultdict

def query_mcp(cypher):
    req = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "axon_cypher", "arguments": {"cypher": cypher}}}
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.connect("/tmp/axon-v2.sock")
    client.sendall((json.dumps(req) + "\n").encode())
    client.settimeout(5.0)
    buffer = b""
    while True:
        data = client.recv(4096)
        if not data: break
        buffer += data
        while b"\n" in buffer:
            line, buffer = buffer.split(b"\n", 1)
            text = line.decode().strip()
            if text.startswith("{") and "jsonrpc" in text and "id" in text and "result" in text:
                return json.loads(text)
    return None

def main():
    # Get all files
    res = query_mcp("MATCH (f:File) RETURN f.path")
    if not res or "result" not in res or not res["result"]["content"]:
        print("Failed to get files.")
        return
    
    text = res["result"]["content"][0]["text"]
    
    projects = defaultdict(int)
    
    # Text looks like: [["Node(NodeVal { id: InternalID { offset: 0, table_id: 0 }, label: \"File\", properties: [(\"path\", String(\"/home/dstadel/projects/trader-elixir-v2/lib/trader_elixir_v2/portfolio/copula.ex\"))] })"]]
    
    # We can use regex to find all String("/home/dstadel/projects/... ")
    paths = re.findall(r'String\(\\\"/home/dstadel/projects/([^/]+)/.*?\\\"\)', text)
    
    for p in paths:
        projects[p] += 1
        
    print("Project | Files Indexed")
    print("---|---")
    for proj, count in sorted(projects.items(), key=lambda x: x[1], reverse=True):
        print(f"{proj} | {count}")

if __name__ == "__main__":
    main()
