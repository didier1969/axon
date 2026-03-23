import socket
import json

# Send to Telemetry
try:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect('/tmp/axon-telemetry.sock')
    s.recv(1024)
    req = 'PARSE_FILE ' + json.dumps({'path': '/tmp/test_dummy_graph.ex', 'lane': 'fast'}) + '\n'
    s.sendall(req.encode())
    print('Telemetry Response:', s.recv(4096).decode().strip())
    s.close()
except Exception as e:
    print('Telemetry Error:', e)

# Send to MCP to check symbols
try:
    m = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    m.connect('/tmp/axon-mcp.sock')
    query = 'MATCH (f:File {path: "/tmp/test_dummy_graph.ex"})-[:CONTAINS]->(s:Symbol) RETURN s.name'
    req2 = {'jsonrpc': '2.0', 'method': 'tools/call', 'params': {'name': 'axon_cypher', 'arguments': {'cypher': query}}, 'id': 1}
    m.sendall((json.dumps(req2) + '\n').encode())
    print('MCP Query Response:', m.recv(4096).decode().strip())
    m.close()
except Exception as e:
    print('MCP Error:', e)
