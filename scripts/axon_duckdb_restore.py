import socket
import time
import json

def send_sql(query):
    sock_path = "/tmp/axon-telemetry.sock"
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        client.connect(sock_path)
        client.recv(1024) # welcome message
        # We still use EXECUTE_CYPHER command name because it's what main.rs expects, 
        # but the content is now SQL.
        client.sendall(f"EXECUTE_CYPHER {query}\n".encode())
        time.sleep(0.05) 
    except Exception as e:
        print(f"Error: {e}")
    finally:
        client.close()

print("🚀 Starting DuckDB SOLL Restoration...")

# 1. VISION
send_sql("""
INSERT INTO soll.Vision (title, description, goal) 
VALUES ('Souveraineté Sémantique Totale', 'Axon est l Oracle Structurel Universel. Sa mission est de fournir une couche de vérité absolue, instantanée et omnisciente sur l intégralité du patrimoine logiciel de l utilisateur. Le code n est pas du texte, c est une structure ; la structure n est pas statique, c est un organisme vivant (The Living Lattice).', 'Éliminer 100% des hallucinations structurelles des agents IA par la preuve physique et la certification Witness.')
ON CONFLICT (title) DO UPDATE SET description=EXCLUDED.description, goal=EXCLUDED.goal;
""")

# 2. PILLARS
pillars = [
    ("PIL-AXO-001", "Ingestion & Orchestration (Pull Mode)", "Scanner, filtrage .axonignore et pilotage 1:1 Agent/Worker."),
    ("PIL-AXO-002", "Analyse Sémantique (WASM & AST)", "Parsers multi-langages, intégrité AST et Lattice Refiner."),
    ("PIL-AXO-003", "Gestion des Ressources & Sécurité", "Jemalloc, Watchdog RSS, SIG_ABORT et scan de secrets."),
    ("PIL-AXO-004", "Traçabilité & Certification (Witness)", "Digital Thread MBSE et vérification de rendu UI."),
    ("PIL-AXO-005", "Interface & Monitoring Temps Réel", "Dashboard PubSub, StatsCache ETS et feedback loop.")
]

for pid, title, desc in pillars:
    send_sql(f"INSERT INTO soll.Pillar (id, title, description) VALUES ('{pid}', '{title}', '{desc}') ON CONFLICT (id) DO UPDATE SET title=EXCLUDED.title, description=EXCLUDED.description;")
    send_sql(f"INSERT INTO soll.EPITOMIZES (source_id, target_id) VALUES ('{pid}', 'Souveraineté Sémantique Totale');")

