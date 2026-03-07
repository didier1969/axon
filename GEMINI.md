# Axon : Copilote Architectural

Axon transforme une base de code en un graphe de connaissances structurelles. Il permet aux agents IA et aux développeurs de comprendre instantanément n'importe quel projet à grande échelle en naviguant dans les relations (appels, types, imports) plutôt que de simplement chercher du texte.

## 🧠 Usage & Stratégie
Axon est une **Boussole** (intelligence sémantique et structurelle) et un **Bouclier** (Audit de sécurité OWASP).

### 🛠️ Axon Tool Routing (MANDATOIRE)

| Besoin | Outil Axon |
|--------|------------|
| Trouver symbole par nom/concept | `axon_query` |
| Résumé sémantique d'un fichier | `axon_summarize` |
| Analyser les dépendances | `axon_context` |
| Tracer le flux d'une variable | **`axon trace`** |
| Évaluer le rayon d'impact | `axon_impact` |
| **Audit Architectural (OWASP/Anti-patterns)** | **`axon audit`** |
| Lister les points d'entrée (Entry Points) | `axon_entry_points` |
| Identifier les zones non testées | `axon_coverage_gaps` |

### ⚠️ Paramètre `repo` (Slug)
Tous les outils Axon **exigent** le paramètre `repo`. Appelez `axon_list_repos` une fois par session.

## 🛠️ Commandes "Docker-style"
- **Lancer le Daemon :** `axon start`
- **Ré-indexer (Deep) :** `axon up`
- **Lancer l'Audit :** `axon check`
- **Arrêter le Daemon :** `axon stop`

## 🏗️ Technologies Core
- **Graphe :** KuzuDB (Cypher, embarqué)
- **Analyse :** Tree-sitter (12 Langages Experts)
- **Vecteur :** FastEmbed (Embeddings locaux)
- **Sécurité :** Moteur d'audit OWASP intégré

## 💎 Vision v0.9+
- **Data Flow End-to-End :** Suivi de la donnée de l'UI (HTML) à la DB (SQL).
- **Audit d'Alignement :** Vérification automatique Code vs Documentation stratégique.
- **Moteur HydraDB :** Backend multi-moteur avec versionnage Dolt.
