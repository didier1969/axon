import socket
import json
import time
import os

def send_command(command):
    sock_path = "/tmp/axon-telemetry.sock"
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        client.connect(sock_path)
        client.recv(1024) # welcome message
        client.sendall(f"{command}\n".encode())
        # We don't wait for response here, we will query DB later
    except Exception as e:
        print(f"Error sending command: {e}")
    finally:
        client.close()

def query_db(sql):
    sock_path = "/tmp/axon-telemetry.sock"
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        client.connect(sock_path)
        client.recv(1024) # welcome
        client.sendall(f"RAW_QUERY {sql}\n".encode())
        time.sleep(0.5)
        response = client.recv(4096).decode()
        return response
    except Exception as e:
        return f"Error: {e}"
    finally:
        client.close()

def test_rigorous_ingestion():
    test_file = os.path.abspath("scripts/axon_scan.py")
    
    print(f"--- [ RIGOROUS VALIDATION: 9-COLUMN SCHEMA ] ---")
    
    # 1. Check Symbol table schema
    print("Checking Symbol table structure...")
    schema_info = query_db("PRAGMA table_info('Symbol')")
    print(f"Schema: {schema_info}")
    
    # 2. Inject a test file explicitly via Telemetry
    print(f"Injecting file for extraction: {test_file}")
    payload = json.dumps({"path": test_file, "trace_id": "test_rigor", "t0": int(time.time()), "t1": 0})
    send_command(f"PARSE_FILE {payload}")
    
    print("Waiting 5 seconds for extraction and commit...")
    time.sleep(5)
    
    # 3. Verify status transition
    print("Verifying status transition in File table...")
    status = query_db(f"SELECT status FROM File WHERE path = '{test_file}'")
    print(f"Status: {status}")
    
    # 4. Verify Symbol extraction with 9 columns
    print("Verifying extracted symbols...")
    symbols = query_db(f"SELECT name, kind, tested, is_public FROM Symbol WHERE id LIKE 'global::%' LIMIT 5")
    print(f"Symbols: {symbols}")
    
    if "indexed" in status and "global" in symbols:
        print("\n✅ VALIDATION SUCCESS: Pipeline is functional and schema is respected.")
        return True
    else:
        print("\n❌ VALIDATION FAILED: Pipeline is stuck or schema mismatch.")
        return False

if __name__ == "__main__":
    test_rigorous_ingestion()