# 3. REQUIREMENTS
roadmap = [
    ("PIL-AXO-001", "REQ-AXO-001", "Pilotage Déterministe (Nexus Pull)", 
     "Orchestration granulaire de 1 thread Rust par coeur physique piloté par un Agent Elixir dédié.", 
     "Éliminer la congestion I/O et garantir que chaque cycle CPU est utilisé de manière prévisible et réactive.", "critical"),
    
    ("PIL-AXO-001", "REQ-AXO-002", "Souveraineté du filtrage .axonignore", 
     "Ignorer les conventions Git pour utiliser exclusivement les règles métier .axonignore.", 
     "Permettre l indexation de fichiers sémantiquement riches (config, docs) souvent exclus par Git.", "high"),
    
    ("PIL-AXO-001", "REQ-AXO-003", "Structure de Super-Projet (Hiérarchie)", 
     "Support natif des sous-projets et des pods via la relation récursive HAS_SUBPROJECT.", 
     "Briser les silos des dépôts Git pour permettre une analyse d impact transverse et une fédération totale.", "medium"),
    
    ("PIL-AXO-001", "REQ-AXO-004", "Régulation de Flux Adaptative", 
     "Ajustement dynamique de la trémie ETS selon le débit réel pour garantir une latence < 10s pour les urgences.", 
     "Équilibrer mathématiquement le débit massif d ingestion et la réactivité instantanée du serveur MCP.", "high"),
    
    ("PIL-AXO-002", "REQ-AXO-008", "Couverture Polyglotte", 
     "Parsers WASM 12+ langages.", "Supporter tous les langages du projet sans latence.", "high"),

    ("PIL-AXO-002", "REQ-AXO-010", "Reconciliation Inter-Langages", 
     "Lattice Refiner (NIF->Rust).", "Lier les appels entre Elixir et Rust nativement.", "high"),

    ("PIL-AXO-003", "REQ-AXO-006", "Robustesse et Annulation Coopérative", 
     "Signal SIG_ABORT et AtomicBool permettant d interrompre une tâche lourde sans crash ni fuite RAM.", 
     "Protéger l intégrité du système face aux fichiers Titan ou aux calculs AI dépassant le budget temps.", "high"),
    
    ("PIL-AXO-004", "REQ-AXO-005", "Traçabilité MBSE (Digital Thread)", 
     "Ancrage physique de chaque intention (SOLL) dans le code source (IST) via l adressage sémantique DNA.", 
     "Éradiquer les hallucinations en obligeant les agents IA à justifier leur code par rapport aux exigences certifiées.", "critical"),
    
    ("PIL-AXO-004", "REQ-AXO-007", "Base SOLL & Backup Automatique", 
     "Sauvegarde préventive de la couche intentionnelle (DuckDB) avant toute opération de remise à zéro.", 
     "La structure SOLL est le fruit de l intelligence humaine et doit être protégée des aléas techniques de l indexation.", "critical")
]

for pid, rid, title, desc, just, prio in roadmap:
    send_sql(f"INSERT INTO soll.Requirement (id, title, description, justification, priority) VALUES ('{rid}', '{title}', '{desc}', '{just}', '{prio}') ON CONFLICT (id) DO UPDATE SET title=EXCLUDED.title, description=EXCLUDED.description, justification=EXCLUDED.justification, priority=EXCLUDED.priority;")
    send_sql(f"INSERT INTO soll.BELONGS_TO (source_id, target_id) VALUES ('{rid}', '{pid}');")

# 4. CONCEPTS
concepts = [
    ("REQ-AXO-001", "CPT-AXO-001: Flux Sans Temps Mort", "Pipelining et Double-buffering dans le socket UNIX.", "S assurer que Rust n attend jamais l ordre suivant en maintenant une tâche d avance."),
    ("REQ-AXO-001", "CPT-AXO-002: Centralisation des Ecritures", "Writer Actor unique en Rust pour toutes les mutations.", "Garantir l intégrité MVCC et éviter les erreurs Database busy."),
    ("REQ-AXO-004", "CPT-AXO-003: Fenêtre Glissante Dynamique", "Taille du buffer ETS = (Débit Files/sec) * Latence_Cible.", "Ajuster dynamiquement la réserve de travail pour préserver la réactivité aux urgences."),
    ("REQ-AXO-005", "CPT-AXO-004: Autorité Sémantique", "Génération d identifiants uniques (REQ-PRJ-001) gérée par le système.", "Garantir l unicité absolue et la non-corruption du Digital Thread.")
]

for rid, name, expl, rat in concepts:
    send_sql(f"INSERT INTO soll.Concept (name, explanation, rationale) VALUES ('{name}', '{expl}', '{rat}') ON CONFLICT (name) DO UPDATE SET explanation=EXCLUDED.explanation, rationale=EXCLUDED.rationale;")
    send_sql(f"INSERT INTO soll.EXPLAINS (source_id, target_id) VALUES ('{name}', '{rid}');")

# 5. REGISTRY (Initialisation)
send_sql("INSERT INTO soll.Registry (id, last_req, last_cpt, last_dec) VALUES ('AXON_GLOBAL', 25, 13, 0) ON CONFLICT (id) DO UPDATE SET last_req=EXCLUDED.last_req, last_cpt=EXCLUDED.last_cpt;")

print("✅ Full DuckDB SOLL Migration Complete.")
