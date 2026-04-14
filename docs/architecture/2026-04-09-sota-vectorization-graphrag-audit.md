# Audit d'Architecture : SOTA Vectorization & GraphRAG (2025-2026)

L'objectif strict est la maximisation de la capacité de recherche et d'inférence des LLMs à travers la base de code, en minimisant la perte de contexte et l'hallucination (Precision Gap). La méthode SOTA actuelle s'éloigne du RAG naïf (Vector-only) pour s'appuyer sur le **Hybrid GraphRAG** combiné à des **Bases de Connaissances Déterministes (DKB)**.

Voici les directives architecturales répondant aux enjeux d'indexation d'Axon.

## 1. Qu'est-ce qu'il faut vectoriser ? (Périmètre d'ingestion)

Il est inefficace de tout vectoriser aveuglément. La vectorisation doit être ciblée sur les unités sémantiques.

*   **Le Code source (Couche IST) :** Les signatures, les implémentations de fonctions, les classes et les types.
*   **La Documentation (Couche SOLL) :** Les documents d'architecture (comme les fichiers dans `docs/architecture/`), les décisions (ADR), et les règles d'affaires.
*   **Les Méta-données :** Les messages de commit importants, les tickets ou issues si disponibles, car ils lient l'intention (SOLL) à l'implémentation (IST).
*   **Les Fichiers d'Infrastructure :** OUI, les fichiers de configuration complexes (ex: `flake.nix`, `devenv.yaml`, `docker-compose.yml`) doivent être vectorisés car ils définissent la "réalité physique" de l'infrastructure, ce qui est crucial pour le debugging par un LLM.

## 2. Comment découper les documents ? (Stratégie de Chunking)

Le découpage basé sur un nombre de tokens fixes (ex: 512 tokens) est **obsolète** en 2026.

*   **Pour le Code (AST-Based Chunking) :** L'approche SOTA exige l'utilisation de **Tree-Sitter**. Le code doit être découpé selon ses frontières logiques (par fonction, par méthode, par classe). Une fonction ne doit jamais être scindée au milieu de sa logique.
*   **Pour le Texte/Doc (Semantic Breakpoints & Late Chunking) :** La méthode SOTA ("Late Chunking") consiste à passer le document entier dans le modèle d'embedding (pour capturer le contexte global), puis à le segmenter aux ruptures sémantiques (changement de sujet ou de paragraphe).
*   **Contextual Retrieval :** Chaque chunk (code ou texte) doit être préfixé par un court résumé généré (ou le chemin du fichier complet) pour que le vecteur ne perde pas son contexte "parent".

## 3. Faut-il vectoriser le graphe lui-même ?

**NON, pas au sens d'un aplatissement.** Le graphe (ex: géré via TypeDB/CozoDB ou DuckDB dans notre pipeline) ne doit pas être "aplati" en vecteurs bruts. SOTA dicte une approche **GraphRAG**.

*   **Les Nœuds (Nodes) sont vectorisés :** On vectorise le contenu et le résumé de chaque nœud (une fonction, un fichier, un concept).
*   **Les Arêtes (Edges) restent des relations structurelles :** Les dépendances (ex: `A appelle B`, `C hérite de D`) sont conservées sous forme de graphe relationnel.
*   **Communautés (Leiden Algorithm) :** SOTA consiste à détecter des "communautés" dans le graphe (des modules très couplés) et à vectoriser un **résumé généré de cette communauté**. Cela permet au LLM de comprendre l'architecture globale sans lire chaque fichier.

## 4. Qu'est-ce qui est optimal en termes de recherche ?

L'optimum SOTA est la recherche hybride avec routage de requête (Hybrid Search Routing) :

1.  **Recherche Hybride (Vector + Keyword) :** Combinaison de la recherche sémantique (via FastEmbed/DuckDB ou pgvector) et de la recherche par mots-clés exacte (BM25) pour ne pas rater les noms de variables spécifiques ou les IDs d'erreurs.
2.  **Graph Traversal (Multi-hop) :** Lorsqu'un LLM cherche un point de départ, le vecteur le trouve. Ensuite, le système **parcourt le graphe** pour injecter dans le contexte du LLM les nœuds adjacents (ex: la signature de la fonction appelée, le schéma de la base de données utilisée par cette fonction).
3.  **Re-Ranking :** Passage obligatoire par un modèle Cross-Encoder pour re-trier les 50 meilleurs résultats avant de les fournir au LLM.

## 5. Importance et contribution de chacun (La Synthèse)

Dans l'architecture Axon actuelle (Elixir Control Plane / Rust Data Plane / DuckDB pour les embeddings) :

*   **Vecteurs (Contribution : 30%) :** Servent uniquement de point d'entrée (Entrypoint). Ils traduisent l'intention sémantique de l'utilisateur ("Comment marche l'authentification ?") vers des coordonnées physiques dans le code.
*   **Graphe (Contribution : 50%) :** C'est la colonne vertébrale (Deterministic Knowledge Base). Il garantit qu'il n'y a **pas d'hallucination**. Si le vecteur trouve la fonction `login`, le graphe apporte la garantie physique de toutes les dépendances réelles de `login` (fichiers importés, interfaces).
*   **Documentation SOLL (Contribution : 20%) :** Apporte le *Pourquoi*. Le code dit *Comment*, le vecteur trouve le *Où*. Sans la documentation vectorisée, le LLM peut modifier le code mais risque de briser les invariants architecturaux.

**Conclusion pragmatique pour Axon :** 
Ne modifiez pas l'approche par graphe pur pour tout passer en vectoriel. Optimisez l'ingestion avec un parseur AST (Tree-Sitter), stockez ces chunks propres dans DuckDB (via `resume_vectorization.py`), et utilisez le graphe pour extraire les dépendances lors de la génération de la réponse au MCP. C'est l'état de l'art pour les LLMs sur de larges bases de code en 2026.