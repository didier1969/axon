import socket
import json
import time
import os
import sys
import tempfile
import logging

logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')

def send_parse_request(path):
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(10.0)
            s.connect('/tmp/axon-telemetry.sock')
            
            # Read the welcome message
            s.recv(1024)
            
            # Send the parse request
            payload = json.dumps({'path': path, 'lane': 'fast'})
            req = f"PARSE_FILE {payload}\n"
            s.sendall(req.encode())
            
            # Wait for the response
            response = s.recv(4096).decode()
            return response
    except FileNotFoundError:
        logging.error("Telemetry socket not found. Is Axon running?")
        sys.exit(1)
    except socket.timeout:
        logging.error("Socket operation timed out.")
        sys.exit(1)
    except Exception as e:
        logging.error(f"Failed to communicate with Axon: {e}")
        sys.exit(1)

def test_pipeline():
    logging.info("Starting E2E Pipeline Validation...")
    
    with tempfile.NamedTemporaryFile(mode="w", suffix=".ex", delete=False) as tf:
        tf.write("defmodule DummyTest do\n  def hello(), do: :world\nend")
        test_file_path = tf.name
        
    try:
        logging.info(f"1. Sending dummy file {test_file_path} to Telemetry Socket...")
        response = send_parse_request(test_file_path)
        logging.info(f"Telemetry Response: {response}")
        assert "FileIndexed" in response, "Failed to get indexing confirmation"
        logging.info("✅ Phase 1 Successful: File ingested by Rust Data Plane.")
        
        try:
            logging.info("2. Querying MCP Socket for AST Symbols...")
            
            max_retries = 5
            success = False
            
            for attempt in range(max_retries):
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as mcp_s:
                    mcp_s.settimeout(10.0)
                    mcp_s.connect('/tmp/axon-mcp.sock')
                    
                    query = f"MATCH (f:File)-[:CONTAINS]->(s:Symbol) WHERE f.path = '{test_file_path}' RETURN count(s)"
                    req = {
                        'jsonrpc': '2.0', 
                        'method': 'tools/call', 
                        'params': {'name': 'axon_cypher', 'arguments': {'cypher': query}}, 
                        'id': 1
                    }
                    mcp_s.sendall((json.dumps(req) + '\n').encode())
                    
                    mcp_response_raw = mcp_s.recv(4096).decode()
                    mcp_response = json.loads(mcp_response_raw)
                    result_text = mcp_response['result']['content'][0]['text']
                    logging.info(f"Attempt {attempt + 1}: MCP Cypher Result: {result_text}")
                    
                    if 'Int64(2)' in result_text:
                        logging.info("✅ Phase 2 Successful: Symbols verified in KuzuDB Graph.")
                        success = True
                        break
                        
                time.sleep(1)
                
            assert success, f"Expected 2 symbols after {max_retries} attempts, last result was {result_text}"
            
        except Exception as e:
            logging.error(f"MCP Query Failed: {e}")
            raise
        
    finally:
        os.remove(test_file_path)
        logging.info(f"Cleaned up dummy file {test_file_path}")
    
if __name__ == "__main__":
    test_pipeline()
