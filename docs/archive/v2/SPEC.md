# Spécification Technique Axon v2

## 1. Contrat de Communication (Data Plane ↔ Control Plane)

La communication s'effectue via un **Unix Domain Socket** localisé par défaut dans `/tmp/axon-v2.sock`.
Le protocole utilisé est **MsgPack** pour minimiser l'overhead de sérialisation.

### Types de Messages (Control Plane -> Data Plane)
- `CONFIG_UPDATE`: Envoie de nouveaux patterns d'exclusion ou clés d'API.
- `TRIGGER_SCAN`: Force un scan complet d'un répertoire.
- `QUERY_GRAPH`: Exécute une requête Cypher brute (pour le Dashboard).
- `GET_STATS`: Demande les métriques de performance actuelles.

### Types de Événements (Data Plane -> Control Plane)
- `FILE_INDEXED`: Détails sur un fichier parsé (temps, nombre de symboles).
- `SCAN_PROGRESS`: Pourcentage de complétion du scan initial.
- `ALERT`: Notification d'erreur critique ou violation de règle d'audit.
- `TELEMETRY`: Métriques système (CPU/Mem) du noyau Rust.

## 2. Structure du Data Plane (Rust)

Le binaire `axon-core` est structuré en modules :
- `scanner`: Abstraction sur `notify` et `ignore`.
- `parser`: Pool de workers `tree-sitter`. Supporte initialement Python, Elixir, Rust et TypeScript.
- `graph`: Wrapper autour de `KuzuDB` avec gestion des transactions.
- `mcp`: Serveur JSON-RPC implémentant le SDK MCP.
- `bridge`: Gestion de la socket UDS vers le dashboard.

## 3. Structure du Control Plane (Elixir)

L'application `axon_dashboard` (OTP) :
- `BridgeClient`: Gère la connexion persistante au Data Plane.
- `MetricsStore`: Cache local (ETS) pour les données de monitoring.
- `LiveView`: Pages `Dashboard`, `GraphExplorer`, `AuditCenter`.

---

## 4. Sécurité & Robustesse
- **Sandboxing :** Le Data Plane ne lit que les fichiers autorisés par la configuration.
- **Panic Recovery :** Utilisation de `catch_unwind` en Rust et supervision OTP en Elixir pour assurer que le système redémarre en cas de crash d'un composant.
- **Resource Limiting :** Capping du nombre de threads de parsing pour éviter d'asphyxier l'OS hôte.
