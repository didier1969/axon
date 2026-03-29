#!/usr/bin/env python3
import socket
import time
import sys

def trigger_scan():
    sock_path = "/tmp/axon-telemetry.sock"
    print(f"📡 Connecting to Axon Telemetry Bridge...")
    
    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.settimeout(5)
        client.connect(sock_path)
        
        # Read welcome
        welcome = client.recv(1024).decode('utf-8')
        print(f"✅ Bridge Ready: {welcome.strip()}")
        
        print("🚀 Sending SCAN_ALL command...")
        client.sendall(b"SCAN_ALL\n")
        
        print("🏁 Command transmitted. The Living Lattice is now mapping your software estate.")
        client.close()
    except FileNotFoundError:
        print(f"❌ Error: Socket {sock_path} not found. Is Axon Core running?")
        sys.exit(1)
    except Exception as e:
        print(f"❌ Error: {e}")
        sys.exit(1)

if __name__ == "__main__":
    trigger_scan()
