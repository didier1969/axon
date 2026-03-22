import socket
import json

client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
client.connect('/tmp/axon-mcp.sock')
print("Connected")

request = {
    'jsonrpc': '2.0',
    'method': 'initialize',
    'params': {},
    'id': 1
}

client.sendall((json.dumps(request) + '\n').encode())
print("Sent")

data = client.recv(4096)
print(f'Received: {data}')
