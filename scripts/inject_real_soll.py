import socket
import json
import time

def send_command(cmd):
    sock_path = "/tmp/axon-telemetry.sock"
    max_retries = 10
    for i in range(max_retries):
        try:
            client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            client.settimeout(5)
            client.connect(sock_path)
            # Wait for welcome message
            client.recv(1024)
            client.sendall(f"{cmd}\n".encode())
            time.sleep(0.05)
            client.close()
            return True
        except Exception as e:
            if i == max_retries - 1:
                print(f"Error connecting to socket after {max_retries} attempts: {e}")
                return False
            time.sleep(1)
    return False

print("🚀 Starting Axon Lattice (SOLL) Full Restoration...")

# 1. Initialize Registry
send_command("EXECUTE_CYPHER INSERT INTO soll.Registry (id, last_req, last_cpt, last_dec, last_mil, last_val) VALUES ('AXON_GLOBAL', 0, 0, 0, 0, 0) ON CONFLICT DO NOTHING")

# 2. Vision
send_command("""
EXECUTE_CYPHER INSERT INTO soll.Vision (title, description, goal) 
VALUES ('Système de Vérité Structurelle (The Lattice)', 'Infrastructure de cartographie AST multi-projets. Axon fournit une interface de requêtage sémantique sur l intégralité du patrimoine logiciel local.', 'Éliminer les erreurs de contexte des agents IA par la preuve physique et la certification Witness.') 
ON CONFLICT (title) DO UPDATE SET description=EXCLUDED.description, goal=EXCLUDED.goal
""")

# 3. Stakeholders
send_command("EXECUTE_CYPHER INSERT INTO soll.Stakeholder (name, role) VALUES ('Nexus Lead Architect', 'System Architect / Product Owner') ON CONFLICT DO NOTHING")

# 4. Pillars
pillars = [
    ("PIL-AXO-001", "Ingestion Ghost (Background Process)", "Consommation CPU < 5% hors phases de scan initial."),
    ("PIL-AXO-002", "Nexus Pull (Orchestration)", "Inversion du flux : les workers tirent les tâches selon la latence réelle."),
    ("PIL-AXO-003", "Protocole Witness (Certification)", "Validation physique de l état du DOM et de l AST."),
    ("PIL-AXO-004", "Résilience Zero-Sleep", "Disponibilité continue du serveur MCP < 100ms."),
    ("PIL-AXO-005", "Architecture Multi-DB (SOLL/IST)", "Isolation physique entre l intention (soll.db) et la forge (ist.db).")
]

for pid, title, desc in pillars:
    send_command(f"EXECUTE_CYPHER INSERT INTO soll.Pillar (id, title, description) VALUES ('{pid}', '{title}', '{desc}') ON CONFLICT (id) DO UPDATE SET title=EXCLUDED.title, description=EXCLUDED.description")
    send_command(f"EXECUTE_CYPHER INSERT INTO soll.EPITOMIZES (source_id, target_id) VALUES ('{pid}', 'Système de Vérité Structurelle (The Lattice)')")

# 5. Milestones
milestones = [
    ("MIL-AXO-001", "Phase Apollo (v2.5 - v3.0)", "in_progress", "2026-04-30")
]
for mid, title, status, deadline in milestones:
    send_command(f"EXECUTE_CYPHER INSERT INTO soll.Milestone (id, title, status, deadline) VALUES ('{mid}', '{title}', '{status}', '{deadline}') ON CONFLICT (id) DO UPDATE SET status=EXCLUDED.status")

