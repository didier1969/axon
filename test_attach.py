import socket
import time
import json

def test_query(q):
    sock_path = "/tmp/axon-telemetry.sock"
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(2.0) # 2 seconds timeout
    try:
        client.connect(sock_path)
        client.recv(1024) # welcome
        client.sendall(f"EXECUTE_CYPHER {q}\n".encode())
        res = client.recv(4096).decode()
        print(f"Query: {q}\nResult: {res}\n")
        return res
    except socket.timeout:
        print(f"Query: {q}\nResult: TIMEOUT (2s)\n")
        return "TIMEOUT"
    except Exception as e:
        print(f"Query: {q}\nError: {e}\n")
        return str(e)
    finally:
        client.close()

# Start with version check
test_query("CALL version() RETURN *")

# Try various ATTACH syntaxes
variants = [
    "ATTACH '/home/dstadel/projects/axon/.axon/graph_v2/soll.db' AS soll ",
    "ATTACH '/home/dstadel/projects/axon/.axon/graph_v2/soll.db' AS soll (TYPE KUZU) ",
    "ATTACH '/home/dstadel/projects/axon/.axon/graph_v2/soll.db' AS soll (dbtype='kuzu') ",
    "ATTACH DATABASE '/home/dstadel/projects/axon/.axon/graph_v2/soll.db' AS soll ",
    "ATTACH '/home/dstadel/projects/axon/.axon/graph_v2/soll.db' AS soll (KUZU) "
]

for v in variants:
    test_query(v)
