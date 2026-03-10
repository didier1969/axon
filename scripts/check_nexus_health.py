import socket
import msgpack
import sys

def check_hydradb():
    host = '127.0.0.1'
    port = 6040
    api_key = "dev_key"
    
    print(f"Connecting to HydraDB at {host}:{port}...")
    try:
        # Create socket
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(5)
        sock.connect((host, port))
        
        # 1. Ping (before auth)
        ping_payload = msgpack.packb({"op": "ping"}, use_bin_type=True)
        sock.sendall(len(ping_payload).to_bytes(4, byteorder='big') + ping_payload)
        
        header = sock.recv(4)
        length = int.from_bytes(header, byteorder='big')
        resp = msgpack.unpackb(sock.recv(length), raw=False)
        print(f"Initial Ping response: {resp}")
        
        if resp.get("code") != "PONG":
            print(f"FAILED: Unexpected initial ping response: {resp}")
            return False

        # 2. Auth (must follow ping if we didn't close)
        auth_payload = msgpack.packb({"auth": api_key}, use_bin_type=True)
        sock.sendall(len(auth_payload).to_bytes(4, byteorder='big') + auth_payload)
        
        header = sock.recv(4)
        length = int.from_bytes(header, byteorder='big')
        resp = msgpack.unpackb(sock.recv(length), raw=False)
        print(f"Auth response: {resp}")
        
        if resp.get("code") == "AUTH_SUCCESS":
            print("SUCCESS: HydraDB is industrial-ready!")
            return True
        else:
            print(f"FAILED: Auth failed: {resp}")
            return False
            
    except Exception as e:
        print(f"FAILED: Connection error: {e}")
        return False
    finally:
        sock.close()

if __name__ == "__main__":
    if check_hydradb():
        sys.exit(0)
    else:
        sys.exit(1)