# 6. Requirements
reqs = [
    ("PIL-AXO-001", "REQ-AXO-001", "Allocation 1:1 Agent/Worker", "Chaque thread Rust est piloté par un Agent Elixir dédié."),
    ("PIL-AXO-002", "REQ-AXO-002", "Fédération du Treillis", "Support natif HAS_SUBPROJECT brisant les silos des dépôts Git."),
    ("PIL-AXO-003", "REQ-AXO-003", "Certification Witness L1/L2/L3", "Preuve physique de vérité sémantique pour chaque réponse fournie à l IA."),
    ("PIL-AXO-004", "REQ-AXO-004", "Orchestration Asynchrone", "Pattern du Ticket pour les requêtes SQL/PGQ dépassant 60s."),
    ("PIL-AXO-005", "REQ-AXO-005", "Isolation Multi-DB", "DuckDB Multi-DB avec soll.db monté en READ_ONLY pour les agents IA.")
]

for pid, rid, title, desc in reqs:
    send_command(f"EXECUTE_CYPHER INSERT INTO soll.Requirement (id, title, description, justification, priority) VALUES ('{rid}', '{title}', '{desc}', 'N/A', 'P1') ON CONFLICT (id) DO UPDATE SET title=EXCLUDED.title, description=EXCLUDED.description")
    send_command(f"EXECUTE_CYPHER INSERT INTO soll.BELONGS_TO (source_id, target_id) VALUES ('{rid}', '{pid}')")
    send_command(f"EXECUTE_CYPHER INSERT INTO soll.TARGETS (source_id, target_id) VALUES ('MIL-AXO-001', '{rid}')")
    send_command(f"EXECUTE_CYPHER INSERT INTO soll.ORIGINATES (source_id, target_id) VALUES ('Nexus Lead Architect', '{rid}')")

# 7. Decisions (ADR)
decisions = [
    ("DEC-AXO-001", "Migration DuckDB Native", "Transition vers un moteur SQL embarqué pour supporter l isolation physique.", "KuzuDB ATTACH limitation.", "Seul DuckDB permet des jointures cross-DB robustes.", "accepted", "REQ-AXO-005"),
    ("DEC-AXO-002", "Abandon de DuckPGQ", "Utilisation du SQL natif (WITH RECURSIVE) pour les requêtes de graphe.", "404 Errors on community extensions.", "Supprime la dépendance aux serveurs universitaires tiers.", "accepted", "REQ-AXO-002")
]

for did, title, ctx, alt, rat, status, rid in decisions:
    send_command(f"EXECUTE_CYPHER INSERT INTO soll.Decision (id, title, context, alternative, rationale, status) VALUES ('{did}', '{title}', '{ctx}', '{alt}', '{rat}', '{status}') ON CONFLICT (id) DO UPDATE SET status=EXCLUDED.status")
    send_command(f"EXECUTE_CYPHER INSERT INTO soll.SOLVES (source_id, target_id) VALUES ('{did}', '{rid}')")

# 8. Concepts
concepts = [
    ("REQ-AXO-001", "CPT-AXO-001: Tracer T0-T4", "Mesure des latences entre Ingress, Parsing, Embedding et Commit.", "Optimisation dynamique du flux."),
    ("REQ-AXO-002", "CPT-AXO-002: CTE USING KEY", "Algorithmes de graphes implémentés en SQL standard optimisé.", "Performance O(log n) sur les traversées."),
    ("REQ-AXO-005", "CPT-AXO-003: Registre Souverain", "Table soll.Registry centralisant la génération des identifiants.", "Garantie d unicité et DNA unique.")
]

for rid, name, expl, rat in concepts:
    send_command(f"EXECUTE_CYPHER INSERT INTO soll.Concept (name, explanation, rationale) VALUES ('{name}', '{expl}', '{rat}') ON CONFLICT (name) DO UPDATE SET explanation=EXCLUDED.explanation")
    send_command(f"EXECUTE_CYPHER INSERT INTO soll.EXPLAINS (source_id, target_id) VALUES ('{name}', '{rid}')")

# Update Registry counters
send_command("EXECUTE_CYPHER UPDATE soll.Registry SET last_req = 5, last_cpt = 3, last_dec = 2, last_mil = 1, last_val = 0 WHERE id = 'AXON_GLOBAL'")

print("✅ Full Architectural Truth injected into Axon Lattice (soll.db).")
