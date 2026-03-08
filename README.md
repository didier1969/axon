# Axon v1.0 (Triple-Pod)

**Axon** est un moteur d'intelligence de code distribué haute performance. Il transforme n'importe quelle base de code en un graphe de connaissances structurelles exploitable par des agents IA et des développeurs.

## 🏗️ Architecture Triple-Pod

Depuis la v1.0, Axon abandonne le modèle monolithique pour une architecture distribuée basée sur trois unités autonomes (Pods) :

1.  **Pod A : Axon Watcher (Orchestrateur - Elixir/OTP)**
    - Surveille le système de fichiers en temps réel.
    - Orchestre le flux de travail entre le Parser et la Base de données.
    - Gère la file d'attente d'ingestion et la résilience.

2.  **Pod B : Axon Parser (Analyseur - Python/Tree-sitter)**
    - **Stateless** : Reçoit du code, renvoie de la structure (Symboles + Relations).
    - Utilise `tree-sitter` pour une analyse multi-langage précise.
    - Communication ultra-rapide via MsgPack/TCP.

3.  **Pod C : HydraDB (Persistence - Elixir/Rust/Dolt)**
    - Le "Cerveau" central de persistence.
    - Supporte le versionnage atomique du graphe via Dolt.
    - Exécute les analyses lourdes (PageRank, Taint Analysis, Audit).

## ⚡ Performance & Protocoles

Axon v1.0 utilise des protocoles de communication à ultra-faible latence :
- **Lien A ↔ B (Watcher ↔ Parser) :** Unix Domain Socket (UDS) + MsgPack via `/tmp/axon-parser.sock`.
- **Lien B ↔ C (Parser ↔ HydraDB) :** TCP Socket brute + MsgPack sur le port `4040`.
- **Lien A ↔ C (Watcher ↔ HydraDB) :** In-process BEAM (Erlang Distribution).

## 🚀 Démarrage Rapide (Nix)

Axon est entièrement géré par **Nix** pour garantir un environnement reproductible.

```bash
# Lancer l'environnement de développement complet
nix develop

# Lancer le daemon Axon (connecte les Pods)
axon start

# Indexer un projet
axon up --repo mon-projet
```

## 🧠 Intelligence de Graphe

Axon expose ses capacités via le **Model Context Protocol (MCP)**, permettant à n'importe quel agent IA (Claude Code, Gemini CLI) de :
- Naviguer dans les relations (`CALLS`, `IMPORTS`, `TYPES`).
- Calculer le rayon d'impact d'une modification.
- Auditer la conformité architecturale et la sécurité (OWASP).

---
© 2025-2026 Nexus AI Agency - Didier Stadelmann
