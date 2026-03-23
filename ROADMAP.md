# Roadmap Axon : Objectif Apollo (v2.5 - v3.0)

## Aperçu
Axon passe du statut d'outil d'indexation à celui de **Treillis de Connaissance Vivant**. La priorité n'est plus la simple couverture linguistique, mais la **Souveraineté Sémantique Totale** et l'**Ingestion Douce Omniprésente**.

## Jalons Actuels

**v2.5 : L'Infrastucture de Vérité (Phase Apollo 1)**
Status: 🚀 En cours d'exécution Maestria

| Phase | Nom | Statut | Cible |
|-------|------|--------|-------|
| 1 | Ingestion "Fantôme" Haute Performance | ✅ Validé | Zero-Bloat Oban / Rust Spawn Blocking |
| 2 | Robustesse du Système Nerveux (MCP) | 🚧 En cours | Multiplexage / Synthèse Sémantique |
| 3 | Fédération du Treillis (Global Graph) | 📅 Prochainement | Jointure multi-projets Cypher |
| 4 | Réconciliateur Sémantique (Lattice Refiner) | 🚧 En cours | Déduplication Fuzzy/Vectorielle Rust |

### Phase 2 : Robustesse du Système Nerveux (MCP)
- [x] Isolation des threads de calcul (`tokio::task::spawn_blocking`).
- [x] Déduplication atomique des symboles (Protection KuzuDB).
- [x] **Synthèse Sémantique :** Rapports de décision structurés en Markdown.
- [x] **Notifications Proactives :** Système de notifications JSON-RPC fonctionnel.

### Phase 4 : Réconciliateur Sémantique (Lattice Refiner) - Branched: `feat/lattice-refiner`
- [ ] Moteur de similarité fuzzy native (RapidFuzz).
- [ ] Algorithme de Blocking par kind/signature.
- [ ] Création automatique des relations `[:SAME_AS]`.
- [ ] Intégration dans l'outil `axon_inspect`.

### Phase 3 : Fédération du Treillis (Global Graph)
- **Objectif :** Supprimer la notion de "Project" isolée. Le graphe devient global.
- **Support Cypher étendu :** Requêtes traversant les dépendances entre dépôts différents dans `/home/dstadel/projects`.
- **Analyse d'Impact Prédictive :** `axon_impact` fonctionnel à 100% sur le graphe global.

## Jalons Futurs

**v3.0 : L'Oracle Omniscient (Phase Apollo 2)**
- **Ingestion Temps Réel Native :** Intégration OS (Inotify/Fanotify) pour indexation à la microseconde.
- **Certification Witness :** Preuves physiques de vérité sémantique pour chaque réponse fournie à l'IA.
- **Clustering Nexus :** Support du clustering multi-nœuds pour l'analyse de graphes distribués.

---
*Roadmap réalignée par le Nexus Lead Architect le 22 Mars 2026.*

### Phase 5 : Asynchronous MCP Orchestration (Job Polling Pattern)
- **Objectif :** Supporter les requêtes analytiques Cypher extrêmement lourdes (> 60 secondes) sans provoquer de Timeout côté LLM/Cloud.
- **Architecture :** Implémentation du "Pattern du Ticket".
  - Nouveaux outils MCP : `axon_start_job`, `axon_check_job_status`.
  - Le serveur Rust délègue le calcul lourd à un thread de fond et retourne un Job ID immédiat.
  - L'Agent IA utilise une boucle de polling pour interroger le statut du ticket.
