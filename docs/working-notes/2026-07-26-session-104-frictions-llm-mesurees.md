# Session 104 — 2026-07-26 · Frictions LLM mesurées + clôture 902192

> Note d'AUDIT (append-only). Reprise canonique = session_pointer **`CPT-AXO-052`**.
> Ici = le narratif, et surtout **ce que la mesure a démenti**.

## Le fil conducteur : trois fois, la mesure a contredit l'intitulé

Cette session s'est jouée sur un même réflexe appliqué trois fois — **mesurer avant de
croire le libellé du problème**. Les trois fois, le diagnostic initial était faux.

### 1. « query renvoie des lignes dupliquées » → c'était une perte de rappel

Rapporté comme un défaut cosmétique (feedback NEX #43). La mesure dit autre chose : un
`LEFT JOIN Chunk` servant **uniquement** à projeter `file_path` démultipliait chaque symbole
par son nombre de chunk-parts — **avant** le `LIMIT`.

- Mesuré sur AXO : `LIMIT 10` → **10 lignes pour 4 symboles distincts** (60 % des slots perdus).
- 23 % des symboles (2 236/9 633) ont ≥2 chunks ; le pire en a **690** → il pouvait manger un
  `LIMIT 10` à lui seul.
- Bonus : c'était **aussi** un bug de perf. Même requête : **424 ms → 16 ms (26×)**, parce que
  PG ne matérialise plus les lignes fantômes.

Un `.dedup()` post-hoc aurait rendu la sortie propre **en laissant le bug intact** — le budget
`LIMIT` était déjà brûlé. C'est le piège que la mesure a évité.

**3 sites, 3 formes différentes.** C'est le point d'ingénierie de la session : le même
`LEFT JOIN` y jouait des rôles différents.

| Site | Rôle de `ch` | Forme |
|---|---|---|
| `axon_query` (6 bras) | projection seule | `LATERAL … LIMIT 1` (fragment partagé unique) |
| `tools_risk.rs` | **dans le prédicat** (`WHERE ch.file_path LIKE`) | `EXISTS` |
| `entry_candidates.rs` ×2 | prédicat **+** filtre `ch.project_code` | `DISTINCT ON (s.id)` |

Appliquer partout la recette du premier site aurait **cassé le matching** sur les deux autres
(un symbole dont le chunk correspondant n'est pas la 1re part aurait disparu). On ne le voit
qu'en lisant chaque site.

**Le test a été prouvé sensible** : ancienne forme restaurée temporairement → échec
`got 3 in ["dedup_alpha","dedup_alpha","dedup_alpha","dedup_beta"]`. Les tests `query`
existants étaient structurellement **aveugles** — ils sèment 1 chunk par symbole.

### 2. « soll_acyclic_audit est cassé, 100 % d'échec » → il marche parfaitement

82/82 appels en échec en télémétrie. L'outil répond très bien — **avec** `project_code`. Il
l'**exige** là où `query`/`inspect` l'auto-résolvent du cwd. Les LLM l'appellent comme ils
appellent `query`, et échouent. C'est une **classe** : 12 outils rejettent explicitement, +1
tombe en « Invalid arguments ».

Deux pièges évités au moment d'implémenter :
- Le point d'injection évident (`handle_call_tool`, le dispatch) est **faux** : `axon_batch`
  appelle `execute_tool_direct` en le **contournant**, et les LLM batchent.
- **La liste d'exclusion est la moitié critique.** Pour une dizaine d'outils, omettre le
  projet signifie « tous les projets » (`embedding_status` rend un rollup per-project).
  Y injecter ne lèverait **aucune erreur** — ça **rétrécirait silencieusement** la réponse.
  Une régression **pire** que l'échec visible qu'on corrige. D'où un test **négatif** dédié.
- Exclues aussi : les mutations SOLL (écrire dans un projet **deviné** est irréversible, et
  elles passent par un second resolver aux règles divergentes) et la mailbox (ses champs
  projet adressent des **tiers**).

### 3. « relation_type : il manque la matrice » → elle existe, et un outil l'expose

Friction n°1 ouverte : **217 occurrences**. La matrice est dans `relation_policy.rs` et
`soll_relation_schema` la publie déjà. Le défaut est **en amont** : rien dans le schéma ne dit
que les valeurs légales dépendent du **couple** (source, cible) — donc le LLM devine d'abord
et découvre après. Je l'ai heurté moi-même (`REFINES` refusé sur REQ → CPT).

Fix en deux temps : documenter la contrainte **dans le schéma** (là où le LLM lit avant
d'appeler) + rendre un **`corrected_call`** quand la paire n'admet qu'**une** relation légale.
Deux retenues assumées : un **patch**, pas un appel complet reconstitué (le payload d'origine
n'est pas reproductible ; un appel inventé serait pire qu'inutile) ; et **absent** dès que
plusieurs relations sont légales — sa présence signifie donc toujours « applique verbatim ».

## Le refus le plus utile de la session : ne pas construire l'image S4

Le scoping de 902192 S4-complet a conclu à **ne pas le faire**, et c'est la bonne décision :

- **4 clauses sur 5 de S4 étaient déjà livrées** (sélecteur projet, cross-projet, orphelins
  rouges, consommable LLM). Seule l'image manquait.
- **Elle mentirait.** Données réelles : 79 racines pour 2 985 candidats, 815 non-atteints,
  76 îlots morts — parce que `main → run_brain` n'a **aucune** arête CALLS enregistrée (trou
  d'extraction documenté). Une **liste** porte ce caveat en prose (le corps du REQ le fait) ;
  une **image** ne le peut pas : elle se lit comme faisant autorité. Artefact **beau, alarmant
  et faux**.
- **Substitution de catégorie** : « source→transform→embouchure », « lac→job→transform→sortie »
  est le vocabulaire du **lignage de données** (volet 2, OpenLineage/Marquez), pas du graphe
  d'appels — qui n'a ni « transform » ni « embouchure ».
- Techniquement, le layout demandé n'est pas gratuit : ECharts `sankey` (déjà dans le
  dashboard) est **DAG-only**, or AXO a **64 SCC** dont une de 34.

Décision opérateur : clôturer 902192, reporter l'image au volet 2. **Et tracer ce qui n'est
pas fait** plutôt que le laisser disparaître : le trou de spec **S1** (`wiring_coverage` +
roots + leaves, promis en S1 et jamais livrés — ≈60 lignes puisque `reached` est déjà calculé
puis jeté), le volet 2 lui-même (qui n'avait **aucun REQ**), et la classe `sql`.

## Ce que je logge sans le corriger

`sql` = **68 % de tout le trafic MCP** et **436 frictions ouvertes**. Tentant à « corriger »,
mais l'enveloppe repair **fonctionne déjà** (elle m'a rattrapé 3 fois aujourd'hui en listant
les vraies colonnes). Le problème est le **taux d'entrée**, pas la récupération — et ce volume
suggère surtout que les LLM ne **trouvent pas** les outils structurés. Un correctif rapide
traiterait le symptôme. → REQ d'**étude** (échantillonner les requêtes réelles d'abord).

## Erreurs que j'ai commises

- **Un script regex a abîmé 2 entrées du catalogue** (elles contenaient des guillemets
  échappés `\"AXO\"` ; le motif s'arrêtait au `\`). Le compilateur l'a attrapé immédiatement.
  Leçon : compiler après **toute** réécriture automatisée, avant de passer à la suite.
- **J'ai référencé REQ-AXO-902247 dans le code et le commit avant de le créer.** L'ID alloué
  est tombé juste par chance. La discipline est : logger le REQ **d'abord**.
- Un `#[test]` dupliqué et 4 temporaires `&json!()` empruntés par un `Cow` — attrapés au
  build, sans conséquence, mais signe qu'il faut compiler tôt et souvent.

## État machine

Driver NVIDIA **610.52 — INCHANGÉ** après le reboot du 25/07 : le DDU n'a pas pris effet. Le
calme depuis le 14/07 est une **dormance** (la charge GPU a disparu), pas une guérison.
Conséquence directe : **un promote redémarre l'indexeur GPU** (~9-13 min, cutover
« full stop→swap→start ») — donc c'est un déclencheur TDR. Ironie : le fix graceful-shutdown,
qui rend précisément ces arrêts plus propres, est lui-même en attente de ce promote.
