import socket
import sys
import argparse
import json

SOCK_PATH = "/tmp/axon-telemetry.sock"

def send_command(command):
    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.connect(SOCK_PATH)
        # Skip welcome message
        client.recv(1024)
        client.sendall(f"{command}\n".encode())
        client.close()
        return True
    except Exception as e:
        print(f"Error: {e}")
        return False

def execute_cypher(query):
    return send_command(f"EXECUTE_CYPHER {query}")

def main():
    parser = argparse.ArgumentParser(description="Axon SOLL/IST Traceability Helper")
    subparsers = parser.add_subparsers(dest="command")

    # Add Concept
    parser_concept = subparsers.add_parser("add_concept")
    parser_concept.add_argument("--name", required=True)
    parser_concept.add_argument("--explanation", required=True)
    parser_concept.add_argument("--rationale", default="")
    parser_concept.add_argument("--req_id", help="Link to an existing Requirement ID")

    # Link Code
    parser_link = subparsers.add_parser("link_code")
    parser_link.add_argument("--concept", required=True)
    parser_link.add_argument("--symbol", required=True, help="FQN of the symbol (e.g. Module.func)")

    args = parser.parse_args()

    if args.command == "add_concept":
        query = f"CREATE (c:Concept {{name: '{args.name}', explanation: '{args.explanation}', rationale: '{args.rationale}'}})"
        if execute_cypher(query):
            print(f"Concept '{args.name}' created.")
            if args.req_id:
                link_query = f"MATCH (c:Concept {{name: '{args.name}'}}), (r:Requirement {{id: '{args.req_id}'}}) CREATE (c)-[:EXPLAINS]->(r)"
                execute_cypher(link_query)
        
    elif args.command == "link_code":
        query = f"MATCH (c:Concept {{name: '{args.concept}'}}), (s:Symbol {{id: '{args.symbol}'}}) CREATE (c)-[:SUBSTANTIATES]->(s)"
        if execute_cypher(query):
            print(f"Linked concept '{args.concept}' to symbol '{args.symbol}'.")

if __name__ == "__main__":
    main()
