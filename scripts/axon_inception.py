import socket
import json
import time

def send_cypher(query):
    sock_path = "/tmp/axon-telemetry.sock"
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        client.connect(sock_path)
        # Skip welcome message
        client.recv(1024)
        command = f"EXECUTE_CYPHER {query}\n"
        client.sendall(command.encode())
        print(f"Sent query to Writer Actor...")
        time.sleep(0.2) 
    except Exception as e:
        print(f"Error: {e}")
    finally:
        client.close()

# 1. VISION MAÎTRE
send_cypher("CREATE (v:Vision {title: 'Souveraineté Sémantique Totale', description: 'Fournir une couche de vérité absolue, instantanée et omnisciente sur l intégralité du patrimoine logiciel (The Living Lattice).', goal: 'Eliminer 100% des hallucinations structurelles via le protocole Witness.'})")

# 2. PHASE
send_cypher("MATCH (v:Vision {title: 'Souveraineté Sémantique Totale'}) CREATE (p:Phase {name: 'Phase Apollo', goal: 'Unification sur KuzuDB, modèle Pull granulaire et Traçabilité MBSE.', status: 'in_progress'}) CREATE (p)-[:CONTRIBUTES_TO]->(v)")

# 3. REQUIREMENTS (Exemples fondamentaux)
send_cypher("MATCH (p:Phase {name: 'Phase Apollo'}) CREATE (r:Requirement {id: 'REQ-AXO-001', title: 'Pilotage Déterministe', description: '1 Agent Elixir par coeur Rust.', justification: 'Garantir le contrôle 1:1 et éviter la congestion.', priority: 'critical'}) CREATE (r)-[:REFINES]->(p)")
send_cypher("MATCH (p:Phase {name: 'Phase Apollo'}) CREATE (r:Requirement {id: 'REQ-AXO-002', title: 'Souveraineté .axonignore', description: 'Bypass GitIgnore.', justification: 'Couverture sémantique totale.', priority: 'high'}) CREATE (r)-[:REFINES]->(p)")

print("Inception Script Updated & Executed.")
