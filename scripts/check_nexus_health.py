#!/usr/bin/env python3
import sys
import socket
import msgpack
import struct

def check_pod_c():
    print("[Nexus] Checking Pod C (HydraDB) on port 5000...", end=" ")
    try:
        with socket.create_connection(("127.0.0.1", 5000), timeout=2) as sock:
            print("ONLINE 🟢")
            return True
    except ConnectionRefusedError:
        print("OFFLINE 🔴 (Start HydraDB Elixir service)")
        return False
    except Exception as e:
        print(f"ERROR 🔴 ({e})")
        return False

def check_axon_version():
    print("[Nexus] Checking Axon Core Version...", end=" ")
    try:
        import axon
        from importlib.metadata import version
        v = version("axoniq")
        print(f"{v} 🟢")
    except Exception:
        print("UNKNOWN 🔴 (Check installation)")

if __name__ == "__main__":
    print("--- Axon v1.0 Nexus Health Check ---")
    check_axon_version()
    check_pod_c()
    print("-------------------------------------")
