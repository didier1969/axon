import time
import socket
import json

client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
client.connect("/tmp/axon-v2.sock")
client.settimeout(2.0)
try:
    while True:
        client.recv(4096)
except socket.timeout:
    pass

req = {
    "jsonrpc": "2.0",
    "method": "tools/call",
    "params": {
        "name": "axon_health",
        "arguments": {"project": "axon"}
    },
    "id": 1
}

start = time.time()
client.sendall((json.dumps(req) + "\n").encode())
client.settimeout(30.0)

try:
    data = client.recv(4096)
    end = time.time()
    print(f"Health check answered in {end - start:.2f}s")
except Exception as e:
    print(f"Error: {e}")

