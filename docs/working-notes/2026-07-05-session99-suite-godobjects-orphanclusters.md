# Session 99 suite (2026-07-05) — god-objects 13/14 langages + orphan_clusters

Audit-only. Détail canonique = SOLL (`CPT-AXO-052` session_pointer, `REQ-AXO-902185`, `REQ-AXO-902211`, `REQ-AXO-902213`). Ce fichier trace le narratif de session, pas l'état projet.

## Contexte d'entrée

Continuation de la session 99 (voir `project_session99_shi_duplication_wiring.md` pour la partie 1). Opérateur : "termine le code object [god-objects], documente bien le point de wiring, puis traite-le [clusters morts] selon le même principe".

## Livré

### 1. God-objects — 8 langages restants (REQ-AXO-902185 slice 3)

Java/C/C++/C#/Ruby/Kotlin/PHP/Scheme, via 8 agents de recherche parallèles (un par langage, lecture du fichier parser existant + grammaire tree-sitter réelle). Deux bugs trouvés en dev-test (pas en review de code) :
- C# : `for_each_statement`/`case_switch_label` supposés → faux, vrais noms `foreach_statement`/`switch_section` (trouvé via dump `to_sexp()` réel).
- Kotlin : fixture de test tassé sur une ligne produisait des nœuds ERROR (le parseur Kotlin est sensible aux retours à la ligne) — le code de production était correct du premier coup.

Scheme = cas à part (grammaire list-level, reconnaissance par symbole de tête, `and`/`or` exclus pour cohérence avec `&&`/`||` déjà exclus sur Rust).

LLL exclu (cross-repo, pas de grammaire tree-sitter locale) — message mailbox envoyé au projet LLL, sans réponse en fin de session.

Commit `1cdcc7c0`. Preuve : 76/76 tests parser::, suite complète 1553/1553.

### 2. Clusters morts — orphan_clusters (REQ-AXO-902211)

Question opérateur initiale : un LLM peut livrer une fonction testée mais jamais câblée dans l'application réelle ; `wiring` (per-symbole) ne voit pas un CLUSTER de N fonctions qui s'appellent UNIQUEMENT entre elles (chacune a un appelant : un autre membre du cluster mort).

Conception : BFS multi-sources depuis les racines (main/handler/nif + soll.Traceability role=entry) sur CALLS/CALLS_NIF, puis Union-Find (réutilisé de `petgraph`, pas réinventé — rappel opérateur "nous avons des librairies de graphe spécialisées") sur le sous-graphe non-atteint.

Dogfood réel sur AXO (curl direct au MCP dev puis live) a immédiatement trouvé 2 vrais faux positifs, chacun corrigé et testé avant le suivant :
1. **Dispatch dynamique** (`Box<dyn Parser>`) — 117 symboles faussement "morts" (tous les parseurs de langage). Cause réelle vérifiée par SQL : l'appel `parser.parse(&content)` résout vers un nœud FANTÔME (`stage_a2.rs::parse`, aucune ligne dans `ist.symbol`, aucune arête sortante), PAS vers le nœud du trait — le pont contrat→implémentant existant (`REQ-AXO-902028`, déjà utilisé par `bfs_shortest_path`) ne s'applique donc pas tel quel. Fix : pont par correspondance de NOM (candidat qui implémente un trait ET partage le nom nu d'un fantôme atteint), en point fixe borné avec l'expansion structurelle normale (termine forcément : monotone, borné par le nombre de nœuds).
2. **Assistants de test privés** (ex. `fn parser() -> CParser` dans `mod tests`, appelés uniquement par des `#[test]`) — même exclusion que `wiring_classify_node` (test_helper), absente jusqu'ici d'`orphan_clusters`.

Décision de conception rejetée puis retirée : une heuristique "axon_* = racine" pour contourner le fait que `main -> run_brain` (démarrage HTTP du brain) n'a AUCUNE arête CALLS sortante enregistrée (trou d'extraction parseur sur du code type builder-pattern). Retirée sur remarque opérateur : spécifique à Axon, zéro valeur pour analyser un autre projet. Le vrai manque (aucun outil MCP n'écrit `soll.Traceability role=entry`) tracké séparément en `REQ-AXO-902213`.

Commit `d63c4263`. Preuve : 12 nouveaux tests unitaires, suite complète 1570/1570, dogfood réel (89→80 clusters, taille max 117→90).

### 3. Promote unique (les deux ensemble, sur demande explicite opérateur)

`v0.8.0-1364-gd63c4263`, phase=clean, qualify-mcp verdict=ok. Vérifié en direct post-promote (`curl` sur le port live) que `orphan_clusters` répond bien à côté de `wiring`.

## Leçons méthodologiques (voir aussi practice_put 210-213)

- Agent de recherche "idle" ≠ "a livré son rapport" — 3 des 8 agents god-objects sont revenus idle sans contenu, ont dû être relancés explicitement.
- Ne jamais coder une heuristique de nommage spécifique au projet hôte dans un algorithme générique multi-tenant — même découverte en dogfood légitime, ça ne vaut rien pour un autre projet.
- Vérifier les hypothèses de graphe par SQL réel avant de concevoir un pont/une correction — la première hypothèse (le dispatch dynamique atterrit sur le nœud du trait) était fausse ; la vraie forme (nœud fantôme scopé au fichier appelant) n'était pas devinable sans vérification.

## Reste (tracké en SOLL, pas dans ce fichier)

- LLL : réponse mailbox en attente (non-bloquant).
- REQ-AXO-902213 : chemin d'écriture MCP pour `role=entry`, P2, pas commencé.
