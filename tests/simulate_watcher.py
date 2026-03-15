import socket
import json
import time
import sys

SOCK_PATH = "/tmp/axon-v2.sock"

def test_watcher_event():
    print(f"Connecting to {SOCK_PATH} as a mock Watcher...")
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.connect(SOCK_PATH)
    except Exception as e:
        print(f"Failed to connect: {e}")
        sys.exit(1)

    # We need to wait for the SystemReady message
    time.sleep(0.5)

    payload = {
        "type": "WatcherFileIndexed",
        "payload": {
            "path": "test/python_script.py",
            "status": "ok"
        }
    }
    
    msg = f"WATCHER_EVENT {json.dumps(payload)}\n"
    print(f"Sending: {msg.strip()}")
    s.sendall(msg.encode('utf-8'))
    
    time.sleep(1)
    s.close()
    print("Done.")

if __name__ == "__main__":
    test_watcher_event()
