# Axon : Copilote Architectural

Axon transforme une base de code en un graphe de connaissances structurelles. Il permet aux agents IA et aux développeurs de comprendre instantanément n'importe quel projet à grande échelle en naviguant dans les relations (appels, types, imports) plutôt que de simplement chercher du texte.

## 🧠 Usage & Stratégie
Axon est une **Boussole** (intelligence sémantique et structurelle). Grep est un **Scalpel** (recherche de texte brut).

### 🛠️ Axon Tool Routing (MANDATOIRE)

| Besoin | Outil Axon |
|--------|------------|
| Trouver une fonction, classe ou module par nom ou concept | `axon_query` |
| Obtenir un résumé sémantique d'un fichier ou module | `axon_summarize` |
| Analyser les dépendances (qui appelle qui, références de types) | `axon_context` |
| Évaluer le rayon d'impact avant une modification | `axon_impact` |
| Tracer le chemin critique entre deux fonctions | `axon_path` |
| Auditer la qualité structurelle (cycles, couplage) | `axon_lint` |
| **Audit Architectural (Détection proactive d'anomalies)** | `axon audit` |
| Lister les points d'entrée (entry points) du projet | `axon_entry_points` |
| Identifier les zones à risque non testées | `axon_coverage_gaps` |

### ⚠️ Paramètre `repo` (Slug)
Tous les outils Axon **exigent** le paramètre `repo`.
- Exécutez `axon_list_repos` une fois au début de la session pour obtenir les noms exacts.
- Dans `axon_batch`, chaque appel doit inclure son propre argument `repo`.

## 🏗️ Technologies Core
- **Graphe :** [KuzuDB](https://kuzudb.com/) (Cypher, embarqué)
- **Analyse :** Tree-sitter (Multi-langages)
- **Vecteur :** FastEmbed (Embeddings locaux)
- **Runtime :** Python 3.11+, Daemon avec cache LRU

## 🛠️ Commandes Utiles
- **Indexer :** `axon analyze .`
- **Lancer le Daemon :** `axon daemon start`
- **Serveur MCP :** `axon serve` (mode standard) ou `axon mcp` (stdio)
- **Tests :** `uv run pytest`

## 💎 Vision v0.9+
- **Polyglottisme Profond :** Support Java, C#, Ruby.
- **Transparence Elixir ↔ Rust :** Traversée automatique des frontières NIF.
- **Audit d'Alignement :** Vérification automatique Code vs Documentation.
- **Moteur HydraDB :** Migration vers un backend multi-moteur (SQL/Graphe/Vecteur) avec versionnage Dolt.
