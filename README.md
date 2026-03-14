# Axon v2.0 (Immune & Autonomous)

**Axon** est un moteur d'intelligence de code distribué haute performance. Il transforme n'importe quelle base de code en un graphe de connaissances structurelles exploitable par des agents IA et des développeurs.

## 🛡️ Système Immunitaire Autonome

Depuis la v2.0, Axon devient totalement autonome et proactif. Il n'est plus nécessaire de lancer manuellement les composants ; Axon surveille, indexe et se répare tout seul dès le démarrage de votre environnement WSL.

### 🏗️ Architecture Triple-Pod v2

1.  **Pod A : Axon Watcher (Orchestrateur - Elixir/OTP)**
    - **Priority Streaming Scanner** : Scan disque asynchrone (Rust NIF) avec priorité sémantique.
    - **Auto-Trigger** : Déclenche l'indexation dès le démarrage du service.
    - Gère la résilience et le "back-pressure" entre les Pods.

2.  **Pod B : Axon Parser (Analyseur - Rust/Python/Tree-sitter)**
    - Analyseur hybride haute performance intégré au moteur Rust.
    - Support natif de **TypeQL** et **Datalog** (via pont Python optimisé).
    - Communication via Unix Domain Socket (`/tmp/axon-v2.sock`).

3.  **Pod C : HydraDB (Persistence - Elixir/Rust/Dolt)**
    - Persistence atomique et versionnage du graphe (Dolt).
    - Moteur d'audit OWASP et analyses structurelles lourdes.

## ⚡ Performance & Protocoles

Axon v2.0 utilise une communication unifiée via UDS :
- **Lien A ↔ B (Watcher ↔ Parser) :** Unix Domain Socket (UDS) via `/tmp/axon-v2.sock`.
- **Lien Dashboard :** LiveView réactif sur le port `44921`.

## 🚀 Activation Automatique (WSL)

Pour qu'Axon surveille votre code en permanence dès le lancement de WSL, ajoutez cette ligne à votre `~/.bashrc` :

```bash
bash /home/dstadel/projects/axon/scripts/ensure-axon-running.sh
```

Le script de garde-fou vérifiera l'état de la stack à chaque ouverture de terminal et la ressuscitera si nécessaire.

## 📊 Cockpit de Contrôle

Accédez au dashboard live pour suivre l'indexation en temps réel :
**[http://localhost:44921](http://localhost:44921)**

## 🧠 Modèle MCP (Intelligence IA)

Axon expose ses capacités via le **Model Context Protocol (MCP)**.
- **`axon_query`** : Recherche hybride sémantique/structurelle.
- **`axon_audit`** : Audit de sécurité OWASP automatisé.
- **`axon_impact`** : Analyse de rayon d'impact des changements.

---
© 2025-2026 Nexus AI Agency - Didier Stadelmann
