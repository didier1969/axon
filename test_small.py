
import socket
import json
import time
import os

SOCKET_PATH = "/tmp/axon-telemetry.sock"

def test_small_file_ok():
    file_path = "/tmp/small_test_file.rs"
    with open(file_path, "w") as f:
        f.write("fn main() { println!(\"Hello\"); }")
    
    print(f"Created {file_path}")

    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.connect(SOCKET_PATH)
        s.settimeout(10)
    except Exception as e:
        print(f"Failed to connect: {e}")
        return

    payload = {
        "path": file_path,
        "lane": "fast",
        "trace_id": "test_small_trace",
        "t0": int(time.time() * 1000000),
        "t1": int(time.time() * 1000000)
    }
    msg = f"PARSE_FILE {json.dumps(payload)}\n"
    s.sendall(msg.encode())

    start_time = time.time()
    try:
        while True:
            data = s.recv(4096).decode()
            if "FileIndexed" in data:
                print(f"SUCCESS: Received FileIndexed response for SMALL file: {data}")
                break
            if time.time() - start_time > 10:
                print("TIMEOUT for small file!")
                break
    finally:
        s.close()

if __name__ == "__main__":
    test_small_file_ok()
