import requests
import json
import time

def test_db_init():
    url = "http://127.0.0.1:44129/mcp"
    headers = {"Content-Type": "application/json"}
    
    # Query to check if 'soll' schema and its tables exist
    query = "SELECT count(*) FROM soll.Vision"
    payload = {
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_query",
            "arguments": {"query": query}
        },
        "id": 1
    }
    
    print(f"Checking Lattice Integrity: {query}")
    try:
        response = requests.post(url, headers=headers, json=payload, timeout=5)
        print(f"Response: {response.status_code}")
        print(response.text)
        
        if response.status_code == 200:
            result = response.json()
            if "error" in result:
                print(f"❌ TEST FAILED: {result['error']}")
                return False
            print("✅ TEST PASSED: 'soll' schema is accessible.")
            return True
        else:
            print(f"❌ TEST FAILED: HTTP {response.status_code}")
            return False
    except Exception as e:
        print(f"❌ TEST FAILED: {str(e)}")
        return False

if __name__ == "__main__":
    test_db_init()
