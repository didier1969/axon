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

# 1. VISION
send_cypher("MERGE (v:soll.Vision {title: 'La Source de Vérité Structurelle'}) SET v.description = 'Transformer le code source en un graphe de connaissances vivant (The Living Lattice).', v.goal = 'Eliminer 100% des hallucinations IA par la preuve physique.'")

# 2. PHASE
# Note: Phase is currently in the default namespace in graph.rs schema, but we want it in soll
send_cypher("MATCH (v:soll.Vision {title: 'La Source de Vérité Structurelle'}) MERGE (p:soll.Phase {name: 'Phase Apollo'}) SET p.goal = 'Unification sur KuzuDB, modèle Pull granulaire et Traçabilité MBSE.', p.status = 'in_progress' MERGE (p)-[:CONTRIBUTES_TO]->(v)")

# 3. PILIERS FACTUELS
pillars = [
    ("PIL-AXO-001", "Ingestion & Orchestration (Pull Mode)", "Scanner, filtrage .axonignore et pilotage 1:1 Agent/Worker."),
    ("PIL-AXO-002", "Analyse Sémantique (WASM & AST)", "Parsers multi-langages, intégrité AST et Lattice Refiner."),
    ("PIL-AXO-003", "Gestion des Ressources & Sécurité", "Jemalloc, Watchdog RSS, SIG_ABORT et scan de secrets."),
    ("PIL-AXO-004", "Traçabilité & Certification (Witness)", "Digital Thread MBSE et vérification de rendu UI."),
    ("PIL-AXO-005", "Interface & Monitoring Temps Réel", "Dashboard PubSub, StatsCache ETS et feedback loop.")
]

for pid, title, desc in pillars:
    send_cypher(f"MERGE (pi:soll.Pillar {{id: '{pid}'}}) SET pi.title = '{title}', pi.description = '{desc}'")
    send_cypher(f"MATCH (v:soll.Vision {{title: 'La Source de Vérité Structurelle'}}), (pi:soll.Pillar {{id: '{pid}'}}) MERGE (pi)-[:EPITOMIZES]->(v)")

# 4. REQUIREMENTS (Groupés par Pilier)
roadmap = [
    ("PIL-AXO-001", "REQ-AXO-001", "Pilotage Déterministe", "Allocation 1:1 Agent/Worker."),
    ("PIL-AXO-001", "REQ-AXO-002", "Souveraineté .axonignore", "Bypass Git conventions."),
    ("PIL-AXO-001", "REQ-AXO-003", "Structure Super-Projet", "Support HAS_SUBPROJECT."),
    ("PIL-AXO-001", "REQ-AXO-004", "Régulation Adaptative", "Buffer dynamique Débit/Latence."),
    
    ("PIL-AXO-002", "REQ-AXO-008", "Couverture Polyglotte", "Parsers WASM 12+ langages."),
    ("PIL-AXO-002", "REQ-AXO-010", "Reconciliation Inter-Langages", "Lattice Refiner (NIF->Rust)."),
    ("PIL-AXO-002", "REQ-AXO-024", "Indivisibilité AST", "Atomic Parsing Rule."),
    
    ("PIL-AXO-003", "REQ-AXO-006", "Robustesse Data Plane", "SIG_ABORT & Watchdog."),
    ("PIL-AXO-003", "REQ-AXO-009", "Oracle de Sécurité", "Scan de secrets AST."),
    ("PIL-AXO-003", "REQ-AXO-011", "Optimisation Jemalloc", "Gestion mémoire haute performance."),
    
    ("PIL-AXO-004", "REQ-AXO-005", "Traçabilité MBSE", "Ancrage code dans l intention."),
    ("PIL-AXO-004", "REQ-AXO-016", "Certification Witness", "Vérification réalité physique UI."),
    
    ("PIL-AXO-005", "REQ-AXO-014", "Cockpit Live", "Dashboard Phoenix PubSub."),
    ("PIL-AXO-005", "REQ-AXO-021", "Zero-SELECT Loop", "Feedback loop haute vitesse.")
]

for pid, rid, title, desc in roadmap:
    send_cypher(f"MATCH (p:soll.Phase {{name: 'Phase Apollo'}}) MERGE (r:soll.Requirement {{id: '{rid}'}}) SET r.title = '{title}', r.description = '{desc}' MERGE (r)-[:REFINES]->(p)")
    send_cypher(f"MATCH (pi:soll.Pillar {{id: '{pid}'}}), (r:soll.Requirement {{id: '{rid}'}}) MERGE (r)-[:BELONGS_TO]->(pi)")

# 5. REGISTRY
send_cypher("MERGE (reg:soll.Registry {id: 'AXON_GLOBAL'}) SET reg.last_req = 25, reg.last_cpt = 13")

print("Full Factual Pillar Restoration Complete.")
