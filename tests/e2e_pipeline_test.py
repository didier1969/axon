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
    s.settimeout(30.0)
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
