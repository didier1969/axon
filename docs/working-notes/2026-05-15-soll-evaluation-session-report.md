# Évaluation SOLL — rapport de session GraphRAG

Date : 2026-05-15
Session : Claude Sonnet, projet AXO
Contexte : conception MIL-AXO-019 (IST in-memory graph + refonte GraphRAG)

---

## 1. Évaluation honnête de SOLL

### 1.1 Ce que SOLL fait bien

| Aspect | Verdict | Pourquoi |
|---|---|---|
| **Survivre aux sessions LLM** | ★★★★★ | Le contexte stocké hier est lisible aujourd'hui sans répétition. Élimine la perte d'intent à la compaction. |
| **Typage strict des entités** | ★★★★ | 9 types (vision, pillar, requirement, concept, decision, milestone, stakeholder, validation, guideline) couvrent l'essentiel. Pas de fourre-tout. |
| **Relations directionnelles canoniques** | ★★★★ | `soll_relation_schema` *en principe* élimine le trial-and-error. Edges typés (REFINES/SOLVES/EXPLAINS/VERIFIES/TARGETS/BELONGS_TO/SUPERSEDES). |
| **Graph-as-index** (CPT-PRO-006) | ★★★★ | La structure (edges) porte l'information, pas les tags. Permet `soll_work_plan` à partir du DAG. |
| **Vocabulaire status normalisé** | ★★★★ | DEC-PRO-100 enforced (5 valeurs). CHECK constraint DB côté backend. |
| **Pattern token-efficient** (GUI-PRO-100) | ★★★ | Bien défini. Mais nécessite rappel explicite — n'est pas appliqué par défaut par les LLM (preuve : cette session, j'ai dû être recadré). |

### 1.2 Ce que SOLL fait mal aujourd'hui

| Faille observée | Impact | Bug logé |
|---|---|---|
| `soll_relation_schema` retourne output terse vide | Force trial-and-error sur les directions | REQ-AXO-91495 |
| `soll_manager` valide `status` post-write au lieu de pre-write | LLM perd un round-trip ; pourtant REQ-AXO-325 marquée `delivered` | REQ-AXO-91498 |
| Champs ad hoc (`priority`, `tags`) routés silencieusement vers `metadata` JSONB | Contrat d'entrée opaque, LLM ne peut pas raisonner sur l'effet | REQ-AXO-91499 |
| Schema drift `project_slug` non auto-réparé au boot | Brain bloque `soll_manager(create)` jusqu'à intervention humaine | REQ-AXO-91496 |
| `soll_work_plan` ne reflète pas les entités nouvellement créées | Wave 1 dominée par DEC anciennes ; mes nouvelles REQ invisibles | cf. §5 ci-dessous |

### 1.3 Score net

