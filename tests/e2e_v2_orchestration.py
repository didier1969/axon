import subprocess
import socket
import os
import time

def test_e2e_connection_persistence():
    print("🚀 Starting E2E Connectivty Validation (Daemon Mode)...")
    
    # 1. Get binary path
    axon_bin = os.getenv("AXON_BIN", "./target/release/axon-core")
    print(f"Using binary: {axon_bin}")
    
    socket_path = "/tmp/axon-v2.sock"
    if os.path.exists(socket_path):
        os.remove(socket_path)
    
    # 2. Start axon-core in DAEMON mode
    process = subprocess.Popen(
        [axon_bin, "--daemon"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True
    )
    
    # 2. Wait for socket
    print("⏳ Waiting for UDS Socket...")
    for _ in range(20):
        if os.path.exists(socket_path):
            break
        time.sleep(0.5)
    
    if not os.path.exists(socket_path):
        print("❌ FAILED: Socket not created by Rust Core.")
        process.kill()
        return False

    # 3. Connect as Dashboard
    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.settimeout(5.0)
        client.connect(socket_path)
        print("✅ Dashboard successfully connected to UDS Bridge.")
        
        msg = client.recv(1024)
        if b"Axon Bridge Ready" in msg:
            print("✅ Protocol Handshake Received.")
            client.close()
            process.terminate()
            return True
        else:
            print(f"❌ Protocol Error: Received {msg}")
            client.close()
            process.terminate()
            return False
            
    except Exception as e:
        print(f"❌ E2E Connection failed: {e}")
        process.kill()
        return False

if __name__ == "__main__":
    if test_e2e_connection_persistence():
        print("\n🏆 E2E VALIDATION SUCCESSFUL")
        exit(0)
    else:
        print("\n💥 E2E VALIDATION FAILED")
        exit(1)
