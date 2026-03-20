#!/usr/bin/env python3
import socket
import sys
import threading

def forward_stdin_to_uds(sock):
    for line in sys.stdin:
        if not line:
            break
        try:
            sock.sendall(line.encode('utf-8'))
        except Exception:
            break
    sock.close()

def forward_uds_to_stdout(sock):
    buffer = b""
    while True:
        try:
            data = sock.recv(4096)
            if not data:
                break
            buffer += data
            while b'\n' in buffer:
                line, buffer = buffer.split(b'\n', 1)
                # Ensure we only write valid JSON-RPC back to stdout to prevent MCP protocol violations
                # We skip lines that don't look like JSON dicts (like Axon Bridge Ready)
                decoded = line.decode('utf-8', errors='ignore').strip()
                if decoded.startswith('{') and decoded.endswith('}'):
                    sys.stdout.write(decoded + '\n')
                    sys.stdout.flush()
        except Exception:
            break

def main():
    sock_path = "/tmp/axon-v2.sock"
    
    # Retry loop to allow Axon Core to finish booting (FastEmbed takes ~5-10s)
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    connected = False
    for i in range(30):
        try:
            client.connect(sock_path)
            connected = True
            break
        except Exception:
            import time
            time.sleep(1)
            
    if not connected:
        sys.stderr.write(f"Error connecting to UDS {sock_path} after 30 seconds. Is Axon running?\n")
        sys.exit(1)

    t1 = threading.Thread(target=forward_stdin_to_uds, args=(client,), daemon=True)
    t2 = threading.Thread(target=forward_uds_to_stdout, args=(client,), daemon=True)

    t1.start()
    t2.start()

    t1.join()
    t2.join()

if __name__ == "__main__":
    main()
