import socket
import time

def send_cypher(query):
    sock_path = "/tmp/axon-telemetry.sock"
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        client.connect(sock_path)
        client.recv(1024)
        client.sendall(f"EXECUTE_CYPHER {query}\n".encode())
        time.sleep(0.1) 
    except Exception as e:
        print(f"Error: {e}")
    finally:
        client.close()

# --- 1. VISION ---
send_cypher("MERGE (v:Vision {title: 'La Source de Vérité Structurelle'}) SET v.description = 'Transformer le code source en un graphe de connaissances vivant (The Living Lattice).', v.goal = 'Eliminer 100% des hallucinations IA par la preuve physique.'")

# --- 2. PILIERS FACTUELS ---
pillars = [
    ("PIL-AXO-001", "Ingestion & Orchestration (Pull Mode)", "Scanner, filtrage .axonignore et pilotage 1:1 Agent/Worker."),
    ("PIL-AXO-002", "Analyse Sémantique (WASM & AST)", "Parsers multi-langages, intégrité AST et Lattice Refiner."),
    ("PIL-AXO-003", "Gestion des Ressources & Sécurité", "Jemalloc, Watchdog RSS, SIG_ABORT et scan de secrets."),
    ("PIL-AXO-004", "Traçabilité & Certification (Witness)", "Digital Thread MBSE et vérification de rendu UI."),
    ("PIL-AXO-005", "Interface & Monitoring Temps Réel", "Dashboard PubSub, StatsCache ETS et feedback loop.")
]

for pid, title, desc in pillars:
    send_cypher(f"MERGE (pi:Pillar {{id: '{pid}'}}) SET pi.title = '{title}', pi.description = '{desc}'")
    send_cypher(f"MATCH (v:Vision {{title: 'La Source de Vérité Structurelle'}}), (pi:Pillar {{id: '{pid}'}}) MERGE (pi)-[:EPITOMIZES]->(v)")

# --- 3. REGROUPEMENT DES REQUIREMENTS ---
mapping = {
    "PIL-AXO-001": ["REQ-AXO-001", "REQ-AXO-002", "REQ-AXO-003", "REQ-AXO-004"],
    "PIL-AXO-002": ["REQ-AXO-008", "REQ-AXO-010", "REQ-AXO-024", "REQ-AXO-025"],
    "PIL-AXO-003": ["REQ-AXO-006", "REQ-AXO-009", "REQ-AXO-011", "REQ-AXO-012", "REQ-AXO-013"],
    "PIL-AXO-004": ["REQ-AXO-005", "REQ-AXO-007", "REQ-AXO-016", "REQ-AXO-017", "REQ-AXO-018"],
    "PIL-AXO-005": ["REQ-AXO-014", "REQ-AXO-015", "REQ-AXO-019", "REQ-AXO-020", "REQ-AXO-021", "REQ-AXO-022", "REQ-AXO-023"]
}

for pid, reqs in mapping.items():
    for rid in reqs:
        send_cypher(f"MATCH (pi:Pillar {{id: '{pid}'}}), (r:Requirement {{id: '{rid}'}}) MERGE (r)-[:BELONGS_TO]->(pi)")

print("Factual Pillar Restructuring Complete.")
