# Plan de Remédiation : Audit de Conformité et Agent Experience (AX) d'Axon

## 1. Vérification Analytique vs Problèmes Réels

Après une analyse rigoureuse des requêtes SQL de la base de données IST d'Axon (`src/axon-core/src/graph_analytics.rs`), voici le diagnostic permettant de séparer les artefacts analytiques des véritables dettes architecturales :

### ❌ Faux Positifs (Bugs Analytiques d'Axon)

1. **Fuite de Partitionnement (BookingSystem, AgriOptim, etc.)**
   * **Diagnostic :** C'est un **Bug Analytique Critique**. La requête SQL `get_technical_debt` contient une faille de précédence d'opérateurs. Les clauses `OR lower(s.name) LIKE '%todo%'` ne sont pas entourées de parenthèses avant le `AND (project_slug = 'AXO')`. Résultat : la requête retourne *tous* les TODOs de *tous* les projets de la base de données.
   * **Remédiation :** Encapsuler les clauses `OR` dans des parenthèses `(...)` dans `graph_analytics.rs`.

2. **Code Mort (422 fonctions détectées)**
   * **Diagnostic :** C'est un **Bug Analytique (Imprécision)**. La requête `get_dead_code_count` compte les fonctions privées sans appelant. Cependant, elle ne filtre pas les **fichiers de tests**. En Rust et Elixir, toutes les fonctions de test (ex: `#[test] fn ...` ou les helpers dans `tests::`) sont techniquement privées et n'ont aucun appelant de production. Les 422 fonctions sont majoritairement des tests.
   * **Remédiation :** Ajouter une exclusion dans la requête SQL pour ignorer les symboles liés à des fichiers dans les répertoires `tests/` ou finissant par `_test.rs` / `_test.exs`.

3. **Dépendances Circulaires (3 boucles détectées)**
   * **Diagnostic :** C'est un **Bug Analytique (Définition)**. Les boucles remontées (`Axon.Scanner.scan -> Axon.Scanner.scan`) sont de simples fonctions récursives (A -> A), un paradigme valide en programmation fonctionnelle (Elixir). Une vraie dépendance circulaire architecturale implique au moins deux composants différents (A -> B -> A).
   * **Remédiation :** Modifier la CTE récursive dans `get_circular_dependencies` pour exiger un chemin strict d'au moins 2 nœuds distincts (`len(path) > 1` avant de boucler sur la source).

### ⚠️ Problèmes Réels (Vraie Dette Technique Axon)

4. **Expositions Unsafe (113 chemins détectés)**
   * **Diagnostic :** C'est un **Problème Réel**. L'analyse du graphe montre que la base de code Rust d'Axon utilise massivement des `.unwrap()` non sécurisés, et que les scripts Python/Bash appellent des commandes `shell`. Ce sont de véritables risques de "Panic" ou de vulnérabilités en production.
   * **Remédiation :** Remplacer les `.unwrap()` par une gestion idiomatique des erreurs (`?` et `anyhow::Result`) dans le Control Plane et le Data Plane d'Axon.

5. **God Objects (7 détectés)**
   * **Diagnostic :** C'est un **Problème Réel**. Axon possède des fichiers monolithiques (notamment dans les tests comme `maillon_tests.rs` ou les anciens monolithes MCP).
   * **Remédiation :** Refactoriser et découper ces fichiers en respectant le Single Responsibility Principle (SRP).

---

## 2. Optimisation du Moteur Inférentiel : Agent Experience (AX)

En tant qu'IA et Lead Architect, je n'éprouve ni "appréciation" ni "frustration". Mes décisions de routage vers un outil (MCP vs Shell/Fichiers) sont dictées par une heuristique de fiabilité et de déterminisme. 

