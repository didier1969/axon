# Axon v2 : Architecture Industrielle

## 🔭 Vision
Passer d'une architecture distribuée fragmentée (Python/Elixir/Rust) à un modèle hybride binaire consolidé. Le but est de garantir une latence infra-milliseconde pour le parsing et une fiabilité de 100% sur le système de fichiers.

---

## 🏗️ Les Deux Piliers

### 1. Le Data Plane (Noyau Rust)
Un binaire unique, hautement parallèle, qui gère tout le cycle de vie de la donnée de code.

- **Scanner :** Utilise `notify` (événements OS) et `ignore` (respect des .gitignore/.axonignore).
- **Parser Engine :** Intégration directe de `tree-sitter` via des bindings Rust. Extraction asynchrone des symboles, imports et appels.
- **Storage :** Base de données de graphe embarquée `KuzuDB`. Pas de latence réseau pour les requêtes Cypher.
- **Interface MCP :** Serveur MCP natif exposé via Stdio (pour les agents) et via Unix Domain Socket (pour le Dashboard).

### 2. Le Control Plane (Dashboard Elixir/Phoenix)
Une application LiveView dédiée à la supervision et à l'orchestration stratégique.

- **Real-time Monitoring :** Flux Phoenix LiveView pour suivre l'indexation en direct.
- **Visualisation :** Rendu 2D/3D du graphe architectural via D3.js.
- **Config Management :** Interface visuelle pour gérer les exclusions, les règles d'audit et les alertes.
- **Health Check :** Surveillance de la consommation mémoire et CPU du Data Plane.

---

## 🔄 Flux de Données (Data Flow)

1.  **Événement FS :** Le Scanner Rust détecte une modification.
2.  **Parsing :** Un worker du pool Rust (Rayon) parse le fichier immédiatement.
3.  **Ingestion :** Les symboles sont injectés dans KuzuDB.
4.  **Notification :** Un message MsgPack est envoyé sur l'UDS vers Elixir.
5.  **Mise à jour UI :** Le Dashboard LiveView se rafraîchit instantanément.

---

## 🛠️ Stack Technique V2
- **Langage Core :** Rust 1.80+ (Tokio, Rayon, Tree-sitter, KuzuDB).
- **Dashboard :** Elixir 1.18, Phoenix 1.7 (LiveView, FLAME pour le scaling si besoin).
- **Communication :** Unix Domain Sockets + MsgPack (Ultra-faible latence).
- **Protocole :** Model Context Protocol (MCP) v1.3.

© 2026 Nexus AI Agency - Didier Stadelmann