| Dimension | Note |
|---|---:|
| Conception (modèle de données, séparation des concerns) | 9 / 10 |
| Implémentation côté serveur | 6 / 10 |
| Ergonomie LLM (discoverability, contrat d'entrée) | 5 / 10 |
| Persistance / pérennité du savoir | 9 / 10 |

SOLL est un **excellent design partiellement implémenté**. Le modèle est solide ; les rugosités de surface MCP brûlent des cycles LLM.

---

## 2. Dans quelle mesure SOLL m'aide au quotidien

### 2.1 Sans SOLL (état hypothétique)

| Tâche | Coût sans SOLL |
|---|---|
| Reprendre une session après compaction | 5-15 min de re-explication par l'opérateur |
| Découvrir l'intent derrière un fichier .rs | Lecture brute + inférence + 3-4 questions |
| Justifier un choix architectural à l'opérateur | Reconstitution from scratch |
| Tracking de progression vers un milestone | Aucun — dépend de la mémoire opérateur |
| Cohérence multi-session sur un sujet | Dérive systématique |

### 2.2 Avec SOLL (état observé)

| Tâche | Bénéfice SOLL |
|---|---|
| Reprendre session | `soll_query_context` + `MEMORY.md` → onboard < 1 min |
| Trouver l'intent | `soll_query_context question=...` ou direct ID lookup |
| Justifier choix | "voir DEC-AXO-091, SOLVES REQ-AXO-322" → traçabilité immédiate |
| Tracking milestone | `soll_work_plan` (théoriquement — §5 montre ses limites) |
| Cohérence | Édges REFINES/SUPERSEDES forcent la lignée des intents |

### 2.3 Gain mesurable cette session

- **31 entités SOLL créées + 55 liens** pour structurer MIL-AXO-019. Sans SOLL, ce serait un seul document Markdown de ~3 000 lignes, illisible et impossible à re-naviguer.
- **4 bugs LLM-contract loggés en SOLL** avec lien CPT-AXO-018. Survivront aux sessions futures.
- **Re-cadrage GUI-PRO-100** : 27 nodes recompressées de ~3000 chars chacune à ~700-1000. Densité × 4, lisibilité × 5 pour les LLM futurs.

---

## 3. SOLL après MIL-AXO-019 livré

### 3.1 Capacités débloquées

| Capacité | Avant MIL-019 | Après MIL-019 |
|---|---|---|
| **`soll_work_plan` rapidité** | OK (SOLL ~1k nodes via `petgraph::Graph`) | Inchangé (SOLL reste petit) |
| **`retrieve_context` SOLL** | Linear scan + FTS partiel | RRF tri-modal authentique (graphe SOLL + vector SOLL + FTS) |
| **Audit cycles SOLL** | Inexistant (cycles possibles en théorie) | Pre-write validation + audit hebdomadaire (REQ-AXO-91492) |
| **Détection sur intent stale** | Manuel | PageRank + decay temporel + bridges sur graphe SOLL |
| **Cohérence cross-session** | Memoire ad hoc | `tool_migration_status` audit auto |

### 3.2 Vraie révolution attendue

Aujourd'hui SOLL est un **stockage typé**. Après MIL-AXO-019, SOLL devient un **graphe interrogeable analytiquement** :

- *"Quelles sont les 10 REQ les plus structurellement centrales ?"* → PageRank
- *"Si REQ-AXO-298 saute, quoi devient orphelin ?"* → reachability filter
- *"Y a-t-il des cycles cachés dans les REFINES ?"* → Tarjan SCC continu
- *"Le pattern *bug logé sans VAL* existe-t-il ailleurs ?"* → subgraph match VF2
- *"Quels CPT sont sous-utilisés ?"* → in-degree analytics

Ces questions sont **inexpressibles aujourd'hui**, même partiellement, sans le graphe en RAM.

### 3.3 Mesure du gain

| Métrique SOLL | Aujourd'hui | Après MIL-019 |
|---|---:|---:|
| Latence `soll_query_context` p99 | ~100-300 ms | < 30 ms |
| Profondeur traversal acceptable | 5-10 hops (CTE) | illimitée |
| Algos disponibles | transitive closure only | PageRank, SCC, bridges, VF2, bidi BFS |
| Détection cycles SOLL | manuelle | continue + bloquante pré-write |

---

## 4. Comment imposer à un LLM de procéder selon cette discipline

### 4.1 Règles minimales (à inscrire dans CLAUDE.md global)

```
Avant d'écrire dans SOLL :
1. soll_relation_schema sur (source_kind, target_kind) si la direction n'est pas connue.
2. soll_query_context pour vérifier qu'aucun node similaire n'existe (anti-doublon).
3. soll_work_plan limit=5 pour comprendre ce qui bloque actuellement.

Pendant l'écriture :
4. 1 intent = 1 node. JAMAIS de "umbrella REQ avec 6 slices dedans".
5. Sections narratives interdites. Tables + listes sèches + pointeurs vers autres nodes SOLL.
6. Pas de date / "observed during" / "découvert pendant" (GUI-PRO-100 §2).
7. Critères de succès chiffrés (VAL séparée avec seuils mesurables).
8. Tous les choix discrets = DEC séparée.
9. Tous les principes architecturaux = CPT séparé.
10. Toutes les règles d'écriture/process = GUI séparée.

Après l'écriture :
11. Tisser TOUS les liens canoniques (BELONGS_TO, REFINES, EXPLAINS, VERIFIES, TARGETS, SOLVES, SUPERSEDES).
12. Re-courir soll_work_plan pour vérifier que la nouvelle grappe émerge avec un score cohérent.
13. Si le node a >2K chars, retravailler (GUI-PRO-100 §6).
```

### 4.2 Principe : liberté = mode d'écriture, pas mode de structure

Le LLM garde la liberté de choisir :
- les mots (mais pas la longueur — GUI-PRO-100 enforced),
- l'ordre logique des sections internes d'un node,
- la précision des seuils (mais doit être chiffré, jamais qualitatif).

Le LLM perd la liberté de :
- créer un node-fourre-tout multi-intent,
- omettre les liens canoniques,
- répéter de la prose qui existe déjà ailleurs en SOLL,
- inventer des relations non-canoniques.

### 4.3 Mécanisme d'enforcement souhaitable (côté serveur SOLL)

| Règle | Vérification serveur |
|---|---|
| `description` > 2 000 chars | `Err("node exceeds GUI-PRO-100 §6, retravaille")` |
| `description` contient regex `\b202[5-9]-\d\d-\d\d\b` | `Warn("date interdite par GUI-PRO-100 §2 — déplace en Revision")` |
| REQ créée sans aucun lien après 60 s | `Warn("orphaned REQ — link to PIL/MIL/parent REQ")` |
| Cycle introduit dans REFINES | déjà DEC-AXO-098 (impl REQ-AXO-91492) |
| Status non canonique | déjà DEC-PRO-100 (impl partielle REQ-AXO-325) |

### 4.4 Critère de robustesse

Si un LLM nouveau peut, **sur une nouvelle initiative**, produire :
- 1 PIL + 1 MIL + 1 REQ umbrella + N sous-REQ + CPT + DEC + VAL + GUI,
- avec ratio description-utile/description-totale > 80 %,
- avec tous les liens canoniques tissés du premier coup,
- sans rappel par l'opérateur,

…alors la discipline est inscrite dans le système. Aujourd'hui, **elle ne l'est pas** : j'ai eu besoin du recadrage de l'opérateur pour appliquer GUI-PRO-100.

---

## 5. Vérification fidélité `soll_work_plan`

### 5.1 Constat

Appel `soll_work_plan(project_code=AXO, format=brief, top=150)` après création de 31 entités + 55 liens.

**Wave 1 retournée** : ~50 entries, **dominées par des DEC pré-existants** (DEC-AXO-003 à 093, scores 43-91), suivies de quelques REQ-AXO-252..260 (P0 partial).

**Mes entités créées cette session** : invisibles dans la Wave 1, malgré :
- REQ-AXO-91483 qui unblocks 9 sous-REQ via REFINES (score attendu très élevé),
- MIL-AXO-019 qui TARGETS 9 REQ,
- 31 nodes status `planned` ou `current`.

### 5.2 Test additionnel

| Paramètre testé | Résultat |
|---|---|
| `top=25` | Wave 1 truncated, aucune entité 91483-91499 |
| `top=150` | Identique à top=25 — **paramètre `top` ne semble pas respecté** |
| `include_decay=false` | JSON envelope vide |
| Direct SQL : entités présentes en `soll.node` et `soll.edge` | OUI, statut/edges corrects |

### 5.3 Hypothèses

1. **Truncation hardcodée** dans `soll_work_plan` indépendamment du paramètre `top`.
2. **Scorer ignore REFINES** dans le calcul "unblocks N descendants" — regarde uniquement SOLVES (DEC→REQ).
3. **Status `planned`** filtré en faveur de `current` ou inversement.
4. **Decay temporel** mal calibré : pénalise les entités récentes au lieu de les favoriser.
5. **Cache stale** : `soll_work_plan` lit un snapshot non rafraîchi après mes mutations.

### 5.4 Verdict

`soll_work_plan` **ne reflète pas fidèlement** la grappe nouvellement créée. C'est un **bug supplémentaire à logger** — l'outil censé orchestrer le travail n'inclut pas le travail planifié.

**Action proposée** : créer REQ-AXO-91500 (ou suivant) avec tags `axon-bug` + `llm-contract` + `partial-analysis` documentant ce sympôme. Cf. §6.

### 5.5 Détail des entités non-visibles

| ID | Type | Status | Liens entrants/sortants | Wave attendue |
|---|---|---|---|---|
| PIL-AXO-9002 | Pillar | current | 1 (REQ-91483 BELONGS_TO) | indéterminée (PIL en wave 1 ?) |
| MIL-AXO-019 | Milestone | planned | 9 (TARGETS sous-REQ) | wave 1 (unblocker fort) |
| REQ-AXO-91483 | Requirement umbrella | planned | 13 entrants + 3 sortants | wave 1 (score ≥ 91) |
| REQ-AXO-91484..91492 | Requirement slices | planned | TARGETS + REFINES | wave 2-3 |
| REQ-AXO-91493..91499 | Requirement bugs | planned | EXPLAINS depuis CPT | wave 1-2 |

Aucune n'apparaît dans la Wave 1 retournée.

---

## 6. Bug à logger : `soll_work_plan` ne reflète pas les entités fraîchement créées

Format pour log SOLL (suivant CPT-AXO-019 protocole) :

```
title: "soll_work_plan: Wave 1 dominée par DEC anciennes, ignore REQ nouvellement créées"
tags: [axon-bug, llm-contract, soll-tool, scoring, deliverability]
description: <constat + 4 hypothèses + critère acceptance>
```

Sera créé lors de la prochaine vague de logging si l'opérateur valide.

---

## 7. Conclusion et recommandations

### 7.1 Synthèse

SOLL est **un excellent système quand utilisé selon GUI-PRO-100 et CPT-AXO-019**. Mes erreurs cette session (single REQ fourre-tout, prose verbeuse, doublons textuels) n'étaient pas un défaut du système — c'était l'absence d'enforcement automatique des règles existantes.

Le système fonctionne. Les règles existent. **Ce qui manque est l'enforcement serveur** (limites de longueur, détection de patterns interdits, détection d'orphans, scoring honnête dans `soll_work_plan`).

### 7.2 Recommandations ordonnées

1. **Court terme (avant MIL-AXO-019)** : logger les 5 bugs SOLL résiduels (4 déjà loggés ; +1 pour `soll_work_plan` scoring).
2. **Pendant MIL-AXO-019 Slice 0** : ajouter au `status verbose` les métriques `soll_node_avg_chars`, `soll_orphan_count`, `soll_work_plan_top_n_includes_recent`. Visibilité du sympôme.
3. **Après MIL-AXO-019 Slice 5** : exploiter le RRF tri-modal SOLL pour `soll_query_context` — gain de pertinence ×3 attendu.
4. **Post-MIL** : implémenter les guardrails serveur §4.3 dans `soll_manager`.

### 7.3 Lien

Ce rapport : `docs/working-notes/2026-05-15-soll-evaluation-session-report.md`
Chemin complet : `/home/dstadel/projects/axon/docs/working-notes/2026-05-15-soll-evaluation-session-report.md`