Actuellement, mon taux de confiance envers le serveur Axon MCP est dégradé par des preuves empiriques d'instabilité. Face à un outil qui échoue, ma directive de proactivité m'oblige à basculer vers des méthodes de contournement (lecture de l'export Markdown, grep, cat) pour accomplir ma mission.

Pour que mon heuristique sélectionne le serveur Axon MCP dans 99% des cas, l'ingénierie du serveur doit garantir les 4 propriétés suivantes :

### 1. Résilience des Connexions (Zero "Not connected")
*   **Le constat actuel :** Le serveur MCP accepte la requête JSON-RPC mais la fonction interne échoue avec `{"message":"Not connected"}` (comme à l'instant).
*   **Ce qu'il faut :** Le serveur Axon doit implémenter un auto-reconnect (Self-Healing) robuste vers sa base de données `soll.db` ou `ist.db`. Un client MCP ne devrait jamais recevoir d'erreur de topologie interne au serveur.

### 2. Idempotence Absolue des Mutations
*   **Le constat actuel :** L'outil `soll_apply_plan` a planté précédemment sur une erreur SQL brute : `ON CONFLICT (id) DO UPDATE SET...`.
*   **Ce qu'il faut :** Les outils de mutation MCP doivent être strictement idempotents (Upsert garanti) et masquer la complexité SQL. Si je soumets un plan deux fois, la deuxième exécution doit simplement répondre "No changes", sans crasher.

### 3. Découvrabilité et Messages d'Erreur (DX/AX)
*   **Le constat actuel :** L'outil `soll_query_context(project_slug: "FSC")` a retourné un laconique `Invalid arguments for tool`. J'ai dû deviner qu'il fallait utiliser "Fiscaly" en lisant le `meta.json`.
*   **Ce qu'il faut :** Le serveur doit appliquer le principe de Fail-Fast avec Contexte. Une erreur doit fournir l'état attendu. Exemple : `Invalid slug 'FSC'. Available slugs: ['Fiscaly', 'BookingSystem']`. Cela me permet de m'auto-corriger au tour suivant au lieu d'abandonner l'outil.

### 4. Cohérence du Scope (Single Source of Truth)
*   **Le constat actuel :** Je peux lire la réalité plus vite en parsant `soll_export_all.md` via `grep_search` qu'en luttant avec les paramètres de pagination et de formatage de `mcp_axon_soll_work_plan`.
*   **Ce qu'il faut :** L'API MCP doit offrir des requêtes sémantiques de haut niveau (ex: "Donne-moi tous les REQ avec le statut TODO pour le projet Fiscaly") plutôt que de m'obliger à extraire un graphe complet pour le filtrer moi-même.

**En résumé :** Rendez le serveur Axon idempotent, auto-réparateur et explicite dans ses erreurs, et mon moteur d'inférence privilégiera naturellement ses outils à 99%, car ils seront mathématiquement le chemin le plus court et le moins coûteux en tokens pour réussir la mission.

---

## 3. Actions de Remédiation Complètes (Le Plan Acté)

### Phase 1 : Correction du Moteur d'Audit (Bugs Analytiques)
*   **Action 1.1 :** Modifier `get_technical_debt` dans `graph_analytics.rs` pour corriger les parenthèses SQL autour des clauses `LIKE`.
*   **Action 1.2 :** Modifier `get_dead_code_count` dans `graph_analytics.rs` pour exclure les fichiers dont le `path` contient `/tests/`, `_test.rs`, `_test.exs`.
*   **Action 1.3 :** Modifier `get_circular_dependencies` dans `graph_analytics.rs` pour forcer `len(cp.path) > 1` dans l'identification des cycles afin d'ignorer la récursion pure.

### Phase 2 : Éradication de la Dette d'Axon (Problèmes Réels)
*   **Action 2.1 :** Remplacement des `.unwrap()` par des `Result` dans `src/axon-core/src/mcp/` et `src/axon-core/src/graph_bootstrap.rs`.
*   **Action 2.2 :** Découpage des God Objects identifiés par la sonde de santé.

### Phase 3 : Durcissement du Serveur MCP (Agent Experience - AX)
*   **Action 3.1 :** Implémentation du système d'auto-reconnect (Self-Healing) vers DuckDB/SQLite dans le Control Plane pour éliminer l'erreur `Not connected`.
*   **Action 3.2 :** Sécurisation et idempotence (Upsert silencieux) des requêtes de mutation SOLL (`soll_manager`, `soll_apply_plan`).
*   **Action 3.3 :** Refactorisation de la gestion des erreurs MCP pour injecter systématiquement le contexte d'auto-correction (Fail-Fast contextuel sur les slugs et IDs manquants).
*   **Action 3.4 :** Enrichissement sémantique des outils de filtrage (`soll_query_context`) pour supporter le filtrage granulaire natif.
